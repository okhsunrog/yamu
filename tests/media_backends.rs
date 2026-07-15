#![cfg(any(feature = "media-ffmpeg-cli", feature = "media-ffmpeg"))]

use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use yamu::media::{EmbeddedLyrics, MediaBackend, TrackMetadata, verify_audio_file, write_metadata};

#[cfg(feature = "media-ffmpeg")]
use yamu::media::ffmpeg::Ffmpeg;
#[cfg(feature = "media-ffmpeg-cli")]
use yamu::media::ffmpeg_cli::FfmpegCli;

const TEST_LYRICS: &str = "[00:00.00]Test line\n[00:01.00]Second line";

#[cfg(feature = "media-ffmpeg-cli")]
#[tokio::test]
async fn cli_backend_preserves_m4a_audio_and_cover() {
    exercise_m4a_backend(FfmpegCli).await;
}

#[cfg(feature = "media-ffmpeg")]
#[tokio::test]
async fn linked_backend_preserves_m4a_audio_and_cover() {
    exercise_m4a_backend(Ffmpeg).await;
}

#[cfg(feature = "media-ffmpeg-cli")]
#[tokio::test]
async fn cli_backend_normalizes_flac_in_mp4() {
    exercise_flac_remux(FfmpegCli).await;
}

#[cfg(feature = "media-ffmpeg")]
#[tokio::test]
async fn linked_backend_normalizes_flac_in_mp4() {
    exercise_flac_remux(Ffmpeg).await;
}

#[cfg(feature = "media-ffmpeg-cli")]
#[tokio::test]
async fn cli_backend_transcodes_aac_to_mp3() {
    exercise_mp3_transcode(FfmpegCli).await;
}

#[cfg(feature = "media-ffmpeg")]
#[tokio::test]
async fn linked_backend_transcodes_aac_to_mp3() {
    exercise_mp3_transcode(Ffmpeg).await;
}

#[cfg(feature = "media-ffmpeg-cli")]
#[tokio::test]
async fn cli_backend_accepts_vbr_mp3_without_xing() {
    exercise_vbr_mp3_without_xing(FfmpegCli).await;
}

#[cfg(feature = "media-ffmpeg")]
#[tokio::test]
async fn linked_backend_accepts_vbr_mp3_without_xing() {
    exercise_vbr_mp3_without_xing(Ffmpeg).await;
}

async fn exercise_m4a_backend<B: MediaBackend>(backend: B) {
    let directory = temporary_directory(backend.name());
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let audio = directory.join("track.m4a");
    let cover = directory.join("cover.jpg");
    require_command("ffmpeg").await;
    require_command("ffprobe").await;
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
        "-c:a",
        "aac",
        "-f",
        "ipod",
        audio.to_str().unwrap(),
    ])
    .await;
    let original_duration = probe_duration(&audio).await;
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=32x32",
        "-frames:v",
        "1",
        "-update",
        "1",
        cover.to_str().unwrap(),
    ])
    .await;

    let metadata = TrackMetadata {
        title: "Title".into(),
        artist: "Artist".into(),
        album: Some("Album".into()),
        album_artist: Some("Album artist".into()),
        genre: Some("Genre".into()),
        year: Some(2026),
        track_number: Some(2),
        disc_number: Some(1),
        lyrics: Some(EmbeddedLyrics {
            text: TEST_LYRICS.into(),
            synchronized: true,
        }),
    };
    write_metadata(
        &backend,
        &audio,
        &metadata,
        Some(tokio::fs::read(&cover).await.unwrap()),
    )
    .await
    .unwrap();
    verify_audio_file(&backend, &audio, "m4a").await.unwrap();
    assert_embedded_lyrics(&audio).await;
    assert_duration_preserved(original_duration, probe_duration(&audio).await);
    assert_rejects_truncated_audio(&backend, &audio, "m4a").await;

    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type:format_tags=title,album_artist",
            "-of",
            "json",
        ])
        .arg(&audio)
        .output()
        .await
        .unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = probe["streams"].as_array().unwrap();
    assert_eq!(
        streams
            .iter()
            .filter(|value| value["codec_type"] == "audio")
            .count(),
        1
    );
    assert_eq!(
        streams
            .iter()
            .filter(|value| value["codec_type"] == "video")
            .count(),
        1
    );
    assert_eq!(probe["format"]["tags"]["title"], "Title");
    assert_eq!(probe["format"]["tags"]["album_artist"], "Album artist");
    tokio::fs::remove_dir_all(directory).await.unwrap();
}

async fn exercise_flac_remux<B: MediaBackend>(backend: B) {
    let directory = temporary_directory(backend.name());
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let source = directory.join("source.m4a");
    let destination = directory.join("track.flac");
    require_command("ffmpeg").await;
    require_command("ffprobe").await;
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=2",
        "-c:a",
        "flac",
        "-strict",
        "experimental",
        "-metadata",
        "title=Original title",
        "-f",
        "mp4",
        source.to_str().unwrap(),
    ])
    .await;
    let original_duration = probe_duration(&source).await;
    backend
        .remux_flac(source, destination.clone(), false)
        .await
        .unwrap();
    write_metadata(
        &backend,
        &destination,
        &TrackMetadata {
            title: "Title".into(),
            artist: "Artist".into(),
            lyrics: Some(EmbeddedLyrics {
                text: TEST_LYRICS.into(),
                synchronized: true,
            }),
            ..TrackMetadata::default()
        },
        None,
    )
    .await
    .unwrap();
    verify_audio_file(&backend, &destination, "flac")
        .await
        .unwrap();
    assert_embedded_lyrics(&destination).await;
    assert_duration_preserved(original_duration, probe_duration(&destination).await);
    assert_rejects_truncated_audio(&backend, &destination, "flac").await;
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name:format_tags=title",
            "-of",
            "json",
        ])
        .arg(&destination)
        .output()
        .await
        .unwrap();
    assert!(probe.status.success());
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(probe["streams"][0]["codec_name"], "flac");
    assert_eq!(probe["format"]["tags"]["TITLE"], "Title");
    tokio::fs::remove_dir_all(directory).await.unwrap();
}

async fn exercise_mp3_transcode<B: MediaBackend>(backend: B) {
    let directory = temporary_directory(backend.name());
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let source = directory.join("source.m4a");
    let destination = directory.join("track.mp3");
    require_command("ffmpeg").await;
    require_command("ffprobe").await;
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=96000:duration=1",
        "-c:a",
        "aac",
        "-f",
        "ipod",
        source.to_str().unwrap(),
    ])
    .await;
    backend
        .transcode_mp3(source, destination.clone(), 320, false)
        .await
        .unwrap();
    write_metadata(
        &backend,
        &destination,
        &TrackMetadata {
            title: "Title".into(),
            artist: "Artist".into(),
            lyrics: Some(EmbeddedLyrics {
                text: TEST_LYRICS.into(),
                synchronized: true,
            }),
            ..TrackMetadata::default()
        },
        None,
    )
    .await
    .unwrap();
    verify_audio_file(&backend, &destination, "mp3")
        .await
        .unwrap();
    assert_embedded_lyrics(&destination).await;
    assert_mp3_full_decode_rejects_invalid_stream(&backend, &destination).await;
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(&destination)
        .output()
        .await
        .unwrap();
    assert!(probe.status.success());
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(probe["streams"][0]["codec_name"], "mp3");
    assert_eq!(probe["streams"][0]["sample_rate"], "48000");
    let duration = probe["format"]["duration"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(
        (0.9..1.2).contains(&duration),
        "unexpected duration: {duration}"
    );
    tokio::fs::remove_dir_all(directory).await.unwrap();
}

async fn exercise_vbr_mp3_without_xing<B: MediaBackend>(backend: B) {
    use lofty::file::AudioFile as _;

    let directory = temporary_directory(backend.name());
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let audio = directory.join("vbr-without-xing.mp3");
    require_command("ffmpeg").await;
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "anoisesrc=d=5:c=pink:r=44100:a=0.5",
        "-c:a",
        "libmp3lame",
        "-q:a",
        "4",
        "-write_xing",
        "0",
        "-f",
        "mp3",
        audio.to_str().unwrap(),
    ])
    .await;

    let bytes = tokio::fs::read(&audio).await.unwrap();
    let header = &bytes[..bytes.len().min(4096)];
    assert!(
        !header
            .windows(4)
            .any(|window| window == b"Xing" || window == b"Info" || window == b"VBRI"),
        "fixture unexpectedly contains an authoritative VBR frame count"
    );
    let shallow_duration = lofty::read_from_path(&audio)
        .unwrap()
        .properties()
        .duration();
    assert!(
        shallow_duration.abs_diff(Duration::from_secs(5)) > Duration::from_millis(250),
        "fixture does not exercise the inaccurate bitrate-based MP3 duration estimate"
    );

    verify_audio_file(&backend, &audio, "mp3").await.unwrap();
    tokio::fs::remove_dir_all(directory).await.unwrap();
}

async fn require_command(command: &str) {
    let available = tokio::process::Command::new(command)
        .arg("-version")
        .output()
        .await
        .is_ok_and(|output| output.status.success());
    assert!(
        available,
        "{command} is required to execute the media backend tests"
    );
}

async fn probe_duration(path: &Path) -> f64 {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn assert_duration_preserved(expected: f64, actual: f64) {
    let difference = (actual - expected).abs();
    assert!(
        difference <= 0.002,
        "duration changed from {expected:.6}s to {actual:.6}s"
    );
}

async fn assert_rejects_truncated_audio<B: MediaBackend>(
    backend: &B,
    original: &Path,
    extension: &str,
) {
    use lofty::file::AudioFile as _;

    let truncated = original.with_extension(format!("truncated.{extension}"));
    tokio::fs::copy(original, &truncated).await.unwrap();
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&truncated)
        .await
        .unwrap();
    let length = file.metadata().await.unwrap().len();
    file.set_len(length / 2).await.unwrap();
    drop(file);

    let parsed = lofty::read_from_path(&truncated)
        .expect("truncated fixture should pass the shallow container parser");
    assert!(!parsed.properties().duration().is_zero());
    verify_audio_file(backend, &truncated, extension)
        .await
        .expect_err("verifier accepted a truncated audio stream");
}

async fn assert_mp3_full_decode_rejects_invalid_stream<B: MediaBackend>(
    backend: &B,
    original: &Path,
) {
    let invalid = original.with_extension("invalid.mp3");
    tokio::fs::write(&invalid, b"not an MPEG audio frame".repeat(128))
        .await
        .unwrap();
    backend
        .verify_audio(invalid, None)
        .await
        .expect_err("MP3 backend accepted an invalid stream without decoding it");
}

async fn assert_embedded_lyrics(path: &Path) {
    let path = path.to_owned();
    let lyrics = tokio::task::spawn_blocking(move || {
        use lofty::{file::TaggedFileExt, tag::ItemKey};

        let file = lofty::read_from_path(&path).unwrap();
        let tag = file
            .primary_tag()
            .or_else(|| file.first_tag())
            .expect("audio file should contain a metadata tag");
        tag.get_string(ItemKey::Lyrics)
            .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
            .map(str::to_owned)
    })
    .await
    .unwrap();
    assert_eq!(lyrics.as_deref(), Some(TEST_LYRICS));
}

async fn run_ffmpeg(arguments: &[&str]) {
    let output = tokio::process::Command::new("ffmpeg")
        .args(["-nostdin", "-y", "-v", "error"])
        .args(arguments)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temporary_directory(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "yandex-music-media-{label}-{}-{nonce}",
        std::process::id()
    ))
}
