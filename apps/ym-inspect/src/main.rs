use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use yandex_music_api::{
    Client,
    models::{Album, Id, Playlist, SearchResult, Track},
};
use yandex_music_credentials::{CredentialStore, DEFAULT_PROFILE};

#[derive(Debug, Parser)]
#[command(about = "Read-only Yandex Music API inspection client")]
struct Cli {
    /// Credential profile created by ym-auth.
    #[arg(long, global = true, default_value = DEFAULT_PROFILE)]
    profile: String,

    /// Print complete modeled responses as pretty JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show current account information.
    Account,
    /// Search the catalog.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
    },
    /// Fetch one or more tracks by ID.
    Track {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Fetch an album together with its tracks.
    Album { id: String },
    /// Show liked tracks from the current account.
    Likes {
        /// Maximum number of full tracks to fetch and print.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List playlists belonging to the current account.
    Playlists,
    /// Fetch a playlist by owner and kind.
    Playlist { owner: String, kind: String },
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
    let credentials = store.load_effective(&cli.profile).with_context(|| {
        format!(
            "failed to load profile {:?}; run `ym-auth login`",
            cli.profile
        )
    })?;
    if credentials.is_expired()? {
        bail!(
            "profile {:?} has expired; run `ym-auth login --force`",
            cli.profile
        );
    }
    let client = Client::new(credentials.access_token())?;

    match cli.command {
        Command::Account => {
            let status = client.account_status().await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &status)?;
            } else if let Some(account) = status.account {
                writeln!(
                    output,
                    "{} ({})",
                    account.display_name.as_deref().unwrap_or("unknown account"),
                    account.login.as_deref().unwrap_or("unknown login")
                )?;
                if let Some(uid) = account.uid {
                    writeln!(output, "uid: {uid}")?;
                }
            } else {
                writeln!(output, "Account data is absent in the API response")?;
            }
        }
        Command::Search { query } => {
            let result = client.search(&query.join(" ")).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &result)?;
            } else {
                print_search(&mut output, &result)?;
            }
        }
        Command::Track { ids } => {
            let tracks = client.tracks(ids).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &tracks)?;
            } else {
                for track in &tracks {
                    print_track(&mut output, track)?;
                }
            }
        }
        Command::Album { id } => {
            let album = client.album_with_tracks(id).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &album)?;
            } else {
                print_album(&mut output, &album)?;
            }
        }
        Command::Likes { limit } => {
            let uid = current_account_uid(&client).await?;
            let library = client
                .liked_tracks(uid, 0)
                .await?
                .context("liked-track library was not returned")?;
            if cli.json {
                let mut output = io::stdout().lock();
                print_json(&mut output, &library)?;
            } else {
                let ids = library.track_ids().take(limit);
                let tracks = client.tracks(ids).await?;
                let mut output = io::stdout().lock();
                writeln!(
                    output,
                    "liked tracks: {}, revision: {}",
                    library.tracks.len(),
                    library.revision
                )?;
                for track in &tracks {
                    print_track(&mut output, track)?;
                }
            }
        }
        Command::Playlists => {
            let uid = current_account_uid(&client).await?;
            let playlists = client.user_playlists(uid).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &playlists)?;
            } else {
                writeln!(output, "playlists: {}", playlists.len())?;
                for playlist in &playlists {
                    print_playlist_summary(&mut output, playlist)?;
                }
            }
        }
        Command::Playlist { owner, kind } => {
            let playlist = client.playlist(owner, kind).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &playlist)?;
            } else {
                print_playlist(&mut output, &playlist)?;
            }
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

fn print_json(output: &mut impl Write, value: &impl Serialize) -> Result<()> {
    writeln!(output, "{}", serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn print_search(output: &mut impl Write, search: &SearchResult) -> io::Result<()> {
    if let Some(best) = &search.best {
        let kind = best
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let title = best
            .get("result")
            .and_then(|result| result.get("name").or_else(|| result.get("title")))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("untitled");
        writeln!(output, "best: {kind} — {title}")?;
    }
    if let Some(tracks) = &search.tracks {
        writeln!(output, "tracks: {} total", tracks.total.unwrap_or_default())?;
        for track in &tracks.results {
            print_track(output, track)?;
        }
    }
    if let Some(albums) = &search.albums {
        writeln!(output, "albums: {} total", albums.total.unwrap_or_default())?;
    }
    if let Some(artists) = &search.artists {
        writeln!(
            output,
            "artists: {} total",
            artists.total.unwrap_or_default()
        )?;
    }
    Ok(())
}

fn print_track(output: &mut impl Write, track: &Track) -> io::Result<()> {
    let artists = track
        .artists
        .iter()
        .filter_map(|artist| artist.name.as_deref())
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "{} — {} [{}]",
        if artists.is_empty() {
            "unknown artist"
        } else {
            &artists
        },
        track.title.as_deref().unwrap_or("untitled"),
        track.id
    )
}

fn print_album(output: &mut impl Write, album: &Album) -> io::Result<()> {
    writeln!(
        output,
        "{} ({})",
        album.title.as_deref().unwrap_or("untitled"),
        album
            .year
            .map_or_else(|| "unknown year".to_owned(), |year| year.to_string())
    )?;
    let tracks = album
        .volumes
        .as_ref()
        .map(|volumes| volumes.iter().map(Vec::len).sum::<usize>())
        .unwrap_or_default();
    writeln!(output, "tracks loaded: {tracks}")
}

fn print_playlist_summary(output: &mut impl Write, playlist: &Playlist) -> io::Result<()> {
    let id = playlist
        .playlist_id()
        .map_or_else(|| "unknown id".to_owned(), |id| id.to_string());
    writeln!(
        output,
        "{} [{}], {} tracks",
        playlist.title.as_deref().unwrap_or("untitled"),
        id,
        playlist.track_count.unwrap_or_default()
    )
}

fn print_playlist(output: &mut impl Write, playlist: &Playlist) -> io::Result<()> {
    print_playlist_summary(output, playlist)?;
    if let Some(revision) = playlist.revision {
        writeln!(output, "revision: {revision}")?;
    }
    if let Some(pager) = &playlist.pager {
        writeln!(
            output,
            "page: {}, per page: {}, total: {}",
            pager.page.unwrap_or_default(),
            pager.per_page.unwrap_or_default(),
            pager.total.unwrap_or_default()
        )?;
    }
    for short in &playlist.tracks {
        if let Some(track) = &short.track {
            print_track(output, track)?;
        } else {
            writeln!(output, "track [{}]", short.track_id())?;
        }
    }
    Ok(())
}
