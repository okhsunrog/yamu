use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, method, path, query_param},
};
use yamu::{Client, models::Id};

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .build()
        .unwrap()
}

#[tokio::test]
async fn fetches_and_expands_liked_tracks() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/42/likes/tracks"))
        .and(query_param("if-modified-since-revision", "7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "library": {
                    "uid": 42,
                    "revision": 8,
                    "tracks": [
                        { "id": "10", "albumId": "20", "timestamp": "2026-07-12T00:00:00Z" },
                        { "id": 11 }
                    ]
                }
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/tracks"))
        .and(body_string("track-ids=10%3A20&track-ids=11"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [
                { "id": "10", "title": "First" },
                { "id": 11, "title": "Second" }
            ]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let library = client.liked_tracks(42_u64, 7).await.unwrap().unwrap();
    let tracks = client.tracks_from_list(&library).await.unwrap();

    assert_eq!(library.revision, 8);
    assert_eq!(library.tracks[0].track_id(), "10:20");
    assert_eq!(library.tracks[1].track_id(), "11");
    assert_eq!(tracks[0].title.as_deref(), Some("First"));
}

#[tokio::test]
async fn liked_tracks_returns_none_when_revision_is_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/42/likes/tracks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "library": null }
        })))
        .mount(&server)
        .await;

    let library = client_for(&server).liked_tracks(42_u64, 8).await.unwrap();

    assert!(library.is_none());
}

#[tokio::test]
async fn fetches_user_playlist_summaries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/42/playlists/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [{
                "owner": { "uid": 42, "login": "alice" },
                "kind": 100,
                "title": "Favorites",
                "trackCount": 2,
                "futureField": true
            }]
        })))
        .mount(&server)
        .await;

    let playlists = client_for(&server).user_playlists(42_u64).await.unwrap();

    assert_eq!(playlists[0].title.as_deref(), Some("Favorites"));
    assert_eq!(playlists[0].playlist_id().unwrap().to_string(), "42:100");
    assert_eq!(playlists[0].extra["futureField"], true);
}

#[tokio::test]
async fn fetches_full_playlist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/alice/playlists/100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "owner": { "uid": 42, "login": "alice" },
                "kind": "100",
                "title": "Favorites",
                "revision": 3,
                "tracks": [{
                    "id": "10",
                    "albumId": "20",
                    "track": { "id": "10", "title": "First" }
                }],
                "pager": { "total": 1, "page": 0, "perPage": 100 }
            }
        })))
        .mount(&server)
        .await;

    let playlist = client_for(&server)
        .playlist(Id::from("alice"), Id::from("100"))
        .await
        .unwrap();

    assert_eq!(playlist.revision, Some(3));
    assert_eq!(playlist.tracks[0].track_id(), "10:20");
    assert_eq!(
        playlist.tracks[0]
            .track
            .as_ref()
            .and_then(|track| track.title.as_deref()),
        Some("First")
    );
    assert_eq!(playlist.pager.unwrap().total, Some(1));
}
