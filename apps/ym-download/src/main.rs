use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::AsyncWriteExt;
use tokio::{process::Command as TokioCommand, sync::Semaphore, task::JoinSet};
use yandex_music_api::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    models::{DownloadInfo, DownloadOptions, DownloadQuality, Id},
};

#[derive(Debug, Parser)]
#[command(about = "Download tracks and playlists from Yandex Music")]
struct Cli {
    /// Credential profile created by ym-auth.
    #[arg(long, default_value = DEFAULT_PROFILE)]
    profile: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download one track.
    Track {
        /// Numeric Yandex Music track ID.
        track_id: String,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, value_enum, default_value_t = Quality::Lossless)]
        quality: Quality,
        /// Destination path; defaults to `track-id.negotiated-extension`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Replace an existing destination file.
        #[arg(long)]
        force: bool,
    },
    /// Download every track from a playlist in playlist order.
    Playlist {
        /// Playlist owner UID or login.
        owner: String,
        /// Playlist kind.
        kind: String,
        /// Destination directory; defaults to a sanitized playlist title.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, value_enum, default_value_t = Quality::Lossless)]
        quality: Quality,
        /// Replace existing destination files.
        #[arg(long)]
        force: bool,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum Quality {
    Low,
    Normal,
    #[default]
    Lossless,
}

impl From<Quality> for DownloadQuality {
    fn from(value: Quality) -> Self {
        match value {
            Quality::Low => Self::Low,
            Quality::Normal => Self::Normal,
            Quality::Lossless => Self::Lossless,
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = CredentialStore::open_default().context("failed to open credential store")?;
    let auth = DeviceAuth::new().context("failed to create OAuth client")?;
    let resolved = store
        .resolve(&cli.profile, &auth, RefreshPolicy::default())
        .await
        .with_context(|| {
            format!(
                "failed to load profile {:?}; run `ym-auth login`",
                cli.profile
            )
        })?;
    let credentials = resolved.credentials;
    if credentials.is_expired()? {
        bail!(
            "profile {:?} has expired; run `ym-auth login --force`",
            cli.profile
        );
    }

    let client = Client::new(credentials.access_token())?;
    let uid = current_account_uid(&client).await?;
    match cli.command {
        Command::Track {
            track_id,
            quality,
            output,
            force,
        } => download_track(&client, uid, &track_id, quality, output, force).await,
        Command::Playlist {
            owner,
            kind,
            output,
            quality,
            force,
            jobs,
        } => {
            download_playlist(
                &client,
                uid,
                PlaylistDownloadRequest {
                    owner,
                    kind,
                    quality,
                    output,
                    force,
                    jobs,
                },
            )
            .await
        }
    }
}

async fn download_track(
    client: &Client,
    uid: Id,
    track_id: &str,
    quality: Quality,
    output: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    let info = negotiate(client, uid, track_id, quality).await?;
    let destination = match output {
        Some(path) => validate_output_extension(path, &info)?,
        None => PathBuf::from(format!(
            "{}.{}",
            safe_file_component(track_id),
            normalized_extension(&info)
        )),
    };
    download_normalized(client, &info, &destination, force, true).await?;
    println!(
        "saved {} ({} {}, {} kbps)",
        destination.display(),
        info.quality,
        info.codec,
        info.bitrate
    );
    Ok(())
}

struct PlaylistDownloadRequest {
    owner: String,
    kind: String,
    quality: Quality,
    output: Option<PathBuf>,
    force: bool,
    jobs: u8,
}

async fn download_playlist(
    client: &Client,
    uid: Id,
    request: PlaylistDownloadRequest,
) -> Result<()> {
    let PlaylistDownloadRequest {
        owner,
        kind,
        quality,
        output,
        force,
        jobs,
    } = request;
    let playlist = client.playlist(owner.as_str(), kind.as_str()).await?;
    let directory = output.unwrap_or_else(|| {
        PathBuf::from(safe_file_component(
            playlist.title.as_deref().unwrap_or("playlist"),
        ))
    });
    tokio::fs::create_dir_all(&directory).await?;
    let width = playlist.tracks.len().to_string().len().max(2);

    let semaphore = Arc::new(Semaphore::new(jobs as usize));
    let mut tasks = JoinSet::new();
    let total = playlist.tracks.len();
    for (index, short) in playlist.tracks.into_iter().enumerate() {
        let track = short
            .track
            .as_ref()
            .with_context(|| format!("playlist entry {} has no full track metadata", index + 1))?;
        let artists = track
            .artists
            .iter()
            .filter_map(|artist| artist.name.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        let title = track.title.as_deref().unwrap_or("Untitled").to_owned();
        let stem = format!(
            "{:0width$} - {} - {}",
            index + 1,
            safe_file_component(if artists.is_empty() {
                "Unknown artist"
            } else {
                &artists
            }),
            safe_file_component(&title),
        );
        let job = PlaylistJob {
            index: index + 1,
            total,
            track_id: short.id.to_string(),
            label: format!(
                "{} — {}",
                if artists.is_empty() {
                    "Unknown artist"
                } else {
                    &artists
                },
                title
            ),
            stem,
            directory: directory.clone(),
        };
        let client = client.clone();
        let uid = uid.clone();
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.expect("semaphore is open");
            download_playlist_track(&client, uid, quality, force, job).await
        });
    }

    let mut downloaded = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let outcome = result.context("playlist download task panicked")?;
        match outcome.result {
            Ok(PlaylistTrackStatus::Downloaded {
                path,
                quality,
                codec,
            }) => {
                downloaded += 1;
                println!(
                    "[{}/{}] downloaded {} ({} {})",
                    outcome.index,
                    outcome.total,
                    path.display(),
                    quality,
                    codec
                );
            }
            Ok(PlaylistTrackStatus::Skipped { path }) => {
                skipped += 1;
                println!(
                    "[{}/{}] skipped existing {}",
                    outcome.index,
                    outcome.total,
                    path.display()
                );
            }
            Err(error) => {
                eprintln!(
                    "[{}/{}] failed {}: {error}",
                    outcome.index, outcome.total, outcome.label
                );
                failures.push((outcome.index, outcome.label, error));
            }
        }
    }

    failures.sort_by_key(|failure| failure.0);
    println!(
        "playlist summary: {downloaded} downloaded, {skipped} skipped, {} failed",
        failures.len()
    );
    for (index, label, error) in &failures {
        eprintln!("  {index:0width$}. {label}: {error}");
    }
    if !failures.is_empty() {
        bail!("playlist completed with {} failed tracks", failures.len());
    }
    Ok(())
}

struct PlaylistJob {
    index: usize,
    total: usize,
    track_id: String,
    label: String,
    stem: String,
    directory: PathBuf,
}

struct PlaylistOutcome {
    index: usize,
    total: usize,
    label: String,
    result: std::result::Result<PlaylistTrackStatus, String>,
}

enum PlaylistTrackStatus {
    Downloaded {
        path: PathBuf,
        quality: String,
        codec: yandex_music_api::models::AudioCodec,
    },
    Skipped {
        path: PathBuf,
    },
}

async fn download_playlist_track(
    client: &Client,
    uid: Id,
    quality: Quality,
    force: bool,
    job: PlaylistJob,
) -> PlaylistOutcome {
    let result = async {
        let info = negotiate(client, uid, &job.track_id, quality).await?;
        let destination =
            job.directory
                .join(format!("{}.{}", job.stem, normalized_extension(&info)));
        if tokio::fs::try_exists(&destination).await? && !force {
            return Ok(PlaylistTrackStatus::Skipped { path: destination });
        }
        download_normalized(client, &info, &destination, force, false).await?;
        Ok(PlaylistTrackStatus::Downloaded {
            path: destination,
            quality: info.quality,
            codec: info.codec,
        })
    }
    .await
    .map_err(|error: anyhow::Error| format!("{error:#}"));

    PlaylistOutcome {
        index: job.index,
        total: job.total,
        label: job.label,
        result,
    }
}

async fn negotiate(
    client: &Client,
    uid: Id,
    track_id: &str,
    quality: Quality,
) -> Result<DownloadInfo> {
    let options = DownloadOptions {
        quality: quality.into(),
        ..DownloadOptions::default()
    };
    let info = client.download_info(uid, track_id, &options).await?;
    if info.decryption_key.is_some() {
        bail!("the server returned encrypted audio for a raw transport request");
    }
    Ok(info)
}

async fn current_account_uid(client: &Client) -> Result<Id> {
    client
        .account_status()
        .await?
        .account
        .and_then(|account| account.uid)
        .context("account status response does not contain a uid")
}

async fn download_to_file(
    client: &Client,
    info: &DownloadInfo,
    destination: &Path,
    force: bool,
    show_progress: bool,
) -> Result<()> {
    if tokio::fs::try_exists(destination).await? && !force {
        bail!(
            "destination {} already exists; pass --force to replace it",
            destination.display()
        );
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .context("destination must contain a file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.part-{}", std::process::id()));
    let _ = tokio::fs::remove_file(&temporary).await;

    let result = try_download_urls(client, info, &temporary, show_progress).await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    #[cfg(windows)]
    if force && tokio::fs::try_exists(destination).await? {
        tokio::fs::remove_file(destination).await?;
    }
    tokio::fs::rename(&temporary, destination)
        .await
        .with_context(|| {
            format!(
                "failed to move completed download to {}",
                destination.display()
            )
        })?;
    Ok(())
}

async fn download_normalized(
    client: &Client,
    info: &DownloadInfo,
    destination: &Path,
    force: bool,
    show_progress: bool,
) -> Result<()> {
    if matches!(info.codec, yandex_music_api::models::AudioCodec::FlacMp4) {
        let source = sibling_temporary(destination, "source.m4a");
        let result = async {
            download_to_file(client, info, &source, true, show_progress).await?;
            remux_flac(&source, destination, force).await
        }
        .await;
        let _ = tokio::fs::remove_file(&source).await;
        result
    } else {
        download_to_file(client, info, destination, force, show_progress).await
    }
}

async fn remux_flac(source: &Path, destination: &Path, force: bool) -> Result<()> {
    if tokio::fs::try_exists(destination).await? && !force {
        bail!("destination {} already exists", destination.display());
    }
    let temporary = sibling_temporary(destination, "remux.part");
    let output = TokioCommand::new("ffmpeg")
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
        .context("failed to run ffmpeg; install it to normalize FLAC-in-MP4")?;
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&temporary).await;
        bail!(
            "ffmpeg remux failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    tokio::fs::File::open(&temporary).await?.sync_all().await?;
    #[cfg(windows)]
    if force && tokio::fs::try_exists(destination).await? {
        tokio::fs::remove_file(destination).await?;
    }
    tokio::fs::rename(&temporary, destination).await?;
    Ok(())
}

fn sibling_temporary(destination: &Path, suffix: &str) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    parent.join(format!(".{name}.{suffix}-{}", std::process::id()))
}

fn normalized_extension(info: &DownloadInfo) -> &'static str {
    use yandex_music_api::models::AudioCodec;
    match &info.codec {
        AudioCodec::Flac | AudioCodec::FlacMp4 => "flac",
        AudioCodec::Aac | AudioCodec::HeAac | AudioCodec::AacMp4 | AudioCodec::HeAacMp4 => "m4a",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Other(_) => "bin",
        _ => "bin",
    }
}

fn validate_output_extension(mut output: PathBuf, info: &DownloadInfo) -> Result<PathBuf> {
    let expected = normalized_extension(info);
    match output.extension().and_then(|extension| extension.to_str()) {
        Some(actual) if !actual.eq_ignore_ascii_case(expected) => bail!(
            "output extension .{actual} does not match negotiated {} audio; use .{expected}",
            info.codec
        ),
        None => {
            output.set_extension(expected);
            Ok(output)
        }
        Some(_) => Ok(output),
    }
}

async fn try_download_urls(
    client: &Client,
    info: &DownloadInfo,
    temporary: &Path,
    show_progress: bool,
) -> Result<()> {
    let mut failures = Vec::new();
    for url in &info.urls {
        match download_url(client, url, temporary, show_progress).await {
            Ok(()) => return Ok(()),
            Err(error) => failures.push(error.to_string()),
        }
    }
    bail!(
        "all {} CDN URLs failed: {}",
        info.urls.len(),
        failures.join("; ")
    )
}

async fn download_url(
    client: &Client,
    url: &url::Url,
    temporary: &Path,
    show_progress: bool,
) -> Result<()> {
    let mut response = client.open_audio_stream(url).await?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(temporary).await?;
    let mut downloaded = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        let finished = total.is_some_and(|total| downloaded >= total);
        if show_progress && (finished || last_progress.elapsed() >= Duration::from_millis(200)) {
            match total {
                Some(total) => eprint!("\r{downloaded}/{total} bytes"),
                None => eprint!("\r{downloaded} bytes"),
            }
            io::stderr().flush()?;
            last_progress = Instant::now();
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    if show_progress {
        eprintln!();
    }
    Ok(())
}

fn safe_file_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}
