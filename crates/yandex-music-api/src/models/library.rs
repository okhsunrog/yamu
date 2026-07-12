use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Artist, Cover, Id, Track};

/// A compact track entry used by liked-track lists and playlists.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackShort {
    pub id: Id,
    pub timestamp: Option<String>,
    pub album_id: Option<Id>,
    pub play_count: Option<u64>,
    pub recent: Option<bool>,
    pub track: Option<Track>,
    pub original_index: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TrackShort {
    /// Returns the composite `track:album` ID accepted by the batch endpoint.
    pub fn track_id(&self) -> String {
        match &self.album_id {
            Some(album_id) => format!("{}:{album_id}", self.id),
            None => self.id.to_string(),
        }
    }
}

/// A revisioned list of compact tracks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TracksList {
    pub uid: Id,
    pub revision: u64,
    #[serde(default)]
    pub tracks: Vec<TrackShort>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TracksList {
    pub fn track_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.tracks.iter().map(TrackShort::track_id)
    }
}

/// A playlist owner or another user embedded in an API response.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub uid: Option<Id>,
    pub login: Option<String>,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub full_name: Option<String>,
    pub verified: Option<bool>,
    #[serde(default)]
    pub regions: Vec<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Pagination metadata embedded in some playlist responses.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pager {
    pub total: Option<u64>,
    pub page: Option<u64>,
    pub per_page: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A stable owner/kind playlist identifier.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct PlaylistId {
    pub owner: Id,
    pub kind: Id,
}

impl PlaylistId {
    pub fn new(owner: impl Into<Id>, kind: impl Into<Id>) -> Self {
        Self {
            owner: owner.into(),
            kind: kind.into(),
        }
    }
}

impl std::fmt::Display for PlaylistId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.owner, self.kind)
    }
}

/// A Yandex Music playlist, either a summary or a full response with tracks.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub owner: Option<User>,
    pub cover: Option<Cover>,
    pub uid: Option<Id>,
    pub kind: Option<Id>,
    pub title: Option<String>,
    pub track_count: Option<u64>,
    pub revision: Option<u64>,
    pub snapshot: Option<u64>,
    pub visibility: Option<String>,
    pub collective: Option<bool>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub available: Option<bool>,
    pub duration_ms: Option<u64>,
    pub og_image: Option<String>,
    pub description: Option<String>,
    pub description_formatted: Option<String>,
    pub playlist_uuid: Option<String>,
    pub generated_playlist_type: Option<String>,
    pub animated_cover_uri: Option<String>,
    pub likes_count: Option<u64>,
    #[serde(default)]
    pub coauthors: Vec<Id>,
    #[serde(default)]
    pub top_artist: Vec<Artist>,
    #[serde(default)]
    pub tracks: Vec<TrackShort>,
    pub pager: Option<Pager>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Playlist {
    pub fn playlist_id(&self) -> Option<PlaylistId> {
        let owner = self
            .owner
            .as_ref()
            .and_then(|owner| owner.uid.clone())
            .or_else(|| self.uid.clone())?;
        Some(PlaylistId {
            owner,
            kind: self.kind.clone()?,
        })
    }
}
