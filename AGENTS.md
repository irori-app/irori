# AGENTS.md

Instructions for anyone, human or coding agent, changing Irori. Read this file
before your first change. Where it disagrees with `docs/AGENT_BRIEF.md`,
`docs/UI_BRIEF.md` or `docs/I18N_BRIEF.md`, this file wins. Those briefs were written
for the first, crate-by-crate build of Irori and point at scratch paths that no
longer exist.

Contents:

1. [What Irori is](#1-what-irori-is)
2. [Commands](#2-commands)
3. [Repository map](#3-repository-map)
4. [Architecture](#4-architecture)
5. [Rust conventions](#5-rust-conventions)
6. [UI conventions](#6-ui-conventions)
7. [Translations](#7-translations)
8. [Tests](#8-tests)
9. [Platforms and packaging](#9-platforms-and-packaging)
10. [User data, secrets and safety](#10-user-data-secrets-and-safety)
11. [Branches, commits and pull requests](#11-branches-commits-and-pull-requests)
12. [Definition of done](#12-definition-of-done)
13. [Pitfalls](#13-pitfalls)
14. [Notes for coding agents](#14-notes-for-coding-agents)

---

## 1. What Irori is

Irori is a native game launcher for Linux, macOS and Windows. It covers Epic Games,
GOG, Amazon Games and ZOOM Platform libraries, plus games the user adds by hand
("sideloaded" apps). Windows games run natively on Windows. On Linux they run through
Wine, Proton or umu, and on macOS through Wine, CrossOver or Game Porting Toolkit.

- Written in Rust (edition 2024, Rust 1.95+), with a [GPUI](https://www.gpui.rs) UI
  through [GPUI Kit](https://gpui-kit.com) (`gpui-kit = "0.8.0"`).
- It aims to match the features of Heroic Games Launcher 2.22.4 without Electron. It
  is **not a fork or port of Heroic's code**. Heroic is a reference for behaviour only
  (`docs/parity/*.md`). Its bugs, marked `[QUIRK]` in the parity docs, are fixed, not
  copied.
- Store protocols are Rust ports of legendary (Epic), gogdl (GOG) and nile (Amazon);
  itch.io uses its public API directly.
  GOG Galaxy's comet runs as a library. Irori never shells out to those tools.
- Product goals, in order: correct, fast, calm, one click to play. It runs as one
  process, renders on the GPU, uses no CPU when idle, keeps memory bounded, and
  virtualizes every long list.
- License: MIT. Bundled themes are from GPUI Kit (Apache-2.0). The
  vendored `crates/webview`, `crates/comet` and `crates/gpui-pre-windows` are
  Apache-2.0 and keep their own `LICENSE-APACHE`.

## 2. Commands

Run everything from the repository root.

| Task | Command |
| --- | --- |
| Run Irori (release, as users get it) | `cargo run --release -p irori` |
| Run Irori with the Linux sign-in window | `cargo build --release -p irori-sign-in && cargo run --release -p irori` |
| Build both shipped binaries | `cargo build --locked --release -p irori -p irori-sign-in` |
| Format | `cargo fmt --all` (CI: `cargo fmt --all --check`) |
| Lint, as CI does | `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings` |
| Test everything | `cargo test --locked --workspace --no-fail-fast` (Linux CI wraps it in `xvfb-run -a`) |
| Test one crate | `cargo test -p <package>`, for example `cargo test -p irori-manage` |
| Packaging tests | `python3 -m unittest discover -s packaging -p "test_*.py"` |
| PR-rule tests | `python3 -m unittest discover -s .github/scripts -p "test_*.py"` |
| Check a PR name locally | `python3 .github/scripts/pr_rules.py --title "fix(epic): …" --head fix/epic-login` |
| Package | `python3 packaging/package.py --platform linux\|macos\|windows --binary-dir target/release --output dist` |
| Release | Push a `vX.Y.Z` tag; see `docs/RELEASING.md` |
| Install for this user (Linux) | `packaging/install.sh` |
| Profile with symbols | `cargo build --profile profiling -p irori` |

Commands Irori offers without opening a window (`crates/irori/src/headless.rs`):

```text
irori accounts                                   which stores are logged in
irori import-heroic                              import Heroic accounts, installs and settings
irori import-lutris                              import Lutris games, categories and playtime (read-only)
irori import-bottles                             import the programs of every Bottles bottle (read-only)
irori import-faugus                              import Faugus Launcher's games (read-only)
irori import-playnite                            import Playnite's games, art and playtime (Windows, read-only)
irori refresh                                    refresh every library
irori list [--installed]
irori info <runner>/<app> [--platform P]
irori playtime <runner>/<app>
irori install <runner>/<app> [--path DIR] [--platform windows|linux]   Ctrl+C stops; rerun resumes
irori uninstall <runner>/<app> [--keep-files]
irori sunshine-sync [--remove]                   bring Irori's games in Sunshine up to date, or take them out
```

Irori's other arguments (`crates/irori/src/cli.rs`) are:

- `-l/--launch <runner>/<app>`, `--big-picture`/`--console`, `--minimized`/`--tray`,
  `-V/--version` and `-h/--help`.
- `--wait` with `--launch` or `--big-picture` (`crates/irori/src/waiting.rs`): the
  invocation stays until the game's session (or Big Picture) ends, exiting 0, or 1 with
  the reason on stderr; ending it first stops the game (leaves Big Picture). It starts
  Irori first when none runs, in a process group of its own and, on Windows, outside the
  caller's job, so Irori outlives a stream. Sunshine's applications use it.
- Links: `irori://launch|game|open/<runner>/<app>`, `irori://library|downloads|updates|accounts|stores|stats|wine|emulators|bigpicture`,
  `irori://settings/<section>`, `irori://stats/year[/<year>]` (the Year in review),
  `irori://lutris/<slug>` (the Lutris script installer at
  a lutris.net game; Lutris's own `lutris:<slug>` is accepted as an argument too), and
  Heroic's `heroic://launch/...`, and Nexus Mods' `nxm://<game>/mods/<mod>/files/<file>`
  (installs the file after asking which game it is for). Any web page or program can open a link, so
  `irori://launch` asks before it starts a game; `--launch` doesn't.
- A Windows file (`.exe`, `.msi`, `.bat`, `.cmd`, `.reg`, as a path or `file://` URL; not on
  Windows) opens in the Wine prefix of a game the user picks (`irori_game::open_windows_file_dialog`,
  `Backend::open_windows_file`; `docs/PLATFORMS.md`, "Windows files").
- A link of any other scheme (`ddnet:[::1]:8303`; not Irori's, Heroic's, Lutris's or Nexus
  Mods' links, `file:` or web pages; not on Windows) opens the same way, through Wine's
  `start` (`irori_runtime::windows_uri`, `Backend::open_windows_uri`,
  `irori_game::open_windows_uri_dialog`; `docs/PLATFORMS.md`, "Other programs' links").
  Irori registers no such scheme; the user points one at it (`xdg-mime default`).
- `--sign-in-window` is internal. On macOS and Windows it makes Irori run the store
  sign-in window instead of itself (see [4.6](#46-store-sign-in)).

Environment variables:

| Variable | Effect |
| --- | --- |
| `IRORI_HOME` | Puts all config, data and cache under one root (`<root>/{config,data,cache}`). Use it for experiments so your real data stays untouched. |
| `IRORI_LOG` | `tracing` filter, e.g. `IRORI_LOG=irori_services=debug,info`. The default is `info` in the app and `warn` for the commands without a window. |
| `IRORI_SIGN_IDENTITY` | macOS signing identity for `package.py` (default `-`, ad-hoc). |
| `IRORI_WINDOWS_CRT_DIR` | Windows: explicit `Microsoft.VC*.CRT` directory for packaging. |
| `IRORI_UPDATE_EXPLANATION` | For packagers, at build or run time: Irori never looks for its own new versions and shows this text instead (`irori-services::app_update`). |
| `IRORI_RELEASE_TEST_URL`, `IRORI_RUNTIME_SMOKE`, `IRORI_KEYRING_SMOKE`, `IRORI_SANDBOX_SMOKE`, `IRORI_SANDBOX_PROTON` | Opt-in, `#[ignore]`d runtime tests (see [8](#8-tests)). |

Build profiles (root `Cargo.toml`):

- `release` is fat LTO, one codegen unit, `panic = "abort"`, stripped.
- `dev` builds Irori at `opt-level = 1` and its dependencies at 2, so debug builds stay
  usable.
- `ci` is unoptimized and keeps no debug info, so builds fit hosted runners.
- `profiling` is release plus line tables, for `perf` and `heaptrack`.

## 3. Repository map

Directory names and package names differ. Use the package name with `-p`.

| Directory | Package | Role |
| --- | --- | --- |
| `crates/core` | `irori-core` | Domain types, settings schema, the `Store` trait, events, paths, process/HTTP/JSON utilities |
| `crates/secrets` | `irori-secrets` | Files holding store logins, encrypted with a key kept in the system keyring |
| `crates/runtime` | `irori-runtime` | Wine/Proton/CrossOver/GPTK discovery and downloads, prefixes, DXVK/VKD3D, umu, launch environment, the bubblewrap sandbox (`sandbox.rs`) |
| `crates/engines` | `irori-engines` | Emulators ("engines"): manifests, discovery, managed versions, RetroArch cores, BIOS index, launch commands |
| `crates/scripts` | `irori-scripts` | Lutris install scripts: typed parser, validation, variables, consent plan, confined executor with rollback, the `ScriptHost` trait, the lutris.net client, Wine build matching |
| `crates/depot` | `irori-depot` | Download engine: parallel chunked downloads, resume, verification, delta updates; a `test-server` feature |
| `crates/epic` | `irori-epic-api` | Epic protocol (port of legendary): auth, library, manifests, cloud saves, launch parameters |
| `crates/gog` | `irori-gog-api` | GOG protocol (port of gogdl): auth, content system, depots, Linux installers, saves |
| `crates/amazon` | `irori-amazon-api` | Amazon Games protocol (port of nile): device sign-in, library, signed manifests |
| `crates/itch` | `irori-itch-api` | itch.io protocol (its public API): API-key login, owned keys, uploads, download links |
| `crates/steam` | `irori-steam-api` | Steam Web API: login (QR, password and Steam Guard, tokens), owned and Family games, store art and pages, achievements |
| `crates/retroachievements` | `irori-retroachievements-api` | RetroAchievements: Web API (profile, console game lists, game progress), the emulators' token login, and rcheevos-compatible ROM hashes |
| `crates/gog-galaxy` | `irori-gog-galaxy` | Hosts comet (Galaxy service for games: achievements, stats, leaderboards) on `127.0.0.1:9977` for the games Irori started |
| `crates/comet` | `comet` | Small fork of comet, vendored so Irori can fix it; its README lists every difference from upstream |
| `crates/store-epic` | `irori-epic` | Epic `Store` (`Runner::Legendary`) |
| `crates/store-gog` | `irori-gog` | GOG `Store` |
| `crates/store-amazon` | `irori-amazon` | Amazon `Store` (`Runner::Nile`) |
| `crates/store-zoom` | `irori-zoom` | ZOOM Platform `Store`, opt-in through a setting |
| `crates/store-itch` | `irori-itch` | itch.io `Store` (`Runner::Itch`), opt-in through `iroriItchIo` |
| `crates/store-steam` | `irori-steam` | Steam `Store` (`Runner::Steam`): local library, optional account, game watch, the login API the UI draws |
| `crates/store-sideload` | `irori-sideload` | Apps the user adds (native, Windows, browser) |
| `crates/launchers` | `irori-launchers-data` | Reads third-party launchers' local data (Battle.net `product.db`, Ubisoft cache and registry, EA manifests, Rockstar registry) and Xbox app packages (`.GamingRoot`, `MicrosoftGame.config`, `AppxManifest.xml`, Gaming Services) |
| `crates/store-launchers` | `irori-launchers` | Battle.net, EA app, Ubisoft Connect, Rockstar and, on Windows, Xbox app games (`Runner::Launchers`): setup and library, opt-in through `iroriThirdPartyLaunchers` |
| `crates/store-emulation` | `irori-emulation` | The ROM library (`Runner::Emulation`): scanned folders, played with emulators |
| `crates/mods` | `irori-mods` | Per-game mod manager: mod storage and load order, deployment (overlay or recorded links) and undeployment, conflicts, archives, FOMOD installers, Thunderstore and Nexus Mods |
| `crates/reshade` | `irori-reshade` | ReShade per game: PE reader (architecture, imports), program and render API detection, ReShade's `Compatibility.ini`, downloads from the official sources, journaled installs |
| `crates/extras` | `irori-extras` | Shortcuts, PCGamingWiki/HLTB/ProtonDB/anticheat info, known fixes, SteamGridDB, deals, Heroic import, Lutris import planning |
| `crates/services` | `irori-services` | The `Backend` facade: library, settings, user data, download queue, launching, playtime, Lutris script installs (`scripts.rs`) |
| `crates/ui` | `irori-ui` | Shared UI: backend bridge, models, navigation, art, theme and translucency, components, i18n |
| `crates/feature-library` | `irori-library` | Library grid/list, search, filters, categories, stats page, sideload dialogs, the Lutris script installer |
| `crates/feature-game` | `irori-game` | Game page and its sheets: install, uninstall, logs, cloud saves, import, modify |
| `crates/feature-manage` | `irori-manage` | Downloads, Wine manager, accounts and login, stores, onboarding |
| `crates/feature-settings` | `irori-settings` | App settings, game defaults, per-game overrides, logs, system info |
| `crates/feature-bigpicture` | `irori-bigpicture` | Big Picture mode and controller input for the whole app |
| `crates/irori` | `irori` | The application shell and binary: window, sidebar, routing, palette, tray, single instance, CLI |
| `crates/sign-in` | `irori-sign-in` | Store sign-in and store windows (Wry/Tao): library and the `irori-sign-in` program |
| `crates/webview` | `gpui-webview` | Vendored GPUI WebView, **excluded from the workspace and unused** |
| `crates/gpui-pre-windows` | `gpui-pre-windows` | GPUI's Windows backend with Irori's fixes, used through `[patch.crates-io]` and **excluded from the workspace**; `PATCHES.md` lists every change |

Other top-level paths:

- `assets/`: bundled theme JSON files, the Manrope font, and `engines/` (emulator
  manifests and the systems catalog, see [4.8](#48-engines-emulators)).
- `packaging/`: `package.py`, its tests, the Linux `install.sh`, the Windows
  `install-windows.ps1`, the desktop entry, icons and entitlements.
- `docs/`:
  - `PLATFORMS.md`: paths, desktop behaviour, translucency, packaging, release checklist.
  - `parity/`: the Heroic behaviour Irori matches, and the Lutris behaviour
    (`parity/lutris.md`).
  - `design/`: designs of larger features (`design/engines.md`: emulators, ROM
    libraries, Lutris import and scripts, third-party launchers).
  - The three briefs (see the top of this file).
- `website/`: the project site (Vue, shadcn-vue), deployed with Coolify; see its README.
- `postman/`: contract, mock and simulation used by an ignored runtime test.
- `.github/`: CI, the PR-rule check and the PR template.

### Layering

The crates form a strict hierarchy. Never add a dependency that points upward.

```text
irori-secrets                     (depends on nothing internal; usable by protocol crates)
irori-core                        (depends on nothing internal)
  ├─ irori-runtime, irori-depot, irori-extras
  ├─ irori-engines   (core, runtime, depot)
  ├─ irori-scripts   (core, runtime, depot; knows nothing of stores or the UI)
  ├─ irori-mods      (core, depot; knows nothing of stores or the UI)
  ├─ irori-reshade   (core, runtime; knows nothing of stores or the UI)
  ├─ protocol crates: irori-epic-api, irori-gog-api, irori-amazon-api, irori-itch-api,
  │                   irori-steam-api, irori-retroachievements-api
  │                   (depot and secrets only, not core)
  ├─ store crates:    irori-epic, irori-gog, irori-amazon, irori-zoom, irori-itch,
  │                   irori-steam, irori-sideload, irori-emulation (+ engines,
  │                   irori-retroachievements-api),
  │                   irori-launchers (+ runtime, irori-launchers-data)
  └─ irori-services   (core, depot, runtime, extras, engines, scripts, mods, reshade, irori-steam-api
                      for Steam's public achievement schema; stores reach it as `DynStore`)
irori-ui          (services, core)
feature crates    (ui, services, core; concrete store crates only where a screen needs a store's own API)
irori (binary)    (everything; the only crate that names every store)
irori-sign-in     (standalone; Irori links it only on macOS/Windows)
```

- Protocol crates know nothing about Irori: no `Paths`, no settings, no events.
- Store crates turn a protocol into a `Store`. Services drive stores only through
  `DynStore = Arc<dyn Store>`.
- Feature crates never depend on each other. The shell routes between them
  (`crates/irori/src/routing.rs`, `pages.rs`) through events such as `LibraryEvent`
  and `GamePageEvent`.
- When a feature needs an operation that requires a concrete store crate (Heroic
  import, the GOG branch password), the shell installs a hook (`irori_ui::hooks`,
  `irori_game::install_hooks`) instead of the feature depending on the store. The
  same goes for another feature's views: the library's Focus view and grid drawer show
  the game page through `irori_ui::hooks::game_panel` (`GamePage::embedded`), and
  Big Picture shows a game's settings in place through `irori_ui::hooks::game_settings`
  (`GameSettingsView::embedded`), so they never take the user out of it.

## 4. Architecture

### 4.1 Core contracts (`irori-core`)

Read `crates/core/src/store.rs`, `events.rs`, `paths.rs`, `settings.rs`, `process.rs`
and `launch.rs` before changing anything below the UI.

**`Store` trait** (`store.rs`, `#[async_trait]`, `Send + Sync + 'static`):

- Accounts:
  - `init`: disk only, no network, and it gates the first render.
  - `user`, `login_flow`, `login(input)`, `logout`.
- Library: `refresh(full)`, `games()`, `game`, `extra_info`, `install_info`,
  `updatable_games`. `games()` returns `Vec<Arc<GameInfo>>`, including DLC flagged
  `is_dlc`.
- Operations: `install`, `update` and `repair` take an `OpContext` and return an
  `Outcome`. Also `uninstall(RemoveArgs)`, `import`, `set_install_path`, and
  `launch` → `LaunchPlan`.
- Optional, with defaults: cloud saves, achievements (`achievements`, and
  `forget_achievements` to drop a cached answer after a session), `remote_playtime`
  (minutes by app name), `after_exit`, extras, `forget_install`, `set_version_pinned`.
- Accounts, with defaults for stores with one login: `accounts`, `can_add_account`,
  `account_login_flow`/`account_login`/`account_logout`/`refresh_account` by slot,
  and `game_account`.

**Other types:**

- `LoginFlow::Browser { url, capture, instructions }`. `capture` is a regex whose
  first group is what `login` needs. The sign-in window and the clipboard watcher both
  use it (see [4.6](#46-store-sign-in)). `LoginFlow::InApp` means Irori draws the
  store's own login (Steam) and passes its result to `login`. `LoginFlow::None` is for
  stores without accounts.
- `OpContext` carries a progress sink, a `CancellationToken`, a step reporter, an
  optional operation log file and the game's effective settings.
  `OpContext::detached()` is the no-op version.
- Two typed errors carry meaning across the `Store` boundary; return them anywhere in
  the `anyhow` chain:
  - `SessionExpired`: the store's token service refused the saved login (a 400 or 401,
    not a network failure or a busy server). Services remember it, tell the user once
    and offer to log in again.
  - `SaveConflict`: saves changed both locally and in the cloud since the last sync.
    Return it for every direction except the forced ones; the launch stops and the UI
    asks which saves to keep (`irori_ui::save_conflict`).
- `Runner` values are Legendary, Gog, Nile, Zoom, Itch, Steam, Sideload, Emulation and
  Launchers. Their
  serialized ids are Heroic's (`legendary`, `gog`, `nile`, …); `from_id` also accepts
  `epic` and `amazon`. Irori-only runners use their store's plain name (`itch`,
  `steam`).
- `Runner::manages_installs()` is false for Steam and Launchers: its games are listed and launched,
  never installed, moved, repaired or removed by Irori. UI code hides those commands and
  services refuse them before touching files. Steam's client takes them instead:
  `Store::client_action` (`ClientAction::{Install, Uninstall, Verify, Show}`, through
  `Backend::client_action`) asks it, and `Runner::has_optional_account()` marks Steam's
  login as optional (its accounts card stays local and adds the account).
- `LaunchPlan::watch` (`Arc<dyn GameWatch>`) is for games another program starts and
  owns: the command only hands the game over, so services follow `GameWatch::running`
  for the session and call `GameWatch::stop` to stop it (never the client).
- `GameId { runner, app_name: Arc<str> }` has `key() == "runner:app"`, which keys
  every user-data map.

**Events** (`events.rs`, a Tokio broadcast channel with capacity 1024):

- Variants: `LibraryChanged(Runner)`, `LibraryUpdated(rev)`, `GameChanged`, `Status`,
  `StatusCleared`, `AccountChanged`, `DownloadQueueChanged`, `WineVersionsChanged`,
  `SettingsChanged`, `GameSettingsChanged`, `Connectivity`, `Notice`, `GameExited`,
  `SunshineChanged`.
- Events carry *identities, not data*: receivers re-read state from the service that
  owns it.
- Emit `LibraryChanged(runner)` after a store's catalogue or install state changes,
  and `AccountChanged(runner)` after login or logout.
- `LibraryChanged` is a raw store signal. UI code waits for `LibraryUpdated`, which
  services emit after rebuilding the snapshot.
- The bus keeps no history, and the UI subscribes after the backend has started. A
  notice sent during startup must use `EventBus::startup_notice`, or nobody sees it.

**Paths** (`paths.rs`):

- `Paths::get()` is global. `IRORI_HOME` relocates everything; so does, on Windows
  only, a `portable` file beside `irori.exe` (the portable zip), with the executable's
  folder as the root. Otherwise the OS config, data and cache directories plus `irori`.
  A portable Irori (`paths::is_portable`) registers no URL handler and adds no Start
  menu shortcuts.
- Store config goes under `store_config_dir(runner)`, as do credentials (written only
  through `irori_secrets`, see below), store caches under
  `store_cache_dir(runner)`, and Irori state (`library.json`, `downloads.json`,
  `sessions.jsonl`) under `state_dir()`.
- Per-game settings are `config/games/<runner>/<app>.json`. Logs go under
  `logs_dir()`; the app log is `data/logs/irori.log`, and a panic writes
  `crash-<time>.txt` beside it (`crates/irori/src/crash.rs`).
- Tests isolate themselves with `Paths::init(Paths::under(tempdir))`.

**Persistence** (`json_store.rs`):

- Always write through `json_store::write`/`write_bytes`. They write a `.tmp`
  sibling and rename it, so a crash never leaves a half-written file.
- `read` returns `Ok(None)` for a missing file; `read_or_default` logs and falls back
  for a corrupt one.

**Secrets** (`irori-secrets`):

- Logins, tokens, API keys and passwords are read and written only with
  `irori_secrets::read_json`/`load`/`write_json`, never `json_store`.
- Files are AES-256-GCM encrypted with one random key kept in the system keyring
  (Secret Service, Keychain, Credential Manager). The logins themselves don't go in the
  keyring because Windows limits an entry to about 2.5 KB.
- Without a keyring they are plain JSON, mode 0600, and logins stay usable. Plain files
  are encrypted when next read with a keyring available.
- `main.rs` turns the keyring on with `use_os_keyring`. Nothing else does, so tests and
  library code use plain files and never reach the user's real keyring.
- An unlock prompt can take a while: don't read a secret on the UI thread. A file that
  can't be opened reads as an error, is left in place, and raises the
  `shell.notice.logins_locked` notice at start.

**Settings** (`settings.rs`):

- JSON keys are Heroic's (camelCase, including Heroic's `enviromentOptions` typo)
  so Heroic configs import unchanged. **Do not rename keys.** Add new ones with
  `#[serde(default)]`.
- `AppSettings` flattens a `GameSettings` that holds the global game defaults.
- Per-game overrides are sparse (`GameSettingsOverrides`, a JSON map): the effective
  settings are the defaults with the overridden keys applied. Reset a field with
  `SettingsService::set_game_value(id, key, None)`.
- Launch presets (`launch_extras.rs`; the user's in `iroriLaunchPresets`, Irori's own
  from `builtin_presets`) hold only `PRESET_KEYS`. Applying one writes just those keys
  as overrides (variables, wrappers and programs merge by key into the current list),
  so everything else keeps following the defaults.

**Processes** (`process.rs`):

- Spawn through `CommandSpec` + `process::run`/`spawn`, never `std::process` directly
  for game or tool processes.
- It handles line callbacks (split on `\n` and `\r`), capped logs, cancellation and
  process-group termination: SIGTERM then SIGKILL on Unix, `taskkill /T /F` on
  Windows.

**HTTP** (`http.rs`):

- Use `irori_core::http::client()`: shared, rustls, `Irori/<version>` user agent,
  10 s connect and 60 s total timeout. It must run inside the Tokio runtime.
- Large downloads use `download_client()` (no total timeout) or `irori-depot`.

**Host programs** (`host.rs`): inside Irori's Flatpak the host's `/usr`, processes
and other apps' `~/.var/app` folders are hidden. A program the user installed on the
host (Steam, Ludusavi, another Flatpak) is found with `host::find_program` /
`has_flatpak_app` and started through `host::command` (`flatpak-spawn --host`), or
`tied_command` when it should stop with its command; `host::processes` and `signal`
list and stop host processes. Outside a Flatpak they act on this system.

**Deleting files:** call `fs_guard::ensure_deletable(dir, app)` before any recursive
delete of a game or prefix directory.

**Untrusted data** (store APIs, community JSON, archives, links):

- Names that become paths go through `paths::sanitize`, which also refuses `.` and `..`.
- Archive extractors use `archive_guard` for every entry and every link target.
- Links opened from store or community data go through `irori_ui::web_url`.
- Logs are shared by users: mask new secret parameters in `process::is_secret_flag`,
  `SECRET_PARAM` and `is_secret_var`, and strip addresses from `reqwest` errors with
  `without_url` wherever an address holds a token or a signature.

### 4.2 Backend (`irori-services`)

`Backend` (`crates/services/src/lib.rs`) is the single entry point for the UI and the
commands without a window.

- `Backend::start(make_stores)` is for the app; `start_headless` is for CLI commands
  (no periodic tasks).
- It owns a Tokio runtime named `irori-io`: 3 workers, 16 blocking threads. Use
  `backend.spawn` and `backend.spawn_blocking`.
- Public parts: `bus`, `settings` (`SettingsService`), `userdata`, `statuses`, plus
  `store(runner)`, `stores()`, `refresh_*`, downloads, launch/stop/uninstall/move/repair/import,
  cloud saves and Wine operations.
- **Library:** store games without DLC, plus user overrides (title, art), become a
  sorted `Arc<LibrarySnapshot>` with a revision number. Rebuilds are debounced (30 ms)
  after `LibraryChanged`/`GameChanged`.
- **Search** (`search.rs`): each snapshot builds a `SearchIndex` on its first search
  (titles, a renamed game's store title in `GameInfo::alt_titles`, folder names,
  developer, publisher; folded, Roman numerals read as digits, initials). Matches rank
  exact, prefix, word, then fuzzy (a typo or two, letters in order), titles before
  people. `related.rs` finds a game's series, studio and genre neighbours the same way.
- **Downloads:** one worker, one download at a time, persisted in
  `state/downloads.json`. Running items resume as Queued after a restart. Pause and
  cancel go through a `CancellationToken`.
- **Updates** (`updates.rs`): the update manager's component side. Wine builds, DXVK,
  VKD3D-Proton, DXVK-NVAPI, umu, the anticheat runtimes, emulators and RetroArch cores are
  checked once a session and daily, from lists cached for a day. Each has a stable id; a
  skipped version stays skipped until a newer one is out (`state/updates.json`). Game
  updates stay with the stores and the download queue, and a store whose check failed
  keeps its last answer. Automatic updates of both kinds start from
  `run_automatic_updates`, which keeps to `iroriAutoUpdateFrom`/`Until` and, where the
  system can tell (not macOS), `iroriAutoUpdateUnmetered`. A Wine update in place keeps the
  replaced build in `<base>/.rollback/`, nested so discovery doesn't list it.
- **User data:** favourites, hidden games, categories, recents, playtime, play
  statuses, yearly backlog goals, art overrides and journal notes live in
  `state/library.json` behind `RwLock<Arc<UserData>>`, updated copy on write. Store
  playtime merges by keeping the larger total and counts games only, never their DLC.
- **Journal** (`journal.rs`): one note per game per local calendar day (optional
  title, text, paths of that day's screenshots), written with
  `UserDataService::set_journal_entry`; an empty note is removed. Backups carry the
  notes, and a restore keeps the later-written note of each day. Screens edit them
  through `irori_ui::journal::open_editor` (the game page's timeline, the stats page's
  Journal tab).
- **Play status** (`play_status.rs`): the user's own `PlayStatus` per game (not played,
  plan to play, playing, on hold, beaten, abandoned) with the date it was set. Nothing
  sets one automatically. `UserData::effective_status` adds `Played` for games with
  playtime and no status; filters and stats group by it. Screens show and set it
  through `irori_ui::play_status` (names, colours, icons, menu, button).
- **Age ratings** (`irori_core::age_rating`, `store_facts.rs`): `AgeRating {system,
  rating, descriptors, age}`, where `age` is the youngest age the rating allows (ESRB
  E → 6, T → 13, CERO B → 12…). Amazon (`esrbRating`, `pegiRating`, `uskRating`) and
  Epic (`ageGatings`) carry them in `GameInfo::age_ratings`; Steam (`appdetails`
  `ratings`) and GOG (`v2/games` `_embedded.*Rating`) in `ExtraInfo::age_ratings`. UI code
  reads a game's store page through `Backend::extra_info`, which keeps those ratings in
  `<cache>/store-facts.json` (one cache of facts learned from store pages, read from the
  older `age-ratings.json` when it is missing) and adds them to the library entry, so no
  request is made for ratings alone. The game page shows the board of the user's region
  (`irori_ui::age_rating`: interface language, then the system locale's region) as a
  text badge, never a board's logo; the library's age filter (`ViewFilter::max_age`)
  goes by the strictest board.
- **Game modes** (`irori_core::game_mode`, `store_facts.rs`): `GameMode` is single,
  co-op, local (split screen, couch), online (LAN included) or MMO; `GameInfo::game_modes`
  holds a normalized list. Sources, none of them asked in the background: Steam's
  `appdetails` `categories` (`irori_steam::entries::game_modes`) and GOG's `v2/games`
  features (`game_modes_from_v2`) in `ExtraInfo::game_modes`, learned through
  `Backend::extra_info`; PCGamingWiki's infobox modes and `{{Network/Multiplayer}}` table
  (`PcGamingWikiInfo::game_modes`, from the wikitext the game page already reads),
  learned through `Backend::wiki_info` and used only when the store says nothing. Epic's
  catalog and store pages carry no modes. The user's choice in the Modify dialog
  (`GameOverrides::game_modes`, kept only when it differs from what is known) wins over
  everything and travels with sync like the custom title. Lists are read with
  `game_mode::lenient`, so a mode a newer Irori writes never breaks reading user data.
  The library filters (`ViewFilter::modes`, any of), groups (`Grouping::Mode`, one
  section per game by `filter::mode_section`) and searches (`mode:coop`,
  `search::ParsedQuery`) by them; the game page shows them as chips that open the
  library filtered by one mode (`GamePageEvent::ShowGameMode`).
- **Saved views and the persisted library view** keep criteria a newer Irori wrote
  (`ViewFilter::unknown`, a flattened map), so rewriting the file never drops them. A
  new facet only needs a `#[serde(default)]` field; one an older Irori doesn't know is
  shown unfiltered there, never lost.
- **Achievements** (`achievements.rs`): UI code asks `Backend::achievements`, never the
  store directly, so every answer is summed up per game in `state/achievements.json`
  (unlocked, points, hardcore, latest and rarest unlocks). The stats page totals that
  file; "Check played games" (`scan_achievements`) asks stores one game at a time.
  After a session the game's cached answer is dropped and asked again; with
  `iroriAchievementNotices` on, new unlocks raise a notice. Stores cache their answers
  with `irori_core::achievement_cache` (15 minutes; stale answers stand in offline).
  Sources: GOG and Epic web APIs, Steam's `IPlayerService` (definitions are public;
  unlocks with the account, else Steam's `appcache/stats` files), RetroAchievements
  for the ROM library. Steam's Achievement Sets (`GetGameAchievements`' `groups` and
  each achievement's `groupid`) become `Achievement::group` (name, DLC app id, order;
  `None` elsewhere and offline); the game page lists a game with two or more sets
  under a heading per set with its progress, while totals stay per game.
  - **Game files** (`file_achievements/`, `iroriFileAchievements`, off by default):
    the unlocks Steam API emulators keep for DRM-free copies, read only, for
    sideloaded, GOG, itch.io, Epic, Amazon and ZOOM games (never Steam's, the ROM
    library's or launchers'). When on, `Backend::achievements` asks it before the
    store; it answers when the game's files give a Steam app id and hold an
    emulator's saves or `steam_settings/achievements.json` (or the user chose the
    id, per-game `iroriSteamAppId`). App id order: chosen,
    `steam_settings/steam_appid.txt`, `steam_appid.txt` beside the program, in the
    install folder or beside the library, then `SteamAppId`/`SteamGameId` in the
    game's variables. Saves: Goldberg's and gbe_fork's `<appid>/achievements.json`
    in the portable folder (`local_save_path` in `configs.user.ini`,
    `local_save.txt`) and in `GSE Saves`/`Goldberg SteamEmu Saves` under
    `%APPDATA%` (every user of the game's Wine prefix) or `$XDG_DATA_HOME`
    (`~/.local/share`) for native builds; CODEX's and RUNE's
    `<appid>/achievements.ini` under Public Documents and `%APPDATA%` for Windows
    builds. Names, icons and rarity come from Steam's public schema (cached a day
    in `<cache>/file-achievements.json`), else the game's `steam_settings`.
- **Save backups** (`ludusavi.rs`, client in `ludusavi/client.rs`): Ludusavi's CLI in
  `--api` mode, found on `PATH`, in the usual install folders, on Flathub or at
  `iroriLudusaviPath`; never bundled. A game matches a Ludusavi title by Steam id, GOG
  id, then title, unless the user chose one (`iroriLudusaviTitle`). Windows games pass
  the prefix that holds `drive_c` (`wine_prefix_root`: `<prefix>/pfx` for Proton run
  directly, flat for Wine and umu). `iroriLudusaviAutoBackup` backs up after a session,
  after the store's cloud upload; `SavesKeeper` says which of the two keeps a game.
  The UI is `irori_ui::save_backups`. Inside Irori's Flatpak it is found and run on
  the host (`irori_core::host`), and the Ludusavi Flatpak is given the prefix and the
  backup folder with `--filesystem`.
- **Backups** (`backup.rs`, `backup_schedule.rs`): a backup is one JSON file of
  settings, per-game settings, library data and playtime; restoring replaces settings
  and merges the rest. Scheduled backups (`iroriBackups`) run from the periodic tick and
  before a new version first runs, and prune only their own files. Store sign-ins and the
  `iroriBackups`/`iroriSync`/`iroriSunshine` settings never go in.
- **Sync** (`sync/`): each computer writes `Irori sync/<device id>.json` to a folder or
  WebDAV (`sync/target.rs`) and merges the others' (`sync/merge.rs`): every value the user
  sets is a last-writer-wins register with a hybrid-clock time and the device that set
  it, sessions merge as a set, and playtime is the largest total beyond sessions plus
  every session, so nothing counts twice. New user data that should travel needs a
  register there and a test in `sync/merge_tests.rs`; data that differs per computer
  (paths, installs) must stay out. A session's playtime and history are written under
  `SyncService::playtime_guard`.
- **Sunshine** (`sunshine/`, settings `iroriSunshine`, login `<config>/sunshine.json`
  through `irori_secrets`): Irori's installed games (or favourites), plus an "Irori Big
  Picture" application, kept among Sunshine's applications through its web API on
  `127.0.0.1` (self-signed HTTPS, Basic auth, no `Origin` header so no CSRF token; the
  port is `sunshine.conf`'s `port` plus one). Each has the command `irori --launch
  <runner>/<app> --wait`, `wait-all` on, `auto-detach` off, and a 600 × 800 PNG cover
  uploaded through `/api/covers/upload`. Irori's applications carry an `irori-id` key
  (Sunshine keeps unknown keys) and are found by their command when an edit dropped it;
  only those are touched, and only Irori's own fields: what the user set there stays, a
  picture they chose too. Sunshine sorts its list after every change, so each step
  re-reads it (`plan.rs` is pure and tested; `tests.rs` runs against a stand-in). Syncs
  follow `LibraryUpdated`/`SettingsChanged` after 3 s of quiet, and talk to Sunshine only
  when what Irori wants differs from `state/sunshine.json`; a failure waits 10 minutes.
  Turning the setting off takes Irori's applications out. A game with `iroriInSunshine` off
  (per game) gets none.
  - **Apollo** (Sunshine's fork) is told apart by its API: Basic auth is refused, so the
    client signs in at `/api/login` (one session cookie for the whole web UI: signing in
    signs a browser out, so the session is kept between syncs) and names applications by
    `uuid` instead of their place.
  - **The client** (`sunshine/stream.rs`): the waiting invocation reads Sunshine's
    `SUNSHINE_CLIENT_WIDTH`/`_HEIGHT`/`_FPS`/`_HDR` and passes them through the instance
    channel to `LaunchParams::stream`. The game gets the same variables; with
    `iroriSunshine.matchClient`, a game in gamescope takes the client's output size,
    frame limit and HDR where its own settings leave them empty.
- **Screenshots** (`screenshots.rs`, `Backend::screenshots`): the user's own
  screenshots, only read, never moved or removed. A Steam app's folder
  (`userdata/<account>/760/remote/<appid>/screenshots`) names its game; every other
  file (the desktop's screenshot folders, Steam shortcuts' folders) belongs to the
  session in `sessions.jsonl` it was taken in, by the time in its name or else its
  modified time. Folder listings are cached in `<cache>/screenshots.json` by the
  folder's modified time; matching is redone each build. The stats page's Album tab
  and its insight card show them, and journal notes link them. Video files (mp4, mkv,
  webm, mov) count too: the replay buffer's clip folders, each named after its game, are
  read the same way.
- **Year in review** (`irori-library::stats::year`, `year_view`): a calendar year of
  `sessions.jsonl` in local time, told as full-window stories on
  `Route::YearInReview(year)`; the shell hides the sidebar there. The numbers are pure
  functions tested in `year.rs`; the shareable summary card is an SVG rendered to PNG
  with resvg (`year_card.rs`, the rasterizer GPUI already builds), never a web engine.
- **Storage** (`storage.rs`, `Backend::storage_report`, `Backend::clean_up`): what games,
  prefixes, Wine builds, emulators and Irori's caches take per disk, measured on a
  blocking thread and cached in `<cache>/storage.json` by folder and modified time, plus
  the cleanups (download leftovers, unused prefixes, old Wine builds, art thumbnails,
  logs). `clean_up` rebuilds the report and deletes only what it still offers, each item
  through `fs_guard::ensure_deletable`. Settings → Storage draws it.
- **Replay buffer** (`replay/`, Linux only): with `iroriReplay.enabled` (game
  defaults or one game), gpu-screen-recorder runs as its own process for the session,
  found on `PATH` or as the Flatpak `com.dec05eba.gpu_screen_recorder` and never
  bundled. `Backend::save_clip` sends it SIGUSR1 (the process is found by its `-o`
  folder, which also finds the Flatpak's); the path it prints is renamed to
  `<game> <date> <time>.mp4` and announced with a notice. Every way to save calls
  `save_clip`: Guide + X on a controller (`gamepad/reader.rs`), the tray, the command
  palette and a key from the XDG GlobalShortcuts portal (`replay/shortcut.rs`).
- **Launch troubleshooting** (`troubleshoot/`): `Backend::troubleshoot_launch` reads
  the game's last log (without Irori's header), its program's imports
  (`troubleshoot::pe`, a headers-only PE reader under `#![forbid(unsafe_code)]`),
  the prefix's `winetricks.log` (`pfx/` too, Steam's through protontricks' path),
  its anticheat entry and, on Linux, `vm.max_map_count`; `analysis.rs` turns that
  into findings, each with the log lines showing it and its fixes, and is pure and
  tested. Fixes tried this session (`mark_fix_tried`) come last.
  `install_prefix_verbs` runs winetricks (protontricks for Steam). Irori never runs
  `sudo`: a low map count is explained with the commands to run. Games through
  Wine or Proton get `WINEDEBUG=-all,err+module,err+mscoree`
  (`irori_runtime::env::game_debug_env`) unless the user set `WINEDEBUG` or verbose
  logs, so the log names a missing DLL; installers and winetricks keep their own.
  The UI is `irori_game::open_launch_problem_sheet` (after a failed start) and
  `open_troubleshooter` (game menu, palette, Ctrl+Shift+T on the game page).
- **Launching** (`games.rs`):
  1. Busy and availability checks.
  2. Optional cloud-save sync.
  3. `irori_runtime::prepare_launch`, which also builds the bubblewrap sandbox for a
     game with `iroriSandbox` on and refuses with `SandboxUnavailable` rather than
     start it without (`docs/PLATFORMS.md`, "Sandboxed games").
  4. Before-launch script.
  5. `store.launch` builds the `LaunchPlan`.
  6. `process::spawn`, with a capped game log, then the programs started
     alongside (`programs.rs`, `iroriLaunchPrograms`), each after its delay.
     Windows ones run with the game's Wine layer and prefix (`PROTON_VERB=run`);
     a Steam game's Proton prefix is reached only through `protontricks-launch`.
  7. Sleep inhibition, the replay buffer, then session tracking. When the started
     program exits but left processes running (a crack's or publisher's launcher
     handing over to the game), the session follows them (`follow::Handoff`): on Unix
     whatever is still in its process group, checked every 3 s (daemons and
     wineserver start a session of their own and don't count); on Windows its
     descendants by parent process started after it, found with sysinfo every 3 s.
     Stopping ends them too. Games another program owns (`LaunchPlan::watch`,
     third-party launchers) and browser apps aren't followed this way. At the end
     the programs that stop with the game are stopped (not-yet-started ones never
     start) before the after-launch script, then `GameExited`. While programs run
     in the prefix, a game whose launcher exits early is followed by its folder,
     not `wineserver -w`, which would wait for them too.

### 4.3 Stores

Every store crate follows the same shape:

- `pub struct <Name>Store` with `pub fn new(bus: EventBus, settings: SettingsFn) -> Arc<Self>`,
  usually built with `Arc::new_cyclic` around a weak self.
- `pub fn import_from_heroic(heroic_config_dir: &Path) -> anyhow::Result<()>`. It copies
  the user's Heroic credentials and installs into Irori's layout. Existing Irori data
  wins over Heroic's.
- Test constructors take explicit paths or endpoints (`with_paths`, `with_endpoints`,
  `with_layout`), so tests run against temp dirs and in-process servers.

**Several accounts.** A store crate never handles more than one login. The shell
wraps Epic, GOG, Amazon, ZOOM and itch.io in `irori_services::MultiAccountStore`,
which runs one instance per account ("slot") and merges them into one `Store`:

- The main slot uses the store's usual directories, so single-account data needs no
  migration. Extra slots use `Paths::account_config_dir`/`account_cache_dir`, built
  through each store's `for_account(bus, settings, config, cache)` constructor.
- Games stay keyed by app name. A game comes from the slot it is installed with, else
  from the first slot owning it, and every per-game call goes to that slot.
- Code that needs a concrete store for one game (GOG branch passwords, REDmod) asks
  the wrapper with `store_for(app)`; store-wide tools (EOS overlay, EGL sync) use
  `main()`.

The shell registers the stores in `crates/irori/src/stores.rs`, in this order: Epic,
GOG, Amazon, ZOOM (only when `settings.zoom_platform` is on), itch.io (always
registered; empty and signed out while `settings.itch_io` is off, so its account card
can turn it on), Steam, Launchers (always registered; empty while
`iroriThirdPartyLaunchers` is off), Emulation (the ROM library), then Sideload. Adding a
store means:

1. A protocol crate, if the store has a protocol.
2. A store crate, with `for_account` if the store has accounts.
3. Registration in `stores.rs` (wrapped with `with_accounts` if it has accounts), plus
   its Heroic import in `HEROIC_IMPORTS`.
4. Its login flow.
5. Its strings in every locale.

### 4.4 UI (`irori-ui` and the feature crates)

`irori_ui::init(backend, cx)` runs after `gpui_kit::init`. It installs i18n,
`AppState`, theme, art, navigation and models.

**Calling the backend from a view.** Never block the UI thread, and never await a
backend future outside `irori_ui::run`, because it needs the Tokio runtime:

```rust
let task = irori_ui::run(cx, async move { backend.launch(&id, None, vec![]).await });
cx.spawn_in(window, async move |this, cx| {
    let result = task.await;
    let _ = this.update_in(cx, |this, window, cx| {
        // Update state; report errors with a notification or an inline Alert.
        cx.notify();
    });
})
.detach();
```

`run_infallible` returns `Task<Option<T>>`. `backend(cx)` clones the `Backend`.

**Models** (`models.rs`):

- `LibraryModel`, `StatusModel`, `DownloadsModel`, `ComponentsModel`, `AccountsModel`,
  `SettingsModel`, `NoticeModel` and `SessionModel`, all reached through `models(cx)`.
- One event pump reads the bus in batches and notifies each affected model once.
- Views observe only the models they render, and keep the returned `Subscription`s in
  a `_subscriptions: Vec<Subscription>` field.

**Navigation** (`nav.rs`):

- `Route` variants: Library, Game, Downloads, Updates, Settings(section), GameSettings,
  WineManager, Emulators, Accounts, Stores, Stats, YearInReview(year), BigPicture,
  Onboarding.
- Use `navigate(route, cx)`, `go_back`, `go_forward`. The shell decides which pages it
  caches.

**Art** (`art.rs`):

- Originals are downloaded once. Thumbnails are cut per width bucket and stored on
  disk; decodes are limited to 2 at a time.
- `ArtStore::get(url, width, keep_alpha)` returns `Ready`, `Loading` or `Missing`.
- Wrap art-heavy regions in `div().image_cache(LruImageCache::new(n, cx))`. Use the
  components (`GameCover`, `art_img`) instead of loading images yourself.

**Theme and translucency:**

- Themes live in `assets/themes`; Irori Night and Irori Day are the defaults. Apply
  them with `theme::apply`.
- On Omarchy, COSMIC and KDE Plasma the default theme is the desktop's own: `omarchy.rs`
  reads Omarchy's `colors.toml`, `cosmic.rs` reads COSMIC's theme files, `kde.rs` reads
  `kdeglobals` and the active colour scheme, each fills in a
  `desktop_theme::Palette`, and Irori follows the desktop's theme switches. These
  themes stay out of the theme registry, which never replaces a theme it already has.
  COSMIC's `frosted_windows` and window opacity also decide Automatic translucency
  (`glass::resolve`, `Glass::Desktop`).
- With `iroriUi.wallpaperColors` on (off by default, every OS) the default theme is the
  wallpaper's instead, over a followed desktop's: `wallpaper.rs` reads pywal's
  `colors.json` (which wallust writes too) or the matugen JSON the user names in
  `iroriUi.wallpaperColorsFile`, into a `Palette` per mode. `theme::follow_wallpaper`
  starts, stops or moves the folder watch as the settings change, and re-themes only
  when the parsed colours differ (`Desktop::Wallpaper`, `docs/PLATFORMS.md`).
- Elsewhere the default theme is Irori's own in the system's accent colour: the settings
  portal's on other Linux desktops, `controlAccentColor` and the highlight colour on macOS,
  `UISettings` and its shades on Windows (`desktop_accent.rs` and its `macos.rs` and
  `windows.rs`, `desktop_theme::system_accented`). Every source signals its changes; nothing
  polls. This replaces only Irori's accent colours, held legible on its surfaces. A followed
  desktop's theme, any theme the user picked and the `iroriUi.systemAccent` switch win;
  `docs/PLATFORMS.md` lists who offers an accent.
- What the system asks for accessibility applies to every theme, a picked one too: macOS
  "Increase contrast" and Windows high contrast strengthen borders, Windows high contrast
  replaces the theme with its own colours (`desktop_theme::high_contrast`), and "Reduce
  transparency" or "Transparency effects" off keep Automatic translucency opaque.
- An accent the user picks (`iroriUi.accentColor`) tints whichever theme shows, held
  legible like the system's; `iroriUi.fontFamily` replaces Manrope while that family is
  installed (`theme::font_names`).
- The library's background (`iroriUi.libraryBackground`: the focused, last played or
  first shown game's art, or a picture) goes through `components::library_backdrop`,
  which keeps `ArtBackdrop`'s measured legibility (strength and dim only weaken the art)
  and stays off while `theme::reduce_transparency` holds.
- Surfaces take their colours from `glass::chrome(..)`, `glass::page(..)` and
  `solid_pages()`. Never add alpha yourself.
- `theme::apply` holds `warning`, `danger`, `info` and `success` to readable contrast,
  because themes pick them by eye. Colours that already read stay as the theme has them.
- On Linux `cx.theme().overlay` is transparent and the shell paints the dim behind
  dialogs and sheets (`theme::modal_dim`), because gpui-kit would dim the window's
  shadow margin too. Read the dim from there, not from the theme.
- The available backdrops differ per platform; see the table in `docs/PLATFORMS.md`.

**Components** (`components/`): `GameCover`, `ArtImage`, `EmptyState`, `PageHeader`,
`SectionTitle`, `ProgressSummary`, `StoreBadge`. Style helpers are in `style.rs`
(`legible` contrast, `reduce_motion(cx)`, `keyboard_focusable`); motion constants are
in `motion.rs`.

**Feature crates:**

- Each exposes `pub fn init(cx)`, its `*View` entities (`impl Render`, with
  `pub fn new(.., window, cx)`), `open_*_dialog` / `open_*_sheet` functions, `*Event`
  enums and `pub use i18n::CATALOGS`.
- Key contexts are `*_CONTEXT` constants. Actions are declared per crate and bound
  once.

### 4.5 Shell (`crates/irori`)

`main.rs` does these steps in order. Keep them in this order:

1. `--sign-in-window` (macOS and Windows): run the sign-in window and exit.
2. Parse arguments. A command such as `list` runs without a window and exits; an
   unknown bare word exits 2.
3. `instance::acquire`. A second instance forwards its arguments over authenticated
   loopback TCP (`instance.json` holds the port and token, mode 0600) and exits. A
   waiting one (`--wait`, before this step) ends its message with a newline and keeps
   the connection: the running instance answers on it when the session ends, and its
   closing means the waiter went away.
4. Logging, `Backend::start(stores::build)`, the GPUI application, URL and reopen
   handlers.
5. Inside `run`: `gpui_kit::init`, `irori_ui::init`, every feature `init`, hooks,
   key bindings, theme, tray, main window, then the `ShellCommand` loop.

Other parts:

- `shell.rs` (`AppShell`) owns the window content: title bar, sidebar, pages,
  notices, language/theme/translucency changes, and the "playing" state.
- `actions.rs` binds global shortcuts. `ctrl-` becomes `cmd-` on macOS, which also
  gets a native menu.
- `palette.rs` is the Ctrl/Cmd+K command palette.
- Tray: `tray.rs` on Linux (ksni) and `tray_native.rs` on macOS and Windows
  (`tray-icon`, kept on the main thread).

### 4.6 Store sign-in

**Sign-in window:**

- `LoginDialog` (`crates/feature-manage/src/login/`) opens the store's page in a
  window of its own.
- That window runs in a **separate process**, so Irori's window never embeds a web
  engine.
- It watches every address and page text for the store's `capture` pattern, prints
  `{"text": "..."}` and exits 0. Closing it exits 2; bad arguments exit 1.

**Which program shows it** (`login/window.rs::helper`), in this order:

1. The `irori-sign-in` program next to Irori's executable, or on `PATH`.
2. On macOS and Windows only, Irori itself, started as
   `irori --sign-in-window --url … --capture … --title …`. Their web engines (WKWebView,
   WebView2) ship with the system, so this adds no runtime dependency.
3. Otherwise the browser. The dialog says why, opens the page in the default browser,
   watches the clipboard for the capture pattern and accepts a pasted code.

On Linux the window needs WebKitGTK 4.1, which stays optional: Irori runs without it.
So never make `irori` link WebKitGTK on Linux.

`irori-sign-in` stays a standalone program with a stable command line. Packages ship
it on every platform, and CI's Windows install test runs it.

**Store window** (`crates/sign-in/src/store.rs`, `irori_manage::store_window`):

- The same program shows a store's website when its first argument is `--store`
  (`--url`, `--title`, `--profile`, `--labels`). It is the sign-in window's sibling, not
  part of it: the sign-in command line stays as it is.
- A toolbar (a page of Irori's own) sits above the store's page. The store's cookies stay in
  `<data>/store-window/<runner>` (on macOS a data store named after that folder), apart
  from Irori's own store login.
- JSON lines: Irori writes `{"open": url}` to show a page in the open window; the window
  writes `{"external": url}` for "Open in browser". Irori opens only web addresses
  (`irori_ui::web_url`), and the page can't ask for the browser itself.
- One window per store. When it closes, a signed-in store's library refreshes. Stores →
  Open store, the palette's "Open the … store" and the game menu's Store page
  (`irori_ui::hooks::open_store_page`) open it; without the helper they open the browser.

**A new store's login flow:**

- Prefer a capture pattern the sign-in window can find without user help: a redirect
  URL parameter, or the JSON a page shows.
- Write `instructions` for the browser fallback.
- Make `login(input)` accept the full page text, the address or the bare code.

### 4.7 Big Picture and controllers

- `feature-bigpicture/src/gamepad/reader.rs` reads controllers on a dedicated gilrs
  thread that blocks until an event arrives, so it uses no CPU when idle.
- `input.rs` normalizes input (repeat, dedupe) and is unit-tested.
- `mod.rs` dispatches `irori_gamepad` actions to the focused element, or synthesizes
  keystrokes (arrows, Enter, Escape, Ctrl+Enter, Ctrl+K) when the element doesn't
  handle them.
- The Guide button toggles Big Picture. Every screen must stay usable with arrows,
  Enter and Escape, because that is how controllers reach it.
- Big Picture's screen (`view/`) has a module per region: `backdrop` (the focused
  game's blurred art, crossfading), `chrome` (top bar, button hints drawn per
  controller family), `home` (shelves), `library` (chips and grid), `page` (a game's
  action list and details), `log` (the page's live game log, in place of the details;
  the shoulders page through it, and rows under Show log filter it with
  `irori_ui::game_log::visible_lines`, as the desktop log viewer does),
  `settings` (the game's settings over its page, outside the `BigPicture` key context so
  keys reach their controls; Escape returns to the page),
  `overlay` (quick menu, uninstall question), `tile` (covers). Pure logic lives in `model.rs` (shelves, filters, page rows) and
  `layout.rs` (sizes from the window size in rem, scrolling); test it there.
- Shelves, the grid and the page's list scroll by springing one offset toward the
  focus and build only the items near the view, so nothing animates once the focus
  rests.
- Home has six layouts (`layout::HomeLayout`: Shelves, Spotlight, Cinema, Wall,
  Carousel, Compact), each with its geometry in `layout/home.rs` and a renderer in
  `view/`. They share one focus (`FocusGrid`), so switching keeps the focused game;
  Compact turns the arrows (`grid_move`) and the Wall crosses sections
  (`wall_step`). The Carousel springs each cover toward its own place on the wheel.
- Themes (`theme/`) lay colours over the interface theme (held legible in
  `view/look.rs`), pick a font, a background (game art, gradient, an embedded
  picture, or plain), cover shape, size and corners, and Home's shelves. The
  gallery is `themes/*.irori-theme.json`, parsed like shared files; the user's
  are `<config>/bigpicture/themes/<id>.irori-theme.json`. Theme files are
  versioned and checked field by field; pictures are embedded PNG/JPEG/WebP of
  bounded size, never a path or address, and ids are made by Irori, so a file
  can't name anything outside that folder. The editor (`view/editor.rs`, quick
  menu → Themes and layouts) edits a draft that Home shows live, and keeps it on
  close. Settings: `iroriUi.bigPictureLayout`, `bigPictureTheme`,
  `bigPictureOnController` (open Big Picture when a controller connects).
- The Library's search field sits outside the `BigPicture` key context
  (`SEARCH_CONTEXT`), so letters bound there (M, Space) reach the field.
- **In-game overlay** (`overlay/`, beta, Linux, `iroriOverlay.enabled`): Guide while a
  game Irori started runs opens a panel above the game (`overlay::open`) instead of
  toggling Big Picture, and every controller input goes to it while it is open. It is
  a second GPUI window in Irori's process (`overlay/window.rs`): a wlr layer-shell
  surface on the overlay layer with on-demand keyboard where the compositor has one,
  else a pop-up window, and it exists only while open. Its tabs and focus are pure
  logic in `overlay/model.rs`. While it is open the controllers are grabbed
  (`gamepad/grab.rs`, `EVIOCGRAB`) and read raw (`gamepad/evdev.rs`) on a thread that
  ends with it. Volume is `Backend::audio`/`change_volume` (`services/src/audio.rs`,
  `pactl` matched to the game's process group), screenshots `Backend::take_screenshot`
  (`services/src/capture.rs`, desktop portal, else `grim`). The shell installs "Back to
  Irori" with `overlay::install_show_irori`. Chords with Guide held
  (`gamepad::Chord`): X saves a clip; with the overlay on, RB takes a screenshot and
  Up/Down change the system volume. `docs/PLATFORMS.md`, "In-game overlay".

### 4.8 Engines (emulators)

Design: `docs/design/engines.md`; Lutris behaviour: `docs/parity/lutris.md`.

- **Words.** `Runner` is a store. An emulator, RetroArch, ScummVM or DOSBox is an
  **engine** in code (`irori_engines::EngineManifest`) and an "emulator" in the UI. A
  console is a **system** (`assets/engines/systems.json`, ids like `psx`, `snes`, `dos`).
  An engine choice is an `EngineRef`, written `duckstation` or `retroarch:<core>`.
- **Manifests are data.** Each engine is `assets/engines/<id>.json`: sources per OS
  (GitHub release regexes, the libretro buildbot, a download page, Flatpak id, `PATH`
  names, app bundle, Windows paths), the launch template, options with **one** visibility
  rule (`visibleWhen`; an option limited to other hosts adds nothing to the launch),
  BIOS folder, exit hotkey and licence. Never pin a download URL; describe where
  versions are listed. The only per-engine code is the `kind` match in
  `crates/engines/src/launch.rs` (RetroArch cores, DOSBox, ScummVM, MAME, relinkers). Users may
  override a manifest with `<config>/engines/<id>.json` (merge patch).
- **Engines shipped:** RetroArch (12 cores), DuckStation, PPSSPP, Dolphin, ScummVM,
  DOSBox Staging, PCSX2, melonDS, mGBA, Azahar, Gopher64, MAME, Mupen64Plus, M64Py,
  DeSmuME and ePSXe (these five detected only; ePSXe is closed source), and the
  experimental Cemu, RPCS3, xemu, shadPS4 and AnyPS5 (never a system's default while
  another engine plays it). Systems may be `folderOnly` (arcade zips, Xbox images) and
  folder games may name the file engines open (`folderMain`), the files naming them
  (`folderTitle`: PS5 `param.json`, PS3/PS4 `PARAM.SFO` through `irori_engines::sfo`)
  and the folders of their own layout that never name them (`layoutFolders`); BIOS
  files with many names match a `pattern`.
- **Converting engines.** AnyPS5 (`kind: relinker`, `irori_engines::relink`) turns a PS5
  game into a program of this computer before it plays, in Irori's data folder with the
  game's files linked into `app0/`; it never writes the game folder. Its releases are a
  program plus a library archive (`extraAssets`), for Linux and Windows x86-64 only, so
  macOS explains it isn't available (`EngineUnavailable`) instead of offering an install.
- **Option texts** are `manage.engines.option.<id>.label` / `.help` (and
  `.<choice>` for choice values) in `irori-manage`; a test fails when a built-in option
  has no English text.
- **Installs.** Irori downloads engines from the projects' own servers on request and
  never redistributes them. Managed versions sit side by side under
  `<data>/engines/<id>/<version>/` and are recorded in `<state>/engines.json` with the
  default installation per engine. Installing never changes what games use; Update does,
  and a game pinned with `iroriEngineVersion` keeps its version. Flatpak and system
  installs are detected and labelled, never installed or removed.
- **BIOS.** Never download or link to BIOS files or ROMs. The BIOS index hashes only
  plausible files and caches digests by path, size and modification time; nothing is
  hashed at launch. Files are linked under the expected names before a launch.
- **Settings keys** are Irori-only: `iroriEngine`, `iroriEngineVersion`,
  `iroriEngineOptions` (merged per engine and option over the defaults),
  `iroriEngineOwnSettings`, `iroriSystemEngines`, `iroriRomFolders`, `iroriBiosFolder`.
- **The ROM library** (`irori-emulation`, `Runner::Emulation`, serialized id
  `emulation`, app names are a hash of the game's first file) scans `iroriRomFolders`
  incrementally (detections cached by path, size and mtime) and watches them with
  `notify`. It never deletes ROMs: removing a game only hides it from the library.
  Launch order: BIOS check (typed `BiosMissing`), engine choice (game, then system
  default, then first installed recommended), missing RetroArch cores installed on the
  way, version pin (typed `EngineMissing`), then the command. `GameInfo.system` and
  `GameInfo.tags` (region, revision) drive the system filter, badges, the generated
  cover and Big Picture's per-system shelves.
- **Lutris import** (`irori-extras::lutris_import` reads; `crates/irori/src/lutris.rs`
  applies; Bottles and Faugus Launcher plans from `irori-extras::wine_launchers` use
  the same shapes and apply step). It only reads Lutris: `pga.db` is opened read-only and immutable, YAML is
  parsed, nothing is written there. Windows/Linux games become added apps with their
  prefix, Wine build (matched by name, else reported), arguments and variables;
  emulated games join the ROM library as added games with their emulator; Steam and
  store games are matched by id. Every game not imported comes back with a reason id
  that `irori_ui::lutris_summary` translates. Tests build the SQLite database in the
  test; `rusqlite` is built with `bundled` on every OS.
- **Playnite import** (Windows; `irori-extras::playnite` reads, `crates/irori/src/playnite.rs`
  applies, its summary is the Lutris one). Playnite is found through the program its
  `playnite://` link opens (`HKCU\Software\Classes\Playnite\shell\open\command`), a
  copy without `unins000.exe` being portable, and `DatabasePath` in its `config.json`.
  `games.db` and `categories.db` are read with `playnite::litedb`, a read-only LiteDB 4
  (file version 7) page and BSON reader that bounds-checks every offset and link; tests
  build the file with a writer of that format in the test. Playnite holds the file
  exclusively while it runs, so the import asks to close it; nothing is written there.
  Games of stores Irori lists (plugin ids in `STORE_PLUGINS`) are matched by id or
  install folder; other installed, non-hidden games become added apps with
  `SideloadApp::launch_link` (`playnite://playnite/start/<id>`): Irori opens the link
  through the shell and a `GameWatch` follows the processes in the game's folder, and
  removing one never deletes that folder. Only the discovery is Windows-only.
- **RetroAchievements.** The ROM library's optional account is a RetroAchievements one
  (`irori-emulation::retro`): username and Web API key through `Store::login`, kept with
  `irori_secrets`, shown and changed on the Emulators page's Achievements tab. Games
  match by rcheevos' hash of their file (`irori_retroachievements_api::hash`; CHD, CSO
  and PBP can't be read), against each console's game list (cached a week). "Sign
  emulators in" exchanges the password once for the emulators' token (never kept) and
  writes it where each emulator reads it (`irori_engines::cheevos`: RetroArch, PCSX2,
  PPSSPP, Dolphin); DuckStation encrypts its token per machine, so Irori only turns
  achievements on there.
- **Fix dialogs.** `irori_ui::save_conflict::resolve_launch_error` also turns
  `EngineMissing` and `BiosMissing` into dialogs with a fix (install and play, use the
  default version, choose the BIOS folder). Every place that starts a game already
  calls it.
- **UI.** The Emulators page (`Route::Emulators`, `irori-manage::emulators`) installs,
  updates and removes versions and cores, and picks each system's default emulator.
  Installs run through `irori_manage::start_install` so any screen can start one and the
  page shows its progress. Changes emit `Event::EnginesChanged`.
- **Lutris install scripts** (`irori-scripts` interprets; `irori_services::scripts`
  hosts Wine, DOSBox, programs and prompts; `irori_library::open_lutris_installer` is
  the window, reached from Add game, the command palette and `irori://lutris/<slug>`).
  These rules keep a community script from doing anything the user didn't see:
  - **Typed commands only.** A script is parsed into enums before anything runs. An
    unknown command, task, parameter or variable refuses the whole script, naming the
    step; Irori's own variables (`$GAMEDIR`, `$CACHE`, `$HOME`…) cannot be redefined.
    Never add a "pass through" command or leave an unknown variable as text.
  - **Consent before running.** `irori_scripts::consent` lists downloads (host and
    size), every write outside the game folder, the cache and the prefix (each approved
    on its own), programs, registry edits, winetricks verbs and store autosetups. The
    executor refuses to run without `Approvals::accepted`.
  - **Shell commands verbatim.** Every `execute` command line is shown exactly as the
    script wrote it and as it will run, and needs its own "I understand" acknowledgement
    (`Approvals::shell_acknowledged`); the executor checks it again. Running an
    interpreter (`sh`, `python`, `cmd`…) with `file:` counts as a shell command. Values
    from the script (variables, menu choices, file names) can't change a command:
    `Vars::substitute` refuses shell metacharacters in them.
  - **Play is part of the consent.** The plan lists what the game section starts and
    every variable the script sets. The game's `exe` must be inside the game folder, the
    cache or the prefix, or be a file the script obtained.
  - **Writes are confined** to `$GAMEDIR`, `$CACHE`, the prefix and approved paths,
    with symlinks resolved before every check; archives cannot escape their target, and
    links they hold can't point out of it. File ids are plain names (letters, digits,
    `_`), because they name a folder of the cache.
    Cancel or failure rolls back what the install created in the game folder and
    keeps downloads. An install that only lacks the game's program keeps its files
    (`UnfinishedInstall`) so the user can choose the program.
  - **Native first.** Scripts installing the Epic Games Launcher, the EA app or
    Ubisoft Connect in Wine are refused (`PreferNative`); GOG, Amazon and ZOOM
    autosetups stop with `InstallFromStore`; a library game with the same title in a
    native store is offered before the script.
  - **lutris.net etiquette.** Fetch only when the user searches or opens a game's
    scripts, never in the background; one shared client keeps to two requests a second
    and caches answers for a day; show every script's author, date and lutris.net page;
    never bundle, mirror or commit lutris.net scripts or catalogue data (test fixtures
    are hand-written).
  - **Result.** Wine and Linux scripts become added apps (`winePrefix`, `wineVersion`,
    `enviromentOptions`); emulator runners join the ROM library with `iroriEngine`
    (mapped like the Lutris import). The Wine build is the script's own when installed
    (`match_wine_version`), else the default, and the window says which. Each game
    records its script in the Irori-only per-game setting `iroriLutrisScript`
    (`{slug, installerSlug, version, revision, sourceUrl}`) for later reinstalls and
    updates.

### 4.9 Third-party launchers

- Irori never handles Battle.net, EA app, Ubisoft Connect or Rockstar accounts. The
  Stores page sets a launcher up once: a prefix of its own (`<prefixes>/<launcher
  name>`) with the Wine build the user picks, and the vendor's official installer
  (natively on Windows), or "Use an existing install" adopts a prefix that already has
  it. "Close" stops everything in the launcher's prefix (on Windows, the processes
  started from its folder); "Remove from Irori" forgets it and leaves the prefix and
  games on disk. The user signs in and installs games inside the launcher. A Proton
  prefix keeps `drive_c` in `pfx/`; read the launcher's files there.
- Games are read from the launcher's local data (`irori-launchers-data`) and listed as
  `Runner::Launchers` with `third_party_managed_app` set. App names are
  `<launcher>:<id>` (`battlenet`, `ea`, `ubisoft`, `rockstar`). They start through the
  launcher's flags or URI; Irori never installs, updates or removes them.
- **Sessions follow the game, not the launcher** (`crates/services/src/follow.rs`):
  while such a game runs, services look every 3 s for processes running from its
  install folder; the session ends when they are gone, or when the launcher exits
  without starting the game. Nothing polls when no launcher game runs. Every other
  game whose started program hands over and exits is followed through its process
  group or descendants instead (see Launching, step 7).
- **Rockstar** (`irori_launchers_data::rockstar`): `TITLES` names every Rockstar PC
  title with its Steam app id (art comes from Steam's CDN) and whether store copies
  need the launcher. The launcher keeps no readable list of owned titles (its
  settings and profiles are encrypted; its log lists the whole catalogue), so only
  installed titles are listed. Signed-in state comes from `launcher.log` and the
  profile folders in Documents; the account name in that log is never kept.
- **Store copies that need a launcher.** An Epic Rockstar title (GTA V, RDR2, the
  Definitive Editions…) launched under Wine without the Rockstar Games Launcher in its
  prefix fails with `irori_core::LauncherMissing`; the UI
  (`irori_ui::launcher_problem`) offers to run the vendor's installer in that prefix
  (`Backend::install_launcher_for`) and then plays. Steam installs the launcher for
  its own copies.
- **Xbox app, Microsoft Store and PC Game Pass** (`irori_launchers_data::xbox`),
  Windows only. Nothing to set up and no `Launcher` variant: the scan reads the
  `.GamingRoot` file at each drive's root (`RGBX`, a count, UTF-16 folder names) and
  each `<folder>\<game>\Content`, plus the packages Gaming Services lists
  (`HKLM\SOFTWARE\Microsoft\GamingServices\PackageRepository\Package`, folders from
  the AppModel package repository), read with `windows-registry`. A package is a game
  when it has a `MicrosoftGame.config`, sits in a gaming root, or Gaming Services
  lists it; one without an application (DLC, frameworks) or of the Xbox platform
  itself is not. App names are `xbox:<package family name>` (the family name is
  computed from the identity's publisher, so it survives updates). Titles prefer the
  config's names, then the manifest's, with `ms-resource:` references looked up in
  Windows' `MrtCache`, then the game's folder name. Art is the package's own logos at
  their largest `scale-`/`targetsize-` variant (`file://` paths); sizes are added up in
  the background after a scan. Games start with
  `rundll32 shell32.dll,ShellExec_RunDLL shell:AppsFolder\<family>!<app id>`, which
  returns at once with success, and a `GameWatch` follows (and stops) the processes
  running from the package folder. The game menu's "Show in Microsoft Store"
  (`ClientAction::Show`) opens `ms-windows-store://pdp/`; installs, updates and removal
  stay with the Xbox app. On Linux and macOS the card says it is Windows-only.
- **Online-only anticheat.** `irori_extras::anticheat::KNOWN` lists games whose
  anticheat blocks only an online mode under Wine (GTA Online); the lookup sets
  `online_only`, the install gate lets them through and the game page says which mode
  is blocked.

### 4.10 Steam

- **Without an account** the library is what Steam installed (`scan.rs`: library
  folders, manifests, `localconfig.vdf` playtime, `appcache/librarycache` art), watched
  with `notify` on each `steamapps` folder. With one (`account.rs`), it adds the account's
  owned and Family games, cached in `library.json`; `refresh` asks Steam at most every
  15 minutes unless it is a full refresh, and publishes the installed games even when
  Steam can't be reached.
- **Login** (`signin.rs` for the UI, `irori-steam-api::auth`): SteamClient-type sessions
  through `IAuthenticationService` (protobuf over the Web API). The UI emits
  `login_input(tokens)`; `Store::login` proves the tokens by reading the library, then
  keeps them in `session.json` (through `irori_secrets`). A refused refresh becomes `SessionExpired`.
  Never log tokens, passwords or codes; the API crate's types redact them in `Debug`.
- **Launch** (`client.rs`, `watch.rs`): `steam://rungameid`, or `-applaunch` with the
  game's `launcherArgs`; a `SteamGameWatch` follows the reaper (Linux), the game folder's
  processes and the manifest's launch flags, re-checking known PIDs before a full scan.
- **Compatibility tool** (`compat/`, Linux UI only: `irori_settings` `steam_compat.rs` in
  the game's Wine & Proton section, through `SteamStore::compat_tools`/`set_compat_tool`):
  Valve's Protons (installed apps named `Proton …`, mapped to Steam's names such as
  `proton_9`, `proton_experimental`) and every `compatibilitytool.vdf` in
  `compatibilitytools.d` (Flatpak Steam: its own folder only). The choice is
  `InstallConfigStore/Software/Valve/Steam/CompatToolMapping/<appid>` in
  `config/config.vdf`, edited by splicing only that entry's bytes (`compat/mapping.rs`
  refuses a file it can't parse completely and reads every edit back), written atomically
  after copying the old file to `config.vdf.irori-backup`, and only while Steam is closed:
  Steam rewrites the file on exit. A running Steam fails with `SteamRunning`; the UI asks
  before `close_steam` (`steam -shutdown`), never in Steam's Game Mode. Reached from the
  game menu's "Steam Play…", the palette and the page's Settings button, all through
  `irori_game::ops::open_steam_play`/`settings_section`.
- **Settings that apply**: Steam starts its games with its own Proton, variables and
  wrappers, so a Steam game's settings show only `FieldSpec::applies_to_steam` fields
  (launch arguments, scripts, programs started alongside, replay buffer, Sunshine, save
  backups, art) and hide the Graphics, Tools and Log sections.
- **Art** order: Steam's cache, the store's hashed addresses (asked once per app,
  `IStoreBrowseService/GetItems`), the fixed CDN paths. Store pages (`appdetails`) fill
  the game page in Irori's language.

### 4.11 Mods

- **Storage** (`irori_mods::ModLibrary`): `<data>/mods/<runner>/<app>/` holds
  `mods.json` (load order; later wins), one folder per mod whose `files/` already
  mirror the game folder, `.written/` (what the game wrote into mod files and
  folders, applied on top), and `deployment.json` while mods are applied. Mod ids
  are `[a-z0-9-]` only, so they never name anything outside that folder.
- **Only while playing.** `Backend::launch_with` calls `apply_mods` right before
  the game starts and `restore_mods` after it exits (after the saves sync).
  Leftovers from a crash are taken out at start (`recover_mods`, the app only:
  a command-line run may sit beside a running game). Mods can't change while
  the game runs.
- **Never delete game files.** Links: the full manifest is written before any
  file is touched; a game file a mod replaces is renamed to
  `<name>.irori-original` beside itself; `deploy::undeploy` is safe to rerun at
  any step and calls `fs_guard::ensure_deletable` on the game folder first. A
  file Irori put there that the game changed or replaced goes to `.written/`.
  Overlay (Linux, `fuse-overlayfs`): the game folder is a lower layer and is
  never written; after unmounting, `overlay::settle` moves what the game wrote
  outside the mods' own files and folders into the game folder, where it would
  have gone without mods. Paths compare without letter case and follow the game
  folder's spelling (`align_case`).
- **Sources.** Archives go through `irori_mods::archive` (zip and 7z in process;
  rar through `unrar`/`7z`/`bsdtar` after listing every name), with
  `archive_guard` on every entry and no links. FOMOD (`fomod.rs`) is parsed as
  data only (no DTDs, no scripted installers); the UI asks the steps
  (`FomodPrompt`, `Backend::finish_fomod`). Thunderstore's ecosystem schema (cached
  a day) names each community's Steam/Epic ids, programs, loader and install
  rules; only BepInEx and MelonLoader communities install. Nexus Mods uses the
  user's personal API key (through `irori_secrets`, never logged); free accounts
  install through `nxm://` links, which carry a one-time key.
- **UI**: the game page's Mods tab (`irori_game::ModsPanel`, also a sheet from the
  game menu), `Event::ModsChanged(GameId)` refreshes it.

### 4.12 ReShade

- **Scope.** Windows builds of games Irori installs (`irori_services::supports_reshade`),
  natively on Windows and under Wine elsewhere; the UI is the game settings' Tools
  section (`irori_settings` `ReshadePanel`, route id `reshade`). Direct3D 9,
  DXGI (Direct3D 10/11/12) and OpenGL only: Vulkan needs ReShade's layer
  registered, Direct3D 8 and DirectDraw need wrappers (`ReshadeError::Unsupported`).
- **Detection** (`irori_reshade::detect`): the configured program (`targetExe`, else
  the store's executable) when its PE imports name an API; otherwise the game folder is
  searched (installers, redistributables, crash reporters, launchers and anticheat
  skipped) for one `Compatibility.ini` knows, else the largest that draws. The user can
  choose another program inside the game folder. `pe.rs` reads only the headers and
  import tables, bounded, never the whole file.
- **Compatibility list.** `Compatibility.ini` from crosire/reshade-shaders' `list`
  branch, the file ReShade's installer reads: `Banned` refuses the install (the
  configured launcher counts too), `RenderApi`/`ForceInstallApi` pick the API and DLL,
  `InstallTarget` the folder. AreWeAntiCheatYet anticheat makes the UI ask first.
- **Downloads** (`download.rs`): the version is read from reshade.me's front page
  link; the DLLs come out of the zip appended to `ReShade_Setup_<version>.exe`, each
  checked to be a PE of its architecture; shaders from `slim.zip`; under Wine,
  `d3dcompiler_47.dll` from Mozilla's fxc2 copy, checked against its SHA-256 (a fixed
  file). Lists are cached a day, downloads under `<cache>/reshade`. Never bundle any.
- **Journal** (`journal.rs`): the manifest (`<data>/reshade/<runner>/<app>.json`) lists
  every file with its hash and every folder created, saved before the first write.
  An existing DLL that isn't ours refuses the install (`DllTaken`); an existing
  `ReShade.ini`, compiler or shader is left alone and unrecorded. Off renames the DLL
  to `.irori-disabled`. Removal vets the folder with `fs_guard::ensure_deletable`,
  then deletes only recorded files whose hash still matches and recorded folders left
  empty. A failed or cancelled install removes what it wrote.
- **Launch.** Under Wine, `Backend::launch_with` adds `<dll>=n,b` (and
  `d3dcompiler_47=n,b` for Direct3D) to `WINEDLLOVERRIDES` through
  `irori_runtime::env::add_dll_overrides`, replacing only those DLLs' entries in the
  user's value, while the install is complete, on, and its DLL is there.

## 5. Rust conventions

Match the code around you. In particular:

**Errors**

- Use `anyhow::Result` with `.context("…")` / `.with_context(|| …)` worded for the
  user: these messages appear in the UI. Format chains with `{e:#}`.
- `thiserror` is only for typed errors that callers match on (see `irori-depot`,
  `irori-zoom`).
- No `unwrap()`/`expect()` on fallible runtime paths. They are fine in tests, and on
  invariants with an `expect("why this holds")`.

**Async and threads**

- Async code runs on the backend Tokio runtime.
- Never block it: use `tokio::fs`, `irori_core::process`, or `spawn_blocking` for
  heavy filesystem or CPU work.
- Never hold a `parking_lot` guard across `.await`. Use `tokio::sync::Mutex` when a
  lock must span awaits.
- Shared state is copy on write: `RwLock<Arc<T>>`, with a new `Arc` published on
  change. Share `Arc<GameInfo>` and don't clone game data per frame.

**Performance and memory**

- Precompile regexes with `once_cell::sync::Lazy`.
- Keep caches on disk. Don't hold whole API responses longer than needed.
- Derive filtered or sorted lists when their inputs change (by revision), not on
  every render.

**Dependencies**

- Declare dependencies in `[workspace.dependencies]` and use `dep.workspace = true` in
  crates. A dependency only one crate needs may stay local; give it an explicit
  version and name it in the PR.
- Keep `Cargo.lock` committed; CI builds with `--locked`.
- Prefer small crates with few default features (`default-features = false` is
  common here).

**`unsafe`**

- `irori-depot`, the protocol crates and `irori-gog-galaxy` are
  `#![forbid(unsafe_code)]`. Keep it that way.
- Elsewhere `unsafe` is for OS APIs only, with a `// SAFETY:` comment.

**Platform code**

- Gate it with `#[cfg(target_os = "…")]` (or `cfg!(..)` when both branches compile
  everywhere).
- Keep platform-specific pieces in their own module or file (`tray_native.rs`,
  `desktop_native`, `macos.rs`).
- Every change must build on Linux, macOS and Windows; CI checks all three. A
  `#[cfg]` without an `else` usually needs a fallback for the other platforms.

**Comments and docs**

- Every module starts with a `//!` header saying what it owns. Public items get `///`
  docs that describe behaviour, not implementation.
- Comments explain *why* (a store quirk, an OS limitation, a workaround with its
  reason). No narration ("now we loop over…"), no commented-out code, no TODOs
  without an issue.
- Keep comment density similar to the file you're in. Keep modules focused: split
  before a file passes ~1,000 lines unless the code is one cohesive view.

**Formatting and lints**

- Default `rustfmt` and default `clippy`, with warnings as errors (CI uses
  `-D warnings`).
- Don't silence a lint with `#[allow]` without a comment saying why.

**Naming**

- Crates are `irori-<area>`. Views are `*View`, events `*Event`, entry points
  `open_*_dialog` / `open_*_sheet`, and every crate has an `init(cx)`.
- Settings keys follow Heroic's spelling (camelCase); Irori-only keys start with
  `irori` or describe themselves (`uiLanguage`, `zoomPlatform`).

## 6. UI conventions

**APIs**

- Use only `gpui-kit`: `use gpui_kit::*` for GPUI, `gpui_kit::component::*` for
  components, and `gpui_kit::assets::IconName` for icons (`Icon::new(IconName::…)`).
- **Never invent an API.** Grep the exact sources we compile against:
  `~/.cargo/registry/src/index.crates.io-*/{gpui-kit-0.8.0,gpui-component-0.8.0,gpui-base-0.8.0,gpui-pre-0.3.9}`.

**Visual language**

- Use theme tokens only (`cx.theme().*`) and rem helpers (`p_4()`, `gap_3()`, `w_64()`).
  No raw colours, and no `px(…)` except for hairlines and art.
- Cover art carries the colour; chrome stays neutral. The primary colour is reserved
  for the main action of a region.
- Text must stay legible on every bundled theme in light and dark (`style::legible`,
  WCAG 4.5:1 for text, 3:1 for icons).

**Layout**

- Give every flex child that holds text `min_w_0()` (`min_h_0()` vertically) and
  `truncate()` where text may overflow. Have one scroll owner per region.
- Leave room for German and Russian, which run 30–40% longer than English. Never put
  text in a fixed-width box.
- Controls that come and go with the room (the library header's quick controls,
  `iroriUi.libraryQuickControls`) are picked from measured widths, never fixed
  breakpoints: measure the room and each control with a `canvas`, and keep the choice a
  pure function with tests (`irori-library`'s `quick.rs`).

**Feedback**

- Every async action shows loading, success and failure.
- Report errors to the user with `window.push_notification(Notification::error(format!("{e:#}")).title(t(..)))`
  or an inline `Alert`, never only in the log. Explain offline state; don't fail
  silently.

**Destructive actions**

- Confirm only destructive actions, with `window.open_alert_dialog` and a danger
  button.
- Name the object in the copy: Uninstall “Hades”? / Uninstall. Never write "Are you
  sure?".

**Reachability**

- Every action is reachable from a visible button, the context menu, the command
  palette and the keyboard, all calling the same action or method.
- No hover-only paths. Focus rings stay visible (`keyboard_focusable`).

**Motion:** use `motion::{QUICK, STANDARD}` and the `fade_in`/`rise_in` helpers.
Respect `style::reduce_motion(cx)`.

**Performance**

- Virtualize any list that can exceed ~50 items (`v_virtual_list`, `List`,
  `DataTable`).
- Never clone the library per frame. Re-render only from the models you observe.

**Copy**

- Sentence case, verbs on buttons ("Install", "Verify files"), curly quotes, no
  trailing period on buttons or titles.
- Store and tool names are never translated or changed (Epic Games, GOG, Steam,
  Proton, Wine, DXVK).

You can't see the screen, so when a change affects layout, run Irori and look. If you
can't, say so in the PR.

## 7. Translations

Irori ships 19 languages, in the order of `irori_ui::i18n::LANGUAGES`: `en, cs, de, es,
fr, it, nl, pl, pt, pt_BR, ro, sv, tr, ru, uk, ja, ko, zh_Hans, zh_Hant`.

**Catalogs**

- Each crate owns a key namespace and flat JSON catalogs in
  `crates/<crate>/locales/<code>.json`.
- Namespaces: `ui.`, `library.`, `game.`, `manage.`, `settings.`, `bigpicture.`,
  `shell.` (the `irori` crate, in `strings.rs`).
- `en.json` is the source of truth.

**In code**

- Use `t("ns.area.intent")` and `t_args("ns.area.intent", &[("name", value)])`.
- Keys describe intent, never the English text. Each full sentence gets its own key
  with `{placeholders}`; never assemble sentences from fragments.
- Plurals use `…_one` / `…_other` keys, picked in code.
- Don't translate log messages, identifiers, file paths, key names or brand names.

**Every new or changed string goes into all 19 catalogs in the same change**

- Translate it yourself, idiomatically, following each language's UI conventions.
  Keep the `{placeholder}` names identical.
- If you change an English string's meaning, update every translation too.
- Never leave English in another catalog: the tests treat a missing key as a failure.

**The tests enforce this.** They live in each crate's `i18n.rs`, Big Picture's
`catalog_tests` and `crates/irori/src/strings.rs`. They check:

- Every catalog parses and has every English key.
- No catalog has extra keys.
- Placeholder sets match.
- Every key literal in `src/**/*.rs` exists in `en.json`.
- Big Picture also fails on unused keys.

**Registration:** each crate registers its `CATALOGS` once (`std::sync::Once`) from
`init`. The language switches live; `shell.rs` calls `set_language` and refreshes the
window.

## 8. Tests

**Where tests live:** inline `#[cfg(test)] mod tests` or a `src/tests.rs` module. There
are no `tests/` directories or fixture folders. Fixtures are small inline JSON strings.

**No network, no real user data**

- Tests never reach the network or read the user's real config, Heroic data or
  stores.
- Use `tempfile` directories and `Paths::init(Paths::under(tmp))`.
- For HTTP, use `irori_depot::test_server::TestServer` (dev-dependency feature
  `test-server`) or a store's own test server. Store crates take endpoints or paths in
  test constructors for this.
- Amazon manifests have `irori-amazon-api`'s `test-support` feature.

**What to test**

- Parsers (store JSON → `GameInfo`, manifests, progress lines, login input
  extraction), settings mapping, Heroic import, and view-state logic (filters,
  sorting, model transitions).
- A bug fix comes with a test that fails without it.

**UI tests**

- `#[gpui_kit::test]` with `TestAppContext`. See `crates/feature-library/src/ui_tests.rs`
  for the harness: isolated `Paths`, `Backend::start` over an in-memory `Store`,
  `run_until_parked`, then open and draw a window.
- They need a display on Linux; CI uses `xvfb-run -a`.

**Never enable `gpui-kit`'s `test-support` feature.** It pulls the GPUI Fast crates
into the lockfile, where they conflict with `gpui-pre`. Crates use
`gpui = { package = "gpui-pre", version = "=0.3.9", features = ["test-support"] }` as a
dev-dependency instead.

**Ignored tests** (`#[ignore]`) need explicit setup:

- `irori-runtime`'s release test needs `IRORI_RELEASE_TEST_URL` pointing at the local
  Postman mock in `postman/`.
- The macOS runtime smoke test needs `IRORI_HOME` and `IRORI_RUNTIME_SMOKE=1`, and
  downloads a real runtime.
- `irori-secrets`' keyring test needs `IRORI_KEYRING_SMOKE=1` and writes a real key, so
  on Linux run it in `dbus-run-session` with a throwaway `gnome-keyring-daemon`.
- `irori-runtime`'s sandbox tests start real bubblewrap sandboxes with
  `IRORI_SANDBOX_SMOKE=1`; the umu one also needs `IRORI_SANDBOX_PROTON` naming a
  Proton folder (`docs/PLATFORMS.md`, "Sandboxed games").

Run ignored tests only when you're changing that code.

**Packaging** changes need `packaging/test_package.py`. PR-rule changes need
`.github/scripts/test_pr_rules.py`.

## 9. Platforms and packaging

`docs/PLATFORMS.md` is the reference for paths, desktop integration, translucency,
building and the release checklist.

**Building**

- Build on the target OS; there is no cross-compiling.
- Linux needs the GPUI libraries (Vulkan, Wayland, X11/xcb, xkbcommon, fontconfig,
  freetype) and, for the sign-in window, GTK 3 and WebKitGTK 4.1. The exact apt list
  is in `.github/workflows/ci.yml`.
- macOS needs the Xcode Command Line Tools.
- Windows needs the MSVC toolchain, the Visual Studio C++ Build Tools and the Windows
  SDK, plus the WebView2 Runtime for sign-in.

**Packaging** (`packaging/package.py`, Python 3.11+, standard library only)

- Requires both `irori` and `irori-sign-in`, so a package never silently loses the
  sign-in window.
- Packages are x86-64 or arm64, read from the binaries; x86-64 names have no
  architecture suffix (they predate arm64 packages), arm64 ones add `-arm64`.
- Linux: a tarball with the desktop entry and icon. `--appimage` adds an
  AppImage for the binaries' architecture (appimagetool, zsync update
  information) that bundles no libraries and refuses an `irori` linking
  WebKitGTK.
- macOS: `Irori.app` with the `irori://` scheme.
  - Rejects any dylib outside the system paths.
  - Signs the helper before the bundle (`IRORI_SIGN_IDENTITY`, ad-hoc by default).
  - Notarization is a separate, manual step.
- Windows: a zip with `install.ps1`.
  - Checks that every binary, the redistributable included, has one
    architecture (x64 or arm64).
  - Bundles `vcruntime140.dll` and, for x64, `vcruntime140_1.dll` (arm64 programs
    never import it).
  - The installer installs per user under `%LOCALAPPDATA%\Programs\Irori` and
    registers the URL handler and a Start-menu shortcut.
  - `--portable` adds a zip without `install.ps1` and with the `portable` marker.

**Desktop behaviour to keep working**

- One instance per config root, with argument forwarding.
- `irori://` links.
- The tray.
- Sleep inhibition while playing.
- Shortcuts: `.desktop` files, app bundles or `.lnk` files.
- Translucency per platform.

## 10. User data, secrets and safety

**Secrets**

- Never print, log, commit or put in fixtures any token, password, cookie, session or
  account id.
- Redact real shapes before using them as fixtures. The login code paths handle
  secrets: keep them out of `tracing` output and error messages.

**User data**

- Work with `IRORI_HOME=<scratch dir>` when experimenting, so the user's real
  library, settings and store sessions stay untouched.
- Heroic data (`~/.config/heroic` and the Flatpak path) is read-only reference
  material. Irori's import copies from it and never writes there.
- Store accounts are real accounts. Don't sign in, install, uninstall or sync cloud
  saves against real accounts unless the person you're working for asked you to.

**Deleting files**

- Recursive deletes go through `fs_guard::ensure_deletable`.
- Uninstall and "delete prefix" are confirmed in the UI.
- Never widen what a delete can reach.

**Compatibility**

- Settings, state files and Heroic-compatible keys are user data. A change that
  breaks reading existing files needs a migration and a breaking-change marker on the
  PR (see below).

## 11. Branches, commits and pull requests

These rules are enforced by the **PR rules** check (`.github/workflows/pr-rules.yml`,
logic in `.github/scripts/pr_rules.py`). It fails a pull request that breaks them.

### Types

| Type | Use for | Branch | Title |
| --- | --- | --- | --- |
| `feat` | a new user-facing capability | `feat/cli-login` | `feat(manage): sign in from the command line` |
| `fix` | a bug fix | `fix/epic-dlc-playtime` | `fix(stats): count store playtime for games, not their DLC` |
| `perf` | faster or leaner, no behaviour change | `perf/library-filter` | `perf(library): filter once per revision` |
| `refactor` | restructuring, no behaviour change | `refactor/split-install-sheet` | `refactor(game): split the install sheet` |
| `docs` | documentation only | `docs/agents-md` | `docs: add AGENTS.md` |
| `test` | tests only | `test/gog-manifests` | `test(gog): cover v1 manifests` |
| `i18n` | translations and catalogs only | `i18n/polish-settings` | `i18n(settings): fix Polish plural forms` |
| `build` | Cargo manifests, dependencies, `packaging/` | `build/wry-0.54` | `build: update wry to 0.54` |
| `ci` | `.github/` workflows and scripts | `ci/pr-rules` | `ci: enforce pull request naming` |
| `chore` | maintenance that fits nothing else | `chore/cleanup-assets` | `chore: remove unused theme files` |
| `revert` | reverting an earlier change | `revert/sidebar-blur` | `revert: "feat(ui): blur the sidebar"` |

### Branch names

- `<type>/<short-kebab-description>`: lower case, digits, `-` and `.` only, with no
  further slashes. For example `fix/epic-login`, `feat/big-picture-search`,
  `build/wry-0.54`.
- One branch per topic. Branch from an up-to-date `main`.
- Never commit to `main` directly.
- Dependabot and Renovate branches are exempt.
- Coding agents: branch names such as `claude/…` or `codex/…` fail the check. Create
  or rename the branch with a type prefix before opening the PR.

### Pull request titles

- `<type>(<scope>): <summary>` or `<type>: <summary>`, with **the same type as the
  branch**.
- `scope` is optional, lower-case kebab: the area or crate without the `irori-` prefix
  (`epic`, `gog`, `store-epic`, `sign-in`, `library`, `game`, `manage`, `settings`,
  `bigpicture`, `ui`, `shell`, `runtime`, `depot`, `services`, `packaging`, `i18n`).
- The summary is imperative and lower case ("keep", not "Keeps" or "Kept"). An
  acronym in capitals may lead ("fix(gog): GOG Galaxy …" is fine). No trailing period.
- At most 72 characters for the whole title.
- A breaking change puts `!` before the colon (`feat(settings)!: move overrides to …`)
  and describes the break and the migration in the PR body. Breaking means changing
  settings keys, on-disk state, the CLI, the `irori-sign-in` command line or the
  `irori://` links.

### Base branch

- Pull requests target `main`.
- For stacked work, a PR may target another convention-named branch; retarget it to
  `main` when its parent merges.
- Never target a branch that has already merged: commits merged into it never reach
  `main`. That is how the translucency change in PR #2 missed `main`.

### Commits

- Same format as titles: `fix(epic): treat Mac ports released separately as native`.
- Add a body that explains *why*, wrapped at ~72 columns. Write the subject line in
  the imperative.
- Keep commits focused: unrelated changes go in separate commits, ideally separate
  PRs. Don't mix formatting-only churn with logic changes.
- Never commit secrets, `target/`, `dist/`, editor files or local paths.

### What a pull request contains

- One topic. Large features land in reviewable steps, stacked if necessary.
- The template (`.github/pull_request_template.md`), filled in:
  - What changed and why.
  - How it was checked, with the commands and the platforms tried. Say which
    platforms you didn't try.
  - Translations, docs, secrets and breaking-change checklist.
  - Screenshots for UI changes, in light and dark.
- Docs updated when behaviour changes: README, `docs/PLATFORMS.md`, this file.
- Green CI: the platform matrix, fmt, clippy, tests and packaging, plus PR rules.
- Keep the PR current with `main` by merging `main` in, or by rebasing your own
  unshared branch. Don't force-push a branch someone else is reviewing without saying
  so.

### Merging

- **Squash and merge**, with the PR title as the commit subject. That keeps `main`
  readable as one Conventional Commit per change.
- Delete the branch after merging.

## 12. Definition of done

A change is done when all of these hold:

- [ ] `cargo fmt --all --check` passes.
- [ ] `cargo clippy --locked --workspace --all-targets -- -D warnings` passes, at
      minimum for every crate you touched and the crates that depend on them.
- [ ] `cargo test --locked` passes for those crates; the whole workspace before
      opening a PR.
- [ ] New behaviour has tests; a bug fix has a test that fails without the fix.
- [ ] User-facing text is translated into all 19 languages.
- [ ] It builds on Linux, macOS and Windows: no `#[cfg]` that leaves a platform
      without code.
- [ ] You ran Irori and exercised the change (UI, login, install, launch, whatever it
      touches), or the PR says plainly that you couldn't.
- [ ] Docs match the new behaviour.
- [ ] No secrets, personal data or absolute local paths in the diff.
- [ ] Branch, commits and PR title follow [section 11](#11-branches-commits-and-pull-requests).

## 13. Pitfalls

These are things that have bitten this codebase before:

- **`cargo run -p irori` builds only `irori`.** On Linux, sign-in falls back to the
  browser until you `cargo build -p irori-sign-in` for the same profile. macOS and
  Windows run the window from Irori itself.
- **Package names are not directory names.** `crates/store-epic` is `irori-epic`, and
  `crates/epic` is `irori-epic-api`.
- **Don't wait for `LibraryChanged` in UI code.** Wait for `LibraryUpdated`; the
  snapshot isn't rebuilt yet when `LibraryChanged` fires.
- **DLC are in `store.games()`.** Anything that counts or lists games must filter
  `is_dlc`. Epic even reports playtime per DLC, repeating the base game's time.
- **An Epic game can have several releases.** A Mac port can arrive as a release of
  its own; read every `releaseInfo` entry, not just the first.
- **Settings keys are Heroic's**, typos included. Renaming one silently drops the
  user's setting.
- **`Paths::init` only works once per process.** In tests, initialize once (for
  example in a `Once`) and give each test its own subdirectory.
- **Never enable `gpui-kit/test-support`** (see [8](#8-tests)).
- **Port 9977 is open to every local program.** comet panics abort all of Irori, and a
  client may be anything: gate new handlers on sign-in, turn bad input into an error,
  and never let the account's own token out.
- **Never add a web engine to Irori's window.** `crates/webview` is vendored but
  excluded and unused. Sign-in runs in its own process.
- **WebKitGTK must stay optional on Linux.** Linking it into `irori` would stop Irori
  from starting on systems without it.
- **macOS packages must not link Homebrew libraries.** `package.py` rejects them. Link
  statically (as with `xz2`'s `static` feature) or use system frameworks.
- **The Windows app is a GUI-subsystem executable** (`windows_subsystem = "windows"`).
  It has no console, so anything a parent must read goes through pipes the parent
  provides, as the sign-in mode does.
- **`WindowHandle::update` fails inside that window's own update**, and `let _ =` hides
  it. Dialog callbacks run inside it: reach code that updates the window with `cx.defer`.
- **The UI thread must never wait on I/O.** Even "quick" file reads go through
  `irori_ui::run` or `backend.spawn_blocking`.
- **Updating gpui-pre means updating `crates/gpui-pre-windows` too.** Its version
  must match gpui-pre's, or the patch stops applying and Windows quietly gets the
  unpatched backend. Copy the new crate in and apply `PATCHES.md` again.
- **Checking Windows code from a Mac** works for `cargo check` with the rustup
  toolchain first on `PATH` (Homebrew's Rust has no Windows standard library) and
  Homebrew LLVM's `llvm-rc` for gpui-pre's resources:
  `PATH="$(dirname "$(rustup which rustc)"):/opt/homebrew/opt/llvm/bin:$PATH" cargo check --target x86_64-pc-windows-msvc -p <package>`.

## 14. Notes for coding agents

- Read the files you're changing and their callers before editing. Read the relevant
  `docs/parity/*.md` before changing store, launch or download behaviour.
- When several agents build in one checkout, give each its own target directory
  (`CARGO_TARGET_DIR=target/agents/<name>`) to avoid lock contention. It's
  unnecessary on your own.
- A full workspace build is heavy. While iterating, use `cargo check -p <package>` and
  `cargo test -p <package>`. Run the full clippy and test commands before you push.
- Don't change public contracts (`Store`, `Backend`, events, settings keys,
  `irori-sign-in` arguments, CLI) unless the task needs it. If it does, update every
  implementation and caller in the same change and call it out in the PR.
- Don't touch unrelated files, reformat code you didn't change, or "tidy" other
  crates in passing.
- You can't see the UI. Describe what you verified and how; never claim you checked
  a screen you didn't render.
- Report honestly in the PR: what is done, what isn't, what you couldn't test, and on
  which platforms.
