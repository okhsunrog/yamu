#![cfg(feature = "downloader")]

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use aes::Aes128;
use ctr::{
    Ctr128BE,
    cipher::{KeyIvInit as _, StreamCipher as _},
};
use url::Url;
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
use yamu::{
    Client,
    downloader::{
        CancellationToken, DownloadEvent, DownloadPhase, DownloadPolicy, DownloadRequest,
        Downloader, Error,
    },
    media::{self, MediaBackend, TrackMetadata},
    models::{AudioCodec, DownloadInfo},
};

#[derive(Clone)]
struct TestBackend;

impl MediaBackend for TestBackend {
    fn name(&self) -> &'static str {
        "test"
    }

    async fn write_m4a_metadata(
        &self,
        _path: PathBuf,
        _metadata: TrackMetadata,
        _artwork: Option<Vec<u8>>,
    ) -> media::Result<()> {
        Ok(())
    }

    async fn remux_flac(
        &self,
        source: PathBuf,
        destination: PathBuf,
        _replace: bool,
    ) -> media::Result<()> {
        tokio::fs::copy(source, destination).await?;
        Ok(())
    }

    async fn transcode_mp3(
        &self,
        source: PathBuf,
        destination: PathBuf,
        _bitrate_kbps: u32,
        _replace: bool,
    ) -> media::Result<()> {
        tokio::fs::copy(source, destination).await?;
        Ok(())
    }

    async fn verify_m4a(&self, _path: PathBuf) -> media::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn streams_progress_and_atomically_finishes() {
    let server = MockServer::start().await;
    let body = vec![0x5a; 16 * 1024];
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&server)
        .await;
    let destination = temporary_path("progress.mp3");
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend);
    let mut events = Vec::new();

    let result = downloader
        .download(
            request(&server, AudioCodec::Mp3, None, destination.clone()),
            CancellationToken::new(),
            |event| events.push(event),
        )
        .await
        .unwrap();

    assert_eq!(result.downloaded, body.len() as u64);
    assert_eq!(tokio::fs::read(&destination).await.unwrap(), body);
    assert!(events.contains(&DownloadEvent::PhaseChanged(DownloadPhase::Downloading)));
    assert!(events.contains(&DownloadEvent::Progress {
        downloaded: result.downloaded,
        total: Some(result.downloaded),
    }));
    assert!(events.contains(&DownloadEvent::PhaseChanged(DownloadPhase::Finalizing)));
    cleanup(&destination).await;
}

#[tokio::test]
async fn decrypts_aes_ctr_while_streaming() {
    let server = MockServer::start().await;
    let plaintext = b"streaming encrypted audio bytes".repeat(512);
    let key = [0x2a_u8; 16];
    let mut encrypted = plaintext.clone();
    Ctr128BE::<Aes128>::new(&key.into(), &[0_u8; 16].into()).apply_keystream(&mut encrypted);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(encrypted))
        .mount(&server)
        .await;
    let destination = temporary_path("encrypted.flac");
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend);

    downloader
        .download(
            request(
                &server,
                AudioCodec::Flac,
                Some(hex::encode(key)),
                destination.clone(),
            ),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(tokio::fs::read(&destination).await.unwrap(), plaintext);
    cleanup(&destination).await;
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_request_and_cleans_up() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_bytes(vec![1; 4096]),
        )
        .mount(&server)
        .await;
    let destination = temporary_path("cancelled.m4a");
    let token = CancellationToken::new();
    let cancel = token.clone();
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend);

    let (result, ()) = tokio::join!(
        downloader.download(
            request(&server, AudioCodec::AacMp4, None, destination.clone()),
            token,
            |_| {},
        ),
        async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
        }
    );

    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!tokio::fs::try_exists(&destination).await.unwrap());
    cleanup(&destination).await;
}

#[tokio::test]
async fn normalizes_flac_in_mp4_through_the_backend() {
    let server = MockServer::start().await;
    let body = b"fake mp4 payload for the test backend".repeat(64);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&server)
        .await;
    let destination = temporary_path("normalized.flac");
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend);
    let mut events = Vec::new();

    downloader
        .download(
            request(&server, AudioCodec::FlacMp4, None, destination.clone()),
            CancellationToken::new(),
            |event| events.push(event),
        )
        .await
        .unwrap();

    assert_eq!(tokio::fs::read(&destination).await.unwrap(), body);
    assert!(events.contains(&DownloadEvent::PhaseChanged(DownloadPhase::Normalizing)));
    cleanup(&destination).await;
}

#[tokio::test]
async fn reports_invalid_decryption_keys_without_trying_every_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1; 4096]))
        .mount(&server)
        .await;
    let destination = temporary_path("invalid-key.flac");
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend);

    let result = downloader
        .download(
            request(
                &server,
                AudioCodec::Flac,
                Some("not-hex".to_owned()),
                destination.clone(),
            ),
            CancellationToken::new(),
            |_| {},
        )
        .await;

    assert!(matches!(result, Err(Error::InvalidDecryptionKey)));
    assert!(!tokio::fs::try_exists(&destination).await.unwrap());
    cleanup(&destination).await;
}

#[tokio::test]
async fn emits_retry_events_for_transient_source_failures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let destination = temporary_path("retry.mp3");
    let downloader = Downloader::new(Client::builder().build().unwrap(), TestBackend).with_policy(
        DownloadPolicy {
            retry_delay: Duration::from_millis(1),
            ..DownloadPolicy::default()
        },
    );
    let mut events = Vec::new();

    let result = downloader
        .download(
            request(&server, AudioCodec::Mp3, None, destination.clone()),
            CancellationToken::new(),
            |event| events.push(event),
        )
        .await;

    assert!(matches!(
        result,
        Err(Error::AllSourcesFailed { sources: 1 })
    ));
    assert!(events.contains(&DownloadEvent::Retrying {
        source: 1,
        attempt: 2,
    }));
    assert!(events.contains(&DownloadEvent::Retrying {
        source: 1,
        attempt: 3,
    }));
    cleanup(&destination).await;
}

fn request(
    server: &MockServer,
    codec: AudioCodec,
    decryption_key: Option<String>,
    destination: PathBuf,
) -> DownloadRequest {
    DownloadRequest {
        info: DownloadInfo {
            quality: "lossless".to_owned(),
            codec,
            bitrate: 1_411,
            urls: vec![Url::parse(&format!("{}/audio", server.uri())).unwrap()],
            decryption_key,
            extra: BTreeMap::new(),
        },
        destination,
        replace: true,
    }
}

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("ya-music-downloader-test-{}", Uuid::new_v4()))
        .join(name)
}

async fn cleanup(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::remove_dir_all(parent).await;
    }
}
