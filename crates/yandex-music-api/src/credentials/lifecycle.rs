use std::time::Duration;

use crate::auth::DeviceAuth;

use super::{CredentialStore, Credentials, Error, ProfileLock, Result, TOKEN_ENV};

const DEFAULT_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
const DEFAULT_EXPIRY_MARGIN: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Policy controlling proactive token refresh.
#[derive(Clone, Copy, Debug)]
pub struct RefreshPolicy {
    pub max_age: Duration,
    pub expiry_margin: Duration,
    pub force: bool,
}

impl RefreshPolicy {
    pub fn force() -> Self {
        Self {
            force: true,
            ..Self::default()
        }
    }
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            max_age: DEFAULT_MAX_AGE,
            expiry_margin: DEFAULT_EXPIRY_MARGIN,
            force: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Environment,
    Profile,
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Environment => f.write_str("environment"),
            Self::Profile => f.write_str("profile"),
        }
    }
}

/// Credentials resolved for an application, together with lifecycle metadata.
#[derive(Debug)]
pub struct ResolvedCredentials {
    pub credentials: Credentials,
    pub source: CredentialSource,
    pub refreshed: bool,
}

impl CredentialStore {
    /// Resolves an environment override or a stored profile, refreshing the
    /// stored profile when policy requires it.
    pub async fn resolve(
        &self,
        profile: &str,
        auth: &DeviceAuth,
        policy: RefreshPolicy,
    ) -> Result<ResolvedCredentials> {
        if let Ok(token) = std::env::var(TOKEN_ENV)
            && !token.is_empty()
        {
            return Ok(ResolvedCredentials {
                credentials: Credentials::from_access_token(token)?,
                source: CredentialSource::Environment,
                refreshed: false,
            });
        }

        let (credentials, refreshed) = self.refresh_if_needed(profile, auth, policy).await?;
        Ok(ResolvedCredentials {
            credentials,
            source: CredentialSource::Profile,
            refreshed,
        })
    }

    /// Refreshes a stored profile with a double-check after acquiring its lock.
    pub async fn refresh_if_needed(
        &self,
        profile: &str,
        auth: &DeviceAuth,
        policy: RefreshPolicy,
    ) -> Result<(Credentials, bool)> {
        let credentials = self.load(profile)?;
        if !needs_refresh(&credentials, policy)? {
            return Ok((credentials, false));
        }

        let _lock = self.lock_profile_async(profile).await?;
        let credentials = self.load(profile)?;
        if !needs_refresh(&credentials, policy)? {
            return Ok((credentials, false));
        }

        let refresh_token = credentials
            .refresh_token()
            .ok_or_else(|| Error::MissingRefreshToken(profile.to_owned()))?;
        let token = auth.refresh_token(refresh_token).await?;
        let refreshed = Credentials::from_oauth_token(&token)?;
        self.save_unlocked(profile, &refreshed)?;
        Ok((refreshed, true))
    }

    async fn lock_profile_async(&self, profile: &str) -> Result<ProfileLock> {
        let store = self.clone();
        let profile = profile.to_owned();
        tokio::task::spawn_blocking(move || store.lock_profile(&profile)).await?
    }
}

fn needs_refresh(credentials: &Credentials, policy: RefreshPolicy) -> Result<bool> {
    if policy.force || credentials.is_expired()? || credentials.age()? >= policy.max_age {
        return Ok(true);
    }
    Ok(credentials
        .expires_in()?
        .is_some_and(|remaining| remaining <= policy.expiry_margin))
}
