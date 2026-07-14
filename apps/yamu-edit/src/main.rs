use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use yamu::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    models::{Id, Playlist, PlaylistDiff, PlaylistTrackId, PlaylistVisibility},
    resource::{AlbumRef, PlaylistRef, TrackRef},
};

#[derive(Debug, Parser)]
#[command(about = "Explicit mutation client for Yandex Music")]
struct Cli {
    /// Credential profile created by yamu-auth.
    #[arg(long, global = true, default_value = DEFAULT_PROFILE)]
    profile: String,

    /// Print complete mutation responses as pretty JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add one or more tracks to the liked-track library.
    Like {
        #[arg(required = true)]
        tracks: Vec<TrackRef>,
    },
    /// Remove one or more tracks from the liked-track library.
    Unlike {
        #[arg(required = true)]
        tracks: Vec<TrackRef>,
    },
    /// Create a playlist.
    PlaylistCreate {
        title: String,
        #[arg(long, value_enum, default_value_t = Visibility::Private)]
        visibility: Visibility,
    },
    /// Rename a playlist.
    PlaylistRename { kind: String, title: String },
    /// Change playlist visibility.
    PlaylistVisibility {
        kind: String,
        #[arg(value_enum)]
        visibility: Visibility,
    },
    /// Insert a track into a playlist using its current revision.
    PlaylistAdd {
        kind: String,
        track: TrackRef,
        album: Option<AlbumRef>,
        #[arg(long, default_value_t = 0)]
        at: usize,
    },
    /// Delete the half-open track range [from, to) using the current revision.
    PlaylistRemove {
        kind: String,
        #[arg(long)]
        from: usize,
        #[arg(long)]
        to: usize,
    },
    /// Permanently delete a playlist.
    PlaylistDelete {
        kind: String,
        /// Confirm permanent deletion.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum Visibility {
    #[default]
    Private,
    Public,
}

impl From<Visibility> for PlaylistVisibility {
    fn from(value: Visibility) -> Self {
        match value {
            Visibility::Private => Self::Private,
            Visibility::Public => Self::Public,
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        if error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        {
            return;
        }
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
    let mut output = io::stdout().lock();

    match cli.command {
        Command::Like { tracks } => {
            let revision = client
                .like_tracks(uid, tracks.iter().map(TrackRef::track_id))
                .await?;
            print_value(&mut output, cli.json, &revision, || {
                format!("liked-track revision: {}", revision.revision)
            })?;
        }
        Command::Unlike { tracks } => {
            let revision = client
                .unlike_tracks(uid, tracks.iter().map(TrackRef::track_id))
                .await?;
            print_value(&mut output, cli.json, &revision, || {
                format!("liked-track revision: {}", revision.revision)
            })?;
        }
        Command::PlaylistCreate { title, visibility } => {
            let playlist = client
                .create_playlist(uid, title, visibility.into())
                .await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistRename { kind, title } => {
            let playlist = client
                .rename_playlist(uid, playlist_kind(kind)?, title)
                .await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistVisibility { kind, visibility } => {
            let playlist = client
                .set_playlist_visibility(uid, playlist_kind(kind)?, visibility.into())
                .await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistAdd {
            kind,
            track,
            album,
            at,
        } => {
            let kind = playlist_kind(kind)?;
            let album_id = album
                .as_ref()
                .map(AlbumRef::album_id)
                .or_else(|| track.album_id())
                .context("album ID is required when the track is not an album/track URL")?;
            let current = client.playlist(uid.clone(), kind.clone()).await?;
            let revision = playlist_revision(&current)?;
            let diff =
                PlaylistDiff::new().insert(at, [PlaylistTrackId::new(track.track_id(), album_id)]);
            let playlist = client.change_playlist(uid, kind, revision, &diff).await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistRemove { kind, from, to } => {
            if from >= to {
                bail!("invalid range: --from must be smaller than --to");
            }
            let kind = playlist_kind(kind)?;
            let current = client.playlist(uid.clone(), kind.clone()).await?;
            let revision = playlist_revision(&current)?;
            let diff = PlaylistDiff::new().delete(from, to);
            let playlist = client.change_playlist(uid, kind, revision, &diff).await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistDelete { kind, yes } => {
            if !yes {
                bail!("refusing permanent deletion without --yes");
            }
            client.delete_playlist(uid, playlist_kind(kind)?).await?;
            writeln!(output, "playlist deleted")?;
        }
    }

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

fn playlist_revision(playlist: &Playlist) -> Result<u64> {
    playlist
        .revision
        .context("playlist response does not contain a revision")
}

fn playlist_kind(value: String) -> Result<String> {
    if value.contains("://") {
        Ok(value.parse::<PlaylistRef>()?.kind().to_owned())
    } else {
        Ok(value)
    }
}

fn print_playlist(output: &mut impl Write, json: bool, playlist: &Playlist) -> Result<()> {
    print_value(output, json, playlist, || {
        format!(
            "playlist {}: {}, revision {}",
            playlist
                .playlist_id()
                .map_or_else(|| "unknown id".to_owned(), |id| id.to_string()),
            playlist.title.as_deref().unwrap_or("untitled"),
            playlist
                .revision
                .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
        )
    })
}

fn print_value(
    output: &mut impl Write,
    json: bool,
    value: &impl Serialize,
    summary: impl FnOnce() -> String,
) -> Result<()> {
    if json {
        writeln!(output, "{}", serde_json::to_string_pretty(value)?)?;
    } else {
        writeln!(output, "{}", summary())?;
    }
    Ok(())
}
