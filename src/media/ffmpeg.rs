//! In-process media backend linked against FFmpeg libraries.

use std::path::{Path, PathBuf};

use ffmpeg_next as av;

use super::{Error, MediaBackend, Result, TrackMetadata, detect_mime};

#[derive(Clone, Debug, Default)]
pub struct Ffmpeg;

impl MediaBackend for Ffmpeg {
    fn name(&self) -> &'static str {
        "ffmpeg-libav"
    }

    async fn write_m4a_metadata(
        &self,
        path: PathBuf,
        metadata: TrackMetadata,
        artwork: Option<Vec<u8>>,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let output = sibling_temporary(&path, "metadata.m4a");
            remux_audio(&path, &output, Some(&metadata), artwork.as_deref(), "ipod")?;
            replace_file_blocking(&output, &path, true)
        })
        .await?
    }

    async fn remux_flac(&self, source: PathBuf, destination: PathBuf, replace: bool) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            if destination.exists() && !replace {
                return Err(backend(format!(
                    "destination {} already exists",
                    destination.display()
                )));
            }
            let output = sibling_temporary(&destination, "remux.flac");
            remux_audio(&source, &output, None, None, "flac")?;
            replace_file_blocking(&output, &destination, replace)
        })
        .await?
    }

    async fn transcode_mp3(
        &self,
        source: PathBuf,
        destination: PathBuf,
        bitrate_kbps: u32,
        replace: bool,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            if destination.exists() && !replace {
                return Err(backend(format!(
                    "destination {} already exists",
                    destination.display()
                )));
            }
            let output = sibling_temporary(&destination, "transcode.mp3");
            if let Err(error) = transcode_audio_mp3(&source, &output, bitrate_kbps) {
                let _ = std::fs::remove_file(&output);
                return Err(error);
            }
            replace_file_blocking(&output, &destination, replace)
        })
        .await?
    }

    async fn verify_m4a(&self, path: PathBuf) -> Result<()> {
        tokio::task::spawn_blocking(move || decode_audio(&path)).await?
    }
}

fn initialize() -> Result<()> {
    av::init().map_err(|error| backend(format!("failed to initialize FFmpeg: {error}")))
}

fn remux_audio(
    source: &Path,
    destination: &Path,
    metadata: Option<&TrackMetadata>,
    artwork: Option<&[u8]>,
    output_format: &str,
) -> Result<()> {
    initialize()?;
    let mut input = av::format::input(source).map_err(av_error("failed to open media input"))?;
    let audio = input
        .streams()
        .best(av::media::Type::Audio)
        .ok_or_else(|| backend("input contains no audio stream"))?;
    let audio_index = audio.index();
    let audio_time_base = audio.time_base();
    let audio_parameters = audio.parameters();

    let mut output = av::format::output_as(destination, output_format)
        .map_err(av_error("failed to create media output"))?;
    let output_audio_index;
    {
        let mut stream = output
            .add_stream(av::encoder::find(av::codec::Id::None))
            .map_err(av_error("failed to add audio stream"))?;
        stream.set_parameters(audio_parameters);
        stream.set_time_base(audio_time_base);
        unsafe {
            (*stream.parameters().as_mut_ptr()).codec_tag = 0;
        }
        output_audio_index = stream.index();
    }

    let cover_index = if let Some(bytes) = artwork {
        let dimensions = imagesize::blob_size(bytes)
            .map_err(|error| backend(format!("failed to read cover dimensions: {error}")))?;
        let codec = match detect_mime(bytes) {
            lofty::picture::MimeType::Png => av::codec::Id::PNG,
            lofty::picture::MimeType::Jpeg => av::codec::Id::MJPEG,
            _ => return Err(backend("unsupported cover image format")),
        };
        let mut stream = output
            .add_stream(av::encoder::find(av::codec::Id::None))
            .map_err(av_error("failed to add cover stream"))?;
        stream.set_time_base((1, 90_000));
        unsafe {
            let raw_stream = stream.as_mut_ptr();
            (*raw_stream).disposition |= av::ffi::AV_DISPOSITION_ATTACHED_PIC;
            let parameters = (*raw_stream).codecpar;
            (*parameters).codec_type = av::ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*parameters).codec_id = codec.into();
            (*parameters).codec_tag = 0;
            (*parameters).width = i32::try_from(dimensions.width)
                .map_err(|_| backend("cover width exceeds FFmpeg limits"))?;
            (*parameters).height = i32::try_from(dimensions.height)
                .map_err(|_| backend("cover height exceeds FFmpeg limits"))?;
        }
        Some(stream.index())
    } else {
        None
    };

    output.set_metadata(match metadata {
        Some(metadata) => metadata_dictionary(metadata),
        None => input.metadata().to_owned(),
    });
    let mut options = av::Dictionary::new();
    if output_format == "ipod" {
        options.set("movflags", "+faststart");
    }
    output
        .write_header_with(options)
        .map_err(av_error("failed to write media header"))?;
    let output_time_base = output
        .stream(output_audio_index)
        .ok_or_else(|| backend("output audio stream disappeared"))?
        .time_base();

    for (stream, mut packet) in input.packets() {
        if stream.index() != audio_index {
            continue;
        }
        packet.rescale_ts(audio_time_base, output_time_base);
        packet.set_position(-1);
        packet.set_stream(output_audio_index);
        packet
            .write_interleaved(&mut output)
            .map_err(av_error("failed to write audio packet"))?;
    }
    if let (Some(index), Some(bytes)) = (cover_index, artwork) {
        let mut packet = av::Packet::copy(bytes);
        packet.set_stream(index);
        packet.set_flags(av::packet::Flags::KEY);
        packet.set_pts(Some(0));
        packet.set_dts(Some(0));
        packet.set_position(-1);
        packet
            .write_interleaved(&mut output)
            .map_err(av_error("failed to write cover packet"))?;
    }
    output
        .write_trailer()
        .map_err(av_error("failed to write media trailer"))?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}

fn decode_audio(path: &Path) -> Result<()> {
    initialize()?;
    let mut input = av::format::input(path).map_err(av_error("failed to open M4A"))?;
    let stream = input
        .streams()
        .best(av::media::Type::Audio)
        .ok_or_else(|| backend("M4A contains no audio stream"))?;
    let index = stream.index();
    let context = av::codec::context::Context::from_parameters(stream.parameters())
        .map_err(av_error("failed to create audio decoder"))?;
    let mut decoder = context
        .decoder()
        .audio()
        .map_err(av_error("failed to open audio decoder"))?;
    let mut frame = av::frame::Audio::empty();
    let mut decoded_frames = 0_usize;
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        decoder
            .send_packet(&packet)
            .map_err(av_error("failed to send audio packet"))?;
        decoded_frames += drain_decoder(&mut decoder, &mut frame)?;
    }
    decoder
        .send_eof()
        .map_err(av_error("failed to flush audio decoder"))?;
    decoded_frames += drain_decoder(&mut decoder, &mut frame)?;
    if decoded_frames == 0 {
        return Err(backend("M4A decoder produced no audio frames"));
    }
    Ok(())
}

struct AudioTranscoder {
    input_index: usize,
    decoder: av::decoder::Audio,
    encoder: av::encoder::Audio,
    filter: av::filter::Graph,
    encoder_time_base: av::Rational,
    output_time_base: av::Rational,
}

fn transcode_audio_mp3(source: &Path, destination: &Path, bitrate_kbps: u32) -> Result<()> {
    initialize()?;
    let mut input = av::format::input(source).map_err(av_error("failed to open media input"))?;
    let input_stream = input
        .streams()
        .best(av::media::Type::Audio)
        .ok_or_else(|| backend("input contains no audio stream"))?;
    let input_index = input_stream.index();
    let input_time_base = input_stream.time_base();
    let decoder_context = av::codec::context::Context::from_parameters(input_stream.parameters())
        .map_err(av_error("failed to create audio decoder"))?;
    let mut decoder = decoder_context
        .decoder()
        .audio()
        .map_err(av_error("failed to open audio decoder"))?;
    decoder.set_time_base(input_time_base);

    let codec = av::encoder::find_by_name("libmp3lame")
        .ok_or_else(|| backend("FFmpeg was built without the libmp3lame encoder"))?;
    let audio_codec = codec
        .audio()
        .map_err(av_error("libmp3lame is not an audio encoder"))?;
    let mut output = av::format::output_as(destination, "mp3")
        .map_err(av_error("failed to create MP3 output"))?;
    let global_header = output
        .format()
        .flags()
        .contains(av::format::flag::Flags::GLOBAL_HEADER);
    let mut encoder = av::codec::context::Context::new_with_codec(codec)
        .encoder()
        .audio()
        .map_err(av_error("failed to create MP3 encoder"))?;
    let channel_layout = audio_codec
        .channel_layouts()
        .map(|layouts| layouts.best(decoder.channel_layout().channels()))
        .unwrap_or(av::channel_layout::ChannelLayout::STEREO);
    let sample_format = audio_codec
        .formats()
        .and_then(|mut formats| formats.next())
        .ok_or_else(|| backend("libmp3lame reports no supported sample format"))?;
    if global_header {
        encoder.set_flags(av::codec::flag::Flags::GLOBAL_HEADER);
    }
    encoder.set_rate(decoder.rate() as i32);
    encoder.set_channel_layout(channel_layout);
    encoder.set_format(sample_format);
    encoder.set_bit_rate((bitrate_kbps as usize) * 1_000);
    encoder.set_time_base((1, decoder.rate() as i32));
    let encoder = encoder
        .open_as(codec)
        .map_err(av_error("failed to open libmp3lame encoder"))?;
    let encoder_time_base = encoder.time_base();

    let output_index;
    {
        let mut stream = output
            .add_stream(codec)
            .map_err(av_error("failed to add MP3 stream"))?;
        stream.set_parameters(&encoder);
        stream.set_time_base(encoder_time_base);
        output_index = stream.index();
    }
    let output_time_base = output
        .stream(output_index)
        .ok_or_else(|| backend("output audio stream disappeared"))?
        .time_base();
    let filter = audio_filter(&decoder, &encoder)?;
    output
        .write_header()
        .map_err(av_error("failed to write MP3 header"))?;

    let mut transcoder = AudioTranscoder {
        input_index,
        decoder,
        encoder,
        filter,
        encoder_time_base,
        output_time_base,
    };
    for (stream, mut packet) in input.packets() {
        if stream.index() != transcoder.input_index {
            continue;
        }
        packet.rescale_ts(stream.time_base(), input_time_base);
        transcoder
            .decoder
            .send_packet(&packet)
            .map_err(av_error("failed to send audio packet"))?;
        transcoder.process_decoded(&mut output)?;
    }
    transcoder
        .decoder
        .send_eof()
        .map_err(av_error("failed to flush audio decoder"))?;
    transcoder.process_decoded(&mut output)?;
    transcoder
        .filter
        .get("in")
        .ok_or_else(|| backend("audio filter input disappeared"))?
        .source()
        .flush()
        .map_err(av_error("failed to flush audio filter"))?;
    transcoder.process_filtered(&mut output)?;
    transcoder
        .encoder
        .send_eof()
        .map_err(av_error("failed to flush MP3 encoder"))?;
    transcoder.process_encoded(&mut output)?;
    output
        .write_trailer()
        .map_err(av_error("failed to write MP3 trailer"))?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}

fn audio_filter(
    decoder: &av::decoder::Audio,
    encoder: &av::encoder::Audio,
) -> Result<av::filter::Graph> {
    let mut filter = av::filter::Graph::new();
    let arguments = format!(
        "time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
        decoder.time_base(),
        decoder.rate(),
        decoder.format().name(),
        decoder.channel_layout().bits()
    );
    let input = av::filter::find("abuffer").ok_or_else(|| backend("abuffer filter is missing"))?;
    let output =
        av::filter::find("abuffersink").ok_or_else(|| backend("abuffersink filter is missing"))?;
    filter
        .add(&input, "in", &arguments)
        .map_err(av_error("failed to add audio filter input"))?;
    filter
        .add(&output, "out", "")
        .map_err(av_error("failed to add audio filter output"))?;
    {
        let mut sink = filter
            .get("out")
            .ok_or_else(|| backend("audio filter output disappeared"))?;
        sink.set_sample_format(encoder.format());
        sink.set_channel_layout(encoder.channel_layout());
        sink.set_sample_rate(encoder.rate());
    }
    filter
        .output("in", 0)
        .map_err(av_error("failed to connect audio filter input"))?
        .input("out", 0)
        .map_err(av_error("failed to connect audio filter output"))?
        .parse("anull")
        .map_err(av_error("failed to configure audio filter"))?;
    filter
        .validate()
        .map_err(av_error("failed to validate audio filter"))?;
    if !codec_has_variable_frames(encoder) {
        filter
            .get("out")
            .ok_or_else(|| backend("audio filter output disappeared"))?
            .sink()
            .set_frame_size(encoder.frame_size());
    }
    Ok(filter)
}

fn codec_has_variable_frames(encoder: &av::encoder::Audio) -> bool {
    encoder.codec().is_some_and(|codec| {
        codec
            .capabilities()
            .contains(av::codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE)
    })
}

impl AudioTranscoder {
    fn process_decoded(&mut self, output: &mut av::format::context::Output) -> Result<()> {
        let mut decoded = av::frame::Audio::empty();
        loop {
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let timestamp = decoded.timestamp();
                    decoded.set_pts(timestamp);
                    self.filter
                        .get("in")
                        .ok_or_else(|| backend("audio filter input disappeared"))?
                        .source()
                        .add(&decoded)
                        .map_err(av_error("failed to filter decoded audio"))?;
                    self.process_filtered(output)?;
                }
                Err(error) if is_drain_complete(error) => break,
                Err(error) => return Err(backend(format!("audio decode failed: {error}"))),
            }
        }
        Ok(())
    }

    fn process_filtered(&mut self, output: &mut av::format::context::Output) -> Result<()> {
        let mut filtered = av::frame::Audio::empty();
        loop {
            let result = self
                .filter
                .get("out")
                .ok_or_else(|| backend("audio filter output disappeared"))?
                .sink()
                .frame(&mut filtered);
            match result {
                Ok(()) => {
                    self.encoder
                        .send_frame(&filtered)
                        .map_err(av_error("failed to send audio to MP3 encoder"))?;
                    self.process_encoded(output)?;
                }
                Err(error) if is_drain_complete(error) => break,
                Err(error) => return Err(backend(format!("audio filter failed: {error}"))),
            }
        }
        Ok(())
    }

    fn process_encoded(&mut self, output: &mut av::format::context::Output) -> Result<()> {
        let mut packet = av::Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(0);
                    packet.rescale_ts(self.encoder_time_base, self.output_time_base);
                    packet.set_position(-1);
                    packet
                        .write_interleaved(output)
                        .map_err(av_error("failed to write MP3 packet"))?;
                }
                Err(error) if is_drain_complete(error) => break,
                Err(error) => return Err(backend(format!("MP3 encode failed: {error}"))),
            }
        }
        Ok(())
    }
}

fn is_drain_complete(error: av::Error) -> bool {
    matches!(error, av::Error::Eof)
        || matches!(error, av::Error::Other { errno } if errno == av::error::EAGAIN)
}

fn drain_decoder(decoder: &mut av::decoder::Audio, frame: &mut av::frame::Audio) -> Result<usize> {
    let mut count = 0;
    loop {
        match decoder.receive_frame(frame) {
            Ok(()) => count += 1,
            Err(av::Error::Other { errno }) if errno == av::error::EAGAIN => break,
            Err(av::Error::Eof) => break,
            Err(error) => return Err(backend(format!("audio decode failed: {error}"))),
        }
    }
    Ok(count)
}

fn metadata_dictionary(metadata: &TrackMetadata) -> av::Dictionary<'static> {
    let mut dictionary = av::Dictionary::new();
    dictionary.set("title", &metadata.title);
    dictionary.set("artist", &metadata.artist);
    if let Some(value) = &metadata.album {
        dictionary.set("album", value);
    }
    if let Some(value) = &metadata.album_artist {
        dictionary.set("album_artist", value);
    }
    if let Some(value) = &metadata.genre {
        dictionary.set("genre", value);
    }
    let year = metadata.year.map(|value| value.to_string());
    if let Some(value) = &year {
        dictionary.set("date", value);
    }
    let track = metadata.track_number.map(|value| value.to_string());
    if let Some(value) = &track {
        dictionary.set("track", value);
    }
    let disc = metadata.disc_number.map(|value| value.to_string());
    if let Some(value) = &disc {
        dictionary.set("disc", value);
    }
    if let Some(value) = &metadata.lyrics {
        dictionary.set("lyrics", &value.text);
    }
    dictionary
}

fn replace_file_blocking(source: &Path, destination: &Path, replace: bool) -> Result<()> {
    #[cfg(windows)]
    if replace && destination.exists() {
        std::fs::remove_file(destination)?;
    }
    let _ = replace;
    std::fs::rename(source, destination)?;
    Ok(())
}

fn sibling_temporary(destination: &Path, suffix: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    parent.join(format!(".{name}.{}.{suffix}", std::process::id()))
}

fn av_error(context: &'static str) -> impl FnOnce(av::Error) -> Error {
    move |error| backend(format!("{context}: {error}"))
}

fn backend(message: impl Into<String>) -> Error {
    Error::Backend(message.into())
}
