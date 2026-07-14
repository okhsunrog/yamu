use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, header, method, path, query_param},
};
use yamu::{Client, Error, models::Id};

#[tokio::test]
async fn fetches_tracks_and_preserves_unknown_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/tracks"))
        .and(header("authorization", "OAuth secret"))
        .and(body_string("track-ids=42&track-ids=abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": [{
                "id": "42",
                "title": "Answer",
                "durationMs": 1234,
                "futureField": { "kept": true }
            }]
        })))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .build()
        .unwrap();
    let tracks = client
        .tracks([Id::from(42), Id::from("abc")])
        .await
        .unwrap();

    assert_eq!(tracks[0].title.as_deref(), Some("Answer"));
    assert_eq!(tracks[0].duration_ms, Some(1234));
    assert_eq!(tracks[0].extra["futureField"]["kept"], true);
}

#[tokio::test]
async fn sends_search_defaults() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("text", "Boards of Canada"))
        .and(query_param("type", "all"))
        .and(query_param("page", "0"))
        .and(query_param("nocorrect", "false"))
        .and(query_param("playlist-in-best", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "text": "Boards of Canada", "tracks": { "total": 0, "results": [] } }
        })))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .build()
        .unwrap();
    let result = client.search("Boards of Canada").await.unwrap();

    assert_eq!(result.text.as_deref(), Some("Boards of Canada"));
    assert_eq!(result.tracks.unwrap().total, Some(0));
}

#[tokio::test]
async fn maps_api_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/account/status"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "unauthorized",
            "errorDescription": "Invalid OAuth token"
        })))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .build()
        .unwrap();
    let error = client.account_status().await.unwrap_err();

    match error {
        Error::Api {
            status, message, ..
        } => {
            assert_eq!(status.as_u16(), 401);
            assert_eq!(message, "Invalid OAuth token");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
