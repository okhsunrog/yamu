use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{body_string, method, path},
};
use yandex_music_api::{
    Error,
    auth::{DeviceAuth, DeviceTokenPoll},
};

#[derive(Clone)]
struct SlowDownOnce {
    calls: Arc<AtomicUsize>,
}

impl Respond for SlowDownOnce {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "slow_down",
                "error_description": "Poll less frequently"
            }))
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access-secret"
            }))
        }
    }
}

fn auth_for(server: &MockServer) -> DeviceAuth {
    DeviceAuth::builder()
        .base_url(server.uri())
        .unwrap()
        .client_id("client-id")
        .client_secret("client-secret")
        .device_id("device-id")
        .device_name("test-device")
        .build()
        .unwrap()
}

#[tokio::test]
async fn requests_device_code_with_expected_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .and(body_string(
            "client_id=client-id&device_id=device-id&device_name=test-device",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code",
            "user_code": "USER-CODE",
            "verification_url": "https://oauth.yandex.ru/device",
            "expires_in": 300,
            "interval": 5
        })))
        .mount(&server)
        .await;

    let code = auth_for(&server).request_device_code().await.unwrap();

    assert_eq!(code.user_code, "USER-CODE");
    assert_eq!(code.expires_in, 300);
    assert!(!format!("{code:?}").contains("device-code"));
}

#[tokio::test]
async fn pending_poll_returns_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string(
            "grant_type=device_code&code=device-code&client_id=client-id&client_secret=client-secret",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "authorization_pending",
            "error_description": "User code has not been confirmed"
        })))
        .mount(&server)
        .await;

    let token = auth_for(&server)
        .poll_device_token("device-code")
        .await
        .unwrap();

    assert!(token.is_none());
}

#[tokio::test]
async fn slow_down_poll_is_preserved_for_interval_management() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "slow_down",
            "error_description": "Poll less frequently"
        })))
        .mount(&server)
        .await;

    let event = auth_for(&server)
        .poll_device_token_event("device-code")
        .await
        .unwrap();

    assert!(matches!(event, DeviceTokenPoll::SlowDown));
}

#[tokio::test]
async fn successful_poll_returns_redacted_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-secret",
            "refresh_token": "refresh-secret",
            "expires_in": 31_536_000,
            "token_type": "bearer"
        })))
        .mount(&server)
        .await;

    let token = auth_for(&server)
        .poll_device_token("device-code")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(token.access_token(), "access-secret");
    assert_eq!(token.refresh_token(), Some("refresh-secret"));
    let debug = format!("{token:?}");
    assert!(!debug.contains("access-secret"));
    assert!(!debug.contains("refresh-secret"));
}

#[tokio::test]
async fn refreshes_oauth_token_with_expected_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string(
            "grant_type=refresh_token&refresh_token=old-refresh&client_id=client-id&client_secret=client-secret",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 31_536_000,
            "token_type": "bearer"
        })))
        .mount(&server)
        .await;

    let token = auth_for(&server)
        .refresh_token("old-refresh")
        .await
        .unwrap();

    assert_eq!(token.access_token(), "new-access");
    assert_eq!(token.refresh_token(), Some("new-refresh"));
}

#[tokio::test]
async fn non_pending_oauth_error_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "expired_token",
            "error_description": "Device code expired"
        })))
        .mount(&server)
        .await;

    let error = auth_for(&server)
        .poll_device_token("device-code")
        .await
        .unwrap_err();

    match error {
        Error::OAuth {
            status,
            code,
            description,
            ..
        } => {
            assert_eq!(status.as_u16(), 400);
            assert_eq!(code, "expired_token");
            assert_eq!(description.as_deref(), Some("Device code expired"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn authorize_calls_callback_and_returns_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code",
            "user_code": "USER-CODE",
            "verification_url": "https://oauth.yandex.ru/device",
            "expires_in": 300,
            "interval": 1
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-secret"
        })))
        .mount(&server)
        .await;

    let mut seen_code = None;
    let token = auth_for(&server)
        .authorize(|code| seen_code = Some(code.user_code.clone()))
        .await
        .unwrap();

    assert_eq!(seen_code.as_deref(), Some("USER-CODE"));
    assert_eq!(token.access_token(), "access-secret");
}

#[tokio::test]
async fn authorize_waits_before_polling_and_honors_slow_down() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code",
            "user_code": "USER-CODE",
            "verification_url": "https://oauth.yandex.ru/device",
            "expires_in": 30,
            "interval": 1
        })))
        .mount(&server)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(SlowDownOnce {
            calls: Arc::clone(&calls),
        })
        .mount(&server)
        .await;
    let started = tokio::time::Instant::now();

    let token = auth_for(&server).authorize(|_| {}).await.unwrap();

    assert_eq!(token.access_token(), "access-secret");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(started.elapsed() >= std::time::Duration::from_secs(7));
}

#[tokio::test]
async fn authorize_times_out_when_code_is_already_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code",
            "user_code": "USER-CODE",
            "verification_url": "https://oauth.yandex.ru/device",
            "expires_in": 0,
            "interval": 5
        })))
        .mount(&server)
        .await;

    let error = auth_for(&server).authorize(|_| {}).await.unwrap_err();

    assert!(matches!(error, Error::DeviceAuthorizationTimedOut { .. }));
}
