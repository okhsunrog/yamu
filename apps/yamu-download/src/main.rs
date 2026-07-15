use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::{sync::Semaphore, task::JoinSet};
use yamu::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    downloader::{CancellationToken, DownloadEvent, DownloadRequest, Downloader},
    media::ffmpeg_cli::FfmpegCli,
    models::{
        Album, DownloadInfo, DownloadOptions, DownloadQuality, Id, LyricsFormat, Playlist, Track,
    },
    resource::{AlbumRef, ArtistRef, PlaylistSourceRef, TrackRef},
};

mod atomic_file;
mod metadata;
mod state;

use metadata::{ArtworkCache, EmbeddedLyrics, TrackMetadata, verify_audio_file, write_metadata};
use state::{CollectionStateStore, StateStatus};

const ENRICHMENT_VERSION: u32 = 1;
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Serialize)]
struct EnrichmentMarker {
    version: u32,
    lyrics: Option<String>,
}

#[derive(Debug, Parser)]
#[command(about = "Download tracks and collections from Yandex Music")]
struct Cli {
    /// Credential profile created by yamu-auth.
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
        /// Yandex Music track URL or numeric track ID.
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
        /// Yandex Music album URL or numeric album ID.
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
        /// Yandex Music artist URL or numeric artist ID.
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
        /// Yandex Music playlist URL, UUID, or compact owner:kind reference.
        playlist: PlaylistSourceRef,
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
        /// Yandex Music playlist URL, UUID, or compact owner:kind reference.
        playlist: PlaylistSourceRef,
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
                "failed to load profile {:?}; run `yamu-auth login`",
                cli.profile
            )
        })?;
    let credentials = resolved.credentials;
    if credentials.is_expired()? {
        bail!(
            "profile {:?} has expired; run `yamu-auth login --force`",
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
                if enrichment_is_current(&destination, lyrics).await? {
                    println!("verified existing {}", destination.display());
                    return Ok(());
                }
                enrich_and_mark(client, track_id, &destination, &metadata, &artwork, lyrics)
                    .await?;
                println!("repaired enrichment for {}", destination.display());
                return Ok(());
            }
            Err(error) => eprintln!(
                "existing {} is invalid ({error:#}); replacing it",
                destination.display()
            ),
        }
    }
    clear_enrichment_marker(&destination).await?;
    download_normalized(client, &info, &destination, true, true).await?;
    enrich_and_mark(client, track_id, &destination, &metadata, &artwork, lyrics).await?;
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
        &[],
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
        &[],
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
        &[],
        downloads,
        SyncMode::default(),
        lyrics,
    )
    .await
}

struct PlaylistDownloadRequest {
    playlist: PlaylistSourceRef,
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
    let (playlist, source_id) = match playlist {
        PlaylistSourceRef::User(reference) => {
            let source_id = format!("{}:{}", reference.owner(), reference.kind());
            let playlist = client.playlist(reference.owner(), reference.kind()).await?;
            (playlist, source_id)
        }
        PlaylistSourceRef::Uuid(reference) => {
            let source_id = format!("uuid:{}", reference.playlist_uuid());
            let playlist = client.playlist_by_uuid(reference.playlist_uuid()).await?;
            (playlist, source_id)
        }
    };
    let mut source_aliases = playlist_source_aliases(&playlist);
    source_aliases.retain(|alias| alias != &source_id);
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
        &source_id,
        &source_aliases,
        jobs_to_run,
        sync,
        lyrics,
    )
    .await
}

fn playlist_source_aliases(playlist: &Playlist) -> Vec<String> {
    let mut aliases = Vec::new();
    let mut push = |alias: String| {
        if !aliases.contains(&alias) {
            aliases.push(alias);
        }
    };
    if let Some(uuid) = &playlist.playlist_uuid {
        push(format!("uuid:{uuid}"));
    }
    if let Some(kind) = &playlist.kind {
        if let Some(uid) = &playlist.uid {
            push(format!("{uid}:{kind}"));
        }
        if let Some(owner) = &playlist.owner {
            if let Some(uid) = &owner.uid {
                push(format!("{uid}:{kind}"));
            }
            if let Some(login) = &owner.login {
                push(format!("{login}:{kind}"));
            }
        }
    }
    aliases
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
    source_aliases: &[String],
    jobs: Vec<DownloadJob>,
    sync: SyncMode,
    lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let plan = CollectionStateStore::plan_with_aliases(
        directory,
        source_kind,
        source_id,
        source_aliases,
        &jobs,
    )
    .await?;
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
    let state = CollectionStateStore::open_with_aliases(
        directory,
        source_kind,
        source_id,
        source_aliases,
        &jobs,
    )
    .await?;
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
    state.flush().await?;

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
    let stale_paths = state.stale_paths().await;
    if sync.prune {
        let mut pruned = 0;
        for path in stale_paths {
            if prune_tracked_audio(directory, &path).await? {
                pruned += 1;
            }
            state.forget_stale_path(&path).await?;
        }
        state.flush().await?;
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
    let root = tokio::fs::canonicalize(directory).await?;
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
    let audio = checked_prunable_file(&root, &candidate, "tracked audio").await?;
    let mut sidecars = Vec::new();
    for extension in ["lrc", "txt"] {
        let sidecar = candidate.with_extension(extension);
        if let Some(sidecar) = checked_prunable_file(&root, &sidecar, "lyrics sidecar").await? {
            sidecars.push(sidecar);
        }
    }
    let marker = enrichment_marker_path(&candidate)?;
    if let Some(marker) = checked_prunable_file(&root, &marker, "enrichment marker").await? {
        sidecars.push(marker);
    }
    let mut removed = false;
    for sidecar in sidecars {
        tokio::fs::remove_file(&sidecar).await?;
        println!("pruned {}", sidecar.display());
        removed = true;
    }
    if let Some(audio) = audio {
        tokio::fs::remove_file(&audio).await?;
        println!("pruned {}", audio.display());
        removed = true;
    }
    Ok(removed)
}

async fn checked_prunable_file(root: &Path, path: &Path, kind: &str) -> Result<Option<PathBuf>> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "refusing to prune {kind} that is not a regular file: {}",
            path.display()
        );
    }
    let canonical = tokio::fs::canonicalize(path).await?;
    if !canonical.starts_with(root) {
        bail!(
            "refusing to prune {kind} outside {}: {}",
            root.display(),
            canonical.display()
        );
    }
    Ok(Some(canonical))
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
        codec: yamu::models::AudioCodec,
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
                    if enrichment_is_current(&destination, lyrics).await? {
                        return Ok(DownloadStatus::Skipped { path: destination });
                    }
                    enrich_and_mark(
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
                        reason: "metadata enrichment was incomplete".to_owned(),
                    });
                }
                Err(error) => {
                    let reason = format!("{error:#}");
                    clear_enrichment_marker(&destination).await?;
                    download_normalized(client, &info, &destination, true, false).await?;
                    enrich_and_mark(
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
        clear_enrichment_marker(&destination).await?;
        download_normalized(client, &info, &destination, force, false).await?;
        enrich_and_mark(
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
    client
        .download_info(uid, track_id, &options)
        .await
        .map_err(Into::into)
}

async fn current_account_uid(client: &Client) -> Result<Id> {
    client
        .account_status()
        .await?
        .account
        .and_then(|account| account.uid)
        .context("account status response does not contain a uid")
}

async fn enrich_and_mark(
    client: &Client,
    track_id: &str,
    audio_path: &Path,
    metadata: &TrackMetadata,
    artwork: &ArtworkCache,
    lyrics_format: Option<LyricsFormat>,
) -> Result<()> {
    let saved_lyrics = write_enriched_metadata(
        client,
        track_id,
        audio_path,
        metadata,
        artwork,
        lyrics_format,
    )
    .await?;
    let extension = normalized_path_extension(audio_path)?;
    verify_audio_file(audio_path, &extension).await?;
    write_enrichment_marker(audio_path, saved_lyrics).await
}

async fn enrichment_is_current(
    audio_path: &Path,
    requested_lyrics: Option<LyricsFormat>,
) -> Result<bool> {
    let marker = enrichment_marker_path(audio_path)?;
    let bytes = match tokio::fs::read(marker).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let Ok(marker) = serde_json::from_slice::<EnrichmentMarker>(&bytes) else {
        return Ok(false);
    };
    let requested_lyrics = requested_lyrics.map(|format| format.file_extension());
    Ok(marker.version == ENRICHMENT_VERSION
        && requested_lyrics.is_none_or(|requested| marker.lyrics.as_deref() == Some(requested)))
}

async fn write_enrichment_marker(
    audio_path: &Path,
    saved_lyrics: Option<LyricsFormat>,
) -> Result<()> {
    let marker = EnrichmentMarker {
        version: ENRICHMENT_VERSION,
        lyrics: saved_lyrics.map(|format| format.file_extension().to_owned()),
    };
    let mut bytes = serde_json::to_vec_pretty(&marker)?;
    bytes.push(b'\n');
    write_atomic(&enrichment_marker_path(audio_path)?, &bytes).await
}

async fn clear_enrichment_marker(audio_path: &Path) -> Result<()> {
    match tokio::fs::remove_file(enrichment_marker_path(audio_path)?).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn enrichment_marker_path(audio_path: &Path) -> Result<PathBuf> {
    let parent = audio_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = audio_path
        .file_name()
        .context("audio path must contain a file name")?
        .to_string_lossy();
    Ok(parent.join(format!(".{file_name}.ym-enriched.json")))
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let file_name = path
        .file_name()
        .context("atomic destination must contain a file name")?
        .to_string_lossy();
    let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{file_name}.part-{}-{nonce}", std::process::id()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        atomic_file::persist(&temporary, path, true)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

async fn write_enriched_metadata(
    client: &Client,
    track_id: &str,
    audio_path: &Path,
    metadata: &TrackMetadata,
    artwork: &ArtworkCache,
    lyrics_format: Option<LyricsFormat>,
) -> Result<Option<LyricsFormat>> {
    let mut metadata = metadata.clone();
    let mut saved_lyrics = None;
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
                saved_lyrics = Some(format);
            }
            Err(error) => {
                eprintln!("lyrics unavailable for track {track_id}: {error}");
            }
        }
    }
    write_metadata(audio_path, &metadata, artwork).await?;
    Ok(saved_lyrics)
}

async fn write_lyrics_sidecar(audio_path: &Path, format: LyricsFormat, text: &str) -> Result<()> {
    let sidecar = audio_path.with_extension(format.file_extension());
    write_atomic(&sidecar, text.as_bytes()).await?;
    println!("saved lyrics {}", sidecar.display());
    Ok(())
}

async fn download_normalized(
    client: &Client,
    info: &DownloadInfo,
    destination: &Path,
    force: bool,
    show_progress: bool,
) -> Result<()> {
    let downloader = Downloader::new(client.clone(), FfmpegCli);
    downloader
        .download(
            DownloadRequest {
                info: info.clone(),
                destination: destination.to_owned(),
                replace: force,
            },
            CancellationToken::new(),
            |event| {
                if !show_progress {
                    return;
                }
                if let DownloadEvent::Progress { downloaded, total } = event {
                    match total {
                        Some(total) => eprint!("\r{downloaded}/{total} bytes"),
                        None => eprint!("\r{downloaded} bytes"),
                    }
                    let _ = io::stderr().flush();
                }
            },
        )
        .await?;
    if show_progress {
        eprintln!();
    }
    Ok(())
}

fn normalized_extension(info: &DownloadInfo) -> &'static str {
    use yamu::models::AudioCodec;
    match &info.codec {
        AudioCodec::Flac | AudioCodec::FlacMp4 => "flac",
        AudioCodec::Aac | AudioCodec::HeAac | AudioCodec::AacMp4 | AudioCodec::HeAacMp4 => "m4a",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Other(_) => "bin",
        _ => "bin",
    }
}

fn normalized_path_extension(path: &Path) -> Result<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .context("audio path must contain a UTF-8 extension")
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
    fn parses_yandex_music_urls_for_every_resource_command() {
        let cli = Cli::try_parse_from([
            "yamu-download",
            "track",
            "https://music.yandex.ru/album/1193829/track/10994777?utm_source=web#player",
        ])
        .unwrap();
        let Command::Track { track, .. } = cli.command else {
            panic!("expected track command");
        };
        assert_eq!(track.track_id(), "10994777");
        assert_eq!(track.album_id(), Some("1193829"));

        let cli = Cli::try_parse_from([
            "yamu-download",
            "album",
            "https://music.yandex.ru/album/1193829?utm_source=web",
        ])
        .unwrap();
        let Command::Album { album, .. } = cli.command else {
            panic!("expected album command");
        };
        assert_eq!(album.album_id(), "1193829");

        let cli = Cli::try_parse_from([
            "yamu-download",
            "artist",
            "https://music.yandex.ru/artist/1556?utm_source=web",
        ])
        .unwrap();
        let Command::Artist { artist, .. } = cli.command else {
            panic!("expected artist command");
        };
        assert_eq!(artist.artist_id(), "1556");

        let playlist_url = "https://music.yandex.ru/playlists/fa1b8d08-71c7-3ed8-9c58-8eebbdccdf7f?utm_source=web&utm_medium=copy_link";
        let cli = Cli::try_parse_from(["yamu-download", "playlist", playlist_url]).unwrap();
        let Command::Playlist { playlist, .. } = cli.command else {
            panic!("expected playlist command");
        };
        let PlaylistSourceRef::Uuid(playlist) = playlist else {
            panic!("expected UUID playlist reference");
        };
        assert_eq!(
            playlist.playlist_uuid(),
            "fa1b8d08-71c7-3ed8-9c58-8eebbdccdf7f"
        );

        let owner_kind_url = "https://music.yandex.ru/users/example/playlists/42?utm_source=web";
        let cli =
            Cli::try_parse_from(["yamu-download", "sync", "playlist", owner_kind_url]).unwrap();
        let Command::Sync {
            source: SyncCommand::Playlist { playlist, .. },
        } = cli.command
        else {
            panic!("expected sync playlist command");
        };
        let PlaylistSourceRef::User(playlist) = playlist else {
            panic!("expected user playlist reference");
        };
        assert_eq!(playlist.owner(), "example");
        assert_eq!(playlist.kind(), "42");
    }

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
    fn normalizes_case_of_output_extension_for_verification() {
        assert_eq!(
            normalized_path_extension(Path::new("track.FLAC")).unwrap(),
            "flac"
        );
        assert_eq!(
            normalized_path_extension(Path::new("track.M4A")).unwrap(),
            "m4a"
        );
        assert_eq!(
            normalized_path_extension(Path::new("track.MP3")).unwrap(),
            "mp3"
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
            "yamu-download-sync-test-{}-{nonce}",
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
        write_enrichment_marker(&audio, Some(LyricsFormat::Lrc))
            .await
            .unwrap();
        state
            .record(1, StateStatus::Downloaded, Some(&audio), None)
            .await
            .unwrap();
        let before_flush = tokio::fs::read_to_string(directory.join(".yamu-download-state.json"))
            .await
            .unwrap();
        assert!(before_flush.contains("\"status\": \"pending\""));
        state.flush().await.unwrap();
        let after_flush = tokio::fs::read_to_string(directory.join(".yamu-download-state.json"))
            .await
            .unwrap();
        assert!(after_flush.contains("\"status\": \"downloaded\""));

        let plan = CollectionStateStore::plan(&directory, "liked", "42", &[])
            .await
            .unwrap();
        assert_eq!(plan.stale_paths, [PathBuf::from("01 - One - First.flac")]);
        let retained = CollectionStateStore::open(&directory, "liked", "42", &[])
            .await
            .unwrap();
        let retained_plan = CollectionStateStore::plan(&directory, "liked", "42", &[])
            .await
            .unwrap();
        assert_eq!(retained_plan.stale_paths, plan.stale_paths);
        assert!(
            prune_tracked_audio(&directory, &plan.stale_paths[0])
                .await
                .unwrap()
        );
        retained
            .forget_stale_path(&plan.stale_paths[0])
            .await
            .unwrap();
        retained.flush().await.unwrap();
        assert!(!tokio::fs::try_exists(&audio).await.unwrap());
        assert!(!tokio::fs::try_exists(&lyrics).await.unwrap());
        assert!(
            !tokio::fs::try_exists(enrichment_marker_path(&audio).unwrap())
                .await
                .unwrap()
        );
        assert!(
            CollectionStateStore::plan(&directory, "liked", "42", &[])
                .await
                .unwrap()
                .stale_paths
                .is_empty()
        );

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn prune_removes_orphaned_sidecars_when_audio_is_already_missing() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "yamu-download-orphan-prune-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let audio = directory.join("missing.flac");
        let lyrics = audio.with_extension("lrc");
        tokio::fs::write(&lyrics, b"orphaned").await.unwrap();
        write_enrichment_marker(&audio, Some(LyricsFormat::Lrc))
            .await
            .unwrap();

        assert!(
            prune_tracked_audio(&directory, Path::new("missing.flac"))
                .await
                .unwrap()
        );
        assert!(!tokio::fs::try_exists(lyrics).await.unwrap());
        assert!(
            !tokio::fs::try_exists(enrichment_marker_path(&audio).unwrap())
                .await
                .unwrap()
        );

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prune_rejects_symlinks_without_deleting_their_target() {
        use std::os::unix::fs::symlink;

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "yamu-download-symlink-prune-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let target = directory.join("keep.flac");
        let link = directory.join("stale.flac");
        tokio::fs::write(&target, b"keep").await.unwrap();
        symlink(&target, &link).unwrap();

        let error = prune_tracked_audio(&directory, Path::new("stale.flac"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
        assert!(tokio::fs::try_exists(&target).await.unwrap());
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"keep");

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }

    #[tokio::test]
    async fn writes_lyrics_sidecars_with_the_requested_extension() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "yamu-download-lyrics-test-{}-{nonce}",
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

    #[tokio::test]
    async fn enrichment_marker_requires_the_requested_lyrics_format() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "yamu-download-enrichment-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let audio = directory.join("track.flac");

        write_enrichment_marker(&audio, None).await.unwrap();
        assert!(enrichment_is_current(&audio, None).await.unwrap());
        assert!(
            !enrichment_is_current(&audio, Some(LyricsFormat::Lrc))
                .await
                .unwrap()
        );
        write_enrichment_marker(&audio, Some(LyricsFormat::Lrc))
            .await
            .unwrap();
        assert!(
            enrichment_is_current(&audio, Some(LyricsFormat::Lrc))
                .await
                .unwrap()
        );
        assert!(enrichment_is_current(&audio, None).await.unwrap());

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
