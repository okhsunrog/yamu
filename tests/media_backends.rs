#![cfg(any(feature = "media-ffmpeg-cli", feature = "media-ffmpeg"))]

use std::time::{SystemTime, UNIX_EPOCH};

use yandex_music_api::media::{MediaBackend, TrackMetadata, verify_audio_file, write_metadata};

#[cfg(feature = "media-ffmpeg")]
use yandex_music_api::media::ffmpeg::Ffmpeg;
#[cfg(feature = "media-ffmpeg-cli")]
use yandex_music_api::media::ffmpeg_cli::FfmpegCli;

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

async fn exercise_m4a_backend<B: MediaBackend>(backend: B) {
    let directory = temporary_directory(backend.name());
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let audio = directory.join("track.m4a");
    let cover = directory.join("cover.jpg");
    if !command_available("ffmpeg").await {
        tokio::fs::remove_dir_all(directory).await.unwrap();
        return;
    }
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=0.1",
        "-c:a",
        "aac",
        "-f",
        "ipod",
        audio.to_str().unwrap(),
    ])
    .await;
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
        lyrics: None,
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
    if !command_available("ffmpeg").await {
        tokio::fs::remove_dir_all(directory).await.unwrap();
        return;
    }
    run_ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=0.1",
        "-c:a",
        "flac",
        "-strict",
        "experimental",
        "-f",
        "mp4",
        source.to_str().unwrap(),
    ])
    .await;
    backend
        .remux_flac(source, destination.clone(), false)
        .await
        .unwrap();
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(&destination)
        .output()
        .await
        .unwrap();
    assert!(probe.status.success());
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "flac");
    tokio::fs::remove_dir_all(directory).await.unwrap();
}

async fn command_available(command: &str) -> bool {
    tokio::process::Command::new(command)
        .arg("-version")
        .output()
        .await
        .is_ok()
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
