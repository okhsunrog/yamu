use std::{fmt, time::Duration};

use reqwest::StatusCode;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::time::Instant;
use url::Url;

use crate::{
    Error, Result,
    models::{DeviceCode, OAuthToken},
};

const DEFAULT_OAUTH_BASE_URL: &str = "https://oauth.yandex.ru/";
// Public credentials extracted from an official Yandex Music client. They identify the
// client application and cannot be kept secret in a distributed native application.
const DEFAULT_CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
const DEFAULT_CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
const DEFAULT_DEVICE_NAME: &str = "yamu-rs";

/// A client for Yandex OAuth Device Flow.
#[derive(Clone)]
pub struct DeviceAuth {
    http: reqwest::Client,
    base_url: Url,
    client_id: String,
    client_secret: String,
    device_id: String,
    device_name: String,
}

/// Result of one OAuth Device Flow token poll.
#[derive(Debug)]
pub enum DeviceTokenPoll {
    Pending,
    SlowDown,
    Authorized(OAuthToken),
}

impl fmt::Debug for DeviceAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuth")
            .field("base_url", &self.base_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl DeviceAuth {
    pub fn builder() -> DeviceAuthBuilder {
        DeviceAuthBuilder::default()
    }

    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Starts a new authorization session and returns codes for the user.
    pub async fn request_device_code(&self) -> Result<DeviceCode> {
        #[derive(Serialize)]
        struct Form<'a> {
            client_id: &'a str,
            device_id: &'a str,
            device_name: &'a str,
        }

        self.post_form(
            "device/code",
            &Form {
                client_id: &self.client_id,
                device_id: &self.device_id,
                device_name: &self.device_name,
            },
        )
        .await
    }

    /// Polls once for a token. `None` means that confirmation is still pending.
    ///
    /// Callers that manage their own polling interval should use
    /// [`Self::poll_device_token_event`] so they can honor `slow_down`.
    pub async fn poll_device_token(&self, device_code: &str) -> Result<Option<OAuthToken>> {
        Ok(match self.poll_device_token_event(device_code).await? {
            DeviceTokenPoll::Authorized(token) => Some(token),
            DeviceTokenPoll::Pending | DeviceTokenPoll::SlowDown => None,
        })
    }

    /// Polls once and preserves the RFC 8628 `slow_down` signal.
    pub async fn poll_device_token_event(&self, device_code: &str) -> Result<DeviceTokenPoll> {
        #[derive(Serialize)]
        struct Form<'a> {
            grant_type: &'static str,
            code: &'a str,
            client_id: &'a str,
            client_secret: &'a str,
        }

        let response = self
            .http
            .post(self.base_url.join("token")?)
            .form(&Form {
                grant_type: "device_code",
                code: device_code,
                client_id: &self.client_id,
                client_secret: &self.client_secret,
            })
            .send()
            .await?;
        let status = response.status();
        let body = parse_json_response(response).await?;

        match oauth_error_code(&body) {
            Some("authorization_pending") => return Ok(DeviceTokenPoll::Pending),
            Some("slow_down") => return Ok(DeviceTokenPoll::SlowDown),
            _ => {}
        }
        if !status.is_success() || oauth_error_code(&body).is_some() {
            return Err(oauth_error_from_response(status, body));
        }

        deserialize_oauth(body).map(DeviceTokenPoll::Authorized)
    }

    /// Exchanges a refresh token for a current access/refresh token pair.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthToken> {
        #[derive(Serialize)]
        struct Form<'a> {
            grant_type: &'static str,
            refresh_token: &'a str,
            client_id: &'a str,
            client_secret: &'a str,
        }

        self.post_form(
            "token",
            &Form {
                grant_type: "refresh_token",
                refresh_token,
                client_id: &self.client_id,
                client_secret: &self.client_secret,
            },
        )
        .await
    }

    /// Runs Device Flow until the user confirms access or the code expires.
    pub async fn authorize<F>(&self, on_code: F) -> Result<OAuthToken>
    where
        F: FnOnce(&DeviceCode),
    {
        let code = self.request_device_code().await?;
        on_code(&code);

        let timeout = Duration::from_secs(code.expires_in);
        let mut interval = Duration::from_secs(code.interval.max(1));
        let started = Instant::now();

        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(Error::DeviceAuthorizationTimedOut { timeout });
            }
            tokio::time::sleep(interval.min(remaining)).await;
            if started.elapsed() >= timeout {
                return Err(Error::DeviceAuthorizationTimedOut { timeout });
            }

            match self.poll_device_token_event(&code.device_code).await {
                Ok(DeviceTokenPoll::Authorized(token)) => return Ok(token),
                Ok(DeviceTokenPoll::Pending) => {}
                Ok(DeviceTokenPoll::SlowDown) => {
                    interval = interval.saturating_add(Duration::from_secs(5));
                }
                Err(error) if is_transient_poll_error(&error) => {
                    interval = interval.saturating_mul(2);
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn post_form<T, F>(&self, path: &str, form: &F) -> Result<T>
    where
        T: DeserializeOwned,
        F: Serialize + ?Sized,
    {
        let response = self
            .http
            .post(self.base_url.join(path)?)
            .form(form)
            .send()
            .await?;
        let status = response.status();
        let body = parse_json_response(response).await?;

        if !status.is_success() || oauth_error_code(&body).is_some() {
            return Err(oauth_error_from_response(status, body));
        }

        deserialize_oauth(body)
    }
}

/// Builder for [`DeviceAuth`].
#[derive(Clone)]
pub struct DeviceAuthBuilder {
    base_url: Url,
    client_id: String,
    client_secret: String,
    device_id: String,
    device_name: String,
    timeout: Duration,
}

impl fmt::Debug for DeviceAuthBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceAuthBuilder")
            .field("base_url", &self.base_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Default for DeviceAuthBuilder {
    fn default() -> Self {
        Self {
            base_url: Url::parse(DEFAULT_OAUTH_BASE_URL).expect("the default OAuth URL is valid"),
            client_id: DEFAULT_CLIENT_ID.to_owned(),
            client_secret: DEFAULT_CLIENT_SECRET.to_owned(),
            device_id: uuid::Uuid::new_v4().simple().to_string(),
            device_name: DEFAULT_DEVICE_NAME.to_owned(),
            timeout: Duration::from_secs(15),
        }
    }
}

impl DeviceAuthBuilder {
    pub fn base_url(mut self, base_url: impl AsRef<str>) -> Result<Self> {
        let mut url = Url::parse(base_url.as_ref())?;
        if url.cannot_be_a_base() {
            return Err(Error::NonHierarchicalBaseUrl(url.to_string()));
        }
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        self.base_url = url;
        Ok(self)
    }

    pub fn client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = client_id.into();
        self
    }

    pub fn client_secret(mut self, client_secret: impl Into<String>) -> Self {
        self.client_secret = client_secret.into();
        self
    }

    pub fn device_id(mut self, device_id: impl Into<String>) -> Self {
        self.device_id = device_id.into();
        self
    }

    pub fn device_name(mut self, device_name: impl Into<String>) -> Self {
        self.device_name = device_name.into();
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn build(self) -> Result<DeviceAuth> {
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(Error::BuildClient)?;

        Ok(DeviceAuth {
            http,
            base_url: self.base_url,
            client_id: self.client_id,
            client_secret: self.client_secret,
            device_id: self.device_id,
            device_name: self.device_name,
        })
    }
}

async fn parse_json_response(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let bytes = response.bytes().await?;
    serde_json::from_slice(&bytes).map_err(|source| Error::InvalidResponse {
        message: format!("OAuth response body is not JSON (HTTP {status})"),
        source: Some(source),
    })
}

fn deserialize_oauth<T: DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|source| Error::InvalidResponse {
        message: "OAuth response does not match the expected model".to_owned(),
        source: Some(source),
    })
}

fn oauth_error_code(body: &Value) -> Option<&str> {
    body.get("error").and_then(Value::as_str)
}

fn oauth_error_from_response(status: StatusCode, body: Value) -> Error {
    let code = oauth_error_code(&body)
        .unwrap_or("unknown_oauth_error")
        .to_owned();
    let description = body
        .get("error_description")
        .or_else(|| body.get("errorDescription"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    Error::OAuth {
        status,
        code,
        description,
        body: Some(body),
    }
}

fn is_transient_poll_error(error: &Error) -> bool {
    matches!(error, Error::Http(error) if error.is_connect() || error.is_timeout())
}
