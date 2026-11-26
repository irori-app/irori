# Performance and Resource Budgets

Irori targets one native process, GPU-accelerated rendering via GPUI, zero CPU usage while idle, bounded RAM and disk caches, and virtualized lists for large libraries. This document covers the concrete engineering rules, cache caps, runtime budgets, build profiles, and measurement tools that keep those guarantees intact.

## Product budgets and invariants

- Zero idle CPU: never add background polling timers when no game, download, or active session needs them. Use OS wakeups (`pidfd_open` with `poll`, `notify` filesystem watchers, async channels, blocking event readers).
- Zero UI-thread I/O: the GPUI main thread only renders state and dispatches actions. Even fast filesystem reads, SQLite queries, or keyring lookups must run through `irori_ui::run` or `backend.spawn_blocking`.
- Bounded memory and GPU textures: keep caches on disk rather than pinning full API payloads or decoded images in RAM. Every in-memory or GPU cache must define an explicit size limit or TTL.
- Lazy opt-in integrations: external metadata and optional store/launcher integrations (IGDB, VNDB, LaunchBox, Steam title matching, Steam friends, Xbox friends, OptiScaler release checks) must perform zero network requests at startup or while disabled.

## GPUI render discipline and virtualization

### Virtualization threshold

Virtualize every list or grid that can exceed roughly 50 items using `v_virtual_list`, `List`, or `DataTable`. Examples already virtualized include the library grid and list, field-rule value manager (`Backend::field_values`), Wine prefixes tab, stats Journeys lanes, and Big Picture shelves, grids, and action lists.

Big Picture shelves, grids, and page lists scroll by springing a single offset toward the focused item and constructing only items near the viewport, so animation work stops completely once focus comes to rest.

The command palette (`crates/irori/src/palette/view.rs`) measures every row it renders, so it caps game rows from `SearchIndex` at `search::MAX_GAMES` rather than rendering the full library.

### Pre-computed derived state

Never re-sort, re-filter, compile regexes (`Regex::new`), or clone `LibrarySnapshot` inside `Render::render`:

- Precompile regular expressions with `once_cell::sync::Lazy`.
- Store shared backend state behind `RwLock<Arc<T>>` copy-on-write handles and pass `Arc<GameInfo>` references rather than cloning game records per frame.
- Derive filtered and sorted views only when inputs change (keyed by revision number), triggered through `cx.observe` on the narrowest model (`LibraryModel`, `StatusModel`, `DownloadsModel`, `ComponentsModel`, `AccountsModel`, `SettingsModel`, `NoticeModel`, `SessionModel`).
- `LibrarySnapshot` rebuilds are debounced by 30 ms after `LibraryChanged` or `GameChanged` events.
- `CategoryIndex` (manual and smart category membership and counts) rebuilds only when its `IndexKey` changes (library revision, user-data revision, HowLongToBeat cache change, pending updates, or local calendar day), never per frame.
- `SearchIndex` is built lazily on the first search against a snapshot rather than eagerly on every rebuild.
- `irori-library`'s filter-value search (`value_search.rs`) computes its index once per library, user-data, and category index change.
- `sessions::stats()` reads `sessions.jsonl` once at startup and updates incrementally on `append` and `replace_all`, avoiding UI-thread reads during sorting or smart-category evaluation.
- Library covers in `coverFit = "whole"` mode keep fixed row heights (`grid::whole_sizes`) so virtual list sizing and keyboard navigation stay constant-time.
- Title-bar download pills and sidebar badges (`downloads/indicator.rs`) are isolated views that redraw only when their displayed values change.
- `irori_ui::tour::mark` records bounds in an empty canvas only while a guided tour is actively running.

## GPU and disk art caches

### Cover art pipeline (`crates/ui/src/art.rs`)

- Original images download once; thumbnails are generated per width bucket and persisted to disk.
- Concurrent image decodes are capped at 2 at a time so bulk scrolling never saturates CPU or memory.
- `ArtStore::get(url, width, keep_alpha)` returns `Ready`, `Loading`, or `Missing` synchronously without blocking.
- Wrap art-heavy regions in `div().image_cache(LruImageCache::new(n, cx))` and use `GameCover` or `art_img` instead of loading raw images directly.
- `art::aspect(url)` reads image dimensions once off the UI thread when a thumbnail becomes ready.

### Disk cache trimming (`crates/services/src/art_cache.rs`)

- The art disk cache is capped by `iroriArtCacheLimit` (default 2 GiB, `0` disables the cap).
- The periodic tick trims least-recently-used image groups (each original together with its thumbnails) every 6 hours.
- Active art handed out by `ArtStore` is tracked in memory via `art_cache::note_used` and protected from eviction. Each trim updates timestamps in bulk so recency survives restarts without filesystem writes on the UI thread.

### Media, trailers, posters, and icons

- Game page trailers (`media/trailer.rs`, `irori_ui::trailer::TrailerPlayer`) stream raw BGRA frames from the user's `ffmpeg` at playback rate (`-re`), keep at most 2 frames in the GPU atlas at once, and terminate `ffmpeg` immediately when paused or off-screen.
- Replay-buffer clip posters (`media/posters.rs`) decode keyframes sequentially (one at a time, single decoder thread) and cache results in `<cache>/clip-posters/` keyed by path, size, and mtime.
- Windows executable program icons (`program_icons.rs`) extract icons off the rebuild path on a blocking thread with a bounded PE resource reader (`shortcuts::pe_icon`) and cache negative and positive results in `<cache>/program-icons.json` so library rebuilds do not touch disk.
- Storage measurements (`storage.rs`), screenshot folder listings (`screenshots.rs`), ROM detection caches, and BIOS hashes all cache results on disk keyed by path, size, and modification time so unchanged directories are never re-hashed at launch or rebuild.

## Startup budget and backend concurrency

### First render and store initialization

- `Store::init` must be disk-only with zero network calls because it gates the first UI render.
- `Backend::start` owns a dedicated Tokio runtime named `irori-io` with 3 worker threads and 16 blocking threads. Schedule background async work with `backend.spawn` and heavy CPU/filesystem work with `backend.spawn_blocking`.
- Never hold a `parking_lot` lock guard across `.await`; use `tokio::sync::Mutex` when a lock must span an await point.
- `EventBus` uses a Tokio broadcast channel with capacity 1024; the UI event pump drains events in batches and notifies each affected model once per batch.

### Event-driven monitoring and bounded I/O

- Controller input (`crates/feature-bigpicture/src/gamepad/reader.rs`) runs on a dedicated `gilrs` thread that blocks until an input event arrives, consuming zero CPU when idle. The in-game overlay's raw `evdev` reader thread and Windows XInput watcher exist only while a game or overlay is active.
- Process tracking prefers kernel `pidfd_open(2)` with `poll(2)` on Linux (`GameWatch::wait_for_exit`) and checks known PIDs before falling back to broader scans; polling occurs only while a watched game is running, never at idle.
- Download queue (`irori-services`) processes one download at a time, persisted in `state/downloads.json`, with cancellation via `CancellationToken`.
- All shared HTTP requests use `irori_core::http::client()` (10 s connect timeout, 60 s total timeout unless overridden by `iroriNetworkTimeout`). Large transfers use `download_client()` or `irori-depot`.
- Binary and archive parsers (PE headers in `irori-reshade` / `troubleshoot` / `pe_icon`, ISO 9660 in `irori-engines`, LiteDB 4 in `irori-extras`, LaunchBox XML streaming via `quick-xml`, archive entries through `archive_guard`, and WebSocket/HTTP calls) enforce explicit size caps (`MAX_*_BYTES`) and bounds checks instead of loading unbounded files into memory.
- External API clients respect upstream rate limits and conditional requests: Wine/Proton release checks inspect `releases.atom` before hitting the GitHub API and track `x-ratelimit-*` headers in `state/wine-release-checks.json` (preserving a 10-call reserve); OptiScaler uses 15-minute TTL + ETag caching; IGDB spaces calls to 4/s; VNDB spaces calls to 1.5 s; Lutris API caps at 2/s; Steam title matching batches 25 games with 1.5 s spacing.

## Build profiles and profiling

Root `Cargo.toml` defines five build profiles:

| Profile | Configuration | Use for |
| --- | --- | --- |
| `release` | `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, `strip = true` | Shipped binaries and final release measurements |
| `quick` | Inherits `release`, `lto = false`, `codegen-units = 256`, `incremental = true`, `strip = false` | Fast local contributor testing with release-like runtime speed |
| `dev` | `opt-level = 1` for workspace, `opt-level = 2` for dependencies | Iterative local debugging without sluggish UI performance |
| `ci` | Inherits `dev`, `opt-level = 0`, `debug = 0`, `incremental = false` | Fast compilation with low disk footprint on CI runners |
| `profiling` | Inherits `release`, `debug = "line-tables-only"`, `strip = false`, `lto = "thin"`, `codegen-units = 16` | CPU and heap profiling with symbols |

### Profiling with `--profile profiling`

Build the symbol-bearing release binary:

```bash
cargo build --locked --profile profiling -p irori
```

Run it against an isolated `IRORI_HOME` under `perf` or `heaptrack`:

```bash
IRORI_HOME=/tmp/irori-prof perf record -g --call-graph dwarf target/profiling/irori
IRORI_HOME=/tmp/irori-prof heaptrack target/profiling/irori
```

## Benchmarks

Comparative memory, process-count, binary-size, and idle-CPU methodology and historical results live in [docs/BENCHMARKS.md](BENCHMARKS.md), driven by [docs/benchmarks/compare.py](benchmarks/compare.py).

Summary of the current Irori 1.1.0 benchmark suite on Linux Wayland (2 rounds, 45 s warmup, 30 s idle window; historical 0.2.0 vs Heroic 2.22.4 comparison also lives in [docs/BENCHMARKS.md](BENCHMARKS.md)):

- 0 games (empty throwaway profile): 122.4 MiB median PSS, 1 process, 0.02% median idle CPU
- 300 games (synthetic library + cover/hero art): 146.8 MiB median PSS, 1 process, 0.00% median idle CPU
- 1,000 games (synthetic library + cover/hero art): 147.7 MiB median PSS, 1 process, 0.02% median idle CPU
- 1,440 games (real multi-store library in Focus view): 385.1 MiB median PSS, 1 process, 0.03% median idle CPU
- Release binary size (`irori 1.1.0`): 108.1 MB

Reproduce locally against synthetic throwaway profiles without touching real user data:

```bash
python3 docs/benchmarks/compare.py --irori target/release/irori --irori-only \
  --library-sizes 0,300,1000 --rounds 2 --settle 45 --idle 30 --output results.csv
```

When introducing a change that touches library indexing, cover art decoding, background tasks, or GPUI rendering hot paths, verify that idle CPU stays near 0.0% and memory scales cleanly across a 1,000-game library before merging.
