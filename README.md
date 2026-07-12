# ya-music workspace

Rust libraries and development tools for the unofficial Yandex Music API.

## Packages

- `yandex-music-api` — async API client, models and OAuth Device Flow.
- `yandex-music-api`'s `credentials` feature — shared local credential storage for workspace applications.
- `ym-auth` — login, logout and credential inspection tool.
- `ym-download` — atomic single-track downloader using negotiated CDN streams.
- `ym-inspect` — read-only API exploration client.
- `ym-edit` — explicit liked-track and playlist mutation client.

All applications share the `default` credential profile stored outside the
repository. Run `cargo run -p ym-auth -- path` to display its location.

The single library crate uses additive feature flags:

- `oauth` (default) — Device Flow and token refresh primitives.
- `credentials` — local profiles, locking and automatic refresh; enables `oauth`.
- `downloads` — signed audio negotiation and CDN response streams.
- `lyrics` — signed plain-text and synchronized lyrics retrieval.

The API is private and may change without notice, so the library is currently
experimental. A minimal direct client looks like this:

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

The protocol-research repositories live in the ignored `references/`
directory.

## First login

```console
cargo run -p ym-auth -- login
cargo run -p ym-auth -- status
cargo run -p ym-auth -- refresh
```

File profiles are refreshed automatically when they are at least 90 days old
or within 7 days of expiry. `ym-auth refresh` forces an immediate rotation.
Environment overrides are never refreshed or persisted.

Refresh, login writes and logout use an exclusive per-profile advisory lock.
After acquiring it, refresh reloads the profile before contacting OAuth, so
concurrent applications perform at most one token rotation.

The file store uses `$XDG_STATE_HOME/yandex-music-rs/profiles/default.json` on
Linux, normally `~/.local/state/yandex-music-rs/profiles/default.json`.
Directories are mode `0700`, credential files are mode `0600`, and updates use
an atomic same-directory rename. Lock files are also mode `0600`. Tokens are
redacted from `Debug` output.

`YANDEX_MUSIC_TOKEN` can override the stored profile for a single process.

## Inspecting the API

```console
cargo run -p ym-inspect -- account
cargo run -p ym-inspect -- search "Boards of Canada"
cargo run -p ym-inspect -- track 10994777:1193829
cargo run -p ym-inspect -- album 1193829
cargo run -p ym-inspect -- likes --limit 20
cargo run -p ym-inspect -- playlists
cargo run -p ym-inspect -- playlist <owner> <kind>
cargo run -p ym-inspect -- artist <id>
cargo run -p ym-inspect -- artist-tracks <id> --page-size 100
cargo run -p ym-inspect -- artist-albums <id> --page-size 100
cargo run -p ym-inspect -- lyrics <track-id> --lrc
cargo run -p ym-inspect -- playlist-recommendations <owner> <kind>
cargo run -p ym-inspect -- stations
cargo run -p ym-inspect -- station-tracks user onyourwave
```

Pass `--json` to print the complete modeled response including fields retained
for forward compatibility.

Read-only API calls share a configurable `ReadRequestPolicy`: the default
spaces GET requests by 25 ms and retries connection failures, HTTP 429, and 5xx
responses up to three times with exponential backoff. POST mutations are never
retried automatically. Artist tracks and albums expose both explicit
`PageRequest` methods and helpers that collect every page.

## Editing the library

`ym-edit` keeps mutations separate from the read-only inspector:

```console
cargo run -p ym-edit -- like <track-id>...
cargo run -p ym-edit -- unlike <track-id>...
cargo run -p ym-edit -- playlist-create "My playlist"
cargo run -p ym-edit -- playlist-rename <kind> "New title"
cargo run -p ym-edit -- playlist-visibility <kind> public
cargo run -p ym-edit -- playlist-add <kind> <track-id> <album-id> --at 0
cargo run -p ym-edit -- playlist-remove <kind> --from 0 --to 1
cargo run -p ym-edit -- playlist-delete <kind> --yes
```

Playlist track changes first fetch the current revision and submit a typed
positional diff against that exact revision. A concurrent edit is reported as
`PlaylistRevisionConflict` and is not retried automatically. Permanent playlist
deletion requires `--yes`.

## Downloading audio

The `downloads` feature implements the signed `get-file-info` negotiation and
opens short-lived CDN response streams without forwarding the OAuth token:

```console
cargo run -p ym-download -- track <track-id>
cargo run -p ym-download -- track <track-id> --quality normal -o track.mp3
cargo run -p ym-download -- playlist <owner> <kind> -o ./playlist --jobs 4
```

The server can return a lower tier than requested. `ym-download` writes to a
same-directory `.part` file, syncs it, and only then renames it to the final
path. Playlist downloads continue past individual failures and print a final
report. FLAC-in-MP4 is losslessly remuxed to native `.flac` through `ffmpeg`;
AAC-in-MP4 remains `.m4a`, and MP3 remains `.mp3`. Existing files are preserved
unless `--force` is passed. Every completed or existing file is enriched with
title, artist, album, album artist, year, genre, album track/disc position, and
an embedded 600×600 front cover.

Resume validates each existing container and its duration before trusting it;
truncated or corrupt files are atomically replaced. Playlist progress and the
final per-track result are persisted after every completion in
`.ym-download-state.json`. Transient negotiation failures and CDN transfers use
bounded exponential retries, and every advertised CDN URL is attempted.
