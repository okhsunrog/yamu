use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use yandex_music_api::{
    Client,
    auth::DeviceAuth,
    credentials::{CredentialStore, DEFAULT_PROFILE, RefreshPolicy},
    models::{
        Album, ArtistAlbumSort, Id, LyricsFormat, PageRequest, Playlist, SearchResult, StationId,
        Track,
    },
    resource::{AlbumRef, ArtistRef, PlaylistRef, TrackRef},
};

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
        tracks: Vec<TrackRef>,
    },
    /// Fetch an album together with its tracks.
    Album { album: AlbumRef },
    /// Show liked tracks from the current account.
    Likes {
        /// Maximum number of full tracks to fetch and print.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List playlists belonging to the current account.
    Playlists,
    /// Fetch a playlist by owner and kind.
    Playlist { playlist: PlaylistRef },
    /// Fetch an artist by ID.
    Artist { artist: ArtistRef },
    /// Fetch one page of an artist's tracks.
    ArtistTracks {
        artist: ArtistRef,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long, default_value_t = 20)]
        page_size: u32,
    },
    /// Fetch one page of an artist's albums.
    ArtistAlbums {
        artist: ArtistRef,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long, default_value_t = 20)]
        page_size: u32,
    },
    /// Fetch plain or synchronized lyrics.
    Lyrics {
        track: TrackRef,
        #[arg(long)]
        lrc: bool,
    },
    /// Fetch track recommendations for a playlist.
    PlaylistRecommendations { playlist: PlaylistRef },
    /// List Rotor radio stations.
    Stations {
        #[arg(long, default_value = "ru")]
        language: String,
    },
    /// Fetch the next track sequence from a Rotor station.
    StationTracks {
        kind: String,
        tag: String,
        #[arg(long)]
        queue: Option<String>,
    },
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
        Command::Track { tracks } => {
            let tracks = client.tracks(tracks.iter().map(TrackRef::track_id)).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &tracks)?;
            } else {
                for track in &tracks {
                    print_track(&mut output, track)?;
                }
            }
        }
        Command::Album { album } => {
            let album = client.album_with_tracks(album.album_id()).await?;
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
        Command::Playlist { playlist } => {
            let playlist = client.playlist(playlist.owner(), playlist.kind()).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &playlist)?;
            } else {
                print_playlist(&mut output, &playlist)?;
            }
        }
        Command::Artist { artist } => {
            let artists = client.artists([artist.artist_id()]).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &artists)?;
            } else {
                for artist in artists {
                    writeln!(
                        output,
                        "{} [{}]",
                        artist.name.as_deref().unwrap_or("unknown artist"),
                        artist
                            .id
                            .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
                    )?;
                }
            }
        }
        Command::ArtistTracks {
            artist,
            page,
            page_size,
        } => {
            let result = client
                .artist_tracks(artist.artist_id(), PageRequest::new(page, page_size))
                .await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &result)?;
            } else {
                for track in &result.tracks {
                    print_track(&mut output, track)?;
                }
            }
        }
        Command::ArtistAlbums {
            artist,
            page,
            page_size,
        } => {
            let result = client
                .artist_albums(
                    artist.artist_id(),
                    PageRequest::new(page, page_size),
                    ArtistAlbumSort::Year,
                )
                .await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &result)?;
            } else {
                for album in &result.albums {
                    print_album(&mut output, album)?;
                }
            }
        }
        Command::Lyrics { track, lrc } => {
            let metadata = client
                .track_lyrics(
                    track.track_id(),
                    if lrc {
                        LyricsFormat::Lrc
                    } else {
                        LyricsFormat::Text
                    },
                )
                .await?;
            let lyrics = client.fetch_lyrics(&metadata).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(
                    &mut output,
                    &serde_json::json!({"metadata": metadata, "lyrics": lyrics}),
                )?;
            } else {
                write!(output, "{lyrics}")?;
                if !lyrics.ends_with('\n') {
                    writeln!(output)?;
                }
            }
        }
        Command::PlaylistRecommendations { playlist } => {
            let result = client
                .playlist_recommendations(playlist.owner(), playlist.kind())
                .await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &result)?;
            } else {
                for track in &result.tracks {
                    print_track(&mut output, track)?;
                }
            }
        }
        Command::Stations { language } => {
            let stations = client.stations(language).await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &stations)?;
            } else {
                for result in stations {
                    if let Some(station) = result.station {
                        writeln!(
                            output,
                            "{} [{}]",
                            station.name.as_deref().unwrap_or("unnamed station"),
                            station
                                .id
                                .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
                        )?;
                    }
                }
            }
        }
        Command::StationTracks { kind, tag, queue } => {
            let result = client
                .station_tracks(&StationId { kind, tag }, queue.map(Id::from))
                .await?;
            let mut output = io::stdout().lock();
            if cli.json {
                print_json(&mut output, &result)?;
            } else {
                for sequence in &result.sequence {
                    if let Some(track) = &sequence.track {
                        print_track(&mut output, track)?;
                    }
                }
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
