use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};
use yandex_music_api::{
    Client,
    models::{ArtistAlbumSort, PageRequest, StationId},
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
async fn fetches_paginated_artist_catalog() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/artists/7/tracks"))
        .and(query_param("page", "2"))
        .and(query_param("page-size", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "tracks": [{"id": 10, "title": "Track"}],
                "pager": {"page": 2, "perPage": 50, "total": 101}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/artists/7/direct-albums"))
        .and(query_param("sort-by", "rating"))
        .and(query_param("page", "0"))
        .and(query_param("page-size", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "albums": [{"id": 20, "title": "Album"}],
                "pager": {"page": 0, "perPage": 20, "total": 1}
            }
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let tracks = client
        .artist_tracks(7_u64, PageRequest::new(2, 50))
        .await
        .unwrap();
    let albums = client
        .artist_albums(7_u64, PageRequest::new(0, 20), ArtistAlbumSort::Rating)
        .await
        .unwrap();

    assert_eq!(tracks.tracks[0].title.as_deref(), Some("Track"));
    assert_eq!(tracks.pager.unwrap().total, Some(101));
    assert_eq!(albums.albums[0].title.as_deref(), Some("Album"));
}

#[tokio::test]
async fn fetches_playlist_and_rotor_recommendations() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/42/playlists/100/recommendations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"batchId": "batch", "tracks": [{"id": 10, "title": "Suggested"}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rotor/stations/dashboard"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "dashboardId": "dashboard",
                "stations": [{"station": {"id": {"type": "user", "tag": "onyourwave"}, "name": "My Wave"}}]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/rotor/station/user:onyourwave/tracks"))
        .and(query_param("settings2", "true"))
        .and(query_param("queue", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "id": {"type": "user", "tag": "onyourwave"},
                "batchId": "next",
                "sequence": [{"type": "track", "liked": true, "track": {"id": 11, "title": "Next"}}]
            }
        })))
        .mount(&server)
        .await;

    let client = client_for(&server);
    let recommendations = client
        .playlist_recommendations(42_u64, 100_u64)
        .await
        .unwrap();
    let dashboard = client.stations_dashboard().await.unwrap();
    let tracks = client
        .station_tracks(
            &StationId {
                kind: "user".to_owned(),
                tag: "onyourwave".to_owned(),
            },
            Some(10_u64.into()),
        )
        .await
        .unwrap();

    assert_eq!(
        recommendations.tracks[0].title.as_deref(),
        Some("Suggested")
    );
    assert_eq!(
        dashboard.stations[0]
            .station
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("My Wave")
    );
    assert_eq!(
        tracks.sequence[0].track.as_ref().unwrap().title.as_deref(),
        Some("Next")
    );
}
