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
        /// Yandex Music track URLs or compact track IDs.
        #[arg(required = true)]
        tracks: Vec<TrackRef>,
    },
    /// Remove one or more tracks from the liked-track library.
    Unlike {
        /// Yandex Music track URLs or compact track IDs.
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
    PlaylistRename {
        /// User-scoped owner/kind URL or compact kind owned by the current account.
        kind: String,
        title: String,
    },
    /// Change playlist visibility.
    PlaylistVisibility {
        /// User-scoped owner/kind URL or compact kind owned by the current account.
        kind: String,
        #[arg(value_enum)]
        visibility: Visibility,
    },
    /// Insert a track into a playlist using its current revision.
    PlaylistAdd {
        /// User-scoped owner/kind URL or compact playlist kind.
        kind: String,
        /// Yandex Music track URL or compact track ID.
        track: TrackRef,
        /// Album URL or ID; inferred when the track URL contains its album.
        album: Option<AlbumRef>,
        #[arg(long, default_value_t = 0)]
        at: usize,
    },
    /// Delete the half-open track range [from, to) using the current revision.
    PlaylistRemove {
        /// User-scoped owner/kind URL or compact kind owned by the current account.
        kind: String,
        #[arg(long)]
        from: usize,
        #[arg(long)]
        to: usize,
    },
    /// Permanently delete a playlist.
    PlaylistDelete {
        /// User-scoped owner/kind URL or compact kind owned by the current account.
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
            let (owner, kind) = playlist_target(kind, &uid)?;
            let playlist = client.rename_playlist(owner, kind, title).await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistVisibility { kind, visibility } => {
            let (owner, kind) = playlist_target(kind, &uid)?;
            let playlist = client
                .set_playlist_visibility(owner, kind, visibility.into())
                .await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistAdd {
            kind,
            track,
            album,
            at,
        } => {
            let (owner, kind) = playlist_target(kind, &uid)?;
            let album_id = album
                .as_ref()
                .map(AlbumRef::album_id)
                .or_else(|| track.album_id())
                .context("album ID is required when the track is not an album/track URL")?;
            let current = client.playlist(owner.clone(), kind.clone()).await?;
            let revision = playlist_revision(&current)?;
            let diff =
                PlaylistDiff::new().insert(at, [PlaylistTrackId::new(track.track_id(), album_id)]);
            let playlist = client.change_playlist(owner, kind, revision, &diff).await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistRemove { kind, from, to } => {
            if from >= to {
                bail!("invalid range: --from must be smaller than --to");
            }
            let (owner, kind) = playlist_target(kind, &uid)?;
            let current = client.playlist(owner.clone(), kind.clone()).await?;
            let revision = playlist_revision(&current)?;
            let diff = PlaylistDiff::new().delete(from, to);
            let playlist = client.change_playlist(owner, kind, revision, &diff).await?;
            print_playlist(&mut output, cli.json, &playlist)?;
        }
        Command::PlaylistDelete { kind, yes } => {
            if !yes {
                bail!("refusing permanent deletion without --yes");
            }
            let (owner, kind) = playlist_target(kind, &uid)?;
            client.delete_playlist(owner, kind).await?;
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

fn playlist_target(value: String, current_uid: &Id) -> Result<(Id, String)> {
    if value.contains("://") {
        let playlist = value.parse::<PlaylistRef>()?;
        Ok((Id::from(playlist.owner()), playlist.kind().to_owned()))
    } else {
        Ok((current_uid.clone(), value))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_add_preserves_owner_from_playlist_url() {
        let cli = Cli::try_parse_from([
            "yamu-edit",
            "playlist-add",
            "https://music.yandex.ru/users/example/playlists/42?utm_source=web",
            "https://music.yandex.ru/album/1193829/track/10994777?utm_source=web",
            "--at",
            "3",
        ])
        .unwrap();

        let Command::PlaylistAdd {
            kind,
            track,
            album,
            at,
        } = cli.command
        else {
            panic!("expected playlist-add command");
        };
        let (owner, kind) = playlist_target(kind, &Id::from("current-user")).unwrap();
        assert_eq!(owner.to_string(), "example");
        assert_eq!(kind, "42");
        assert_eq!(track.track_id(), "10994777");
        assert_eq!(track.album_id(), Some("1193829"));
        assert!(album.is_none());
        assert_eq!(at, 3);
    }

    #[test]
    fn compact_playlist_kind_uses_the_current_account() {
        let current = Id::from("current-user");
        let (owner, kind) = playlist_target("42".to_owned(), &current).unwrap();

        assert_eq!(owner, current);
        assert_eq!(kind, "42");
    }
}
