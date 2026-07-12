# ya-music workspace

Rust libraries and development tools for the unofficial Yandex Music API.

## Packages

- `yandex-music-api` — async API client, models and OAuth Device Flow.
- `yandex-music-credentials` — shared local credential storage for workspace applications.
- `ym-auth` — login, logout and credential inspection tool.
- `ym-inspect` — read-only API exploration client.

All applications share the `default` credential profile stored outside the
repository. Run `cargo run -p ym-auth -- path` to display its location.

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
