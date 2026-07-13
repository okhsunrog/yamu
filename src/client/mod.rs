use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Mutex;
use url::Url;

use crate::{Error, Result};

mod account;
mod albums;
mod artists;
#[cfg(feature = "downloads")]
mod downloads;
mod library;
#[cfg(feature = "lyrics")]
mod lyrics;
mod mutations;
mod recommendations;
mod request;
mod search;
mod tracks;

pub use search::{SearchOptions, SearchType};

const DEFAULT_BASE_URL: &str = "https://api.music.yandex.net/";

/// An asynchronous Yandex Music API client.
#[derive(Clone)]
pub struct Client {
    pub(super) http: reqwest::Client,
    pub(super) media_http: reqwest::Client,
    pub(super) base_url: Url,
    pub(super) token: Option<String>,
    pub(super) read_policy: ReadRequestPolicy,
    pub(super) read_gate: Arc<Mutex<Instant>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.token.is_some())
            .field("read_policy", &self.read_policy)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Creates a client authenticated with an OAuth token.
    pub fn new(token: impl Into<String>) -> Result<Self> {
        Self::builder().token(token).build()
    }
}

/// Builder for [`Client`].
#[derive(Clone)]
pub struct ClientBuilder {
    base_url: Url,
    token: Option<String>,
    language: String,
    timeout: Duration,
    media_connect_timeout: Duration,
    media_read_timeout: Duration,
    read_policy: ReadRequestPolicy,
}

/// Throttling and retry policy applied only to idempotent GET requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRequestPolicy {
    pub max_attempts: u8,
    pub min_interval: Duration,
    pub initial_backoff: Duration,
}

impl Default for ReadRequestPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            min_interval: Duration::from_millis(25),
            initial_backoff: Duration::from_millis(200),
        }
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.token.is_some())
            .field("language", &self.language)
            .field("timeout", &self.timeout)
            .field("media_connect_timeout", &self.media_connect_timeout)
            .field("media_read_timeout", &self.media_read_timeout)
            .field("read_policy", &self.read_policy)
            .finish()
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            base_url: Url::parse(DEFAULT_BASE_URL).expect("the default API URL is valid"),
            token: None,
            language: "ru".to_owned(),
            timeout: Duration::from_secs(15),
            media_connect_timeout: Duration::from_secs(15),
            media_read_timeout: Duration::from_secs(30),
            read_policy: ReadRequestPolicy::default(),
        }
    }
}

impl ClientBuilder {
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

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

    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the connection timeout used for CDN and object-storage requests.
    pub fn media_connect_timeout(mut self, timeout: Duration) -> Self {
        self.media_connect_timeout = timeout;
        self
    }

    /// Sets the per-read stall timeout used for CDN and object-storage bodies.
    ///
    /// Unlike [`Self::timeout`], this timeout resets whenever body data arrives,
    /// so large audio files are not constrained by a short total deadline.
    pub fn media_read_timeout(mut self, timeout: Duration) -> Self {
        self.media_read_timeout = timeout;
        self
    }

    pub fn read_request_policy(mut self, policy: ReadRequestPolicy) -> Self {
        self.read_policy = policy;
        self
    }

    pub fn build(self) -> Result<Client> {
        let mut headers = reqwest::header::HeaderMap::new();
        let language = reqwest::header::HeaderValue::from_str(&self.language)?;
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, language);
        headers.insert(
            reqwest::header::HeaderName::from_static("x-yandex-music-client"),
            reqwest::header::HeaderValue::from_static("YandexMusicAndroid/24023621"),
        );
        let user_agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(user_agent)
            .default_headers(headers.clone())
            .build()
            .map_err(Error::BuildClient)?;
        let media_http = reqwest::Client::builder()
            .connect_timeout(self.media_connect_timeout)
            .read_timeout(self.media_read_timeout)
            .user_agent(user_agent)
            .default_headers(headers)
            .build()
            .map_err(Error::BuildClient)?;

        let read_gate = Instant::now()
            .checked_sub(self.read_policy.min_interval)
            .unwrap_or_else(Instant::now);
        Ok(Client {
            http,
            media_http,
            base_url: self.base_url,
            token: self.token,
            read_policy: self.read_policy,
            read_gate: Arc::new(Mutex::new(read_gate)),
        })
    }
}
