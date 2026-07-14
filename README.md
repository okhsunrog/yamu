# yamu workspace

Rust libraries and development tools for the unofficial Yandex Music API.

## Packages

- `yamu` — async API client, models and OAuth Device Flow.
- `yamu`'s `credentials` feature — shared local credential storage for workspace applications.
- `yamu-auth` — login, logout and credential inspection tool.
- `yamu-download` — atomic single-track downloader using negotiated CDN streams.
- `yamu-inspect` — read-only API exploration client.
- `yamu-edit` — explicit liked-track and playlist mutation client.

All applications share the `default` credential profile stored outside the
repository. Run `cargo run -p yamu-auth -- path` to display its location.

The single library crate uses additive feature flags:

- `oauth` (default) — Device Flow and token refresh primitives.
- `credentials` — local profiles, locking and automatic refresh; enables `oauth`.
- `downloads` — signed audio negotiation and CDN response streams.
- `downloader` — reusable atomic transfer pipeline with byte progress,
  cancellation, retries, streaming AES-CTR decryption, and normalization;
  enables `downloads` and `media`.
- `lyrics` — signed plain-text and synchronized lyrics retrieval.
- `media` — backend-neutral audio tagging, normalization, and validation API.
- `media-ffmpeg-cli` — desktop backend that invokes an installed `ffmpeg`.
- `media-ffmpeg` — in-process backend linked to FFmpeg libraries, intended for
  Android and other environments without a child-process executable.

The two FFmpeg features implement the same `MediaBackend` contract and can be
tested together. `media-ffmpeg` does not require the `ffmpeg` executable at
runtime, but the target application must build and package compatible FFmpeg
libraries.

The API is private and may change without notice, so the library is currently
experimental. A minimal direct client looks like this:

```rust,no_run
use yamu::Client;

#[tokio::main]
async fn main() -> yamu::Result<()> {
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
cargo run -p yamu-auth -- login
cargo run -p yamu-auth -- status
cargo run -p yamu-auth -- refresh
```

File profiles are refreshed automatically when they are at least 90 days old
or within 7 days of expiry. `yamu-auth refresh` forces an immediate rotation.
Environment overrides are never refreshed or persisted.

Refresh, login writes and logout use an exclusive per-profile advisory lock.
After acquiring it, refresh reloads the profile before contacting OAuth, so
concurrent applications perform at most one token rotation.

The file store uses `$XDG_STATE_HOME/yandex-music-rs/profiles/default.json` on
Linux, normally `~/.local/state/yandex-music-rs/profiles/default.json`.
The legacy directory name is intentionally retained so existing logins continue
to work after the crate rename.
Directories are mode `0700`, credential files are mode `0600`, and updates use
an atomic same-directory rename. Lock files are also mode `0600`. Tokens are
redacted from `Debug` output.

`YANDEX_MUSIC_TOKEN` can override the stored profile for a single process.

## Inspecting the API

```console
cargo run -p yamu-inspect -- account
cargo run -p yamu-inspect -- search "Boards of Canada"
cargo run -p yamu-inspect -- track 10994777
cargo run -p yamu-inspect -- track 'https://music.yandex.ru/album/1193829/track/10994777?utm_source=web'
cargo run -p yamu-inspect -- album 1193829
cargo run -p yamu-inspect -- likes --limit 20
cargo run -p yamu-inspect -- playlists
cargo run -p yamu-inspect -- playlist <owner>:<kind>
cargo run -p yamu-inspect -- artist <id>
cargo run -p yamu-inspect -- artist-tracks <id> --page-size 100
cargo run -p yamu-inspect -- artist-albums <id> --page-size 100
cargo run -p yamu-inspect -- lyrics <track-id> --lrc
cargo run -p yamu-inspect -- playlist-recommendations <owner>:<kind>
cargo run -p yamu-inspect -- stations
cargo run -p yamu-inspect -- station-tracks user onyourwave
```

Pass `--json` to print the complete modeled response including fields retained
for forward compatibility.

Read-only API calls share a configurable `ReadRequestPolicy`: the default
spaces GET requests by 25 ms and retries connection failures, HTTP 429, and 5xx
responses up to three times with exponential backoff. POST mutations are never
retried automatically. Artist tracks and albums expose both explicit
`PageRequest` methods and helpers that collect every page.

## Editing the library

`yamu-edit` keeps mutations separate from the read-only inspector:

```console
cargo run -p yamu-edit -- like <track-id>...
cargo run -p yamu-edit -- unlike <track-id>...
cargo run -p yamu-edit -- playlist-create "My playlist"
cargo run -p yamu-edit -- playlist-rename <kind> "New title"
cargo run -p yamu-edit -- playlist-visibility <kind> public
cargo run -p yamu-edit -- playlist-add <kind> <track-id> <album-id> --at 0
cargo run -p yamu-edit -- playlist-remove <kind> --from 0 --to 1
cargo run -p yamu-edit -- playlist-delete <kind> --yes
```

Playlist track changes first fetch the current revision and submit a typed
positional diff against that exact revision. A concurrent edit is reported as
`PlaylistRevisionConflict` and is not retried automatically. Permanent playlist
deletion requires `--yes`.

## Downloading audio

The `downloads` feature implements the signed `get-file-info` negotiation and
opens short-lived CDN response streams without forwarding the OAuth token:

```console
cargo run -p yamu-download -- track <track-id>
cargo run -p yamu-download -- track <yandex-music-track-url>
cargo run -p yamu-download -- track <track-id> --quality normal -o track.mp3
cargo run -p yamu-download -- album <album-id-or-url> --jobs 4
cargo run -p yamu-download -- playlist <owner>:<kind> -o ./playlist --jobs 4
cargo run -p yamu-download -- playlist <yandex-music-playlist-url>
cargo run -p yamu-download -- liked -o ./liked --jobs 4
cargo run -p yamu-download -- artist <artist-id-or-url> --limit 100
cargo run -p yamu-download -- sync playlist <playlist-url> -o ./playlist
cargo run -p yamu-download -- sync liked -o ./liked --dry-run
cargo run -p yamu-download -- sync liked -o ./liked --prune
cargo run -p yamu-download -- track <track-url> --lyrics
cargo run -p yamu-download -- album <album-url> --lyrics lrc
```

Track, album, artist, and playlist arguments accept canonical Yandex Music
links as well as compact IDs. URL query parameters and fragments, including
copy-link `utm_*` parameters, are discarded during parsing.

Album downloads use `Artist - Album (year)` directories. Multi-disc releases
are split into `CD1`, `CD2`, and so on, while files retain disc-local track
numbers. Albums and playlists share the same concurrent, atomic, verified
resume pipeline.

Liked libraries are expanded in bounded API batches before downloading. Artist
catalogs are fetched through every available API page. Both commands accept an
optional `--limit` and preserve their source ordering in numbered filenames.

`sync playlist` and `sync liked` compare the current remote ordering with the
versioned local manifest. They download new or renamed entries and retain stale
files by default. `--dry-run` reports the plan without touching audio or the
manifest. `--prune` runs only after every current track succeeds and removes
only previously tracked `.flac`, `.m4a`, or `.mp3` files whose canonical paths
remain inside the destination directory, together with their generated lyrics
sidecars.

The global `--lyrics [text|lrc]` option works with every download and sync
source. Omitting the format after `--lyrics` selects plain text. Lyrics are
written atomically beside the audio as `.txt` or `.lrc` and embedded in the
audio tags; synchronized LRC timestamps are retained. Missing remote lyrics
produce a warning without discarding an otherwise valid audio download.

The server can return a lower tier than requested. `yamu-download` writes to a
same-directory `.part` file, syncs it, and only then renames it to the final
path. Playlist downloads continue past individual failures and print a final
report. FLAC-in-MP4 is losslessly remuxed to native `.flac` through the selected
media backend;
AAC-in-MP4 remains `.m4a`, and MP3 remains `.mp3`. M4A metadata and its single
attached cover are written by a lossless remux; FLAC and MP3 tags use Lofty.
The workspace `yamu-download` binary selects `media-ffmpeg-cli`; applications can
select the in-process `media-ffmpeg` backend instead. Existing files are
preserved unless `--force` is passed. Every newly completed or repaired file is
enriched with title, artist, album, album artist, year,
genre, album track/disc position, and an embedded 600×600 front cover.

Resume validates each existing container and its duration before trusting it;
M4A validation also performs a complete audio decode so broken sample tables or
AAC packets cannot pass a shallow container check. Truncated or corrupt files
are atomically replaced. Playlist progress is checkpointed at most once per
second and flushed at the end of the run in
`.yamu-download-state.json`. Transient negotiation failures and CDN transfers use
bounded exponential retries, and every advertised CDN URL is attempted.

The transfer, retry, decryption, normalization, progress, and cancellation
logic is provided by the library's `downloader` feature and shared with the
Android client. Frontends receive typed `DownloadEvent` values instead of
parsing terminal output. Cancellation interrupts active HTTP requests and retry
delays and removes the incomplete temporary file.
