use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{Method, Response};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::Url;

use super::Client;
use crate::{
    Error, Result,
    models::{DownloadInfo, DownloadOptions, Id},
};

// Public protocol constant extracted from an official Yandex Music client.
const SIGN_KEY: &[u8] = b"7tvSmFbyf5hJnIHhCimDDD";
const TRANSPORT: &str = "raw";

impl Client {
    /// Negotiates the best available audio source for a track.
    ///
    /// The requested quality is a ceiling: the server may return a lower tier.
    /// Returned CDN URLs are short-lived and should not be persisted.
    pub async fn download_info(
        &self,
        user_id: impl Into<Id>,
        track_id: impl Into<Id>,
        options: &DownloadOptions,
    ) -> Result<DownloadInfo> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidSystemClock)?
            .as_secs();
        self.download_info_at(user_id.into(), track_id.into(), options, timestamp)
            .await
    }

    /// Opens a streaming HTTP response for one URL from [`DownloadInfo`].
    ///
    /// This request deliberately does not forward the OAuth token to the CDN.
    pub async fn open_audio_stream(&self, url: &Url) -> Result<Response> {
        self.media_http
            .get(url.clone())
            .send()
            .await?
            .error_for_status()
            .map_err(Error::Http)
    }

    async fn download_info_at(
        &self,
        user_id: Id,
        track_id: Id,
        options: &DownloadOptions,
        timestamp: u64,
    ) -> Result<DownloadInfo> {
        if options.codecs.is_empty() {
            return Err(Error::InvalidDownloadRequest(
                "at least one codec is required".to_owned(),
            ));
        }

        let track_id = track_id.to_string();
        let codecs = options
            .codecs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let quality = options.quality.to_string();
        let sign = sign_file_info(timestamp, &track_id, &quality, &codecs, TRANSPORT);
        let user_id = user_id.to_string();

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Query<'a> {
            ts: u64,
            track_id: &'a str,
            quality: &'a str,
            codecs: &'a str,
            transports: &'a str,
            sign: &'a str,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct FileInfoResponse {
            #[serde(default, alias = "download_info")]
            download_info: Option<DownloadInfo>,
            name: Option<String>,
            message: Option<String>,
        }

        let query = Query {
            ts: timestamp,
            track_id: &track_id,
            quality: &quality,
            codecs: &codecs,
            transports: TRANSPORT,
            sign: &sign,
        };
        let response: FileInfoResponse = self
            .send_read(|| {
                Ok(self
                    .request(Method::GET, "get-file-info")?
                    .header("x-yandex-music-client", "YandexMusicWebNext/1.0.0")
                    .header("x-yandex-music-without-invocation-info", "1")
                    .header("x-yandex-music-multi-auth-user-id", &user_id)
                    .header(reqwest::header::ORIGIN, "https://music.yandex.ru")
                    .header(reqwest::header::REFERER, "https://music.yandex.ru/")
                    .query(&query))
            })
            .await?;
        match response.download_info {
            Some(info) if info.urls.is_empty() => Err(Error::DownloadUnavailable {
                name: response
                    .name
                    .unwrap_or_else(|| "no-download-urls".to_owned()),
                message: response
                    .message
                    .unwrap_or_else(|| "the server returned no audio URLs".to_owned()),
            }),
            Some(info) => Ok(info),
            None => Err(Error::DownloadUnavailable {
                name: response
                    .name
                    .unwrap_or_else(|| "track-download-info-error".to_owned()),
                message: response
                    .message
                    .unwrap_or_else(|| "the server returned no download information".to_owned()),
            }),
        }
    }
}

fn sign_file_info(
    timestamp: u64,
    track_id: &str,
    quality: &str,
    codecs: &str,
    transport: &str,
) -> String {
    let codecs_without_commas = codecs.replace(',', "");
    let message = format!("{timestamp}{track_id}{quality}{codecs_without_commas}{transport}");
    let mut mac = Hmac::<Sha256>::new_from_slice(SIGN_KEY).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    STANDARD_NO_PAD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    use super::sign_file_info;
    use crate::{
        Client,
        models::{AudioCodec, DownloadOptions, DownloadQuality, Id},
    };

    #[test]
    fn signs_known_file_info_request() {
        assert_eq!(
            sign_file_info(
                1_700_000_000,
                "12345",
                "lossless",
                "flac,aac,he-aac,mp3,flac-mp4,aac-mp4,he-aac-mp4",
                "raw",
            ),
            "Nm6It392fRGnljyGblG06Vq9OfnOmvKJj/esqr06yFg"
        );
    }

    #[tokio::test]
    async fn constructs_the_complete_known_file_info_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/get-file-info"))
            .and(query_param("ts", "1700000000"))
            .and(query_param("trackId", "12345"))
            .and(query_param("quality", "lossless"))
            .and(query_param(
                "codecs",
                "flac,aac,he-aac,mp3,flac-mp4,aac-mp4,he-aac-mp4",
            ))
            .and(query_param("transports", "raw"))
            .and(query_param(
                "sign",
                "Nm6It392fRGnljyGblG06Vq9OfnOmvKJj/esqr06yFg",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "downloadInfo": {
                    "quality": "lossless",
                    "codec": "flac",
                    "bitrate": 1411,
                    "urls": [format!("{}/audio", server.uri())]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .base_url(server.uri())
            .unwrap()
            .token("secret")
            .build()
            .unwrap();
        let options = DownloadOptions {
            quality: DownloadQuality::Lossless,
            codecs: vec![
                AudioCodec::Flac,
                AudioCodec::Aac,
                AudioCodec::HeAac,
                AudioCodec::Mp3,
                AudioCodec::FlacMp4,
                AudioCodec::AacMp4,
                AudioCodec::HeAacMp4,
            ],
        };

        client
            .download_info_at(Id::from(42_u64), Id::from("12345"), &options, 1_700_000_000)
            .await
            .unwrap();
    }
}
