#![cfg(feature = "downloads")]

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
use yandex_music_api::{
    Client, Error, ReadRequestPolicy,
    models::{AudioCodec, DownloadOptions, DownloadQuality},
};

#[derive(Clone)]
struct FailFileInfoOnce {
    calls: Arc<AtomicUsize>,
    audio_url: String,
}

impl Respond for FailFileInfoOnce {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            ResponseTemplate::new(503).set_body_string("temporarily unavailable")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "downloadInfo": {
                    "quality": "lossless",
                    "codec": "flac",
                    "bitrate": 1411,
                    "urls": [self.audio_url]
                }
            }))
        }
    }
}

#[test]
fn parses_download_quality_names_and_protocol_values() {
    assert_eq!("low".parse(), Ok(DownloadQuality::Low));
    assert_eq!("lq".parse(), Ok(DownloadQuality::Low));
    assert_eq!("normal".parse(), Ok(DownloadQuality::Normal));
    assert_eq!("nq".parse(), Ok(DownloadQuality::Normal));
    assert_eq!("lossless".parse(), Ok(DownloadQuality::Lossless));
    assert!("studio".parse::<DownloadQuality>().is_err());
}

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .build()
        .unwrap()
}

#[tokio::test]
async fn negotiates_download_info_with_required_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/get-file-info"))
        .and(query_param("trackId", "10"))
        .and(query_param("quality", "nq"))
        .and(query_param("codecs", "flac,mp3"))
        .and(query_param("transports", "raw"))
        .and(header("authorization", "OAuth secret"))
        .and(header("x-yandex-music-multi-auth-user-id", "42"))
        .and(header("x-yandex-music-client", "YandexMusicWebNext/1.0.0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "downloadInfo": {
                "quality": "nq",
                "codec": "mp3",
                "bitrate": 192,
                "urls": [format!("{}/audio", server.uri())],
                "futureField": true
            }
        })))
        .mount(&server)
        .await;

    let options = DownloadOptions {
        quality: DownloadQuality::Normal,
        codecs: vec![AudioCodec::Flac, AudioCodec::Mp3],
    };
    let info = client_for(&server)
        .download_info(42_u64, 10_u64, &options)
        .await
        .unwrap();

    assert_eq!(info.codec, AudioCodec::Mp3);
    assert_eq!(info.bitrate, 192);
    assert_eq!(info.urls.len(), 1);
    assert_eq!(info.extra["futureField"], true);
    assert!(!format!("{info:?}").contains("/audio"));
    let requests = server.received_requests().await.unwrap();
    let query = requests[0].url.query_pairs().collect::<Vec<_>>();
    assert!(
        query
            .iter()
            .any(|(key, value)| key == "ts" && !value.is_empty())
    );
    assert!(
        query
            .iter()
            .any(|(key, value)| key == "sign" && !value.is_empty())
    );
}

#[tokio::test]
async fn maps_download_info_error_from_result_wrapper() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/get-file-info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "name": "track-download-info-error",
                "message": "not-allowed"
            }
        })))
        .mount(&server)
        .await;

    let error = client_for(&server)
        .download_info(42_u64, 10_u64, &DownloadOptions::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::DownloadUnavailable { ref name, ref message }
            if name == "track-download-info-error" && message == "not-allowed"
    ));
}

#[tokio::test]
async fn applies_read_policy_to_download_info_requests() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/get-file-info"))
        .respond_with(FailFileInfoOnce {
            calls: calls.clone(),
            audio_url: format!("{}/audio", server.uri()),
        })
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .read_request_policy(ReadRequestPolicy {
            max_attempts: 2,
            min_interval: Duration::ZERO,
            initial_backoff: Duration::ZERO,
        })
        .build()
        .unwrap();

    let info = client
        .download_info(42_u64, 10_u64, &DownloadOptions::default())
        .await
        .unwrap();

    assert_eq!(info.codec, AudioCodec::Flac);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn streams_cdn_audio_without_forwarding_oauth_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/audio"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"audio"))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let bytes = client
        .open_audio_stream(&format!("{}/audio", server.uri()).parse().unwrap())
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();

    assert_eq!(&bytes[..], b"audio");
    assert!(requests[0].headers.get("authorization").is_none());
}
