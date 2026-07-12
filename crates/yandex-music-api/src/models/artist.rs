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
    pub color: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
