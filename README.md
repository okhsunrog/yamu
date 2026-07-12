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
```

Pass `--json` to print the complete modeled response including fields retained
for forward compatibility.

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
cargo run -p ym-download -- <track-id>
cargo run -p ym-download -- <track-id> --quality normal -o track.mp3
```

The server can return a lower tier than requested. `ym-download` writes to a
same-directory `.part` file, syncs it, and only then renames it to the final
path. Existing files are preserved unless `--force` is passed.
