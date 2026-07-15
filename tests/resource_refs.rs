use yamu::resource::{
    AlbumRef, ArtistRef, PlaylistRef, PlaylistSourceRef, PlaylistUuidRef, TrackRef,
};

#[test]
fn parses_track_url_and_discards_non_semantic_parts() {
    let reference: TrackRef = "https://music.yandex.ru/album/19097174/track/94298678?utm_source=web&utm_medium=copy_link#player"
        .parse()
        .unwrap();

    assert_eq!(reference.track_id(), "94298678");
    assert_eq!(reference.album_id(), Some("19097174"));
    assert_eq!(
        reference.canonical_url().as_str(),
        "https://music.yandex.ru/album/19097174/track/94298678"
    );
}

#[test]
fn parses_simple_ids_and_resource_urls() {
    assert_eq!(
        "19097174".parse::<AlbumRef>().unwrap().album_id(),
        "19097174"
    );
    assert_eq!(
        "https://music.yandex.ru/album/19097174?utm_source=copy"
            .parse::<AlbumRef>()
            .unwrap()
            .album_id(),
        "19097174"
    );
    assert_eq!(
        "https://music.yandex.ru/artist/123?utm_source=copy"
            .parse::<ArtistRef>()
            .unwrap()
            .artist_id(),
        "123"
    );
    let playlist: PlaylistRef = "https://music.yandex.ru/users/example/playlists/42?utm_source=web"
        .parse()
        .unwrap();
    assert_eq!(playlist.owner(), "example");
    assert_eq!(playlist.kind(), "42");
    assert_eq!("example:42".parse::<PlaylistRef>().unwrap(), playlist);
}

#[test]
fn parses_current_playlist_uuid_urls() {
    let uuid = "fa1b8d08-71c7-3ed8-9c58-8eebbdccdf7f";
    let reference: PlaylistSourceRef = format!(
        "https://music.yandex.ru/playlists/{uuid}?utm_source=web&utm_medium=copy_link#player"
    )
    .parse()
    .unwrap();

    let PlaylistSourceRef::Uuid(reference) = reference else {
        panic!("expected UUID playlist reference");
    };
    assert_eq!(reference.playlist_uuid(), uuid);
    assert_eq!(
        reference.canonical_url().as_str(),
        format!("https://music.yandex.ru/playlists/{uuid}")
    );
    assert_eq!(
        format!("lk.{uuid}")
            .parse::<PlaylistUuidRef>()
            .unwrap()
            .playlist_uuid(),
        format!("lk.{uuid}")
    );
    assert!("not-a-uuid".parse::<PlaylistUuidRef>().is_err());
}

#[test]
fn rejects_wrong_resource_and_foreign_hosts() {
    assert!(
        "https://music.yandex.ru/album/1"
            .parse::<TrackRef>()
            .is_err()
    );
    assert!("https://example.com/album/1".parse::<AlbumRef>().is_err());
    assert!(
        "https://music.yandex.evil.com/album/1"
            .parse::<AlbumRef>()
            .is_err()
    );
    assert!("..".parse::<AlbumRef>().is_err());
    assert!(PlaylistRef::new(".", "42").is_err());
}
