use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, method, path},
};
use yandex_music_api::{
    Client, Error,
    models::{PlaylistDiff, PlaylistTrackId, PlaylistVisibility},
};

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .build()
        .unwrap()
}

#[tokio::test]
async fn likes_and_unlikes_multiple_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/42/likes/tracks/add-multiple"))
        .and(body_string("track-ids=10&track-ids=11%3A20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "revision": 8 }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/users/42/likes/tracks/remove"))
        .and(body_string("track-ids=10&track-ids=11%3A20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "revision": 9 }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server);
    let liked = client.like_tracks(42_u64, ["10", "11:20"]).await.unwrap();
    let unliked = client.unlike_tracks(42_u64, ["10", "11:20"]).await.unwrap();

    assert_eq!(liked.revision, 8);
    assert_eq!(unliked.revision, 9);
}

#[tokio::test]
async fn creates_renames_changes_visibility_and_deletes_playlist() {
    let server = MockServer::start().await;
    for (path_value, body, title, visibility) in [
        (
            "/users/42/playlists/create",
            "title=Test+list&visibility=private",
            "Test list",
            "private",
        ),
        (
            "/users/42/playlists/100/name",
            "value=Renamed",
            "Renamed",
            "private",
        ),
        (
            "/users/42/playlists/100/visibility",
            "value=public",
            "Renamed",
            "public",
        ),
    ] {
        Mock::given(method("POST"))
            .and(path(path_value))
            .and(body_string(body))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "uid": 42,
                    "kind": 100,
                    "title": title,
                    "visibility": visibility,
                    "revision": 1
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/users/42/playlists/100/delete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = client_for(&server);
    let created = client
        .create_playlist(42_u64, "Test list", PlaylistVisibility::Private)
        .await
        .unwrap();
    let renamed = client
        .rename_playlist(42_u64, 100_u64, "Renamed")
        .await
        .unwrap();
    let public = client
        .set_playlist_visibility(42_u64, 100_u64, PlaylistVisibility::Public)
        .await
        .unwrap();
    client.delete_playlist(42_u64, 100_u64).await.unwrap();

    assert_eq!(created.title.as_deref(), Some("Test list"));
    assert_eq!(renamed.title.as_deref(), Some("Renamed"));
    assert_eq!(public.visibility.as_deref(), Some("public"));
}

#[tokio::test]
async fn sends_typed_playlist_diff() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/42/playlists/100/change"))
        .and(body_string(concat!(
            "kind=100&revision=3&diff=",
            "%5B%7B%22op%22%3A%22insert%22%2C%22at%22%3A1%2C%22tracks%22%3A%5B",
            "%7B%22id%22%3A%2210%22%2C%22albumId%22%3A%2220%22%7D%5D%7D%2C",
            "%7B%22op%22%3A%22delete%22%2C%22from%22%3A2%2C%22to%22%3A3%7D%5D"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "uid": 42, "kind": 100, "revision": 4, "tracks": [] }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let diff = PlaylistDiff::new()
        .insert(1, [PlaylistTrackId::new("10", "20")])
        .delete(2, 3);
    let playlist = client_for(&server)
        .change_playlist(42_u64, 100_u64, 3, &diff)
        .await
        .unwrap();

    assert_eq!(playlist.revision, Some(4));
}

#[tokio::test]
async fn reports_playlist_revision_conflict_without_retrying() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/users/42/playlists/100/change"))
        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": "conflict",
            "message": "playlist was changed"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let error = client_for(&server)
        .change_playlist(42_u64, 100_u64, 7, &PlaylistDiff::new().delete(0, 1))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        Error::PlaylistRevisionConflict {
            expected_revision: 7,
            ..
        }
    ));
}
