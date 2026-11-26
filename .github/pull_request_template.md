<!--
Title: <type>(<scope>): <summary>, the same type as the branch (<type>/<kebab-name>).
Types: feat, fix, perf, refactor, docs, test, i18n, build, ci, chore, revert.
The "PR rules" check enforces this; see AGENTS.md, "Branches, commits and pull requests".
-->

## Plain-language summary

<!-- Start with 2 to 4 plain sentences explaining what changed for end users, without code jargon. Link issues: "Fixes #123". -->

## Technical details

<!-- Why this approach, touched crates/modules, and edge cases considered. -->

## How it was checked

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --locked --workspace --all-targets -- -D warnings`
- [ ] `cargo test --locked --workspace` (or the crates touched: `-p ...`)
- [ ] Ran Irori and exercised the change: <!-- what you did, on which OS -->
- [ ] Packaging tests, if `packaging/` changed: `python3 -m unittest discover -s packaging -p "test_*.py"`

Platforms tried: <!-- Linux (Wayland/X11) / macOS / Windows; say which were not -->

## Checklist

- [ ] Included a plain-language summary at the top explaining what changes for users
- [ ] Attached before-and-after light and dark screenshots (or a short recording) if UI or UX changed
- [ ] New or changed user-facing text is in `crates/<crate>/locales/en.json` (the other 18 locales are optional in code PRs and fall back cleanly to English at runtime)
- [ ] Disclosed significant AI assistance (if any) and verified the change manually on real hardware
- [ ] Checked runtime efficiency (no polling loops, no blocking I/O on the UI thread, bounded caches, no unjustified new dependencies)
- [ ] Updated `SECURITY.md` (if new network endpoints or subprocesses were added), `THIRD-PARTY-NOTICES.md` (if third-party code or assets were added), and `website/` (if install channels, license, or benchmarks changed)
- [ ] Docs updated where behaviour changed (`README.md`, `docs/`, `AGENTS.md`)
- [ ] No secrets, tokens or real account data in code, fixtures or logs
- [ ] Breaking change (settings, on-disk data, CLI) marked with `!` and described above

## Screenshots

<!-- Required whenever UI or UX changes: attach before and after screenshots in both light and dark themes. Delete this section only for non-UI PRs. -->
