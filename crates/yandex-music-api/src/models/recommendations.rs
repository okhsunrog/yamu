use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Track, User};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistRecommendations {
    #[serde(default)]
    pub tracks: Vec<Track>,
    pub batch_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct StationId {
    #[serde(rename = "type")]
    pub kind: String,
    pub tag: String,
}

impl std::fmt::Display for StationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind, self.tag)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub id: Option<StationId>,
    pub name: Option<String>,
    pub id_for_from: Option<String>,
    pub full_image_url: Option<String>,
    pub parent_id: Option<StationId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationResult {
    pub station: Option<Station>,
    pub settings: Option<Value>,
    pub settings2: Option<Value>,
    pub explanation: Option<String>,
    pub rup_title: Option<String>,
    pub rup_description: Option<String>,
    pub custom_name: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationDashboard {
    pub dashboard_id: Option<String>,
    #[serde(default)]
    pub stations: Vec<StationResult>,
    pub pumpkin: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationSequence {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub track: Option<Track>,
    pub liked: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StationTracks {
    pub id: Option<StationId>,
    #[serde(default)]
    pub sequence: Vec<StationSequence>,
    pub batch_id: Option<String>,
    pub pumpkin: Option<bool>,
    pub user: Option<User>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
