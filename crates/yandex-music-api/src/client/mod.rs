use std::{fmt, time::Duration};

use url::Url;

use crate::{Error, Result};

mod account;
mod albums;
mod library;
mod mutations;
mod request;
mod search;
mod tracks;

pub use search::{SearchOptions, SearchType};

const DEFAULT_BASE_URL: &str = "https://api.music.yandex.net/";

/// An asynchronous Yandex Music API client.
#[derive(Clone)]
pub struct Client {
    pub(super) http: reqwest::Client,
    pub(super) base_url: Url,
    pub(super) token: Option<String>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.token.is_some())
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
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.token.is_some())
            .field("language", &self.language)
            .field("timeout", &self.timeout)
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

    pub fn build(self) -> Result<Client> {
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                let language = reqwest::header::HeaderValue::from_str(&self.language)?;
                headers.insert(reqwest::header::ACCEPT_LANGUAGE, language);
                headers
            })
            .build()
            .map_err(Error::BuildClient)?;

        Ok(Client {
            http,
            base_url: self.base_url,
            token: self.token,
        })
    }
}
