use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Album, Artist, Id};

/// A Yandex Music track.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: Id,
    pub title: Option<String>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub albums: Vec<Album>,
    pub available: Option<bool>,
    pub duration_ms: Option<u64>,
    pub cover_uri: Option<String>,
    pub explicit: Option<bool>,
    pub content_warning: Option<String>,
    pub version: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Track {
    /// Expands the `%%` placeholder used in Yandex artwork URIs.
    pub fn cover_url(&self, size: &str) -> Option<String> {
        self.cover_uri
            .as_deref()
            .map(|uri| format!("https://{}", uri.replace("%%", size)))
    }
}
