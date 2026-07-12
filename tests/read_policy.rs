use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};
use yandex_music_api::{Client, ReadRequestPolicy};

#[derive(Clone)]
struct FailOnce {
    calls: Arc<AtomicUsize>,
}

impl Respond for FailOnce {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            ResponseTemplate::new(503).set_body_string("temporarily unavailable")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {"account": {"uid": 42}}
            }))
        }
    }
}

#[tokio::test]
async fn retries_transient_get_errors_and_accepts_non_json_error_bodies() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/account/status"))
        .respond_with(FailOnce {
            calls: calls.clone(),
        })
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .read_request_policy(ReadRequestPolicy {
            max_attempts: 2,
            min_interval: Duration::ZERO,
            initial_backoff: Duration::ZERO,
        })
        .build()
        .unwrap();

    let status = client.account_status().await.unwrap();

    assert_eq!(status.account.unwrap().uid.unwrap().to_string(), "42");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn spaces_read_requests_by_the_configured_interval() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"account": {"uid": 42}}
        })))
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .read_request_policy(ReadRequestPolicy {
            max_attempts: 1,
            min_interval: Duration::from_millis(50),
            initial_backoff: Duration::ZERO,
        })
        .build()
        .unwrap();

    client.account_status().await.unwrap();
    let started = Instant::now();
    client.account_status().await.unwrap();

    assert!(started.elapsed() >= Duration::from_millis(40));
}

#[tokio::test]
async fn never_retries_post_mutations() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/42/likes/tracks/add-multiple"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .read_request_policy(ReadRequestPolicy {
            max_attempts: 5,
            min_interval: Duration::ZERO,
            initial_backoff: Duration::ZERO,
        })
        .build()
        .unwrap();

    let result = client.like_tracks(42_u64, [10_u64]).await;

    assert!(result.is_err());
}
