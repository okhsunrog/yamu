#![cfg(feature = "credentials")]

use std::{fs, path::PathBuf};

use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, method, path},
};
use yandex_music_api::{
    auth::DeviceAuth,
    credentials::{CredentialStore, Credentials, RefreshPolicy},
    models::OAuthToken,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("yandex-music-lifecycle-{}", uuid::Uuid::new_v4())))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn oauth_token(access: &str, refresh: &str) -> OAuthToken {
    serde_json::from_value(serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        "expires_in": 31_536_000,
        "token_type": "bearer"
    }))
    .unwrap()
}

fn auth_for(server: &MockServer) -> DeviceAuth {
    DeviceAuth::builder()
        .base_url(server.uri())
        .unwrap()
        .client_id("client-id")
        .client_secret("client-secret")
        .build()
        .unwrap()
}

#[tokio::test]
async fn forced_refresh_rotates_and_persists_credentials() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    store
        .save(
            "default",
            &Credentials::from_oauth_token(&oauth_token("old-access", "old-refresh")).unwrap(),
        )
        .unwrap();

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

    let (credentials, refreshed) = store
        .refresh_if_needed("default", &auth_for(&server), RefreshPolicy::force())
        .await
        .unwrap();

    assert!(refreshed);
    assert_eq!(credentials.access_token(), "new-access");
    assert_eq!(credentials.refresh_token(), Some("new-refresh"));
    let persisted = store.load("default").unwrap();
    assert_eq!(persisted.access_token(), "new-access");
    assert_eq!(persisted.refresh_token(), Some("new-refresh"));
}

#[tokio::test]
async fn fresh_credentials_do_not_contact_oauth() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    store
        .save(
            "default",
            &Credentials::from_oauth_token(&oauth_token("access", "refresh")).unwrap(),
        )
        .unwrap();
    let server = MockServer::start().await;

    let (credentials, refreshed) = store
        .refresh_if_needed("default", &auth_for(&server), RefreshPolicy::default())
        .await
        .unwrap();

    assert!(!refreshed);
    assert_eq!(credentials.access_token(), "access");
}

#[tokio::test]
async fn concurrent_refresh_performs_only_one_oauth_request() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    store
        .save(
            "default",
            &Credentials::from_oauth_token(&oauth_token("old-access", "old-refresh")).unwrap(),
        )
        .unwrap();
    age_profile(&store);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(100))
                .set_body_json(serde_json::json!({
                    "access_token": "new-access",
                    "refresh_token": "new-refresh",
                    "expires_in": 31_536_000,
                    "token_type": "bearer"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let auth = auth_for(&server);
    let first_store = store.clone();
    let first_auth = auth.clone();
    let first = tokio::spawn(async move {
        first_store
            .refresh_if_needed("default", &first_auth, RefreshPolicy::default())
            .await
            .unwrap()
            .1
    });
    let second_store = store.clone();
    let second = tokio::spawn(async move {
        second_store
            .refresh_if_needed("default", &auth, RefreshPolicy::default())
            .await
            .unwrap()
            .1
    });

    let (first, second) = tokio::join!(first, second);
    assert_ne!(first.unwrap(), second.unwrap());
    assert_eq!(store.load("default").unwrap().access_token(), "new-access");
}

fn age_profile(store: &CredentialStore) {
    let path = store.profile_path("default").unwrap();
    let mut value: serde_json::Value =
        serde_json::from_reader(fs::File::open(&path).unwrap()).unwrap();
    value["obtained_at"] = serde_json::json!(0);
    serde_json::to_writer_pretty(fs::File::create(&path).unwrap(), &value).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
