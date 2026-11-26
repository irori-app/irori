---
name: irori-store-protocol
description: Implement and test store adapters (crates/store-*), protocol clients (crates/epic, crates/gog, crates/amazon, crates/steam, crates/itch, crates/xbox, crates/msstore, crates/msixvc), credential sealing, and offline TestServer suites.
---

# Store Adapters and Protocol Crates

Use this skill whenever modifying protocol crates (`crates/epic`, `crates/gog`, `crates/amazon`, `crates/itch`, `crates/steam`, `crates/retroachievements`, `crates/xbox`, `crates/msstore`, `crates/msixvc`, `crates/gog-galaxy`), store adapters (`crates/store-*`), `crates/secrets`, or `crates/depot`.

## Strict architectural boundaries

- Keep `#![forbid(unsafe_code)]` at the root of every protocol, credential, archive, and depot crate (`SECURITY.md` lists every crate).
- Protocol crates (`irori-epic-api`, `irori-gog-api`, `irori-amazon-api`, `irori-itch-api`, `irori-steam-api`, `irori-retroachievements-api`, `irori-xbox-api`, `irori-msstore-api`, `irori-msixvc`) must never depend directly on `irori-core`, `Paths`, `EventBus`, or UI crates. Pass endpoints, HTTP clients, and token callbacks explicitly.
- Store crates (`crates/store-*`) adapt protocol clients to `irori_core::Store`. `crates/services` talks to stores strictly via `DynStore = Arc<dyn Store>`, and multi-account wrapping happens in `crates/irori/src/stores.rs` (`MultiAccountStore`).
- Every store crate must expose offline test constructors (`with_paths`, `with_endpoints`, or `with_layout`) so tests never touch `$HOME`, real OS keyrings, or live internet servers.

## Correctness, security, and offline resilience

- Store all tokens, cookies, API keys, and refresh credentials through `crates/secrets/src/lib.rs` (`irori_secrets::read_json`, `load`, `write_json`), never `json_store`. Strip query tokens and sensitive URLs from `reqwest` errors (`without_url`) and never log authorization headers or tokens.
- Keep `Store::init` strictly disk-only with zero network requests so startup never blocks the first GPUI frame.
- `store.games()` returns `Vec<Arc<GameInfo>>` including DLC entries marked `is_dlc: true`. Filter `!game.is_dlc` everywhere full games are counted, listed, or summed for playtime (Epic also mirrors base-game playtime onto DLC records).
- Inspect every `releaseInfo` entry on Epic titles rather than assuming index `0` is the only platform release.
- Return `SessionExpired` only on authoritative `400`/`401` token-service rejections, never on transient timeouts, DNS failures, or `5xx` responses, so temporary outages never sign users out or wipe cached libraries.
- Call `fs_guard::ensure_deletable` (`crates/core/src/fs_guard.rs`) before every recursive directory removal, and sanitize remote path segments (`paths::sanitize`) and archive entries (`archive_guard`).
- Update `SECURITY.md` in the same pull request whenever introducing a new external network domain, local listener, or helper executable.

## Offline HTTP and depot testing

- Enable `irori-depot = { workspace = true, features = ["test-server"] }` under `[dev-dependencies]` and serve deterministic JSON payloads and chunk bytes from `irori_depot::test_server::TestServer` on `127.0.0.1:0`.
- Isolate filesystem state per test under `tempfile::tempdir()`. Remember `Paths::init` runs at most once per process; prefer store `with_paths` / `with_layout` constructors over global `Paths` where available.

## Few-shot examples

<example>
<input>
Task: Add a regression test for a GOG manifest parsing or install flow in `crates/store-gog` without touching the network or user directories.
</input>
<output>
1. Spin up `irori_depot::test_server::TestServer` serving the fixture manifest and chunk responses on `127.0.0.1`.
2. Construct a `tempfile::TempDir` for config, cache, and install directories, and build `GogStore` using its test endpoint and path constructors (`with_endpoints` / `with_paths`).
3. Run `store.install(...)` with `OpContext::detached()`, assert the installed files and cached manifest on disk, and verify that `cargo test --locked -p irori-gog --profile ci` passes with network interfaces disabled.
</output>
</example>

<example>
<input>
Task: Handle an upstream `401 Unauthorized` vs `503 Service Unavailable` during token refresh in a protocol client and store adapter.
</input>
<output>
1. In the protocol crate (`#![forbid(unsafe_code)]`, no `irori-core`), distinguish HTTP `400`/`401` auth rejection via a typed `thiserror` variant (such as `AuthError::Rejected`) while mapping `5xx` and timeouts to `AuthError::Transient` with `.without_url()` on `reqwest::Error`.
2. In `crates/store-*`, map `AuthError::Rejected` into `anyhow::Error::new(irori_core::SessionExpired)` so `Backend` prompts re-authentication, while returning transient errors with `.context(...)` and keeping cached `GameInfo` records intact for offline launches.
</output>
</example>
