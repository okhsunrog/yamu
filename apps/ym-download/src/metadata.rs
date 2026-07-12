use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result};
use lofty::{
    config::WriteOptions,
    file::{AudioFile, FileType, TaggedFileExt},
    picture::{MimeType, Picture, PictureType},
    tag::{Accessor, ItemKey, Tag, TagType},
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
    pub lyrics: Option<EmbeddedLyrics>,
}

#[derive(Clone, Debug)]
pub struct EmbeddedLyrics {
    pub text: String,
    pub synchronized: bool,
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
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("m4a"))
    {
        return write_mp4_metadata(path, metadata, picture).await;
    }
    let path = path.to_owned();
    let metadata = metadata.clone();
    tokio::task::spawn_blocking(move || write_metadata_blocking(&path, &metadata, picture))
        .await
        .context("metadata worker failed")?
}

pub async fn verify_audio_file(path: &Path, expected_extension: &str) -> Result<()> {
    let path = path.to_owned();
    let expected_extension = expected_extension.to_owned();
    let blocking_path = path.clone();
    let blocking_extension = expected_extension.clone();
    tokio::task::spawn_blocking(move || {
        verify_audio_file_blocking(&blocking_path, &blocking_extension)
    })
    .await
    .context("audio verification worker failed")??;
    if expected_extension == "m4a" {
        verify_mp4_audio_decode(&path).await?;
    }
    Ok(())
}

async fn verify_mp4_audio_decode(path: &Path) -> Result<()> {
    let output = tokio::process::Command::new("ffmpeg")
        .arg("-nostdin")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-f", "null", "-"])
        .output()
        .await
        .context("failed to run ffmpeg while validating M4A audio")?;
    if !output.status.success() {
        anyhow::bail!(
            "M4A audio decode failed: {}",
            summarize_ffmpeg_error(&output.stderr)
        );
    }
    Ok(())
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

async fn write_mp4_metadata(
    path: &Path,
    metadata: &TrackMetadata,
    picture: Option<Vec<u8>>,
) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .context("M4A path must contain a file name")?
        .to_string_lossy();
    let output_path = parent.join(format!(".{file_name}.metadata-{}.m4a", std::process::id()));
    let picture_path = picture.as_ref().map(|picture| {
        let extension = match detect_mime(picture) {
            MimeType::Png => "png",
            _ => "jpg",
        };
        parent.join(format!(
            ".{file_name}.cover-{}.{extension}",
            std::process::id()
        ))
    });
    if let (Some(picture_path), Some(picture)) = (&picture_path, &picture) {
        tokio::fs::write(picture_path, picture).await?;
    }

    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .arg("-nostdin")
        .arg("-y")
        .args(["-v", "error", "-i"])
        .arg(path);
    if let Some(picture_path) = &picture_path {
        command.arg("-i").arg(picture_path);
    }
    command.args(["-map", "0:a:0", "-map_metadata", "-1", "-c:a", "copy"]);
    if picture_path.is_some() {
        command.args([
            "-map",
            "1:v:0",
            "-c:v",
            "copy",
            "-disposition:v:0",
            "attached_pic",
        ]);
    }
    push_ffmpeg_metadata(&mut command, "title", Some(&metadata.title));
    push_ffmpeg_metadata(&mut command, "artist", Some(&metadata.artist));
    push_ffmpeg_metadata(&mut command, "album", metadata.album.as_deref());
    push_ffmpeg_metadata(
        &mut command,
        "album_artist",
        metadata.album_artist.as_deref(),
    );
    push_ffmpeg_metadata(&mut command, "genre", metadata.genre.as_deref());
    let year = metadata.year.map(|value| value.to_string());
    push_ffmpeg_metadata(&mut command, "date", year.as_deref());
    let track = metadata.track_number.map(|value| value.to_string());
    push_ffmpeg_metadata(&mut command, "track", track.as_deref());
    let disc = metadata.disc_number.map(|value| value.to_string());
    push_ffmpeg_metadata(&mut command, "disc", disc.as_deref());
    push_ffmpeg_metadata(
        &mut command,
        "lyrics",
        metadata.lyrics.as_ref().map(|lyrics| lyrics.text.as_str()),
    );
    command
        .args(["-movflags", "+faststart", "-f", "ipod"])
        .arg(&output_path);

    let output = command.output().await;
    if let Some(picture_path) = &picture_path {
        let _ = tokio::fs::remove_file(picture_path).await;
    }
    let output = output.context("failed to run ffmpeg for M4A tags")?;
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&output_path).await;
        anyhow::bail!(
            "ffmpeg M4A metadata remux failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    tokio::fs::File::open(&output_path)
        .await?
        .sync_all()
        .await?;
    #[cfg(windows)]
    if tokio::fs::try_exists(path).await? {
        tokio::fs::remove_file(path).await?;
    }
    tokio::fs::rename(&output_path, path).await?;
    Ok(())
}

fn push_ffmpeg_metadata(command: &mut tokio::process::Command, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        command.arg("-metadata").arg(format!("{key}={value}"));
    }
}

fn summarize_ffmpeg_error(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ")
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
    if let Some(lyrics) = &metadata.lyrics {
        tag.remove_key(ItemKey::Lyrics);
        tag.remove_key(ItemKey::UnsyncLyrics);
        let key = if lyrics.synchronized && tag.tag_type() != TagType::Id3v2 {
            ItemKey::Lyrics
        } else {
            ItemKey::UnsyncLyrics
        };
        tag.insert_text(key, lyrics.text.clone());
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        EmbeddedLyrics, TrackMetadata, apply_text_metadata, detect_mime, verify_audio_file,
        write_mp4_metadata,
    };
    use lofty::picture::MimeType;
    use lofty::tag::{ItemKey, Tag, TagType};

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

    #[test]
    fn embeds_plain_and_synchronized_lyrics_with_portable_keys() {
        let mut metadata = TrackMetadata {
            title: "Title".to_owned(),
            artist: "Artist".to_owned(),
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            cover_url: None,
            lyrics: Some(EmbeddedLyrics {
                text: "[00:00]line".to_owned(),
                synchronized: true,
            }),
        };
        let mut vorbis = Tag::new(TagType::VorbisComments);
        apply_text_metadata(&mut vorbis, &metadata);
        assert_eq!(vorbis.get_string(ItemKey::Lyrics), Some("[00:00]line"));

        let mut id3 = Tag::new(TagType::Id3v2);
        apply_text_metadata(&mut id3, &metadata);
        assert_eq!(id3.get_string(ItemKey::UnsyncLyrics), Some("[00:00]line"));

        metadata.lyrics.as_mut().unwrap().synchronized = false;
        let mut plain = Tag::new(TagType::VorbisComments);
        apply_text_metadata(&mut plain, &metadata);
        assert_eq!(plain.get_string(ItemKey::UnsyncLyrics), Some("[00:00]line"));
    }

    #[tokio::test]
    async fn mp4_metadata_remux_preserves_audio_and_one_cover() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ym-download-mp4-tags-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let audio = directory.join("track.m4a");
        let cover = directory.join("cover.jpg");
        if tokio::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .is_err()
        {
            tokio::fs::remove_dir_all(directory).await.unwrap();
            return;
        }
        assert!(
            tokio::process::Command::new("ffmpeg")
                .arg("-nostdin")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.1",
                    "-c:a",
                    "aac",
                    "-f",
                    "ipod",
                ])
                .arg(&audio)
                .status()
                .await
                .unwrap()
                .success()
        );
        assert!(
            tokio::process::Command::new("ffmpeg")
                .arg("-nostdin")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=red:s=32x32",
                    "-frames:v",
                    "1",
                    "-update",
                    "1",
                ])
                .arg(&cover)
                .status()
                .await
                .unwrap()
                .success()
        );
        let metadata = TrackMetadata {
            title: "Title".to_owned(),
            artist: "Artist".to_owned(),
            album: Some("Album".to_owned()),
            album_artist: Some("Album artist".to_owned()),
            genre: Some("Genre".to_owned()),
            year: Some(2026),
            track_number: Some(2),
            disc_number: Some(1),
            cover_url: None,
            lyrics: None,
        };
        write_mp4_metadata(
            &audio,
            &metadata,
            Some(tokio::fs::read(&cover).await.unwrap()),
        )
        .await
        .unwrap();
        verify_audio_file(&audio, "m4a").await.unwrap();

        let probe = tokio::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type:format_tags=title,album_artist",
                "-of",
                "json",
            ])
            .arg(&audio)
            .output()
            .await
            .unwrap();
        assert!(probe.status.success());
        let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(
            probe["streams"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|stream| stream["codec_type"] == "audio")
                .count(),
            1
        );
        assert_eq!(
            probe["streams"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|stream| stream["codec_type"] == "video")
                .count(),
            1
        );
        assert_eq!(probe["format"]["tags"]["title"], "Title");
        assert_eq!(probe["format"]["tags"]["album_artist"], "Album artist");

        tokio::fs::remove_dir_all(directory).await.unwrap();
    }
}
