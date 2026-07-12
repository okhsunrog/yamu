use reqwest::{Method, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::Client;
use crate::{Error, Result};

impl Client {
    pub(super) async fn get<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T> {
        let max_attempts = self.read_policy.max_attempts.clamp(1, 10);
        for attempt in 1..=max_attempts {
            self.wait_for_read_slot().await;
            let request = self.request(Method::GET, path)?.query(query);
            match self.send(request).await {
                Err(error) if attempt < max_attempts && is_transient_read_error(&error) => {
                    let exponent = u32::from(attempt.saturating_sub(1).min(10));
                    let multiplier = 1_u32 << exponent;
                    let delay = self
                        .read_policy
                        .initial_backoff
                        .checked_mul(multiplier)
                        .unwrap_or(std::time::Duration::from_secs(60))
                        .min(std::time::Duration::from_secs(60));
                    tokio::time::sleep(delay).await;
                }
                result => return result,
            }
        }
        unreachable!("the read-attempt loop always returns")
    }

    pub(super) fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
        let url = self.base_url.join(path.trim_start_matches('/'))?;
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.header(reqwest::header::AUTHORIZATION, format!("OAuth {token}"));
        }
        Ok(request)
    }

    pub(super) async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let parsed = serde_json::from_slice(&bytes);
        if !status.is_success() {
            return Err(match parsed {
                Ok(body) => api_error_from_response(status, body),
                Err(_) => Error::Api {
                    status,
                    message: status
                        .canonical_reason()
                        .unwrap_or("non-JSON API error")
                        .to_owned(),
                    body: None,
                },
            });
        }
        let mut body: Value = parsed.map_err(|source| Error::InvalidResponse {
            message: format!("response body is not JSON (HTTP {status})"),
            source: Some(source),
        })?;

        let api_error = body.get("error").filter(|value| !value.is_null());
        if api_error.is_some() {
            return Err(api_error_from_response(status, body));
        }

        let result = match body.get_mut("result") {
            Some(value) if !value.is_null() => value.take(),
            _ => body,
        };

        serde_json::from_value(result).map_err(|source| Error::InvalidResponse {
            message: "the `result` field does not match the expected model".to_owned(),
            source: Some(source),
        })
    }

    async fn wait_for_read_slot(&self) {
        let mut next = self.read_gate.lock().await;
        let now = std::time::Instant::now();
        if *next > now {
            tokio::time::sleep_until((*next).into()).await;
        }
        let now = std::time::Instant::now();
        *next = now
            .checked_add(self.read_policy.min_interval)
            .unwrap_or(now);
    }
}

fn is_transient_read_error(error: &Error) -> bool {
    match error {
        Error::Http(error) => {
            error.is_connect()
                || error.is_timeout()
                || error.status().is_some_and(|status| {
                    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                })
        }
        Error::Api { status, .. } => {
            *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
        }
        _ => false,
    }
}

fn api_error_from_response(status: StatusCode, body: Value) -> Error {
    let message = body
        .get("error_description")
        .or_else(|| body.get("errorDescription"))
        .or_else(|| body.get("message"))
        .or_else(|| body.get("error"))
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("unknown API error")
                .to_owned()
        });

    Error::Api {
        status,
        message,
        body: Some(body),
    }
}
