# Architecture

Technical reference for Irori's workspace layout, crate boundaries, core contracts, backend services, stores, emulators, third-party launchers, Steam, mods, graphics mods, Xbox support, sustainability policy, Rust/testing conventions, and known pitfalls.

## 1. Repository map and layering

Directory names and Cargo package names differ; always pass the package name to `-p`.

| Directory | Package | Role |
| --- | --- | --- |
| `crates/core` | `irori-core` | Domain types, settings schema, `Store` trait, events, paths, process/HTTP/JSON utilities, `fs_guard` |
| `crates/secrets` | `irori-secrets` | AES-256-GCM encrypted login files keyed in the system keyring |
| `crates/runtime` | `irori-runtime` | Wine/Proton/CrossOver/GPTK discovery and downloads, prefixes, DXVK/VKD3D, umu, launch environment, bubblewrap sandbox (`sandbox.rs`) |
| `crates/engines` | `irori-engines` | Emulators ("engines"): manifests, discovery, managed versions, RetroArch cores, BIOS index, launch commands |
| `crates/scripts` | `irori-scripts` | Lutris install scripts: typed parser, validation, variables, consent plan, confined executor with rollback, `ScriptHost` trait, lutris.net client, Wine build matching |
| `crates/depot` | `irori-depot` | Parallel chunked download engine with resume, verification, delta updates, and `test-server` feature |
| `crates/epic` | `irori-epic-api` | Epic protocol (port of legendary): auth, library, manifests, cloud saves, launch parameters |
| `crates/gog` | `irori-gog-api` | GOG protocol (port of gogdl): auth, content system, depots, Linux installers, saves |
| `crates/amazon` | `irori-amazon-api` | Amazon Games protocol (port of nile): device sign-in, library, signed manifests |
| `crates/itch` | `irori-itch-api` | itch.io public API: API-key login, owned keys, uploads, download links |
| `crates/steam` | `irori-steam-api` | Steam Web API: login (QR, password, Steam Guard, tokens), owned and Family games, store art/pages, achievements, friends |
| `crates/retroachievements` | `irori-retroachievements-api` | RetroAchievements Web API, emulator token login, rcheevos-compatible ROM hashes |
| `crates/xbox` | `irori-xbox-api` | Xbox network: Microsoft OAuth PKCE, Xbox Live user and XSTS tokens, title history, playtime, achievements, friends |
| `crates/msstore` | `irori-msstore-api` | Microsoft Store and Xbox Live protocol (port of Xodus): device registration, RST2 with XML-DSig, CLEP/SPLicense, SISU/XSTS, display catalog, package plans, content licenses, offline leases, Game Pass, offline PC |
| `crates/msixvc` | `irori-msixvc` | MSIXVC packages (port of Xodus): XVD and NTFS volume, AES-XTS, hash tree, streaming downloads with resume checkpoints |
| `crates/gog-galaxy` | `irori-gog-galaxy` | Hosts comet (Galaxy achievements, stats, leaderboards) on `127.0.0.1:9977` for games Irori started |
| `crates/comet` | `comet` | Vendored fork of comet (Apache-2.0); `README.md` lists changes from upstream |
| `crates/store-epic` | `irori-epic` | Epic `Store` (`Runner::Legendary`) |
| `crates/store-gog` | `irori-gog` | GOG `Store` (`Runner::Gog`) |
| `crates/store-amazon` | `irori-amazon` | Amazon `Store` (`Runner::Nile`) |
| `crates/store-zoom` | `irori-zoom` | ZOOM Platform `Store`, opt-in through `zoomPlatform` |
| `crates/store-itch` | `irori-itch` | itch.io `Store` (`Runner::Itch`), opt-in through `iroriItchIo`; handles archives, macOS disk images, silent Windows installers (Inno Setup, NSIS, MSI), and itch app receipts |
| `crates/store-xbox` | `irori-xbox` | Xbox `Store` (`Runner::Xbox`, Linux), opt-in through `iroriXbox`; Microsoft Store and PC Game Pass installs, decrypted launch view, in-game account service |
| `crates/store-steam` | `irori-steam` | Steam `Store` (`Runner::Steam`): local library, optional account, client library reader, game watch, in-app login |
| `crates/store-sideload` | `irori-sideload` | User-added apps (native, Windows, browser) |
| `crates/launchers` | `irori-launchers-data` | Local data readers for Battle.net (`product.db`), Ubisoft cache/registry, EA manifests, Rockstar registry, and Windows Xbox packages (`.GamingRoot`, `MicrosoftGame.config`, `AppxManifest.xml`, Gaming Services) |
| `crates/store-launchers` | `irori-launchers` | Battle.net, EA app, Ubisoft Connect, Rockstar, and Windows Xbox app (`Runner::Launchers`) plus optional Xbox account, opt-in through `iroriThirdPartyLaunchers` |
| `crates/store-emulation` | `irori-emulation` | ROM library (`Runner::Emulation`): scanned folders played via emulators |
| `crates/mods` | `irori-mods` | Per-game mod manager: storage, load order, overlay or link deployment/undeployment, conflicts, archives, FOMOD, Thunderstore, Nexus Mods |
| `crates/reshade` | `irori-reshade` | Per-game graphics mods: PE header reader, API detection, ReShade `Compatibility.ini`, ReShade/RenoDX/OptiScaler downloads and journaled installs |
| `crates/extras` | `irori-extras` | Shortcuts, PCGamingWiki/HLTB/ProtonDB/anticheat metadata, known fixes, SteamGridDB, IGDB, VNDB, LaunchBox, deals, Heroic/Lutris/Bottles/Faugus/Playnite import planners |
| `crates/services` | `irori-services` | `Backend` facade: library, settings, user data, download queue, launching, playtime, sync, backups, Sunshine, storage, audio, capture, Lutris script hosting |
| `crates/ui` | `irori-ui` | Shared UI bridge, reactive models, navigation, art cache, themes, translucency, shared components, i18n |
| `crates/feature-library` | `irori-library` | Library grid/list/Focus, search, filters, categories, stats, Year in review, sideload dialogs, Lutris script installer |
| `crates/feature-game` | `irori-game` | Game page and sheets (install, uninstall, logs, cloud saves, import, modify, mods, troubleshoot) |
| `crates/feature-manage` | `irori-manage` | Downloads, Wine manager, Emulators, accounts and login, stores, onboarding, friends |
| `crates/feature-settings` | `irori-settings` | App settings, game defaults, per-game overrides, ReShade/RenoDX/OptiScaler panels, logs, system info |
| `crates/feature-bigpicture` | `irori-bigpicture` | Big Picture mode, controller input, and in-game overlay |
| `crates/irori` | `irori` | Application shell and binary: window, sidebar, routing, command palette, quick launch, tray, single instance, CLI |
| `crates/sign-in` | `irori-sign-in` | Standalone store sign-in and store browser window (Wry/Tao) |
| `crates/gpui-pre-windows` | `gpui-pre-windows` | Patched GPUI Windows backend via `[patch.crates-io]`, excluded from workspace (Apache-2.0, see `PATCHES.md`) |
| `crates/gilrs-core` | `gilrs-core` | Patched gilrs Windows.Gaming.Input layer via `[patch.crates-io]`, excluded from workspace (MIT, see `PATCHES.md`) |

Other top-level paths: `assets/` (bundled theme JSONs, Manrope font, `assets/engines/` manifests, `assets/brand/stores/` Simple Icons), `packaging/` (`package.py`, tests, installers, icons, entitlements, channel scripts), `docs/` (technical docs and `design/`), `website/` (project site), and `.github/` (CI, PR rules, templates).

### Strict crate layering

Never add a dependency pointing upward:

```text
irori-secrets                     (zero internal deps; usable by protocol crates)
comet, irori-launchers-data       (zero internal deps; irori-gog-galaxy wraps comet)
irori-core                        (zero internal deps)
  |- irori-depot, irori-runtime, irori-extras
  |- irori-engines   (core, runtime, depot)
  |- irori-scripts   (core, runtime, depot; no store or UI knowledge)
  |- irori-mods      (core, depot; no store or UI knowledge)
  |- irori-reshade   (core, runtime; no store or UI knowledge)
  |- protocol crates: irori-epic-api, irori-gog-api, irori-amazon-api, irori-itch-api,
  |                   irori-steam-api, irori-retroachievements-api, irori-xbox-api,
  |                   irori-msstore-api, irori-msixvc
  |                   (never depend directly on irori-core; epic/gog/amazon use depot, gog uses secrets)
  |- store crates:    irori-epic, irori-gog, irori-amazon, irori-zoom, irori-itch,
  |                   irori-steam, irori-sideload, irori-xbox, irori-emulation,
  |                   irori-launchers
  `- irori-services   (core, secrets, depot, runtime, extras, engines, scripts, mods, reshade,
                       irori-steam-api for public achievement schemas; drives stores via DynStore)
irori-ui          (services, core, runtime, engines)
feature crates    (ui, services, core; concrete stores only where a screen needs store-specific API)
irori (binary)    (everything; the only crate naming all stores)
irori-sign-in     (standalone; linked by irori on macOS/Windows only)
```

- Protocol crates have zero knowledge of Irori's app state: no direct `irori-core` dependency, no `Paths`, no settings, no events.
- Store crates adapt protocols into `Store`. Services drive stores strictly through `DynStore = Arc<dyn Store>`.
- Feature crates stay decoupled from each other (the sole direct link is `irori-settings` reading `irori_bigpicture::gamepad` and `irori_bigpicture::overlay::support` for controller mapping and overlay support tables); the shell routes events (`LibraryEvent`, `GamePageEvent`) and installs cross-feature hooks (`irori_ui::hooks`, `irori_game::install_hooks`).

## 2. Store protocol boundaries and sustainability policy

Irori implements store protocols directly in Rust (ports of legendary, gogdl, nile, and Xodus, plus public Steam/itch.io/Xbox/RetroAchievements APIs and comet as a library) and never shells out to Heroic or those CLI tools.

When upstream store endpoints, anti-tamper rules, or third-party launchers change or break:

- Degrade cleanly with typed, actionable user-facing errors (`SessionExpired`, `SaveConflict`, `GameFilesMissing`, `RateLimited`, `ClientLibraryError::NotSteam`, `LauncherMissing`, `XboxUnavailable`) instead of panicking or silently corrupting local state.
- Keep cached libraries, installed games, local playtime, saves, and offline launches working whenever a store API is unreachable or retired.
- Never decrypt private proprietary client databases or spoof private first-party client identities where no public or readable interface exists (for example, EA app's encrypted installation state and private GraphQL service, Rockstar's encrypted profiles, or Discord's restricted friends scope). Explain the limitation clearly in the UI.
- Never bundle proprietary runtimes, emulators, BIOS files, ROMs, `ffmpeg`, `rclone`, `ludusavi`, `gpu-screen-recorder`, or ReShade/RenoDX/OptiScaler binaries in Irori releases. Fetch only from official upstream sources on explicit user action with digest verification.

## 3. Core contracts (`irori-core`)

Read `crates/core/src/{store,events,paths,settings,process,launch}.rs` before modifying backend layers.

### `Store` trait (`store.rs`)

- Async trait (`#[async_trait]`, `Send + Sync + 'static`).
- Accounts: `init` (disk only, zero network, gates first render), `user`, `login_flow`, `login(input)`, `logout`, plus multi-account defaults (`accounts`, `can_add_account`, `account_login_flow`/`account_login`/`account_logout`/`refresh_account`, `game_account`).
- Library: `refresh(full)`, `games()` (returns `Vec<Arc<GameInfo>>` including DLC marked `is_dlc`), `game`, `extra_info`, `install_info`, `updatable_games`.
- Operations: `install`, `update`, `repair` (accept `OpContext`, return `Outcome`), `uninstall(RemoveArgs)`, `import`, `set_install_path`, `launch` returning `LaunchPlan`, and optional hooks (`achievements`, `forget_achievements`, `achievement_signal`, `remote_playtime`, `after_exit`, `extras`, `forget_install`, `set_version_pinned`, `client_action`).
- `LoginFlow`: `Browser { url, capture, instructions }` (`capture` regex group 1 extracts the token for both sign-in window and clipboard fallback), `InApp` (Steam), `MicrosoftAccount` (Xbox CloudExperienceHost mode), or `None`.
- `OpContext` bundles a progress sink, `CancellationToken`, step reporter, optional log file, and effective settings (`OpContext::detached()` for no-op calls).
- Typed cross-boundary errors propagated via `anyhow`: `SessionExpired` (400/401 from token service, never transient network errors) and `SaveConflict` (both local and cloud saves modified; prompts `irori_ui::save_conflict`).
- `Runner` serialized IDs preserve Heroic naming (`legendary`, `gog`, `nile`, plus `zoom`, `itch`, `xbox`, `steam`, `sideload`, `emulation`, `launchers`; `from_id` also accepts `epic` and `amazon`). `Runner::manages_installs()` is false for Steam and Launchers; `Runner::has_optional_account()` marks Steam's optional login.
- `LaunchPlan::watch` (`Arc<dyn GameWatch>`) tracks games owned by external clients (Steam, third-party launchers, Windows Xbox apps).
- `GameId { runner, app_name: Arc<str> }` produces `key() == "runner:app"` used across all user-data maps.

### Events, paths, persistence, and secrets

- `EventBus` (`events.rs`): Tokio broadcast channel (capacity 1024). Events carry identities, never full payloads. Stores emit `LibraryChanged(Runner)` and `AccountChanged(Runner)`; UI listens for `LibraryUpdated(rev)` after snapshot rebuilds. Startup notices must use `EventBus::startup_notice` because the UI subscribes after backend start.
- `Paths` (`paths.rs`): global singleton initialized once per process (`Paths::init(Paths::under(tmp))` in tests). Relocated by `IRORI_HOME` or a sibling `portable` file next to `irori.exe` on Windows (`paths::is_portable` skips URL handler and Start menu registration). Store configs live under `store_config_dir(runner)`, caches under `store_cache_dir(runner)`, state (`library.json`, `downloads.json`, `sessions.jsonl`, `running.json`) under `state_dir()`, per-game settings at `config/games/<runner>/<app>.json`, and logs/crash dumps (`crash-<time>.txt`, parsed and listed via `crates/core/src/crash.rs`) under `logs_dir()`.
- Atomic JSON persistence (`json_store.rs`): always write via `json_store::write`/`write_bytes` (`.tmp` sibling + atomic rename). `read` returns `Ok(None)` when missing; `read_or_default` logs and falls back on corruption. `home_rename.rs` remaps renamed macOS `/Users/<old>` paths only when enabled in `crates/irori/src/main.rs`.
- Secrets (`irori-secrets`): all logins, tokens, cookies, API keys, and passwords use `irori_secrets::read_json`/`load`/`write_json`, never `json_store`. Files are AES-256-GCM sealed with a single random key stored in the OS keyring (keeping credentials out of Windows' ~2.5 KB keyring entry limit), falling back to mode `0600` plain JSON when no keyring exists and upgrading automatically once available. Only `crates/irori/src/main.rs` enables `use_os_keyring`, keeping tests isolated from real keyrings. Never read secrets on the UI thread; locked vaults raise `shell.notice.logins_locked`.

### Settings, processes, HTTP, host helpers, and deletion safety

- Settings (`crates/core/src/settings.rs` and `crates/core/src/settings/`): camelCase keys match Heroic (including `enviromentOptions`); never rename keys, and mark new fields `#[serde(default)]`. `AppSettings` flattens global `GameSettings`; per-game `GameSettingsOverrides` is a sparse JSON map. `merged_lists.rs` merges `enviromentOptions` (by `key`), `wrapperOptions` (by `exe`), and `iroriLaunchPrograms` (by `path`) over defaults using `iroriRemoved: true` tombstones via `GameSettingsOverrides::override_value` (migrated once by `crates/services/src/settings_migration.rs`). Launch presets (`launch_extras.rs`) apply only `PRESET_KEYS`.
- Processes (`process.rs`): spawn games and tools through `CommandSpec` + `process::run`/`spawn`, handling line callbacks (`\n`/`\r`), capped logs, cancellation, process-group termination (SIGTERM then SIGKILL on Unix; `taskkill /T /F` on Windows), and Windows `CREATE_NO_WINDOW` (`run` hides consoles, `spawn` preserves them for console games, standalone helpers call `process::hide_console`). Split arguments with `split_program_args(windows, ..)` for game executables (preserving Windows quoting under Wine) and `split_args` for host wrappers.
- HTTP (`http.rs`, `net.rs`): use `irori_core::http::client()` (shared rustls client, `Irori/<version>` UA, 10 s connect / 60 s total timeout) or `download_client()` (no total timeout) inside Tokio. Both obey `iroriNetworkMode` and `iroriNetworkTimeout` via `net::OfflineAwareResolver` (refusing non-loopback hosts with `net::WorkingOffline`). Custom clients must wire `.dns_resolver(net::resolver())` and `net::request_timeout()`; protocol crates gate calls through `Backend::working_offline` / `net::working_offline()`.
- Flatpak host bridge (`host.rs`): inside Irori's Flatpak, discover and execute host tools (Steam, Ludusavi, rclone, compositor tools) via `host::find_program`, `has_flatpak_app`, `host::command`, `tied_command`, `host::processes`, and `host::signal`.
- Recursive deletion safety (`fs_guard.rs`): call `fs_guard::ensure_deletable(dir, app)` before every recursive delete. It protects system paths, `$HOME`, direct children of `$HOME`, and other launchers' directories (`fs_guard::OtherLaunchers`, `state/other-launchers.json`).
- Untrusted data: sanitize path segments with `paths::sanitize` (rejecting `.` and `..`), validate archive entries and symlink targets with `archive_guard`, validate opened external URLs with `irori_ui::web_url`, mask secrets in logs (`process::is_secret_flag`, `SECRET_PARAM`, `is_secret_var`), and strip sensitive URLs from `reqwest` errors via `without_url`.

## 4. Backend (`irori-services`)

`Backend` (`crates/services/src/lib.rs`) is the single entry point for both GUI and CLI.

- Startup modes: `Backend::start` (app, periodic tasks, connectivity probe), `start_headless` (CLI, no periodic tasks), and `start_for_tests` (in-memory state and workers, zero network probes or startup refreshes).
- Library snapshot and copies (`copies.rs`): merges non-DLC games with user overrides into a sorted `Arc<LibrarySnapshot>` (natural sort with Roman numerals II to XX via `irori_core::sort_key`, debounced 30 ms). `iroriUi.groupCopies` combines cross-store copies by folded title while keeping per-copy `GameId` for launches/installs and sharing favorite, hidden, play status, and rating across copies (`UserDataService::set_copy_groups`, `UserData::share_between`). Possible copies linked to the same Steam app surface on the game page.
- Search and categories (`search.rs`, `smart_categories/`): lazy `SearchIndex` with folded text (`irori_core::text`), ranked exact/prefix/word/fuzzy matching, plus `mode:`, `tag:`, `category:`, and `smart:` terms. Manual categories (`UserData::categories`), smart categories (`UserData::smart_categories` with three-valued evaluation and forward-compatible `extra` retention), and saved views (`ViewFilter::unknown` retention) feed `CategoryIndex`. Categories import/export via `category_file.rs`.
- Hidden games PIN (`hidden_lock.rs`): while `LibrarySnapshot::hidden_locked` is true, every screen, search, Big Picture shelf, palette, stats view (`visible_sessions`), CLI, and Discord RPC (`Backend::concealed`) must exclude hidden games.
- Downloads and updates (`downloads`, `updates.rs`, `screen.rs`, `download_power.rs`): single-worker persisted queue (`state/downloads.json`) with per-disk space accounting (`download_space.rs`), speed-based ETA, display power-off (`kscreen-doctor`/GNOME/DPMS/`pmset`/`SC_MONITORPOWER`, never sudo), and 60 s idle power-down countdown (`download_power.rs`, never interrupting running games). Component updates check Wine, DXVK, VKD3D-Proton, DXVK-NVAPI, umu, anticheat runtimes, emulators, and RetroArch cores using `releases.atom` and GitHub rate-limit budgets (`state/wine-release-checks.json`), keeping replaced Wine builds in `<base>/.rollback/`.
- Metadata, enrichment, and deals: `Backend::extra_info` and `Backend::wiki_info` layer store pages, Steam links (`steam_link.rs`, `<cache>/steam-links.json`), Steam tags (`steam_tags.rs`, `iroriSteamTagLimit` 5 to 20), Steam reception (`steam_reception.rs`), PCGamingWiki bare facts (`pc_facts`), HowLongToBeat (`hltb`), opt-in IGDB (`igdb.rs`), opt-in VNDB (`vndb.rs`, filtered by content rating and `https://*.vndb.org`), age ratings (`store_facts.rs`), controller support, game modes (`game_mode::lenient`), user metadata overrides (`GameOverrides`), and global field rules (`field_rules.rs`). Store deals and giveaways live in `crates/extras/src/discounts/` (`free.rs`, `ggdeals.rs`, `regions.rs`, `search.rs`) and `crates/services/src/ggdeals.rs`.
- Achievements (`achievements.rs`, `file_achievements/`): always read through `Backend::achievements` so totals and monotonic unlock history persist in `state/achievements.json`. Live session watching (`watch_achievements_while_playing`) follows GOG comet events (`Event::AchievementSignal`) or Steam `UserGameStats_*.bin` via `notify`, falling back to periodic polling only when no signal exists. Steam Achievement Sets carry `group`, `achievable`, and `completed_at`. Opt-in `iroriFileAchievements` reads DRM-free Steam-emulator saves (Goldberg, gbe_fork, CODEX, RUNE) using Steam's public schema.
- Saves, snapshots, backups, and sync:
  - Ludusavi (`ludusavi.rs`, `ludusavi/restore.rs`, `ludusavi/undo.rs`): external `--api` CLI (never bundled), per-prefix redirects via temporary config copies without modifying user Ludusavi config, undo snapshots in `<data>/save-restore-undo/<game>/`, optional in-session interval backups (`iroriLudusaviInterval`).
  - Save snapshots (`save_snapshots.rs`, `iroriSaveSnapshots` default 5, 1 GiB cap): automatic pre-sync backups in `<data>/save-snapshots/` with SHA-256 deduplication.
  - Config/library backups (`backup.rs`, `backup_paths.rs`, `backup_restore.rs`): JSON backups omitting store credentials and `iroriBackups`/`iroriSync`/`iroriSunshine`, with cross-machine path remapping and install reattachment.
  - Multi-device sync (`sync/`): folder, WebDAV, or user's `rclone` remote writing `Irori sync/<device id>.json` with hybrid-clock last-writer-wins registers (`sync/merge.rs`, `playtime_guard`, `iroriSync.playtimeOnly`, `playtime_adjust`) and optional cloud copies (`sync/copies.rs`). New synced user data requires a register and tests in `sync/merge_tests.rs`.
- Integrations:
  - Sunshine and Apollo (`sunshine/`): loopback or TOFU-pinned remote HTTPS (`PinVerifier`) syncing `irori --launch <runner>/<app> --wait` entries tagged with `irori-id` without clobbering user fields, plus client resolution/FPS/HDR forwarding (`SUNSHINE_CLIENT_*`).
  - Steam shortcuts (`irori-extras::shortcuts::steam`): writes `shortcuts.vdf` only while Steam is closed (with `.irori-backup` and read-back validation), queueing requests in `state/steam-shortcut-queue.json` while Steam runs, syncing `grid/` art, and forwarding Steam's Linux overlay (`steam_overlay.rs`) only when started from a Steam shortcut.
  - Discord Rich Presence (`discord.rs`, opt-in `discordRPC`): single worker over local IPC socket, active only while a game runs.
  - Screenshots, replay buffer, storage, prefixes, displays, Windows GPU controls, power, and troubleshooting: read-only screenshot/clip indexing (`screenshots.rs`), Linux `gpu-screen-recorder` replay buffer (`replay/`, XDG GlobalShortcuts portal / X11 `XGrabKey` / compositor bindings; never read `/dev/input` keyboards), storage cleanup (`storage.rs`, using OS trash for potentially save-bearing folders), prefix sharing guard (`prefixes.rs`), display EDID/HDR/VRR detection (`displays/`), Windows per-app GPU preference (`gpu_preference.rs`) and elevated GPU hiding (`gpu_hide/`, `--gpu-helper`), `PowerSystem` trait (`power/`, stopping games and awaiting `OpenSession` release before power actions), and launch troubleshooter (`troubleshoot/`, PE import reader, wineserver checks, `vm.max_map_count`, Yama `ptrace_scope`, default `WINEDEBUG=-all,err+module,err+mscoree`).
- Launch pipeline (`games.rs`): cancellable via `Backend::cancel_launch` (`LaunchCancelled`) up to game spawn; records active PIDs and process start times in `state/running.json` for post-restart `adopt_sessions`. Order:
  1. Busy and install-files checks (`GameFilesMissing`).
  2. Cloud-save sync (`SAVE_SYNC_TIMEOUT` 3 min, `SaveDownloadFailed`, `iroriSavesPathFound`).
  3. `irori_runtime::prepare_launch` (`gather_facts` + pure `compose_env` shared with `preview_launch`, native detection/`iroriNoWine`, AppImage FUSE fallback, bubblewrap `iroriSandbox` refusing on `SandboxUnavailable`, DLL overrides, umu `GAMEID` resolution).
  4. Before-launch script (waited or alongside via `iroriBeforeLaunchScriptAlongside`).
  5. `store.launch` producing `LaunchPlan` (macOS Rosetta `arch -x86_64` / `RosettaMissing`, MetalFX setup, mod deployment `apply_mods`, graphics mod restoration `restore_graphics_mods`).
  6. Pre-game programs (`startBeforeGame`), `process::spawn`, and alongside programs (`UMU_CONTAINER_NSENTER=1` under umu for alongside programs, never pre-game programs).
  7. Sleep inhibition, replay buffer, session tracking (`follow::Handoff` for launcher handoffs via process groups on Unix or descendant PIDs on Windows), post-exit cleanup, after-launch script, save upload, mod restore, and `GameExited`.

## 5. Stores, shell, and sign-in

### Stores (`crates/store-*`, `crates/irori/src/stores.rs`)

- Every store crate exposes `pub struct <Name>Store`, `new(bus, settings) -> Arc<Self>`, `import_from_heroic(&Path) -> Result<()>`, and path/endpoint test constructors (`with_paths`, `with_endpoints`, `with_layout`).
- Multi-account support lives outside individual store crates: the shell wraps Epic, GOG, Amazon, ZOOM, and itch.io in `irori_services::MultiAccountStore` (main slot uses default paths; extra slots use `for_account` with `Paths::account_config_dir`/`account_cache_dir`).
- Registration order in `crates/irori/src/stores.rs`: Epic, GOG, Amazon, ZOOM, itch.io, Xbox (Linux only), Steam, Launchers, Emulation, Sideload. ZOOM and itch.io stay registered so toggling `zoomPlatform` or `iroriItchIo` takes effect live without restarting Irori.
- On Windows, `crates/store-gog/src/galaxy.rs` imports owned GOG Galaxy installs from `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games` read-only once per folder (`galaxy-imports.json`).

### Shell (`crates/irori`)

Keep `main.rs` startup in this exact order:

1. Internal helpers (`--gpu-helper`, `--apply-update`, and macOS/Windows `--sign-in-window`).
2. CLI parsing (`cli.rs`, `headless.rs`: `accounts`, `import-heroic`, `import-lutris`, `import-bottles`, `import-faugus`, `import-playnite`, `refresh`, `list [--installed|--json]`, `info`, `playtime`, `install`, `uninstall`, `sunshine-sync`, `steam-shortcuts`, `displays [--json]`). Unknown bare words exit 2.
3. `instance::acquire` (loopback TCP forwarding authenticated with mode `0600` `instance.json`; `--wait` holds the socket until session exit).
4. Logging, `Backend::start(stores::build)`, GPUI app setup, URL and reopen handlers (`irori://`, `heroic://`, `lutris:`, `nxm://`, `ror2mm://`, Windows executables/URIs on non-Windows).
5. Inside `run`: `gpui_kit::init`, `irori_ui::init`, every feature `init`, hooks, key bindings, theme, tray (`tray.rs` ksni on Linux, `tray_native.rs` on macOS/Windows with Windows dark menu via `menu_theme.rs`, `tray_hide.rs`), main window (`placement.rs`, `title_bar.rs`), and the `ShellCommand` loop.

Key environment variables: `IRORI_HOME` (relocate config/data/cache), `IRORI_LOG` (`tracing` filter, default `info` GUI / `warn` CLI), `IRORI_SIGN_IDENTITY` (macOS signing), `IRORI_WINDOWS_CRT_DIR`, `IRORI_XBOX_CLIENT_ID`, `IRORI_UPDATE_EXPLANATION`, and ignored-test gates (`IRORI_RELEASE_TEST_URL`, `IRORI_RUNTIME_SMOKE`, `IRORI_KEYRING_SMOKE`, `IRORI_SANDBOX_SMOKE`, `IRORI_SANDBOX_PROTON`).

### Store sign-in (`crates/sign-in`, `crates/feature-manage/src/login/`)

- Store sign-in always runs in a separate process (`irori-sign-in`), never inside Irori's main window. It watches URLs and page text for `capture`, writes `{"text": "..."}` to stdout and exits 0 (closed = 2, bad args = 1).
- Resolution order (`login/window.rs::helper`): sibling `irori-sign-in` or `PATH`; on macOS/Windows only, `irori --sign-in-window` (using system WKWebView/WebView2); otherwise default browser + clipboard watcher. Never link WebKitGTK into `irori` on Linux.
- Additional `irori-sign-in` modes: `--microsoft-account` (CloudExperienceHost `InlineLogin.srf` for `LoginFlow::MicrosoftAccount`) and `--store` (standalone store browser with separate cookies under `<data>/store-window/<runner>` and JSON-line `open`/`external` IPC guarded by `irori_ui::web_url`).

## 6. Subsystems: engines, launchers, Steam, mods, graphics mods, and Xbox

### Engines and emulators (`irori-engines`, `irori-emulation`, `irori-scripts`)

- Design details live in `docs/design/engines.md`. `Runner` is a store; an emulator is an engine (`EngineManifest` in `assets/engines/<id>.json`, user merge overrides in `<config>/engines/<id>.json`, custom engines in `custom-<name>.json`); consoles live in `assets/engines/systems.json`. Never pin download URLs in manifests; describe release sources.
- Managed installs live under `<data>/engines/<id>/<version>/` (`<state>/engines.json`); Flatpak and system installs are detected only. Converting engines (`AnyPS5`, `kind: relinker`) link game files into Irori's data folder without writing the game folder.
- ROM library (`irori-emulation`) scans `iroriRomFolders` incrementally with `DETECTION_RULES` cache invalidation, `notHeaders` guards (keeping DOOM `IWAD`/`PWAD` out of Wii `.wad`), disc/folder markers (`PARAM.SFO`, `PS3_DISC.SFB`, `UMD_DATA.BIN`, bounded `iso9660`), and `notify` watches. Removing a ROM hides it and never deletes ROM files.
- BIOS files are never downloaded or linked externally; existing local BIOS files are hashed with path/size/mtime caching and linked before launch (`BiosMissing`).
- LaunchBox box art (`iroriLaunchBox`, opt-in only) streams `Metadata.zip` via `quick-xml`, stores per-system JSONs, and deletes the zip.
- Launcher importers (Lutris `pga.db` read-only/immutable, Bottles, Faugus Launcher, Playnite read-only bounded LiteDB 4 reader) never modify external launchers; optional periodic sync (`crates/irori/src/launcher_sync.rs`, `state/launcher-imports.json`) imports only unseen games.
- RetroAchievements (`irori-emulation::retro`) keeps API keys in `irori_secrets`, matches rcheevos hashes, and exchanges passwords once for emulator tokens (`irori_engines::cheevos`).
- Lutris install scripts (`irori-scripts`): strictly typed command/task/variable enums (unknown items fail the whole script), explicit user consent (`Approvals::accepted`) covering all downloads/writes/overrides, verbatim shell command acknowledgement (`Approvals::shell_acknowledged`, shell metacharacters rejected in substitutions), symlink-resolved write confinement with rollback on failure, native-first refusals (`PreferNative`, `InstallFromStore`), and polite `lutris.net` caching (2 req/s, never bundled).

### Third-party launchers (`irori-launchers-data`, `irori-launchers`)

- Battle.net, EA app, Ubisoft Connect, and Rockstar run in dedicated prefixes (`<prefixes>/<launcher>`) or adopted installs; Irori never touches their accounts. Setup applies Wine-friendly defaults (`wine_config`) only for new non-Windows prefixes.
- EA app and Rockstar expose only installed games because neither publishes a readable owned-library API.
- Sessions follow game processes via `GameWatch` (`pidfd_open`/`poll` on Linux, else 3 s folder scan while running; zero polling when idle).
- Windows Xbox / Microsoft Store (`irori_launchers_data::xbox`): reads `.GamingRoot`, `MicrosoftGame.config`, `AppxManifest.xml`, and Gaming Services registry (`xbox:<package family name>`), launching via `shell:AppsFolder`.
- Optional Xbox account (`irori-xbox-api`, `irori-launchers::xbox_account`): OAuth 2.0 PKCE on `login.live.com` (`XBOX_CLIENT_ID` / `IRORI_XBOX_CLIENT_ID`), refresh token in `irori_secrets`, reading title history, playtime, achievements, and friends (loaded only while the Friends window tab is open).

### Steam (`irori-steam-api`, `irori-steam`)

- Local mode scans `steamapps` across native, Flatpak, and Snap Steam installs with `notify`. Optional account mode adds owned/Family games via `IAuthenticationService` mobile-app refresh tokens (only mobile tokens renew via `GenerateAccessTokenForApp`; Steam client tokens fail with `AccessDenied`).
- Opt-in client library reader (`client_library/`) uses `.cef-enable-remote-debugging` on loopback only with a strictly read-only expression (`collectionStore.allGamesCollection.allApps`, guarded by unit test), removing only marker files Irori created and never restarting Steam in Game Mode.
- Hidden games read `cloud-storage-namespace-1.json` (`user-collections.hidden`) and apply once (`store_hidden_imported`).
- Launches use `process::open_detached` with Windows argument quoting (`split_program_args`) for Windows/Proton titles; `SteamGameWatch` narrows to direct game-folder PIDs and waits on `pidfd` on Linux.
- Compatibility tools (`config/config.vdf`) and launch options (`localconfig.vdf`) splice exact byte ranges with full-file parsing, `.irori-backup`, and read-back verification, strictly while Steam is closed (`SteamRunning`, never closing Steam in Game Mode).
- Friends use `IFriendsListService/GetFriendsList` and `IPlayerService/GetPlayerLinkDetails` with the account token (never shipping a Web API key); 401/403 maps to `SteamError::NotShared`, never `SessionRejected`.

### Mods (`irori-mods`)

- Stored under `<data>/mods/<runner>/<app>/` (`mods.json`, `<mod>/files/`, `.written/`, `deployment.json`) with `[a-z0-9-]` IDs only.
- Applied right before `store.launch` and restored after session exit (`recover_mods` cleans crash leftovers at app start, never during CLI runs).
- Never delete game files: link deployment writes the full manifest first, renames replaced files to `<name>.irori-original`, validates `fs_guard::ensure_deletable`, retries Windows sharing locks up to 5 s, and preserves game writes in `.written/`. Linux `fuse-overlayfs` leaves the game directory untouched as a lower layer.
- Supports Bethesda `Plugins.txt`/`ini` management (`bethesda.json`), Unreal Engine `~mods` layouts, Wine proxy DLL overrides (`PROXY_DLLS`), safe archive extraction (`archive_guard`, zero symlinks), data-only FOMOD XML, Thunderstore (`ror2mm://`), and Nexus Mods (`nxm://`, personal API key in `irori_secrets`).

### ReShade, RenoDX, and OptiScaler (`irori-reshade`)

- Scoped to Windows builds (native or Wine/Proton, including Proton Steam games). Shared `GraphicsMod` layer handles bounded PE header/API detection, `Compatibility.ini` bans, official downloads with SHA-256 verification (never bundled), pre-write manifests (`<data>/reshade/`), `.irori-disabled` toggling, pre-launch restoration after store updates, and hash-checked removal via `fs_guard`.
- Wine/Proton launches merge `<dll>=n,b` overrides via `add_dll_overrides`. Steam games update `WINEDLLOVERRIDES` in Steam launch options (ReShade/RenoDX) or surgically sync `user.reg` `[Software\\Wine\\DllOverrides]` (OptiScaler).
- OptiScaler checks GitHub releases only on demand (15 min TTL + ETag), supports `MesaAuto` (FakeNVAPI) and `Nvidia` (`PROTON_ENABLE_NVAPI=1`), optional `OptiPatcher.asi`, and surgical `OptiScaler.ini` `[Menu] ShortcutKey` patching.

### Xbox on Linux (`irori-msstore-api`, `irori-msixvc`, `irori-xbox`)

- Native Rust port of Xodus with zero external CLI helpers; requires `ReleaseKind::NexusRuntime` Proton (`xgameruntime.dll`, kept off umu, provisioning `libnghttp2.dll` into `system32` with `libnghttp2=n`).
- Uses `LoginFlow::MicrosoftAccount` (slots `main` for Store and optional `player` for Xbox), `irori_secrets` via `SecretsBackend`, `irori_msixvc::http_client()` (disabling transparent gzip/brotli so HTTP ranges stay exact), and decrypted private `0700` launch views swept at exit and startup.

## 7. Rust, testing, packaging, and PR conventions

### Rust conventions

- Errors: use `anyhow::Result` with user-facing `.context(...)` / `.with_context(...)` formatted with `{e:#}`. Use `thiserror` only for typed errors matched by callers.
- Concurrency: never block Tokio workers; use `tokio::fs`, `irori_core::process`, or `spawn_blocking`. Never hold `parking_lot` locks across `.await`.
- Dependencies: declare shared crates in `[workspace.dependencies]` (`default-features = false` where practical), keep `Cargo.lock` committed (`--locked` in CI), and keep `deny.toml` (`cargo deny check` in CI) passing.
- Platform code: isolate OS-specific logic in dedicated modules gated with `#[cfg(target_os = "...")]` and ensure every branch builds on Linux, macOS, and Windows.
- Docs and naming: start modules with `//!`, document public items with `///`, explain *why* in comments, avoid TODOs without issues, and follow `irori-<area>` / `*View` / `*Event` / `open_*_dialog` / `init(cx)` naming.
- Workspace lints (`[workspace.lints.clippy]` in root `Cargo.toml`, inherited by all non-vendored crates via `[lints] workspace = true`): `dbg_macro = "deny"`, `todo = "deny"`, `unimplemented = "deny"`, `large_futures = "warn"`, `needless_collect = "warn"`. Root `clippy.toml` sets `allow-unwrap-in-tests = true` and `allow-expect-in-tests = true`.

### Splitting large modules into submodules

- Split `.rs` files before they grow past roughly 600 to 800 lines instead of letting views, forms, or schemas become monoliths.
- Prefer the Rust 2018+ sibling facade layout (`<module>.rs` + `<module>/*.rs`) when refactoring an existing file, as used in `crates/core/src/settings.rs` + `crates/core/src/settings/`, `crates/feature-settings/src/form.rs` + `crates/feature-settings/src/form/`, `crates/feature-library/src/view.rs` + `crates/feature-library/src/view/`, and `crates/feature-settings/src/hud/panel.rs` + `crates/feature-settings/src/hud/panel/` (alongside existing directory modules such as `crates/feature-settings/src/registry/mod.rs`, `crates/feature-game/src/page/mod.rs`, and `crates/extras/src/discounts/mod.rs`).
- Keep `<module>.rs` thin (module docs, `mod` declarations, `pub` or `pub(crate)` re-exports, and core struct definitions) and move focused concerns into `<module>/{actions,render,sections,helpers,tests}.rs` using `pub(super)` or `pub(crate)` visibility so external crate paths never change.
- Run `python3 .github/scripts/check_agent_docs.py` whenever splitting, moving, or renaming modules or crates so `docs/`, `docs/GRAPHS.md`, and `.agents/skills/` stay synchronized with the source tree.

### Testing conventions

- Keep tests inline (`#[cfg(test)] mod tests` or `src/tests.rs`) with small inline fixtures.
- Never reach real networks, real user configs, Heroic directories, store accounts, or OS keyrings. Isolate with `tempfile`, `Paths::init(Paths::under(tmp))`, `irori_depot::test_server::TestServer`, and store test constructors.
- Every bug fix must add a regression test that fails without the fix.
- Ignored (`#[ignore]`) integration tests require explicit environment opt-in (`IRORI_RELEASE_TEST_URL` pointing to the local mock under `tests/postman/`, `IRORI_RUNTIME_SMOKE=1`, `IRORI_KEYRING_SMOKE=1` in throwaway `dbus-run-session`, `IRORI_SANDBOX_SMOKE=1`, `IRORI_SANDBOX_PROTON`).
- Packaging changes must pass `python3 -m unittest discover -s packaging -p "test_*.py"`; PR-rule and documentation-sync changes must pass `python3 -m unittest discover -s .github/scripts -p "test_*.py"` and `python3 .github/scripts/check_agent_docs.py`.

### Branches, commits, and pull requests

Enforced by `.github/workflows/pr-rules.yml` and `.github/scripts/pr_rules.py`:

| Type | Use for | Example branch | Example title |
| --- | --- | --- | --- |
| `feat` | New user-facing capability | `feat/cli-login` | `feat(manage): sign in from the command line` |
| `fix` | Bug fix | `fix/epic-dlc-playtime` | `fix(stats): count store playtime for games, not their DLC` |
| `perf` | Performance or memory improvement | `perf/library-filter` | `perf(library): filter once per revision` |
| `refactor` | Restructuring with no behavior change | `refactor/split-install-sheet` | `refactor(game): split the install sheet` |
| `docs` | Documentation only | `docs/agents-md` | `docs: add AGENTS.md` |
| `test` | Tests only | `test/gog-manifests` | `test(gog): cover v1 manifests` |
| `i18n` | Translations and catalogs only | `i18n/polish-settings` | `i18n(settings): fix Polish plural forms` |
| `build` | Cargo manifests, deps, `packaging/` | `build/wry-0.54` | `build: update wry to 0.54` |
| `ci` | `.github/` workflows and scripts | `ci/pr-rules` | `ci: enforce pull request naming` |
| `chore` | Maintenance fitting no other type | `chore/cleanup-assets` | `chore: remove unused theme files` |
| `revert` | Reverting an earlier change | `revert/sidebar-blur` | `revert: "feat(ui): blur the sidebar"` |

- Branch names must match `<type>/<short-kebab-description>` (lower case, digits, `-`, `.`, no extra slashes; agent prefixes like `claude/` or `codex/` fail CI).
- PR titles must match `<type>(<scope>): <summary>` or `<type>: <summary>`, max 72 characters, same `<type>` as the branch, imperative lower-case summary (acronyms like `GOG` or `UI` may lead), no trailing period. Add `!` before `:` for breaking changes (settings keys, on-disk state, CLI, `irori-sign-in` CLI, or `irori://` schemes).
- Base branch is `main`, or another convention-named branch when stacking (never target an already-merged branch). PRs squash-and-merge using the PR title.
- Pull requests adding more than 1,500 lines across `*.rs` files fail `.github/scripts/pr_rules.py` unless split into smaller stacked PRs or labeled `large-pr-approved`.

## 8. Pitfalls and platform invariants

Things that have bitten this codebase before:

1. `cargo run -p irori` builds only `irori`. On Linux, sign-in falls back to the browser until you also build `irori-sign-in` with the same Cargo profile.
2. Package names do not match directory names (`crates/store-epic` is `irori-epic`; `crates/epic` is `irori-epic-api`).
3. Never wait on `LibraryChanged` in UI code; wait on `LibraryUpdated` so the rebuilt snapshot is ready.
4. `store.games()` includes DLC entries. Filter `is_dlc` everywhere games are listed or counted (Epic reports base playtime on DLC entries too).
5. Epic games can ship multiple releases (such as a separate Mac port); inspect every `releaseInfo` entry, not only the first.
6. Settings keys match Heroic spelling including typos (`enviromentOptions`). Renaming one silently drops user settings.
7. `Paths::init` works only once per process; initialize once in a `Once` and isolate individual tests in subdirectories.
8. Never enable `gpui-kit/test-support` (conflicts with `gpui-pre` in `Cargo.lock`).
9. Port `127.0.0.1:9977` (comet) is reachable by any local process, and a comet panic aborts all of Irori. Gate handlers on sign-in, validate all inputs into errors, and never expose account tokens.
10. Never embed a web engine in Irori's main window or link WebKitGTK into `irori` on Linux (AppImage packaging rejects it, and Irori must start without WebKitGTK).
11. macOS bundles must never link Homebrew dynamic libraries (`package.py` rejects any non-system dylib). Link statically (for example `xz2`'s `static` feature) or use system frameworks.
12. Windows builds use `windows_subsystem = "windows"` (no console). Any parent-child output must travel over explicit pipes.
13. `WindowHandle::update` fails re-entrantly inside that window's own update, and `let _ =` masks the error. Use `cx.defer` from dialog callbacks.
14. Never block the GPUI UI thread on filesystem, network, or keyring I/O.
15. Upgrading `gpui-pre` or `gilrs` requires updating the vendored `crates/gpui-pre-windows` or `crates/gilrs-core` version to match and reapplying their `PATCHES.md`, or Cargo's `[patch.crates-io]` silently stops applying.
16. Checking Windows targets from macOS requires the rustup toolchain first on `PATH` (Homebrew Rust lacks Windows std) plus Homebrew LLVM's `llvm-rc`: `PATH="$(dirname "$(rustup which rustc)"):/opt/homebrew/opt/llvm/bin:$PATH" cargo check --target x86_64-pc-windows-msvc -p <package>`.
17. Every release must keep publishing `SHA256SUMS` over all `package.py` package names (`self_update.rs` tests `release.yml` for this), and `packaging/package.py` requires both `irori` and `irori-sign-in`.
