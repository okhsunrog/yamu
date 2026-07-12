# yandex-music-api

An unofficial asynchronous Rust client for the private Yandex Music API.

> The API is not publicly documented and may change without notice. This crate
> is currently experimental.

## Current scope

- OAuth token authentication
- OAuth Device Flow
- account status
- batch track lookup
- album lookup with tracks
- liked tracks and expansion into full tracks
- user playlists and playlist contents
- search
- forward-compatible models that preserve unknown JSON fields

## Example

```rust,no_run
use yandex_music_api::Client;

#[tokio::main]
async fn main() -> yandex_music_api::Result<()> {
    let token = std::env::var("YANDEX_MUSIC_TOKEN").expect("token is set");
    let client = Client::new(token)?;

    let results = client.search("Boards of Canada").await?;
    if let Some(tracks) = results.tracks {
        for track in tracks.results {
            println!("{}", track.title.unwrap_or_default());
        }
    }

    Ok(())
}
```

The repositories used for protocol research are cloned into the ignored
`references/` directory.

## Device authorization

The crate can obtain a Yandex Music token without printing or manually copying
it:

```console
cargo run -p yandex-music-api --example device_auth
```

Open the displayed Yandex OAuth URL, enter the user code and confirm access.
The example keeps the resulting token in memory and runs read-only smoke checks
against the account, search, track and album endpoints.
