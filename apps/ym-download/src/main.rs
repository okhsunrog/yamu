use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use tokio::io::AsyncWriteExt;
use tokio::{process::Command as TokioCommand, sync::Semaphore, task::JoinSet};
use yandex_music_api::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    models::{Album, DownloadInfo, DownloadOptions, DownloadQuality, Id, LyricsFormat, Track},
    resource::{AlbumRef, ArtistRef, PlaylistRef, TrackRef},
};

mod metadata;
mod state;

use metadata::{ArtworkCache, EmbeddedLyrics, TrackMetadata, verify_audio_file, write_metadata};
use state::{CollectionStateStore, StateStatus};

#[derive(Debug, Parser)]
#[command(about = "Download tracks and collections from Yandex Music")]
struct Cli {
    /// Credential profile created by ym-auth.
    #[arg(long, default_value = DEFAULT_PROFILE)]
    profile: String,

    /// Save and embed lyrics; defaults to plain text when no format is given.
    #[arg(long, global = true, value_name = "FORMAT", num_args = 0..=1, default_missing_value = "text")]
    lyrics: Option<LyricsFormat>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download one track.
    Track {
        /// Numeric track ID or Yandex Music track URL.
        track: TrackRef,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Destination path; defaults to `artist - title.negotiated-extension`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Replace an existing destination file.
        #[arg(long)]
        force: bool,
    },
    /// Download every track from an album in disc and track order.
    Album {
        /// Numeric album ID or Yandex Music album URL.
        album: AlbumRef,
        /// Destination directory; defaults to `artist - album (year)`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Replace existing destination files.
        #[arg(long)]
        force: bool,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
    },
    /// Download tracks liked by the current account.
    Liked {
        /// Destination directory.
        #[arg(short, long, default_value = "Liked tracks")]
        output: PathBuf,
        /// Download at most this many tracks.
        #[arg(long)]
        limit: Option<usize>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Replace existing destination files.
        #[arg(long)]
        force: bool,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
    },
    /// Download an artist's complete track catalog.
    Artist {
        /// Numeric artist ID or Yandex Music artist URL.
        artist: ArtistRef,
        /// Destination directory; defaults to the artist name.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Download at most this many tracks.
        #[arg(long)]
        limit: Option<usize>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Replace existing destination files.
        #[arg(long)]
        force: bool,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
    },
    /// Download every track from a playlist in playlist order.
    Playlist {
        /// Playlist as owner:kind or a Yandex Music playlist URL.
        playlist: PlaylistRef,
        /// Destination directory; defaults to a sanitized playlist title.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Replace existing destination files.
        #[arg(long)]
        force: bool,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
    },
    /// Incrementally synchronize a changing collection.
    Sync {
        #[command(subcommand)]
        source: SyncCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SyncCommand {
    /// Synchronize a playlist with its current remote contents.
    Playlist {
        /// Playlist as owner:kind or a Yandex Music playlist URL.
        playlist: PlaylistRef,
        /// Destination directory; defaults to a sanitized playlist title.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
        /// Show planned changes without writing files or the manifest.
        #[arg(long)]
        dry_run: bool,
        /// Remove previously tracked audio files no longer present remotely.
        #[arg(long)]
        prune: bool,
    },
    /// Synchronize tracks liked by the current account.
    Liked {
        /// Destination directory.
        #[arg(short, long, default_value = "Liked tracks")]
        output: PathBuf,
        /// Download at most this many tracks.
        #[arg(long)]
        limit: Option<usize>,
        /// Highest requested quality; the server may return a lower tier.
        #[arg(long, default_value_t = DownloadQuality::Lossless)]
        quality: DownloadQuality,
        /// Maximum number of simultaneous track downloads.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=32))]
        jobs: u8,
        /// Show planned changes without writing files or the manifest.
        #[arg(long)]
        dry_run: bool,
        /// Remove previously tracked audio files no longer present remotely.
        #[arg(long)]
        prune: bool,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct SyncMode {
    dry_run: bool,
    prune: bool,
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
    let lyrics = cli.lyrics;
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
            track,
            quality,
            output,
            force,
        } => download_track(&client, uid, &track, quality, output, force, lyrics).await,
        Command::Album {
            album,
            output,
            quality,
            force,
            jobs,
        } => download_album(&client, uid, &album, quality, output, force, jobs, lyrics).await,
        Command::Liked {
            output,
            limit,
            quality,
            force,
            jobs,
        } => {
            download_liked(
                &client,
                uid,
                quality,
                output,
                limit,
                force,
                jobs,
                SyncMode::default(),
                lyrics,
            )
            .await
        }
        Command::Artist {
            artist,
            output,
            limit,
            quality,
            force,
            jobs,
        } => {
            download_artist(
                &client, uid, &artist, quality, output, limit, force, jobs, lyrics,
            )
            .await
        }
        Command::Playlist {
            playlist,
            output,
            quality,
            force,
            jobs,
        } => {
            download_playlist(
                &client,
                uid,
                PlaylistDownloadRequest {
                    playlist,
                    quality,
                    output,
                    force,
                    jobs,
                    sync: SyncMode::default(),
                    lyrics,
                },
            )
            .await
        }
        Command::Sync { source } => match source {
            SyncCommand::Playlist {
                playlist,
                output,
                quality,
                jobs,
                dry_run,
                prune,
            } => {
                download_playlist(
                    &client,
                    uid,
                    PlaylistDownloadRequest {
                        playlist,
                        quality,
                        output,
                        force: false,
                        jobs,
                        sync: SyncMode { dry_run, prune },
                        lyrics,
                    },
                )
                .await
            }
            SyncCommand::Liked {
                output,
                limit,
                quality,
                jobs,
                dry_run,
                prune,
            } => {
                download_liked(
                    &client,
                    uid,
                    quality,
                    output,
                    limit,
                    false,
                    jobs,
                    SyncMode { dry_run, prune },
                    lyrics,
                )
                .await
            }
        },
    }
}

async fn download_track(
    client: &Client,
    uid: Id,
    track: &TrackRef,
    quality: DownloadQuality,
    output: Option<PathBuf>,
    force: bool,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let track_id = track.track_id();
    let track = client
        .tracks([track_id])
        .await?
        .into_iter()
        .next()
        .context("track metadata was not returned")?;
    let metadata = TrackMetadata::from_track(&track);
    let artwork = ArtworkCache::new()?;
    let info = negotiate(client, uid, track_id, quality).await?;
    let destination = match output {
        Some(path) => validate_output_extension(path, &info)?,
        None => PathBuf::from(default_track_filename(
            &metadata,
            normalized_extension(&info),
        )),
    };
    if tokio::fs::try_exists(&destination).await? && !force {
        match verify_audio_file(&destination, normalized_extension(&info)).await {
            Ok(()) => {
                write_enriched_metadata(
                    client,
                    track_id,
                    &destination,
                    &metadata,
                    &artwork,
                    lyrics,
                )
                .await?;
                println!("verified existing {}", destination.display());
                return Ok(());
            }
            Err(error) => eprintln!(
                "existing {} is invalid ({error:#}); replacing it",
                destination.display()
            ),
        }
    }
    download_normalized(client, &info, &destination, true, true).await?;
    write_enriched_metadata(client, track_id, &destination, &metadata, &artwork, lyrics).await?;
    println!(
        "saved {} ({} {}, {} kbps)",
        destination.display(),
        info.quality,
        info.codec,
        info.bitrate
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_album(
    client: &Client,
    uid: Id,
    album_ref: &AlbumRef,
    quality: DownloadQuality,
    output: Option<PathBuf>,
    force: bool,
    jobs: u8,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let album = client.album_with_tracks(album_ref.album_id()).await?;
    let directory = output.unwrap_or_else(|| PathBuf::from(album_directory_name(&album)));
    tokio::fs::create_dir_all(&directory).await?;
    let downloads = album_download_jobs(&album, &directory)?;

    download_jobs(
        client,
        uid,
        quality,
        force,
        jobs,
        &directory,
        "album",
        album_ref.album_id(),
        downloads,
        SyncMode::default(),
        lyrics,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn download_liked(
    client: &Client,
    uid: Id,
    quality: DownloadQuality,
    directory: PathBuf,
    limit: Option<usize>,
    force: bool,
    jobs: u8,
    sync: SyncMode,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let library = client
        .liked_tracks(uid.clone(), 0)
        .await?
        .context("liked-track library was not returned")?;
    let mut tracks = client.tracks_from_list(&library).await?;
    if let Some(limit) = limit {
        tracks.truncate(limit);
    }
    tokio::fs::create_dir_all(&directory).await?;
    let downloads = ordered_track_jobs(&tracks, &directory);
    download_jobs(
        client,
        uid.clone(),
        quality,
        force,
        jobs,
        &directory,
        "liked",
        &uid.to_string(),
        downloads,
        sync,
        lyrics,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn download_artist(
    client: &Client,
    uid: Id,
    artist_ref: &ArtistRef,
    quality: DownloadQuality,
    output: Option<PathBuf>,
    limit: Option<usize>,
    force: bool,
    jobs: u8,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let artist = client
        .artists([artist_ref.artist_id()])
        .await?
        .into_iter()
        .next()
        .context("artist metadata was not returned")?;
    let directory = output.unwrap_or_else(|| {
        PathBuf::from(safe_file_component(
            artist.name.as_deref().unwrap_or("Unknown artist"),
        ))
    });
    let mut tracks = client.all_artist_tracks(artist_ref.artist_id()).await?;
    if let Some(limit) = limit {
        tracks.truncate(limit);
    }
    tokio::fs::create_dir_all(&directory).await?;
    let downloads = ordered_track_jobs(&tracks, &directory);
    download_jobs(
        client,
        uid,
        quality,
        force,
        jobs,
        &directory,
        "artist",
        artist_ref.artist_id(),
        downloads,
        SyncMode::default(),
        lyrics,
    )
    .await
}

struct PlaylistDownloadRequest {
    playlist: PlaylistRef,
    quality: DownloadQuality,
    output: Option<PathBuf>,
    force: bool,
    jobs: u8,
    sync: SyncMode,
    lyrics: Option<LyricsFormat>,
}

async fn download_playlist(
    client: &Client,
    uid: Id,
    request: PlaylistDownloadRequest,
) -> Result<()> {
    let PlaylistDownloadRequest {
        playlist,
        quality,
        output,
        force,
        jobs,
        sync,
        lyrics,
    } = request;
    let owner = playlist.owner().to_owned();
    let kind = playlist.kind().to_owned();
    let playlist = client.playlist(owner.as_str(), kind.as_str()).await?;
    let directory = output.unwrap_or_else(|| {
        PathBuf::from(safe_file_component(
            playlist.title.as_deref().unwrap_or("playlist"),
        ))
    });
    tokio::fs::create_dir_all(&directory).await?;
    let width = playlist.tracks.len().to_string().len().max(2);

    let total = playlist.tracks.len();
    let mut jobs_to_run = Vec::with_capacity(total);
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
        let job = DownloadJob {
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
            metadata: TrackMetadata::from_track(track),
        };
        jobs_to_run.push(job);
    }
    download_jobs(
        client,
        uid,
        quality,
        force,
        jobs,
        &directory,
        "playlist",
        &format!("{owner}:{kind}"),
        jobs_to_run,
        sync,
        lyrics,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn download_jobs(
    client: &Client,
    uid: Id,
    quality: DownloadQuality,
    force: bool,
    concurrency: u8,
    directory: &Path,
    source_kind: &str,
    source_id: &str,
    jobs: Vec<DownloadJob>,
    sync: SyncMode,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let plan = CollectionStateStore::plan(directory, source_kind, source_id, &jobs).await?;
    let untracked = jobs.len().saturating_sub(plan.known_paths);
    if sync.dry_run {
        println!(
            "sync plan: {} tracks, {} manifest-known, {} new or changed, {} stale",
            jobs.len(),
            plan.known_paths,
            untracked,
            plan.stale_paths.len()
        );
        for path in &plan.stale_paths {
            println!(
                "{} stale {}",
                if sync.prune {
                    "would prune"
                } else {
                    "would keep"
                },
                tracked_path(directory, path).display()
            );
        }
        return Ok(());
    }
    let stale_paths = plan.stale_paths;
    let state = CollectionStateStore::open(directory, source_kind, source_id, &jobs).await?;
    let semaphore = Arc::new(Semaphore::new(concurrency as usize));
    let artwork = ArtworkCache::new()?;
    let mut tasks = JoinSet::new();
    for job in jobs {
        let client = client.clone();
        let uid = uid.clone();
        let semaphore = Arc::clone(&semaphore);
        let artwork = artwork.clone();
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.expect("semaphore is open");
            download_collection_track(&client, uid, quality, force, lyrics, &artwork, job).await
        });
    }

    let mut downloaded = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();
    let mut total = 0;
    while let Some(result) = tasks.join_next().await {
        let outcome = result.context("collection download task panicked")?;
        total = outcome.total;
        match outcome.result {
            Ok(DownloadStatus::Downloaded {
                path,
                quality,
                codec,
            }) => {
                downloaded += 1;
                state
                    .record(outcome.index, StateStatus::Downloaded, Some(&path), None)
                    .await?;
                println!(
                    "[{}/{}] downloaded {} ({} {})",
                    outcome.index,
                    outcome.total,
                    path.display(),
                    quality,
                    codec
                );
            }
            Ok(DownloadStatus::Skipped { path }) => {
                skipped += 1;
                state
                    .record(outcome.index, StateStatus::Verified, Some(&path), None)
                    .await?;
                println!(
                    "[{}/{}] skipped existing {}",
                    outcome.index,
                    outcome.total,
                    path.display()
                );
            }
            Ok(DownloadStatus::Repaired { path, reason }) => {
                downloaded += 1;
                state
                    .record(
                        outcome.index,
                        StateStatus::Repaired,
                        Some(&path),
                        Some(&reason),
                    )
                    .await?;
                println!(
                    "[{}/{}] repaired {} ({reason})",
                    outcome.index,
                    outcome.total,
                    path.display()
                );
            }
            Err(error) => {
                state
                    .record(outcome.index, StateStatus::Failed, None, Some(&error))
                    .await?;
                eprintln!(
                    "[{}/{}] failed {}: {error}",
                    outcome.index, outcome.total, outcome.label
                );
                failures.push((outcome.index, outcome.label, error));
            }
        }
    }

    failures.sort_by_key(|failure| failure.0);
    let width = total.to_string().len().max(2);
    println!(
        "collection summary: {downloaded} downloaded, {skipped} skipped, {} failed",
        failures.len()
    );
    for (index, label, error) in &failures {
        eprintln!("  {index:0width$}. {label}: {error}");
    }
    if !failures.is_empty() {
        bail!("collection completed with {} failed tracks", failures.len());
    }
    if sync.prune {
        let mut pruned = 0;
        for path in stale_paths {
            if prune_tracked_audio(directory, &path).await? {
                pruned += 1;
            }
        }
        println!("sync prune: {pruned} stale files removed");
    } else if !stale_paths.is_empty() {
        println!(
            "sync kept {} stale tracked files; pass --prune to remove them",
            stale_paths.len()
        );
    }
    Ok(())
}

fn tracked_path(directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    }
}

async fn prune_tracked_audio(directory: &Path, path: &Path) -> Result<bool> {
    let candidate = tracked_path(directory, path);
    if !tokio::fs::try_exists(&candidate).await? {
        return Ok(false);
    }
    let root = tokio::fs::canonicalize(directory).await?;
    let candidate = tokio::fs::canonicalize(&candidate).await?;
    if !candidate.starts_with(&root) {
        bail!(
            "refusing to prune tracked path outside {}: {}",
            root.display(),
            candidate.display()
        );
    }
    let supported_audio = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "flac" | "m4a" | "mp3"));
    if !supported_audio {
        bail!(
            "refusing to prune non-audio tracked path {}",
            candidate.display()
        );
    }
    let mut sidecars = Vec::new();
    for extension in ["lrc", "txt"] {
        let sidecar = candidate.with_extension(extension);
        if tokio::fs::try_exists(&sidecar).await? {
            let sidecar = tokio::fs::canonicalize(sidecar).await?;
            if !sidecar.starts_with(&root) {
                bail!(
                    "refusing to prune lyrics sidecar outside {}: {}",
                    root.display(),
                    sidecar.display()
                );
            }
            sidecars.push(sidecar);
        }
    }
    tokio::fs::remove_file(&candidate).await?;
    println!("pruned {}", candidate.display());
    for sidecar in sidecars {
        tokio::fs::remove_file(&sidecar).await?;
        println!("pruned {}", sidecar.display());
    }
    Ok(true)
}

struct DownloadJob {
    index: usize,
    total: usize,
    track_id: String,
    label: String,
    stem: String,
    directory: PathBuf,
    metadata: TrackMetadata,
}

struct DownloadOutcome {
    index: usize,
    total: usize,
    label: String,
    result: std::result::Result<DownloadStatus, String>,
}

enum DownloadStatus {
    Downloaded {
        path: PathBuf,
        quality: String,
        codec: yandex_music_api::models::AudioCodec,
    },
    Skipped {
        path: PathBuf,
    },
    Repaired {
        path: PathBuf,
        reason: String,
    },
}

async fn download_collection_track(
    client: &Client,
    uid: Id,
    quality: DownloadQuality,
    force: bool,
    lyrics: Option<LyricsFormat>,
    artwork: &ArtworkCache,
    job: DownloadJob,
) -> DownloadOutcome {
    let result = async {
        tokio::fs::create_dir_all(&job.directory).await?;
        let info = negotiate(client, uid, &job.track_id, quality).await?;
        let destination =
            job.directory
                .join(format!("{}.{}", job.stem, normalized_extension(&info)));
        if tokio::fs::try_exists(&destination).await? && !force {
            match verify_audio_file(&destination, normalized_extension(&info)).await {
                Ok(()) => {
                    write_enriched_metadata(
                        client,
                        &job.track_id,
                        &destination,
                        &job.metadata,
                        artwork,
                        lyrics,
                    )
                    .await?;
                    return Ok(DownloadStatus::Skipped { path: destination });
                }
                Err(error) => {
                    let reason = format!("{error:#}");
                    download_normalized(client, &info, &destination, true, false).await?;
                    write_enriched_metadata(
                        client,
                        &job.track_id,
                        &destination,
                        &job.metadata,
                        artwork,
                        lyrics,
                    )
                    .await?;
                    return Ok(DownloadStatus::Repaired {
                        path: destination,
                        reason,
                    });
                }
            }
        }
        download_normalized(client, &info, &destination, force, false).await?;
        write_enriched_metadata(
            client,
            &job.track_id,
            &destination,
            &job.metadata,
            artwork,
            lyrics,
        )
        .await?;
        Ok(DownloadStatus::Downloaded {
            path: destination,
            quality: info.quality,
            codec: info.codec,
        })
    }
    .await
    .map_err(|error: anyhow::Error| format!("{error:#}"));

    DownloadOutcome {
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
    quality: DownloadQuality,
) -> Result<DownloadInfo> {
    let options = DownloadOptions {
        quality,
        ..DownloadOptions::default()
    };
    let mut attempt = 1_u8;
    let info = loop {
        match client.download_info(uid.clone(), track_id, &options).await {
            Ok(info) => break info,
            Err(error) if attempt < 3 && is_transient_api_error(&error) => {
                tokio::time::sleep(retry_delay(attempt)).await;
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    };
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

async fn write_enriched_metadata(
    client: &Client,
    track_id: &str,
    audio_path: &Path,
    metadata: &TrackMetadata,
    artwork: &ArtworkCache,
    lyrics_format: Option<LyricsFormat>,
) -> Result<()> {
    let mut metadata = metadata.clone();
    if let Some(format) = lyrics_format {
        let fetched = async {
            let lyrics = client.track_lyrics(track_id, format).await?;
            client.fetch_lyrics(&lyrics).await
        }
        .await;
        match fetched {
            Ok(text) => {
                write_lyrics_sidecar(audio_path, format, &text).await?;
                metadata.lyrics = Some(EmbeddedLyrics {
                    text,
                    synchronized: format == LyricsFormat::Lrc,
                });
            }
            Err(error) => {
                eprintln!("lyrics unavailable for track {track_id}: {error}");
            }
        }
    }
    write_metadata(audio_path, &metadata, artwork).await
}

async fn write_lyrics_sidecar(audio_path: &Path, format: LyricsFormat, text: &str) -> Result<()> {
    let sidecar = audio_path.with_extension(format.file_extension());
    let parent = sidecar.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let file_name = sidecar
        .file_name()
        .context("lyrics sidecar must contain a file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.part-{}", std::process::id()));
    let mut file = tokio::fs::File::create(&temporary).await?;
    file.write_all(text.as_bytes()).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    #[cfg(windows)]
    if tokio::fs::try_exists(&sidecar).await? {
        tokio::fs::remove_file(&sidecar).await?;
    }
    tokio::fs::rename(&temporary, &sidecar).await?;
    println!("saved lyrics {}", sidecar.display());
    Ok(())
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
        for attempt in 1..=3 {
            match download_url(client, url, temporary, show_progress).await {
                Ok(()) => return Ok(()),
                Err(error) if attempt < 3 && is_transient_download_error(&error) => {
                    failures.push(format!("attempt {attempt}: {error}"));
                    tokio::time::sleep(retry_delay(attempt)).await;
                }
                Err(error) => {
                    failures.push(format!("attempt {attempt}: {error}"));
                    break;
                }
            }
        }
    }
    bail!(
        "all {} CDN URLs failed: {}",
        info.urls.len(),
        failures.join("; ")
    )
}

fn retry_delay(attempt: u8) -> Duration {
    Duration::from_millis(250 * (1_u64 << (attempt - 1)))
}

fn is_transient_api_error(error: &yandex_music_api::Error) -> bool {
    match error {
        yandex_music_api::Error::Http(error) => {
            error.is_connect()
                || error.is_timeout()
                || error.status().is_some_and(|status| {
                    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                })
        }
        yandex_music_api::Error::Api { status, .. } => {
            *status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
        }
        _ => false,
    }
}

fn is_transient_download_error(error: &anyhow::Error) -> bool {
    if let Some(error) = error.downcast_ref::<yandex_music_api::Error>() {
        return is_transient_api_error(error);
    }
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        )
    })
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

fn album_directory_name(album: &Album) -> String {
    let artists = album
        .artists
        .iter()
        .filter_map(|artist| artist.name.as_deref())
        .collect::<Vec<_>>()
        .join(", ");
    let artist = if artists.is_empty() {
        "Unknown artist"
    } else {
        &artists
    };
    let year = album
        .year
        .map(|year| format!(" ({year})"))
        .unwrap_or_default();
    format!(
        "{} - {}{}",
        safe_file_component(artist),
        safe_file_component(album.title.as_deref().unwrap_or("Untitled album")),
        year
    )
}

fn album_download_jobs(album: &Album, directory: &Path) -> Result<Vec<DownloadJob>> {
    let volumes = album
        .volumes
        .as_ref()
        .context("album response does not contain tracks")?;
    let total = volumes.iter().map(Vec::len).sum::<usize>();
    let track_width = volumes
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or_default()
        .to_string()
        .len()
        .max(2);
    let disc_width = volumes.len().to_string().len().max(1);
    let mut downloads = Vec::with_capacity(total);
    for (disc_index, tracks) in volumes.iter().enumerate() {
        let track_directory = if volumes.len() > 1 {
            directory.join(format!("CD{:0disc_width$}", disc_index + 1))
        } else {
            directory.to_owned()
        };
        for (track_index, track) in tracks.iter().enumerate() {
            let metadata = TrackMetadata::from_track_and_album(track, album);
            downloads.push(DownloadJob {
                index: downloads.len() + 1,
                total,
                track_id: track.id.to_string(),
                label: format!("{} — {}", metadata.artist, metadata.title),
                stem: format!(
                    "{:0track_width$} - {} - {}",
                    track_index + 1,
                    safe_file_component(&metadata.artist),
                    safe_file_component(&metadata.title),
                ),
                directory: track_directory.clone(),
                metadata,
            });
        }
    }
    Ok(downloads)
}

fn ordered_track_jobs(tracks: &[Track], directory: &Path) -> Vec<DownloadJob> {
    let total = tracks.len();
    let width = total.to_string().len().max(2);
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            let metadata = TrackMetadata::from_track(track);
            DownloadJob {
                index: index + 1,
                total,
                track_id: track.id.to_string(),
                label: format!("{} — {}", metadata.artist, metadata.title),
                stem: format!(
                    "{:0width$} - {} - {}",
                    index + 1,
                    safe_file_component(&metadata.artist),
                    safe_file_component(&metadata.title),
                ),
                directory: directory.to_owned(),
                metadata,
            }
        })
        .collect()
}

fn default_track_filename(metadata: &TrackMetadata, extension: &str) -> String {
    format!(
        "{} - {}.{extension}",
        safe_file_component(&metadata.artist),
        safe_file_component(&metadata.title),
    )
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn builds_safe_default_track_filename() {
        let metadata = TrackMetadata {
            title: "Song: Part 1".to_owned(),
            artist: "Artist/Band".to_owned(),
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            cover_url: None,
            lyrics: None,
        };

        assert_eq!(
            default_track_filename(&metadata, "flac"),
            "Artist_Band - Song_ Part 1.flac"
        );
    }

    #[test]
    fn builds_multidisc_album_layout_and_metadata() {
        let album: Album = serde_json::from_value(serde_json::json!({
            "id": 7,
            "title": "Album: One",
            "year": 2026,
            "genre": "electronic",
            "coverUri": "example/%%",
            "artists": [{"id": 1, "name": "Artist/Band"}],
            "volumes": [
                [{"id": 11, "title": "First", "artists": [{"id": 1, "name": "Artist/Band"}]}],
                [{"id": 12, "title": "Second", "artists": [{"id": 1, "name": "Artist/Band"}]}]
            ]
        }))
        .unwrap();

        assert_eq!(
            album_directory_name(&album),
            "Artist_Band - Album_ One (2026)"
        );
        let jobs = album_download_jobs(&album, Path::new("album")).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].directory, Path::new("album/CD1"));
        assert_eq!(jobs[1].directory, Path::new("album/CD2"));
        assert_eq!(jobs[0].stem, "01 - Artist_Band - First");
        assert_eq!(jobs[0].metadata.album.as_deref(), Some("Album: One"));
        assert_eq!(jobs[0].metadata.year, Some(2026));
    }

    #[test]
    fn builds_numbered_collection_layout() {
        let tracks = serde_json::from_value::<Vec<Track>>(serde_json::json!([
            {"id": 11, "title": "First", "artists": [{"id": 1, "name": "One"}]},
            {"id": 12, "title": "Second", "artists": [{"id": 2, "name": "Two"}]}
        ]))
        .unwrap();

        let jobs = ordered_track_jobs(&tracks, Path::new("collection"));
        assert_eq!(jobs[0].stem, "01 - One - First");
        assert_eq!(jobs[1].stem, "02 - Two - Second");
        assert_eq!(jobs[1].track_id, "12");
    }

    #[tokio::test]
    async fn manifest_detects_and_safely_prunes_stale_audio() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ym-download-sync-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let tracks = serde_json::from_value::<Vec<Track>>(serde_json::json!([
            {"id": 11, "title": "First", "artists": [{"id": 1, "name": "One"}]}
        ]))
        .unwrap();
        let jobs = ordered_track_jobs(&tracks, &directory);
        let state = CollectionStateStore::open(&directory, "liked", "42", &jobs)
            .await
            .unwrap();
        let audio = directory.join("01 - One - First.flac");
        tokio::fs::write(&audio, b"tracked").await.unwrap();
        let lyrics = audio.with_extension("lrc");
        tokio::fs::write(&lyrics, b"[00:00]tracked").await.unwrap();
        state
            .record(1, StateStatus::Downloaded, Some(&audio), None)
            .await
            .unwrap();

        let plan = CollectionStateStore::plan(&directory, "liked", "42", &[])
            .await
            .unwrap();
        assert_eq!(plan.stale_paths, [PathBuf::from("01 - One - First.flac")]);
        assert!(
            prune_tracked_audio(&directory, &plan.stale_paths[0])
                .await
                .unwrap()
        );
        assert!(!tokio::fs::try_exists(&audio).await.unwrap());
        assert!(!tokio::fs::try_exists(&lyrics).await.unwrap());

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn writes_lyrics_sidecars_with_the_requested_extension() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ym-download-lyrics-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let audio = directory.join("track.flac");

        write_lyrics_sidecar(&audio, LyricsFormat::Lrc, "[00:00]hello")
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(directory.join("track.lrc"))
                .await
                .unwrap(),
            "[00:00]hello"
        );
        let mut entries = tokio::fs::read_dir(&directory).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name());
        }
        assert_eq!(names, [std::ffi::OsString::from("track.lrc")]);

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
