use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use tokio::io::AsyncWriteExt;
use yandex_music_api::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    models::{DownloadInfo, DownloadOptions, DownloadQuality, Id},
};

#[derive(Debug, Parser)]
#[command(about = "Download one Yandex Music track to an atomic local file")]
struct Cli {
    /// Credential profile created by ym-auth.
    #[arg(long, default_value = DEFAULT_PROFILE)]
    profile: String,

    /// Highest requested quality; the server may return a lower tier.
    #[arg(long, value_enum, default_value_t = Quality::Lossless)]
    quality: Quality,

    /// Destination path; defaults to `track-id.negotiated-extension`.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Replace an existing destination file.
    #[arg(long)]
    force: bool,

    /// Numeric Yandex Music track ID.
    track_id: String,
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
    let options = DownloadOptions {
        quality: cli.quality.into(),
        ..DownloadOptions::default()
    };
    let info = client
        .download_info(uid, cli.track_id.as_str(), &options)
        .await?;
    if info.decryption_key.is_some() {
        bail!("the server returned encrypted audio for a raw transport request");
    }

    let destination = cli.output.unwrap_or_else(|| {
        PathBuf::from(format!(
            "{}.{}",
            safe_file_stem(&cli.track_id),
            info.codec.file_extension()
        ))
    });
    download_to_file(&client, &info, &destination, cli.force).await?;
    println!(
        "saved {} ({} {}, {} kbps)",
        destination.display(),
        info.quality,
        info.codec,
        info.bitrate
    );
    Ok(())
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

fn safe_file_stem(track_id: &str) -> String {
    track_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}
