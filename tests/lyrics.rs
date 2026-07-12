#![cfg(feature = "lyrics")]

use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};
use yandex_music_api::{Client, models::LyricsFormat};

#[test]
fn parses_lyrics_formats_for_cli_consumers() {
    assert_eq!("text".parse(), Ok(LyricsFormat::Text));
    assert_eq!("txt".parse(), Ok(LyricsFormat::Text));
    assert_eq!("lrc".parse(), Ok(LyricsFormat::Lrc));
    assert!("karaoke".parse::<LyricsFormat>().is_err());
}

#[tokio::test]
async fn fetches_signed_lyrics_metadata_and_text_without_oauth_on_storage() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/tracks/10/lyrics"))
        .and(query_param("format", "LRC"))
        .and(header(
            "x-yandex-music-client",
            "YandexMusicAndroid/24023621",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "downloadUrl": format!("{}/lyrics.txt", server.uri()),
                "lyricId": 1,
                "writers": ["Writer"]
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/lyrics.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[00:00]hello"))
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .unwrap()
        .token("secret")
        .build()
        .unwrap();
    let lyrics = client
        .track_lyrics("10:20", LyricsFormat::Lrc)
        .await
        .unwrap();
    let text = client.fetch_lyrics(&lyrics).await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let storage_request = requests
        .iter()
        .find(|request| request.url.path() == "/lyrics.txt")
        .unwrap();

    assert_eq!(text, "[00:00]hello");
    assert!(storage_request.headers.get("authorization").is_none());
}
