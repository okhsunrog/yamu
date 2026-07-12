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
        let request = self.request(Method::GET, path)?.query(query);
        self.send(request).await
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
        let mut body: Value =
            serde_json::from_slice(&bytes).map_err(|source| Error::InvalidResponse {
                message: format!("response body is not JSON (HTTP {status})"),
                source: Some(source),
            })?;

        let api_error = body.get("error").filter(|value| !value.is_null());
        if !status.is_success() || api_error.is_some() {
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
