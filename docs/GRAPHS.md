# Architecture Graphs

Canonical Mermaid diagrams for Irori's workspace crate hierarchy, reactive state loop, launch pipeline, download and deletion safety pipeline, and Rust 2018+ submodule split convention.

Every active workspace crate in root `Cargo.toml` must appear in Graph 1, and every explicit repository path referenced in backticks must exist on disk. Automated CI checks in `.github/scripts/check_agent_docs.py` enforce both rules on every pull request.

## 1. Strict workspace crate hierarchy

Dependencies flow strictly downward. Protocol crates never depend directly on `irori-core`, `irori-services` drives stores through `DynStore`, and `feature-*` crates stay decoupled from each other (except `irori-settings` reading `irori-bigpicture` gamepad and overlay support tables; cross-feature UI flows route through `irori_ui::hooks` and shell events).

```mermaid
flowchart TD
    subgraph Foundation["Foundation (zero internal deps)"]
        Secrets["irori-secrets (crates/secrets)"]
        Core["irori-core (crates/core)"]
    end

    subgraph Transfer["Chunked transfer engine"]
        Depot["irori-depot (crates/depot)"]
    end

    subgraph Protocols["Protocol, archive, and companion crates (no direct irori-core dep)"]
        EpicApi["irori-epic-api (crates/epic)"]
        GogApi["irori-gog-api (crates/gog)"]
        AmazonApi["irori-amazon-api (crates/amazon)"]
        ItchApi["irori-itch-api (crates/itch)"]
        SteamApi["irori-steam-api (crates/steam)"]
        RetroApi["irori-retroachievements-api (crates/retroachievements)"]
        XboxApi["irori-xbox-api (crates/xbox)"]
        MsStoreApi["irori-msstore-api (crates/msstore)"]
        Msixvc["irori-msixvc (crates/msixvc)"]
        Comet["comet (crates/comet)"]
        GogGalaxy["irori-gog-galaxy (crates/gog-galaxy)"]
        LaunchersData["irori-launchers-data (crates/launchers)"]
    end

    subgraph Domain["Runtime and domain engines"]
        Runtime["irori-runtime (crates/runtime)"]
        Engines["irori-engines (crates/engines)"]
        Scripts["irori-scripts (crates/scripts)"]
        Mods["irori-mods (crates/mods)"]
        Reshade["irori-reshade (crates/reshade)"]
        Extras["irori-extras (crates/extras)"]
    end

    subgraph Stores["Store adapters (implement irori_core::Store)"]
        StoreEpic["irori-epic (crates/store-epic)"]
        StoreGog["irori-gog (crates/store-gog)"]
        StoreAmazon["irori-amazon (crates/store-amazon)"]
        StoreZoom["irori-zoom (crates/store-zoom)"]
        StoreItch["irori-itch (crates/store-itch)"]
        StoreXbox["irori-xbox (crates/store-xbox)"]
        StoreSteam["irori-steam (crates/store-steam)"]
        StoreLaunchers["irori-launchers (crates/store-launchers)"]
        StoreEmulation["irori-emulation (crates/store-emulation)"]
        StoreSideload["irori-sideload (crates/store-sideload)"]
    end

    subgraph Facade["Backend facade"]
        Services["irori-services (crates/services)"]
    end

    subgraph UiLayer["Shared UI bridge (irori_ui::hooks) and feature crates"]
        Ui["irori-ui + irori_ui::hooks (crates/ui)"]
        FeatLib["irori-library (crates/feature-library)"]
        FeatGame["irori-game (crates/feature-game)"]
        FeatManage["irori-manage (crates/feature-manage)"]
        FeatBp["irori-bigpicture (crates/feature-bigpicture)"]
        FeatSettings["irori-settings (crates/feature-settings)"]
    end

    subgraph Binaries["Executable binaries"]
        AppBin["irori (crates/irori)"]
        SignInBin["irori-sign-in (crates/sign-in)"]
    end

    Core --> Depot
    Secrets --> Protocols
    Depot --> Protocols
    Comet --> GogGalaxy

    Core --> Domain
    Depot --> Engines
    Depot --> Scripts
    Depot --> Mods
    Runtime --> Engines
    Runtime --> Scripts
    Runtime --> Reshade

    Core --> Stores
    Secrets --> Stores
    Depot --> Stores
    Protocols --> Stores
    Domain --> Stores

    Core --> Services
    Secrets --> Services
    Depot --> Services
    Domain --> Services
    SteamApi --> Services
    Stores -. "DynStore trait object" .-> Services

    Services --> Ui
    Core --> Ui
    Runtime --> Ui
    Engines --> Ui
    Extras --> Ui

    Ui -- "models + irori_ui::hooks" --> FeatLib
    Ui -- "models + irori_ui::hooks" --> FeatGame
    Ui -- "models + irori_ui::hooks" --> FeatManage
    Ui -- "models + irori_ui::hooks" --> FeatBp
    Ui -- "models + irori_ui::hooks" --> FeatSettings
    FeatBp -. "gamepad / overlay::support" .-> FeatSettings
    Services --> UiLayer

    FeatLib --> AppBin
    FeatGame --> AppBin
    FeatManage --> AppBin
    FeatBp --> AppBin
    FeatSettings --> AppBin
    Stores --> AppBin
    SignInBin -. "separate process on Linux; linked on macOS/Windows" .-> AppBin
```

Vendored `[patch.crates-io]` crates excluded from workspace lints: `crates/gpui-pre-windows` and `crates/gilrs-core`. Detailed crate responsibilities live in [docs/ARCHITECTURE.md](ARCHITECTURE.md).

## 2. Reactive UI, Backend, Store, and EventBus loop

GPUI views never block on I/O and never subscribe to raw `Event::LibraryChanged`. Backend tasks run on the dedicated `irori-io` Tokio runtime, rebuild `Arc<LibrarySnapshot>` with a 30 ms debounce, and emit `Event::LibraryUpdated(rev)` before the batched model pump notifies GPUI observers.

```mermaid
sequenceDiagram
    participant View as GPUI View (UI thread)
    participant Bridge as irori_ui::run
    participant Backend as Backend (irori-io Tokio)
    participant Store as DynStore (crates/store-*)
    participant Bus as EventBus (broadcast 1024)
    participant Pump as UI Event Pump (crates/ui/src/models.rs)
    participant Model as LibraryModel / StatusModel

    View->>Bridge: irori_ui::run(cx, async move { backend.refresh(...).await })
    Bridge->>Backend: Spawn future on irori-io runtime
    Backend->>Store: store.refresh(full)
    Store->>Bus: emit(Event::LibraryChanged(runner))
    Backend->>Backend: Debounce 30 ms, filter !is_dlc, merge overrides, sort LibrarySnapshot
    Backend->>Bus: emit(Event::LibraryUpdated(revision))
    Bus->>Pump: Drain queued events in batch
    Pump->>Model: Update model once per batch and cx.notify()
    Model->>View: cx.observe callback recomputes derived view state
    View->>View: Render::render (pure layout, zero I/O, zero sorting)
```

## 3. Game launch and session monitoring pipeline

`Backend::launch` in `crates/services/src/games.rs` orchestrates save sync, runtime preparation, mod deployment, process spawning, and event-driven exit tracking (`pidfd_open` with `poll` on Linux, zero polling at idle).

```mermaid
flowchart TD
    Start["Backend::launch (crates/services/src/games.rs)"] --> Check["Busy check + verify game files (GameFilesMissing)"]
    Check --> SaveDown["Cloud save pre-launch sync (SAVE_SYNC_TIMEOUT 3 min)"]
    SaveDown --> Prep["irori_runtime::prepare_launch (crates/runtime)"]

    subgraph RuntimePrep["Environment and containment"]
        Prep --> NativeOrWine["Select native binary, Wine/Proton/CrossOver/GPTK, or umu"]
        NativeOrWine --> Sandbox["Optional bubblewrap sandbox (crates/runtime/src/sandbox.rs)"]
        Sandbox --> EnvCompose["Compose DLL overrides, HUD env, and GAMEID"]
    end

    EnvCompose --> PreScript["Run before-launch script (waited or alongside)"]
    PreScript --> StorePlan["store.launch -> LaunchPlan"]

    subgraph ExecutionKind["Launch target"]
        StorePlan --> DirectExec["Direct native or Wine executable"]
        StorePlan --> EmuExec["Emulator command (crates/engines + BIOS link)"]
        StorePlan --> ExtWatch["External client URI + GameWatch (Steam, Battle.net, Xbox)"]
    end

    DirectExec --> DeployMods["Apply mods and graphics mods (crates/mods, crates/reshade)"]
    EmuExec --> Spawn["Pre-game programs + process::spawn + alongside programs"]
    ExtWatch --> Spawn
    DeployMods --> Spawn

    Spawn --> PersistRun["Record PID + start time in state/running.json"]
    PersistRun --> Watch["Event-driven session watch (pidfd_open/poll, overlay, replay, playtime)"]
    Watch --> PostExit["After-launch script, cloud save upload, mod restore, Event::GameExited"]
```

## 4. Download, install, update, and safe deletion pipeline

`crates/services/src/downloads.rs` persists a single-worker queue in `state/downloads.json`, accounts for per-disk space via `crates/services/src/download_space.rs`, delegates bytes to specialized engines, and routes every recursive deletion through `crates/core/src/fs_guard.rs`.

```mermaid
flowchart LR
    Queue["Downloads queue (crates/services/src/downloads.rs)"] --> Space["Disk space check (crates/services/src/download_space.rs)"]

    Space --> DepotPath["irori-depot: parallel chunks, resume, delta diff, digest verify"]
    Space --> MsixvcPath["irori-msixvc: streaming XVD/NTFS, AES-XTS, checkpoints"]
    Space --> ScriptPath["irori-scripts: typed Lutris plan, Approvals consent, confined rollback"]

    DepotPath --> Commit["Atomic state write (crates/core/src/json_store.rs)"]
    MsixvcPath --> Commit
    ScriptPath --> Commit

    Commit --> Guard["Uninstall / prefix / cache cleanup -> fs_guard::ensure_deletable"]
```

## 5. Rust 2018+ submodule split pattern

When a `.rs` file approaches 600 to 800 lines, split it into a thin `<area>.rs` sibling facade plus cohesive `<area>/*.rs` submodules instead of growing a monolith, exceeding the 1,500-line `*.rs` PR limit, or breaking external import paths. Real examples on `main` include `crates/core/src/settings.rs` + `crates/core/src/settings/`, `crates/feature-settings/src/form.rs` + `crates/feature-settings/src/form/`, `crates/feature-library/src/view.rs` + `crates/feature-library/src/view/`, and `crates/feature-settings/src/hud/panel.rs` + `crates/feature-settings/src/hud/panel/`.

```mermaid
flowchart LR
    subgraph Before["Growing module (600 to 800+ lines)"]
        Monolith["Monolithic module (for example crates/core/src/settings.rs before split)"]
    end

    subgraph After["Rust 2018+ facade + cohesive submodules (under 600 to 800 lines each)"]
        Facade["crates/core/src/settings.rs (thin facade: mod declarations + pub use re-exports)"]
        App["crates/core/src/settings/app.rs (AppSettings schema)"]
        Game["crates/core/src/settings/game.rs (GameSettings schema)"]
        Overrides["crates/core/src/settings/overrides.rs (sparse GameSettingsOverrides)"]
        PrefsUi["crates/core/src/settings/prefs.rs + crates/core/src/settings/ui.rs"]
        Tests["crates/core/src/settings/tests.rs (offline unit tests)"]
    end

    Monolith --> Facade
    Facade --> App
    Facade --> Game
    Facade --> Overrides
    Facade --> PrefsUi
    Facade --> Tests
    After --> Verify["cargo clippy + cargo test + check_agent_docs.py + pr_rules.py"]
```

For step-by-step refactoring instructions and few-shot XML examples (both GPUI views and domain schemas), load [.agents/skills/irori-submodule-refactor/SKILL.md](../.agents/skills/irori-submodule-refactor/SKILL.md).
