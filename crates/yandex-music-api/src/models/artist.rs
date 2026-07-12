use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Id;

/// A compact artist representation embedded in tracks and albums.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: Option<Id>,
    pub name: Option<String>,
    pub cover: Option<Cover>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Artwork information used by several API entities.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cover {
    pub uri: Option<String>,
    #[serde(default)]
    pub items_uri: Vec<String>,
    pub color: Option<String>,
    pub prefix: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub custom: Option<bool>,
    pub is_custom: Option<bool>,
    pub video_url: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Cover {
    /// Expands the `%%` placeholder in the primary or indexed artwork URI.
    pub fn url(&self, size: &str, index: usize) -> Option<String> {
        self.uri
            .as_deref()
            .or_else(|| self.items_uri.get(index).map(String::as_str))
            .map(|uri| format!("https://{}", uri.replace("%%", size)))
    }
}
