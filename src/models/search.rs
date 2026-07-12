use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Album, Artist, Track};

/// A paginated section of search results.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(bound(deserialize = "T: Deserialize<'de>", serialize = "T: Serialize"))]
pub struct SearchPage<T> {
    pub total: Option<u64>,
    pub per_page: Option<u64>,
    pub order: Option<u64>,
    #[serde(default)]
    pub results: Vec<T>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Results returned by `/search`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub text: Option<String>,
    pub best: Option<Value>,
    pub tracks: Option<SearchPage<Track>>,
    pub albums: Option<SearchPage<Album>>,
    pub artists: Option<SearchPage<Artist>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
