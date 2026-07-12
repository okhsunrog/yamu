use std::{collections::BTreeMap, fmt};

use serde::Deserialize;
use serde_json::Value;

/// Codes returned when a Device Flow authorization session starts.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCode")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &self.user_code)
            .field("verification_url", &self.verification_url)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .field("extra", &self.extra)
            .finish()
    }
}

/// OAuth credentials returned after Device Flow confirmation.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct OAuthToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl OAuthToken {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn expires_in(&self) -> Option<u64> {
        self.expires_in
    }

    pub fn token_type(&self) -> Option<&str> {
        self.token_type.as_deref()
    }

    pub fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }

    pub fn into_access_token(self) -> String {
        self.access_token
    }
}

impl fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"[REDACTED]")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .field("extra", &self.extra)
            .finish()
    }
}
