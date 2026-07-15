//! Optional media normalization, tagging, and validation support.
//!
//! The common media layer is enabled with `media`. Concrete FFmpeg backends
//! are selected independently: `media-ffmpeg-cli` invokes an installed
//! executable, while `media-ffmpeg` links to FFmpeg libraries in-process.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{MimeType, Picture, PictureType},
    tag::{Accessor, ItemKey, Tag, TagType},
};
use thiserror::Error;

#[cfg(feature = "media-ffmpeg")]
pub mod ffmpeg;
#[cfg(feature = "media-ffmpeg-cli")]
pub mod ffmpeg_cli;

/// Metadata embedded into a downloaded audio file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub lyrics: Option<EmbeddedLyrics>,
}

/// Lyrics text and whether it carries synchronized timestamps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedLyrics {
    pub text: String,
    pub synchronized: bool,
}

/// Errors produced by a media backend or the common tagging layer.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("media worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("{0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Container operations needed by the downloader.
pub trait MediaBackend: Clone + Send + Sync + 'static {
    fn name(&self) -> &'static str;

    fn write_m4a_metadata(
        &self,
        path: PathBuf,
        metadata: TrackMetadata,
        artwork: Option<Vec<u8>>,
    ) -> impl Future<Output = Result<()>> + Send;

    fn remux_flac(
        &self,
        source: PathBuf,
        destination: PathBuf,
        replace: bool,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Transcodes the first audio stream to MP3 at the requested bitrate.
    fn transcode_mp3(
        &self,
        source: PathBuf,
        destination: PathBuf,
        bitrate_kbps: u32,
        replace: bool,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Fully decodes an audio file when the backend supports generic validation.
    ///
    /// The default keeps existing third-party backends source-compatible. The
    /// built-in FFmpeg backends override it for MP3 and FLAC validation. The
    /// expected duration is absent when the container cannot provide an
    /// authoritative value, as with VBR MP3 files lacking Xing/VBRI metadata.
    fn verify_audio(
        &self,
        path: PathBuf,
        _expected_duration: Option<Duration>,
    ) -> impl Future<Output = Result<()>> + Send {
        async move {
            if extension_is(&path, "m4a") {
                self.verify_m4a(path).await
            } else {
                Ok(())
            }
        }
    }

    /// Fully decodes an M4A file.
    ///
    /// This method predates generic validation and remains required for
    /// compatibility with existing media backend implementations.
    fn verify_m4a(&self, path: PathBuf) -> impl Future<Output = Result<()>> + Send;
}

/// Write portable tags, delegating MP4 atom rewriting to the selected backend.
pub async fn write_metadata<B: MediaBackend>(
    backend: &B,
    path: &Path,
    metadata: &TrackMetadata,
    artwork: Option<Vec<u8>>,
) -> Result<()> {
    if extension_is(path, "m4a") {
        return backend
            .write_m4a_metadata(path.to_owned(), metadata.clone(), artwork)
            .await;
    }
    let path = path.to_owned();
    let metadata = metadata.clone();
    tokio::task::spawn_blocking(move || write_metadata_blocking(&path, &metadata, artwork)).await?
}

/// Verify the container and duration, fully decoding audio through the backend.
pub async fn verify_audio_file<B: MediaBackend>(
    backend: &B,
    path: &Path,
    expected_extension: &str,
) -> Result<()> {
    let path = path.to_owned();
    let extension = expected_extension.to_owned();
    let blocking_path = path.clone();
    let expected_duration =
        tokio::task::spawn_blocking(move || verify_audio_file_blocking(&blocking_path, &extension))
            .await??;
    backend.verify_audio(path, expected_duration).await?;
    Ok(())
}

fn write_metadata_blocking(
    path: &Path,
    metadata: &TrackMetadata,
    artwork: Option<Vec<u8>>,
) -> Result<()> {
    let mut file = lofty::read_from_path(path)
        .map_err(|error| Error::Backend(format!("failed to read {}: {error}", path.display())))?;
    let tag_type = file.primary_tag_type();
    if file.tag(tag_type).is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.tag_mut(tag_type).expect("primary tag was inserted");
    apply_text_metadata(tag, metadata);
    if let Some(picture) = artwork {
        tag.remove_picture_type(PictureType::CoverFront);
        let mime = detect_mime(&picture);
        tag.push_picture(
            Picture::unchecked(picture)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }
    file.save_to_path(path, WriteOptions::default())
        .map_err(|error| Error::Backend(format!("failed to save {}: {error}", path.display())))?;
    Ok(())
}

fn verify_audio_file_blocking(path: &Path, expected_extension: &str) -> Result<Option<Duration>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() < 1024 {
        return Err(Error::Backend(format!(
            "file is only {} bytes",
            metadata.len()
        )));
    }
    let file = lofty::read_from_path(path)
        .map_err(|error| Error::Backend(format!("failed to parse {}: {error}", path.display())))?;
    let (expected_type, compare_decoded_duration) = match expected_extension {
        "flac" => (FileType::Flac, true),
        "m4a" => (FileType::Mp4, true),
        // An MP3 without a Xing/VBRI frame count has no authoritative
        // container duration. Lofty must estimate it from a frame bitrate,
        // which can be far from the decoded duration for valid VBR files.
        "mp3" => (FileType::Mpeg, false),
        extension => {
            return Err(Error::Backend(format!(
                "unsupported extension .{extension}"
            )));
        }
    };
    if file.file_type() != expected_type {
        return Err(Error::Backend(format!(
            "container {:?} does not match .{expected_extension}",
            file.file_type()
        )));
    }
    let duration = file.properties().duration();
    if duration.is_zero() {
        return Err(Error::Backend("audio duration is zero".to_owned()));
    }
    Ok(compare_decoded_duration.then_some(duration))
}

pub(crate) fn ensure_decoded_duration(expected: Duration, actual: Duration) -> Result<()> {
    const TOLERANCE: Duration = Duration::from_millis(250);
    let difference = expected.abs_diff(actual);
    if difference > TOLERANCE {
        return Err(Error::Backend(format!(
            "decoded audio duration {:.3}s differs from container duration {:.3}s by {:.3}s",
            actual.as_secs_f64(),
            expected.as_secs_f64(),
            difference.as_secs_f64(),
        )));
    }
    Ok(())
}

fn apply_text_metadata(tag: &mut Tag, metadata: &TrackMetadata) {
    tag.set_title(metadata.title.clone());
    tag.set_artist(metadata.artist.clone());
    if let Some(value) = &metadata.album {
        tag.set_album(value.clone());
    }
    if let Some(value) = &metadata.genre {
        tag.set_genre(value.clone());
    }
    if let Some(value) = metadata.track_number {
        tag.set_track(value);
    }
    if let Some(value) = metadata.disc_number {
        tag.set_disk(value);
    }
    if let Some(value) = &metadata.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, value.clone());
    }
    if let Some(value) = metadata.year {
        tag.insert_text(ItemKey::RecordingDate, value.to_string());
    }
    if let Some(lyrics) = &metadata.lyrics {
        tag.remove_key(ItemKey::Lyrics);
        tag.remove_key(ItemKey::UnsyncLyrics);
        let key = if lyrics.synchronized && tag.tag_type() != TagType::Id3v2 {
            ItemKey::Lyrics
        } else {
            ItemKey::UnsyncLyrics
        };
        tag.insert_text(key, lyrics.text.clone());
    }
}

pub(crate) fn detect_mime(data: &[u8]) -> MimeType {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        MimeType::Png
    } else if data.starts_with(b"\xff\xd8\xff") {
        MimeType::Jpeg
    } else {
        MimeType::Unknown("application/octet-stream".to_owned())
    }
}

pub(crate) fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        EmbeddedLyrics, TrackMetadata, apply_text_metadata, detect_mime, ensure_decoded_duration,
    };
    use lofty::{
        picture::MimeType,
        tag::{ItemKey, Tag, TagType},
    };

    #[test]
    fn detects_common_artwork_formats() {
        assert_eq!(detect_mime(b"\xff\xd8\xffrest"), MimeType::Jpeg);
        assert_eq!(detect_mime(b"\x89PNG\r\n\x1a\nrest"), MimeType::Png);
    }

    #[test]
    fn embeds_portable_lyrics() {
        let metadata = TrackMetadata {
            lyrics: Some(EmbeddedLyrics {
                text: "[00:00]line".into(),
                synchronized: true,
            }),
            ..TrackMetadata::default()
        };
        let mut tag = Tag::new(TagType::VorbisComments);
        apply_text_metadata(&mut tag, &metadata);
        assert_eq!(tag.get_string(ItemKey::Lyrics), Some("[00:00]line"));
    }

    #[test]
    fn decoded_duration_tolerance_includes_its_boundary() {
        let expected = Duration::from_secs(10);
        assert!(ensure_decoded_duration(expected, expected + Duration::from_millis(250)).is_ok());
        assert!(ensure_decoded_duration(expected, expected - Duration::from_millis(250)).is_ok());
        assert!(ensure_decoded_duration(expected, expected + Duration::from_millis(251)).is_err());
        assert!(ensure_decoded_duration(expected, expected - Duration::from_millis(251)).is_err());
    }
}
