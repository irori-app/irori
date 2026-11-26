# GitHub Copilot Instructions

Follow [AGENTS.md](../AGENTS.md) as the authoritative engineering contract for Irori before proposing or writing any code.

## Required reading order

1. Read [AGENTS.md](../AGENTS.md) for verification commands, architectural invariants, and the mandatory Code Efficiency and Transparency Contract.
2. Load only the domain documentation needed for the current change:
   - [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) for crate boundaries, backend services, stores, launch pipelines, engines, mods, and known pitfalls.
   - [docs/UI_CONVENTIONS.md](../docs/UI_CONVENTIONS.md) for GPUI / `gpui-kit = "0.8.0"` rules, themes, layout, accessibility, i18n, Big Picture, and overlay behavior.
   - [docs/PERFORMANCE.md](../docs/PERFORMANCE.md) for virtualization, GPU/disk cache bounds, startup budgets, and profiling.
   - [docs/PLATFORMS.md](../docs/PLATFORMS.md) and [docs/RELEASING.md](../docs/RELEASING.md) for OS integration, packaging, and release pipelines.
   - [SECURITY.md](../SECURITY.md) whenever adding external domains, paths outside `Paths`, or spawned helpers.

## Non-negotiable checks before finishing

- Run `cargo fmt --all -- --check`.
- Run `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings`.
- Run `cargo test --locked --workspace --no-fail-fast --profile ci` (with `xvfb-run -a` on headless Linux).
- Validate PR naming with `python3 .github/scripts/pr_rules.py --title "..." --head "..."`.
- Keep changes offline-tested, panic-free in production paths, and compatible with Linux, macOS, and Windows.
