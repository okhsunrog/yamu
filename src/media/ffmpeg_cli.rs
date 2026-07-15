//! Media backend invoking an installed `ffmpeg` executable.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::process::Command;

use super::{Error, MediaBackend, Result, TrackMetadata, detect_mime, ensure_decoded_duration};
use crate::atomic_file;

static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default)]
pub struct FfmpegCli;

impl MediaBackend for FfmpegCli {
    fn name(&self) -> &'static str {
        "ffmpeg-cli"
    }

    async fn write_m4a_metadata(
        &self,
        path: PathBuf,
        metadata: TrackMetadata,
        artwork: Option<Vec<u8>>,
    ) -> Result<()> {
        write_m4a_metadata(&path, &metadata, artwork).await
    }

    async fn remux_flac(&self, source: PathBuf, destination: PathBuf, replace: bool) -> Result<()> {
        remux_flac(&source, &destination, replace).await
    }

    async fn transcode_mp3(
        &self,
        source: PathBuf,
        destination: PathBuf,
        bitrate_kbps: u32,
        replace: bool,
    ) -> Result<()> {
        transcode_mp3(&source, &destination, bitrate_kbps, replace).await
    }

    async fn verify_audio(&self, path: PathBuf, expected_duration: Option<Duration>) -> Result<()> {
        let actual_duration = decoded_duration(&path).await?;
        match expected_duration {
            Some(expected_duration) => ensure_decoded_duration(expected_duration, actual_duration),
            None => Ok(()),
        }
    }

    async fn verify_m4a(&self, path: PathBuf) -> Result<()> {
        decoded_duration(&path).await.map(|_| ())
    }
}

async fn decoded_duration(path: &Path) -> Result<Duration> {
    let output = Command::new("ffmpeg")
        .arg("-nostdin")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-progress",
            "pipe:1",
            "-nostats",
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .map_err(|error| {
            backend(format!(
                "failed to run ffmpeg while validating audio: {error}"
            ))
        })?;
    ensure_success_ref(&output, "audio decode")?;
    let progress = String::from_utf8_lossy(&output.stdout);
    let microseconds = progress
        .lines()
        .filter_map(|line| line.strip_prefix("out_time_us="))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .ok_or_else(|| backend("ffmpeg audio decode did not report a duration"))?;
    Ok(Duration::from_micros(microseconds))
}

async fn write_m4a_metadata(
    path: &Path,
    metadata: &TrackMetadata,
    artwork: Option<Vec<u8>>,
) -> Result<()> {
    if path.file_name().is_none() {
        return Err(backend("M4A path must contain a file name"));
    }
    let output_path = sibling_temporary(path, "metadata.m4a");
    let picture_path = artwork.as_ref().map(|picture| {
        let extension = match detect_mime(picture) {
            lofty::picture::MimeType::Png => "png",
            _ => "jpg",
        };
        sibling_temporary(path, &format!("cover.{extension}"))
    });
    let result = async {
        if let (Some(picture_path), Some(picture)) = (&picture_path, &artwork) {
            tokio::fs::write(picture_path, picture).await?;
        }

        let mut command = Command::new("ffmpeg");
        command
            .arg("-nostdin")
            .arg("-y")
            .args(["-v", "error", "-i"])
            .arg(path);
        if let Some(picture_path) = &picture_path {
            command.arg("-i").arg(picture_path);
        }
        command.args(["-map", "0:a:0", "-map_metadata", "-1", "-c:a", "copy"]);
        if picture_path.is_some() {
            command.args([
                "-map",
                "1:v:0",
                "-c:v",
                "copy",
                "-disposition:v:0",
                "attached_pic",
            ]);
        }
        push_metadata(&mut command, "title", Some(&metadata.title));
        push_metadata(&mut command, "artist", Some(&metadata.artist));
        push_metadata(&mut command, "album", metadata.album.as_deref());
        push_metadata(
            &mut command,
            "album_artist",
            metadata.album_artist.as_deref(),
        );
        push_metadata(&mut command, "genre", metadata.genre.as_deref());
        let year = metadata.year.map(|value| value.to_string());
        push_metadata(&mut command, "date", year.as_deref());
        let track = metadata.track_number.map(|value| value.to_string());
        push_metadata(&mut command, "track", track.as_deref());
        let disc = metadata.disc_number.map(|value| value.to_string());
        push_metadata(&mut command, "disc", disc.as_deref());
        push_metadata(
            &mut command,
            "lyrics",
            metadata.lyrics.as_ref().map(|value| value.text.as_str()),
        );
        command
            .args(["-movflags", "+faststart", "-f", "ipod"])
            .arg(&output_path);

        let output = command
            .output()
            .await
            .map_err(|error| backend(format!("failed to run ffmpeg for M4A tags: {error}")))?;
        ensure_success(output, "ffmpeg M4A metadata remux")?;
        tokio::fs::File::open(&output_path)
            .await?
            .sync_all()
            .await?;
        replace_file(&output_path, path, true)
    }
    .await;
    if let Some(picture_path) = &picture_path {
        let _ = tokio::fs::remove_file(picture_path).await;
    }
    if result.is_err() {
        let _ = tokio::fs::remove_file(&output_path).await;
    }
    result
}

async fn remux_flac(source: &Path, destination: &Path, replace: bool) -> Result<()> {
    if tokio::fs::try_exists(destination).await? && !replace {
        return Err(backend(format!(
            "destination {} already exists",
            destination.display()
        )));
    }
    let temporary = sibling_temporary(destination, "remux.part");
    let result = async {
        let output = Command::new("ffmpeg")
            .arg("-nostdin")
            .args(["-v", "error", "-i"])
            .arg(source)
            .args([
                "-map",
                "0:a:0",
                "-map_metadata",
                "0",
                "-c:a",
                "copy",
                "-f",
                "flac",
            ])
            .arg(&temporary)
            .output()
            .await
            .map_err(|error| backend(format!("failed to run ffmpeg for FLAC remux: {error}")))?;
        ensure_success(output, "ffmpeg FLAC remux")?;
        tokio::fs::File::open(&temporary).await?.sync_all().await?;
        replace_file(&temporary, destination, replace)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

async fn transcode_mp3(
    source: &Path,
    destination: &Path,
    bitrate_kbps: u32,
    replace: bool,
) -> Result<()> {
    if tokio::fs::try_exists(destination).await? && !replace {
        return Err(backend(format!(
            "destination {} already exists",
            destination.display()
        )));
    }
    let temporary = sibling_temporary(destination, "transcode.mp3");
    let result = async {
        let output = Command::new("ffmpeg")
            .arg("-nostdin")
            .args(["-v", "error", "-i"])
            .arg(source)
            .args([
                "-map",
                "0:a:0",
                "-map_metadata",
                "-1",
                "-c:a",
                "libmp3lame",
                "-b:a",
                &format!("{bitrate_kbps}k"),
                "-f",
                "mp3",
            ])
            .arg(&temporary)
            .output()
            .await
            .map_err(|error| backend(format!("failed to run ffmpeg for MP3 transcode: {error}")))?;
        ensure_success(output, "ffmpeg MP3 transcode")?;
        tokio::fs::File::open(&temporary).await?.sync_all().await?;
        replace_file(&temporary, destination, replace)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

fn push_metadata(command: &mut Command, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        command.arg("-metadata").arg(format!("{key}={value}"));
    }
}

fn ensure_success(output: std::process::Output, operation: &str) -> Result<()> {
    ensure_success_ref(&output, operation)
}

fn ensure_success_ref(output: &std::process::Output, operation: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ");
    Err(backend(format!("{operation} failed: {stderr}")))
}

fn replace_file(source: &Path, destination: &Path, replace: bool) -> Result<()> {
    atomic_file::persist(source, destination, replace).map_err(Error::from)
}

fn sibling_temporary(destination: &Path, suffix: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{name}.{suffix}-{}-{nonce}", std::process::id()))
}

fn backend(message: impl Into<String>) -> Error {
    Error::Backend(message.into())
}
