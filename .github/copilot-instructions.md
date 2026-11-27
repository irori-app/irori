# GitHub Copilot Instructions

Follow [AGENTS.md](../AGENTS.md) as the authoritative engineering contract for Irori before proposing or writing any code.

## Required reading order

1. Read [AGENTS.md](../AGENTS.md) for the Click-and-Play product vision, tie-breaker order, automation safety boundary, 1-Click UX Litmus Test, verification commands, architectural invariants, and the mandatory Code Efficiency and Transparency Contract.
2. Load only the domain documentation needed for the current change:
   - [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) and [docs/GRAPHS.md](../docs/GRAPHS.md) for crate boundaries, backend services, stores, launch pipelines, engines, mods, submodule splits, and Mermaid system diagrams.
   - [docs/UI_CONVENTIONS.md](../docs/UI_CONVENTIONS.md) and [docs/TRANSLATING.md](../docs/TRANSLATING.md) for GPUI / `gpui-kit = "0.8.0"` rules, themes, layout, accessibility, `en.json`-first i18n, Big Picture, and overlay behavior.
   - [docs/PERFORMANCE.md](../docs/PERFORMANCE.md) for virtualization, GPU/disk cache bounds, startup budgets, and profiling.
   - [docs/PLATFORMS.md](../docs/PLATFORMS.md), [docs/FEATURES.md](../docs/FEATURES.md), [docs/RELEASING.md](../docs/RELEASING.md), and [docs/RELEASE_QA.md](../docs/RELEASE_QA.md) for OS integration, user-facing feature docs, packaging, release pipelines, and release QA checklists.
   - [docs/PROMPTING_PLAYBOOK.md](../docs/PROMPTING_PLAYBOOK.md) for context-window hygiene, model routing, XML prompt templates, and diff audits.
   - [SECURITY.md](../SECURITY.md) whenever adding external domains, paths outside `Paths`, or spawned helpers.
3. Load the matching task skill when executing common workflows:
   - [irori-submodule-refactor](../.agents/skills/irori-submodule-refactor/SKILL.md) for splitting `.rs` modules over 600 to 800 lines.
   - [irori-gpui-ui](../.agents/skills/irori-gpui-ui/SKILL.md) for GPUI views, dialogs, controller focus, and UI tests.
   - [irori-i18n](../.agents/skills/irori-i18n/SKILL.md) for `en.json` keys and `{placeholder}` checks.
   - [irori-store-protocol](../.agents/skills/irori-store-protocol/SKILL.md) for `crates/store-*`, protocol crates, `irori-secrets`, and `TestServer` suites.
   - [irori-runtime-launch](../.agents/skills/irori-runtime-launch/SKILL.md) for Wine/Proton/umu launch env, bubblewrap sandboxing, emulators, and Lutris scripts.
   - [irori-pr-checks](../.agents/skills/irori-pr-checks/SKILL.md) for pre-PR checks, Issues, and plain-language-first PR summaries.

## Non-negotiable checks before finishing

- Run `cargo fmt --all -- --check`.
- Run `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings`.
- Run `cargo test --locked --workspace --no-fail-fast --profile ci` (with `xvfb-run -a` on headless Linux).
- Run `python3 -m unittest discover -s .github/scripts -p "test_*.py"` and `python3 .github/scripts/check_agent_docs.py`.
- Validate PR naming, body (`## Plain-language summary` before `## Technical details`), and Rust diff size with `python3 .github/scripts/pr_rules.py --title "..." --head "..." --base-sha origin/main --head-sha HEAD --body-file <file>`.
- Keep changes offline-tested, panic-free in production paths, and compatible with Linux, macOS, and Windows.
