use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use yandex_music_api::{
    Client,
    models::{Album, SearchResult, Track},
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
    }

    Ok(())
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
