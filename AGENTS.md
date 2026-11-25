# AGENTS.md

Root engineering contract for humans and coding agents working on Irori. Read this file before your first change, then load the relevant on-demand doc from the routing table below.

Irori is a native Rust (edition 2024, Rust 1.95+) game launcher for Linux, macOS, and Windows built with GPUI through GPUI Kit (`gpui-kit = "0.8.0"`). Product goals, in order: correct, fast, calm, one click to play.

## Verification commands

Run all commands from the repository root:

| Task | Command |
| --- | --- |
| Format check | `cargo fmt --all -- --check` |
| Lint (CI parity) | `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings` |
| Full test suite | `cargo test --locked --workspace --no-fail-fast --profile ci` (wrap in `xvfb-run -a` on headless Linux) |
| Fast local build | `cargo build --locked --profile quick -p irori` |
| Sign-in helper (Linux) | `cargo build --locked --profile quick -p irori-sign-in` (build for the same profile as `irori`) |
| Profile with symbols | `cargo build --locked --profile profiling -p irori` |
| Single crate check/test | `cargo test -p <package>` (directory names differ from package names; see `docs/ARCHITECTURE.md`) |
| Packaging tests | `python3 -m unittest discover -s packaging -p "test_*.py"` |
| PR naming check | `python3 .github/scripts/pr_rules.py --title "<type>(<scope>): <summary>" --head <type>/<kebab-name>` |

## Core architectural invariants

1. Never link WebKitGTK or GTK3 into `irori` on Linux, and never embed a web engine inside Irori's main window. Keep `irori-sign-in` as a separate process with a stable command line.
2. Keep `#![forbid(unsafe_code)]` on protocol, secret, depot, archive, script, PE reader, and Galaxy service crates (`SECURITY.md` lists every crate). Elsewhere `unsafe` is restricted to OS APIs with a `// SAFETY:` comment.
3. Preserve strict crate layering: protocol crates never depend on `irori-core`, `irori-services` talks to stores only through `DynStore`, and feature crates never depend on each other (use `irori_ui::hooks` and shell routing).
4. Use theme tokens (`cx.theme().*`) and `rem` helpers only. Never use raw colors, and never use `px(...)` except for hairlines and art. Never invent `gpui-kit` APIs; verify against `~/.cargo/registry/src/index.crates.io-*/{gpui-kit-0.8.0,gpui-component-0.8.0,gpui-base-0.8.0,gpui-pre-0.3.9}`.
5. Put every new or changed user-facing UI string in `en.json` and all 19 locale catalogs in the same change (`en.json` is the source of truth), using intent-based keys with `t()`/`t_args()` and matching `{placeholder}` names. Each crate's i18n tests fail on missing keys or placeholder mismatches.
6. Never rename Heroic-compatible settings keys (including existing typos like `enviromentOptions`), never delete files recursively without `fs_guard::ensure_deletable`, and never write credentials outside `irori-secrets`.
7. Never enable `gpui-kit`'s `test-support` feature (it conflicts with `gpui-pre`); use `gpui = { package = "gpui-pre", version = "=0.3.9", features = ["test-support"] }` as a dev-dependency.
8. Keep vendored crates (`crates/comet`, `crates/gpui-pre-windows`, `crates/gilrs-core`) out of workspace lints and in lockstep with their `PATCHES.md` or `README.md`.

## Code efficiency and transparency contract

Mandatory for all new code:

1. Zero panics in production. Forbid `.unwrap()`, `.expect()`, and unchecked indexing outside `#[cfg(test)]`; return `Result`/`Option` with actionable user-facing errors (`.context(...)` / `.with_context(...)`, formatted with `{e:#}`) and log unexpected failures with `tracing::warn!` instead of silent `let _ = ...`.
2. Event-driven OS primitives over polling. Prefer `pidfd_open`/poll, async channels, `notify`, and wakeups. Keep idle CPU at zero and never run blocking filesystem, network, keyring, or subprocess calls on the GPUI UI thread (use `irori_ui::run` or `backend.spawn_blocking`).
3. GPUI render discipline. Never run I/O, `Regex::new`, or full-library re-sorting or cloning inside `Render::render`. Pre-compute derived state on model or revision change (`cx.observe`) and virtualize lists past ~50 items.
4. Bounded resources and clean cancellation. Every in-memory or GPU cache must have an explicit size cap or TTL (LRU). Every async `Task` and child process must cancel or clean up on `Drop`. Every HTTP, TCP, or WebSocket call must have an explicit timeout and `MAX_*_BYTES` cap.
5. Network, disk, and subprocess transparency. Any new external domain, path outside `Paths`, or spawned helper must update `SECURITY.md` in the same PR. Opt-in integrations stay strictly lazy (zero startup network calls). Never log tokens, cookies, passwords, or API keys.
6. Dependency diet. Check `std` and existing workspace crates before adding any crate to `Cargo.toml`. Declare shared crates in `[workspace.dependencies]` with minimal features and justify every new dependency (license, size, maintenance) in the PR description.
7. File size and lint hygiene. Split `.rs` modules before they grow past roughly 600 to 800 lines to prevent new God files. Workspace lints deny `dbg_macro`, `todo`, and `unimplemented`, and warn on `large_futures` and `needless_collect`. Every new `#[allow(dead_code)]` or `#[allow(clippy::...)]` requires an inline justification comment.
8. Offline cross-platform tests. Every bug fix requires a regression test that fails without the fix. Never touch the real network, real user accounts, real keyring, or `$HOME` (use `tempfile` and `Paths::init(Paths::under(tmp))`). Keep every change building on Linux, macOS, and Windows CI.

## On-demand documentation

Load only the document that matches your task:

| Document | Read when changing |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crates, layering, `irori-core`, `irori-services`, stores, shell, sign-in, engines, launchers, Steam, mods, ReShade/RenoDX/OptiScaler, Xbox, store sustainability policy, Rust/test/PR rules, and platform pitfalls |
| [docs/UI_CONVENTIONS.md](docs/UI_CONVENTIONS.md) | GPUI / `gpui-kit = "0.8.0"` views, models, layout, themes, translucency, accessibility, tooltips, i18n, Big Picture, controllers, in-game overlay, and UI tests |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | Virtualization, LRU GPU and disk art caches, startup budget, runtime concurrency, build profiles, profiling with `--profile profiling`, and benchmarks |
| [docs/PLATFORMS.md](docs/PLATFORMS.md) | OS paths, desktop integration, Wayland/X11/macOS/Windows quirks, translucency matrix, sandboxing, and packaging |
| [docs/RELEASING.md](docs/RELEASING.md) | Release workflow, `SHA256SUMS`, SLSA attestations, and package channels (AUR, Homebrew, Scoop, winget, COPR, Flathub) |
| [SECURITY.md](SECURITY.md) | Threat model, credential storage, `#![forbid(unsafe_code)]` crates, `fs_guard`, script consent, and external network domains |

## Branches, commits, and pull requests

Enforced by `.github/workflows/pr-rules.yml` and `.github/scripts/pr_rules.py`:

- Name branches `<type>/<short-kebab-description>` matching the PR title `<type>(<scope>): <summary>` or `<type>: <summary>` (max 72 chars, imperative lower-case summary, no trailing period, matching type). Valid types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `i18n`, `build`, `ci`, `chore`, `revert`.
- Target `main` (or another convention-named branch for stacked work). Never commit to `main` directly.
- Start every PR body with a plain-language summary for end users, followed by technical details, verification commands run, and any platforms or screens not verified directly.
- Never claim to have visually checked a UI screen you did not render. Use `IRORI_HOME=<scratch>` when running manual experiments.
