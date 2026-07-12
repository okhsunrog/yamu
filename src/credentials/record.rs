use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::models::OAuthToken;

use super::{Error, Result};

pub(crate) const FORMAT_VERSION: u32 = 1;

/// Persisted credentials shared by workspace applications.
#[derive(Deserialize, Serialize)]
pub struct Credentials {
    version: u32,
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    obtained_at: u64,
    expires_at: Option<u64>,
}

impl Credentials {
    pub fn from_oauth_token(token: &OAuthToken) -> Result<Self> {
        let obtained_at = unix_timestamp()?;
        let expires_at = token
            .expires_in()
            .and_then(|expires_in| obtained_at.checked_add(expires_in));

        Ok(Self {
            version: FORMAT_VERSION,
            access_token: token.access_token().to_owned(),
            refresh_token: token.refresh_token().map(str::to_owned),
            token_type: token.token_type().map(str::to_owned),
            obtained_at,
            expires_at,
        })
    }

    pub fn from_access_token(access_token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            version: FORMAT_VERSION,
            access_token: access_token.into(),
            refresh_token: None,
            token_type: None,
            obtained_at: unix_timestamp()?,
            expires_at: None,
        })
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn token_type(&self) -> Option<&str> {
        self.token_type.as_deref()
    }

    pub fn obtained_at(&self) -> u64 {
        self.obtained_at
    }

    pub fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }

    pub fn expires_in(&self) -> Result<Option<Duration>> {
        let now = unix_timestamp()?;
        Ok(self
            .expires_at
            .map(|expires_at| Duration::from_secs(expires_at.saturating_sub(now))))
    }

    pub fn age(&self) -> Result<Duration> {
        Ok(Duration::from_secs(
            unix_timestamp()?.saturating_sub(self.obtained_at),
        ))
    }

    pub fn is_expired(&self) -> Result<bool> {
        let now = unix_timestamp()?;
        Ok(self.expires_at.is_some_and(|expires_at| expires_at <= now))
    }

    pub(crate) fn validate_version(&self) -> Result<()> {
        if self.version == FORMAT_VERSION {
            Ok(())
        } else {
            Err(Error::UnsupportedVersion {
                found: self.version,
                expected: FORMAT_VERSION,
            })
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("version", &self.version)
            .field("access_token", &"[REDACTED]")
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("token_type", &self.token_type)
            .field("obtained_at", &self.obtained_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

fn unix_timestamp() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| Error::InvalidSystemClock)
}
