use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Artist, Id, Track};

/// Position of a track within an album volume.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrackPosition {
    pub index: Option<u32>,
    pub volume: Option<u32>,
}

/// A Yandex Music album, optionally including tracks grouped by volume.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: Option<Id>,
    pub title: Option<String>,
    pub track_count: Option<u64>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    pub available: Option<bool>,
    pub cover_uri: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub release_date: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub duration_ms: Option<u64>,
    pub explicit: Option<bool>,
    pub track_position: Option<TrackPosition>,
    pub volumes: Option<Vec<Vec<Track>>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Album {
    /// Expands the `%%` placeholder used in Yandex artwork URIs.
    pub fn cover_url(&self, size: &str) -> Option<String> {
        self.cover_uri
            .as_deref()
            .map(|uri| format!("https://{}", uri.replace("%%", size)))
    }
}
