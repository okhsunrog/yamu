use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{MimeType, Picture, PictureType},
    tag::{Accessor, ItemKey, Tag},
};
use tokio::sync::Mutex;
use yandex_music_api::models::{Album, Track};

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
}

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
}

#[derive(Clone)]
pub struct ArtworkCache {
    http: reqwest::Client,
    entries: Arc<Mutex<HashMap<String, Arc<Vec<u8>>>>>,
}

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
            entries: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn get(&self, url: Option<&str>) -> Result<Option<Vec<u8>>> {
        let Some(url) = url else {
            return Ok(None);
        };
        if let Some(bytes) = self.entries.lock().await.get(url).cloned() {
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
        self.entries
            .lock()
            .await
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
    let path = path.to_owned();
    let metadata = metadata.clone();
    tokio::task::spawn_blocking(move || write_metadata_blocking(&path, &metadata, picture))
        .await
        .context("metadata worker failed")?
}

pub async fn verify_audio_file(path: &Path, expected_extension: &str) -> Result<()> {
    let path = path.to_owned();
    let expected_extension = expected_extension.to_owned();
    tokio::task::spawn_blocking(move || verify_audio_file_blocking(&path, &expected_extension))
        .await
        .context("audio verification worker failed")?
}

fn verify_audio_file_blocking(path: &Path, expected_extension: &str) -> Result<()> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() < 1024 {
        anyhow::bail!("file is only {} bytes", metadata.len());
    }
    let file = lofty::read_from_path(path)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let expected_type = match expected_extension {
        "flac" => FileType::Flac,
        "m4a" => FileType::Mp4,
        "mp3" => FileType::Mpeg,
        extension => anyhow::bail!("cannot verify unsupported extension .{extension}"),
    };
    if file.file_type() != expected_type {
        anyhow::bail!(
            "container {:?} does not match .{expected_extension}",
            file.file_type()
        );
    }
    if file.properties().duration().is_zero() {
        anyhow::bail!("audio duration is zero");
    }
    Ok(())
}

fn write_metadata_blocking(
    path: &Path,
    metadata: &TrackMetadata,
    picture: Option<Vec<u8>>,
) -> Result<()> {
    let mut file = lofty::read_from_path(path)
        .with_context(|| format!("failed to read tags from {}", path.display()))?;
    let tag_type = file.primary_tag_type();
    if file.tag(tag_type).is_none() {
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file
        .tag_mut(tag_type)
        .expect("the primary tag was inserted above");
    apply_text_metadata(tag, metadata);
    if let Some(picture) = picture {
        tag.remove_picture_type(PictureType::CoverFront);
        let mime = detect_mime(&picture);
        tag.push_picture(
            Picture::unchecked(picture)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    }
    file.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("failed to save tags to {}", path.display()))?;
    Ok(())
}

fn apply_text_metadata(tag: &mut Tag, metadata: &TrackMetadata) {
    tag.set_title(metadata.title.clone());
    tag.set_artist(metadata.artist.clone());
    if let Some(album) = &metadata.album {
        tag.set_album(album.clone());
    }
    if let Some(genre) = &metadata.genre {
        tag.set_genre(genre.clone());
    }
    if let Some(track_number) = metadata.track_number {
        tag.set_track(track_number);
    }
    if let Some(disc_number) = metadata.disc_number {
        tag.set_disk(disc_number);
    }
    if let Some(album_artist) = &metadata.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, album_artist.clone());
    }
    if let Some(year) = metadata.year {
        tag.insert_text(ItemKey::RecordingDate, year.to_string());
    }
}

fn detect_mime(data: &[u8]) -> MimeType {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        MimeType::Png
    } else if data.starts_with(b"\xff\xd8\xff") {
        MimeType::Jpeg
    } else {
        MimeType::Unknown("application/octet-stream".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_mime, verify_audio_file};
    use lofty::picture::MimeType;

    #[test]
    fn detects_common_artwork_formats() {
        assert_eq!(detect_mime(b"\xff\xd8\xffrest"), MimeType::Jpeg);
        assert_eq!(detect_mime(b"\x89PNG\r\n\x1a\nrest"), MimeType::Png);
    }

    #[tokio::test]
    async fn rejects_truncated_audio_file() {
        let path = std::env::temp_dir().join(format!(
            "ym-download-invalid-{}-{}.mp3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(&path, [0_u8; 512]).unwrap();

        let result = verify_audio_file(&path, "mp3").await;
        let _ = std::fs::remove_file(path);

        assert!(result.is_err());
    }
}
