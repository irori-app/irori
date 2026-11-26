---
name: irori-pr-checks
description: Validate branch naming, PR titles, diff budgets, agent documentation sync, formatting, clippy lints, offline tests, cross-OS cfg gates, and honest PR descriptions before opening or updating a pull request.
---

# Pull Request Verification and Hygiene

Use this skill before committing, pushing, or opening a pull request against `irori-app/irori`.

## Automated verification sequence

Run the checks matching your change from the repository root:

```bash
# 1. Branch, title, base, and Rust diff-size conventions (.github/workflows/pr-rules.yml)
python3 .github/scripts/pr_rules.py \
  --title "<type>(<scope>): <imperative lower-case summary>" \
  --head "<type>/<short-kebab-name>" \
  --base "main" \
  --base-sha "$(git merge-base origin/main HEAD)" \
  --head-sha HEAD

# 2. PR rule tests and agent documentation/graph sync
python3 -m unittest discover -s .github/scripts -p "test_*.py"
python3 .github/scripts/check_agent_docs.py

# 3. Packaging scripts and dependency policy (when packaging/, Cargo.toml, Cargo.lock, or deny.toml change)
python3 -m unittest discover -s packaging -p "test_*.py"
cargo deny check

# 4. Rust formatting, workspace clippy guardrails, and offline test suite
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings
xvfb-run -a cargo test --locked --workspace --no-fail-fast --profile ci
```

For fast local manual runs on Linux, build both binaries with the same profile so store sign-in uses the helper window instead of falling back to the browser:

```bash
cargo build --locked --profile quick -p irori -p irori-sign-in
```

## Pre-PR audit rules

- Match branch prefix and Conventional Commit PR title `<type>` (`feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `i18n`, `build`, `ci`, `chore`, `revert`), keep the title at 72 characters or fewer, start the summary in lower case (except leading acronyms like `GOG` or `UI`), and omit trailing periods. Never use agent branch prefixes such as `claude/` or `codex/`.
- Keep pull requests focused and small. `.github/scripts/pr_rules.py` rejects pull requests adding more than 1,500 lines across `*.rs` files unless split into smaller stacked PRs or labeled `large-pr-approved`.
- Audit every `#[cfg(target_os = "...")]` block so imports, helper functions, and struct fields compile cleanly across Linux, macOS, and Windows without dead-code or unused-import warnings (`-D warnings`).
- Check that new or changed UI text exists in `crates/<crate>/locales/en.json` and that `SECURITY.md` reflects any new external domain or helper process.
- Write the PR description using `.github/pull_request_template.md`: open with a plain-language summary for end users (2 to 4 clear sentences without code jargon), follow with technical details and exact verification commands run, and state honestly which platforms or themes were not visually exercised. Never claim to have inspected a screen you did not render.

## Few-shot examples

<example>
<input>
Task: Prepare branch name, PR title, and verification commands for fixing an Epic DLC playtime double-count in `crates/services` and `crates/feature-library`.
</input>
<output>
- Branch: `fix/epic-dlc-playtime`
- PR title: `fix(stats): exclude DLC entries from store playtime totals` (58 chars, matches `fix/`)
- Verification commands:
  1. `python3 .github/scripts/pr_rules.py --title "fix(stats): exclude DLC entries from store playtime totals" --head "fix/epic-dlc-playtime" --base "main"`
  2. `python3 .github/scripts/check_agent_docs.py`
  3. `cargo fmt --all -- --check`
  4. `cargo clippy --locked -p irori-services -p irori-library --all-targets --profile ci -- -D warnings`
  5. `xvfb-run -a cargo test --locked -p irori-services -p irori-library --profile ci`
</output>
</example>

<example>
<input>
Task: Write an honest `Plain-language summary` and `How it was checked` section for a headless Linux UI bug fix where macOS, Windows, and live desktop screenshots were not run manually.
</input>
<output>
```markdown
## Plain-language summary

Fixes an issue where scrolling the library list left the Filters popover open at its old position. The Filters popover now closes automatically when you scroll the game list and sizes itself to stay inside the window.

## Technical details

Updated `crates/feature-library/src/filters_panel.rs` and `crates/feature-library/src/view.rs` to measure trigger bounds via `style::visible_bounds` and dismiss the popover on virtual-list scroll events, with a GPUI regression test in `crates/feature-library/src/ui_tests.rs`.

## How it was checked

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --locked --workspace --all-targets --profile ci -- -D warnings`
- [x] `xvfb-run -a cargo test --locked -p irori-library --profile ci`
- [ ] Ran Irori interactively on a desktop display (verified via headless `xvfb-run` GPUI test only; did not capture manual light/dark screenshots)

Platforms tried: Headless Linux (`xvfb-run`); not tested manually on Wayland/X11 desktop, macOS, or Windows.
```
</output>
</example>
