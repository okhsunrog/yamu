use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Id;

/// Basic account data returned by `/account/status`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub now: Option<String>,
    pub uid: Option<Id>,
    pub login: Option<String>,
    pub full_name: Option<String>,
    pub display_name: Option<String>,
    pub service_available: Option<bool>,
    pub region: Option<u64>,
    pub child: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Account status, including fields not yet modeled by this crate.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub account: Option<Account>,
    #[serde(default)]
    pub permissions: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
