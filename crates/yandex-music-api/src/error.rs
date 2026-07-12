use std::time::Duration;

use reqwest::StatusCode;

/// Errors returned by the client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),

    #[error("failed to build the HTTP client: {0}")]
    BuildClient(reqwest::Error),

    #[error("invalid HTTP header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Yandex Music API error (HTTP {status}): {message}")]
    Api {
        status: StatusCode,
        message: String,
        body: Option<serde_json::Value>,
    },

    #[error(
        "Yandex OAuth error (HTTP {status}): {code}{description}",
        description = description
            .as_deref()
            .map(|value| format!(": {value}"))
            .unwrap_or_default()
    )]
    OAuth {
        status: StatusCode,
        code: String,
        description: Option<String>,
        body: Option<serde_json::Value>,
    },

    #[error("device authorization timed out after {timeout:?}")]
    DeviceAuthorizationTimedOut { timeout: Duration },

    #[error("invalid API response: {message}")]
    InvalidResponse {
        message: String,
        #[source]
        source: Option<serde_json::Error>,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
