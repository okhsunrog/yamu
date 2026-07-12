use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::AsyncWriteExt;
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
        } => download_playlist(&client, uid, &owner, &kind, quality, output, force).await,
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
    let destination = output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}.{}",
            safe_file_component(track_id),
            info.codec.file_extension()
        ))
    });
    download_to_file(client, &info, &destination, force).await?;
    println!(
        "saved {} ({} {}, {} kbps)",
        destination.display(),
        info.quality,
        info.codec,
        info.bitrate
    );
    Ok(())
}

async fn download_playlist(
    client: &Client,
    uid: Id,
    owner: &str,
    kind: &str,
    quality: Quality,
    output: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    let playlist = client.playlist(owner, kind).await?;
    let directory = output.unwrap_or_else(|| {
        PathBuf::from(safe_file_component(
            playlist.title.as_deref().unwrap_or("playlist"),
        ))
    });
    tokio::fs::create_dir_all(&directory).await?;
    let width = playlist.tracks.len().to_string().len().max(2);

    for (index, short) in playlist.tracks.iter().enumerate() {
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
        let title = track.title.as_deref().unwrap_or("Untitled");
        let info = negotiate(client, uid.clone(), &short.id.to_string(), quality).await?;
        let stem = format!(
            "{:0width$} - {} - {}",
            index + 1,
            safe_file_component(if artists.is_empty() {
                "Unknown artist"
            } else {
                &artists
            }),
            safe_file_component(title),
        );
        let destination = directory.join(format!("{stem}.{}", info.codec.file_extension()));
        download_to_file(client, &info, &destination, force).await?;
        println!(
            "[{}/{}] saved {} ({} {})",
            index + 1,
            playlist.tracks.len(),
            destination.display(),
            info.quality,
            info.codec
        );
    }
    Ok(())
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

    let result = try_download_urls(client, info, &temporary).await;
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

async fn try_download_urls(client: &Client, info: &DownloadInfo, temporary: &Path) -> Result<()> {
    let mut failures = Vec::new();
    for url in &info.urls {
        match download_url(client, url, temporary).await {
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

async fn download_url(client: &Client, url: &url::Url, temporary: &Path) -> Result<()> {
    let mut response = client.open_audio_stream(url).await?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(temporary).await?;
    let mut downloaded = 0_u64;
    let mut last_progress = Instant::now() - Duration::from_secs(1);

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        let finished = total.is_some_and(|total| downloaded >= total);
        if finished || last_progress.elapsed() >= Duration::from_millis(200) {
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
    eprintln!();
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
