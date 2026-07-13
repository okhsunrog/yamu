use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
};

use anyhow::Result;
use tokio::sync::Mutex;
use yandex_music_api::{
    media::{self, ffmpeg_cli::FfmpegCli},
    models::{Album, Track},
};

#[derive(Clone, Debug)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub cover_url: Option<String>,
    pub lyrics: Option<EmbeddedLyrics>,
}

pub use yandex_music_api::media::EmbeddedLyrics;

impl TrackMetadata {
    pub fn from_track(track: &Track) -> Self {
        let artist = track
            .artists
            .iter()
            .filter_map(|artist| artist.name.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        let album = track.albums.first();
        let album_artist = album.map(|album| {
            album
                .artists
                .iter()
                .filter_map(|artist| artist.name.as_deref())
                .collect::<Vec<_>>()
                .join(", ")
        });
        Self {
            title: track.title.clone().unwrap_or_else(|| "Untitled".to_owned()),
            artist: if artist.is_empty() {
                "Unknown artist".to_owned()
            } else {
                artist
            },
            album: album.and_then(|album| album.title.clone()),
            album_artist: album_artist.filter(|artist| !artist.is_empty()),
            genre: album.and_then(|album| album.genre.clone()),
            year: album.and_then(|album| album.year),
            track_number: album
                .and_then(|album| album.track_position.as_ref())
                .and_then(|position| position.index),
            disc_number: album
                .and_then(|album| album.track_position.as_ref())
                .and_then(|position| position.volume),
            cover_url: track
                .cover_url("600x600")
                .or_else(|| album.and_then(|album| album.cover_url("600x600"))),
            lyrics: None,
        }
    }

    pub fn from_track_and_album(track: &Track, album: &Album) -> Self {
        let mut metadata = Self::from_track(track);
        if metadata.album.is_none() {
            metadata.album.clone_from(&album.title);
        }
        if metadata.album_artist.is_none() {
            let artists = album
                .artists
                .iter()
                .filter_map(|artist| artist.name.as_deref())
                .collect::<Vec<_>>()
                .join(", ");
            if !artists.is_empty() {
                metadata.album_artist = Some(artists);
            }
        }
        if metadata.genre.is_none() {
            metadata.genre.clone_from(&album.genre);
        }
        if metadata.year.is_none() {
            metadata.year = album.year;
        }
        if metadata.cover_url.is_none() {
            metadata.cover_url = album.cover_url("600x600");
        }
        metadata
    }

    fn as_media_metadata(&self) -> media::TrackMetadata {
        media::TrackMetadata {
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            genre: self.genre.clone(),
            year: self.year,
            track_number: self.track_number,
            disc_number: self.disc_number,
            lyrics: self.lyrics.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ArtworkCache {
    http: reqwest::Client,
    state: Arc<Mutex<ArtworkCacheState>>,
}

#[derive(Default)]
struct ArtworkCacheState {
    entries: HashMap<String, Arc<Vec<u8>>>,
    insertion_order: VecDeque<String>,
}

const MAX_ARTWORK_CACHE_ENTRIES: usize = 32;

impl ArtworkCache {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!(
                    env!("CARGO_PKG_NAME"),
                    "/",
                    env!("CARGO_PKG_VERSION")
                ))
                .build()?,
            state: Arc::new(Mutex::new(ArtworkCacheState::default())),
        })
    }

    async fn get(&self, url: Option<&str>) -> Result<Option<Vec<u8>>> {
        let Some(url) = url else {
            return Ok(None);
        };
        if let Some(bytes) = self.state.lock().await.entries.get(url).cloned() {
            return Ok(Some(bytes.as_ref().clone()));
        }
        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .to_vec();
        let mut state = self.state.lock().await;
        if !state.entries.contains_key(url) {
            while state.entries.len() >= MAX_ARTWORK_CACHE_ENTRIES {
                let Some(oldest) = state.insertion_order.pop_front() else {
                    break;
                };
                state.entries.remove(&oldest);
            }
            state.insertion_order.push_back(url.to_owned());
        }
        state
            .entries
            .insert(url.to_owned(), Arc::new(bytes.clone()));
        Ok(Some(bytes))
    }
}

pub async fn write_metadata(
    path: &Path,
    metadata: &TrackMetadata,
    artwork: &ArtworkCache,
) -> Result<()> {
    let picture = artwork.get(metadata.cover_url.as_deref()).await?;
    media::write_metadata(&FfmpegCli, path, &metadata.as_media_metadata(), picture)
        .await
        .map_err(Into::into)
}

pub async fn verify_audio_file(path: &Path, expected_extension: &str) -> Result<()> {
    media::verify_audio_file(&FfmpegCli, path, expected_extension)
        .await
        .map_err(Into::into)
}
