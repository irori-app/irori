# AI Prompting and Context Playbook

Practical workflow for contributors using coding agents and LLMs (Antigravity, Codex, Claude Code, Cursor, Windsurf, Zed, GitHub Copilot) on Irori.

Language models behave like lossy compressed snapshots of older public codebases until grounded with exact repository files and verification tools. Irori uses Rust 2024 (`1.95+`), `gpui-kit = "0.8.0"`, custom protocol crates, and strict runtime budgets, so strong results come from tight context windows, explicit tool checks, structured XML prompts, and critical diff audits.

## 1. Context window discipline

Treat the context window as finite working memory. Extra irrelevant tokens dilute attention across distant invariants and slow down every turn.

- Start a fresh chat or agent session for every distinct branch, bug, or pull request. Never carry a finished store-protocol debugging thread into a GPUI layout task.
- Keep `AGENTS.md` as the compact entry point (enforced under 20 KB by `.github/scripts/check_agent_docs.py`), then load only the single document in `docs/` and the single skill in `.agents/skills/*/SKILL.md` that match your change.
- Never paste or read all 19 `locales/*.json` catalogs into context during feature work. Read and edit only `crates/<crate>/locales/en.json` unless fixing a specific non-English translation or removing a deleted key.
- When exploring crate dependencies or state flows, read [docs/GRAPHS.md](GRAPHS.md) first so you locate the exact target crate before opening large `.rs` files.
- If an exploratory debugging session accumulates dead ends, summarize the confirmed root cause and file paths in five lines, open a clean session, and execute the fix there.

## 2. Tool verification over model recall

Never rely on a model's pretrained memory for GPUI APIs, workspace crate names, or upstream store behavior:

- Grep actual dependency sources in `~/.cargo/registry/src/index.crates.io-*/{gpui-kit-0.8.0,gpui-component-0.8.0,gpui-base-0.8.0,gpui-pre-0.3.9}` before calling any GPUI or `gpui-kit` builder method.
- Check root `Cargo.toml` or `docs/ARCHITECTURE.md` before passing `-p <package>` (`crates/store-epic` is `irori-epic`; `crates/epic` is `irori-epic-api`).
- Use web search when checking upstream specifications that change outside the repo (Proton/umu environment variables, Steam Web API endpoints, Wayland protocols, Flatpak portals, or platform packaging rules).
- Run Cargo and Python checks directly instead of eyeballing generated Rust or Mermaid code.

## 3. Model routing and council review

Match model depth to task risk:

| Task profile | Recommended model tier | Examples in Irori |
| --- | --- | --- |
| Localized mechanical edits | Fast coding model | Adding `en.json` keys, straightforward GPUI spacing tweaks, splitting a module into `<module>.rs` + `<module>/*.rs` submodules (`irori-submodule-refactor`), writing repetitive test fixtures |
| Stateful UI or multi-crate features | Flagship coding model | New `crates/feature-*` sheets, `irori_ui::hooks` wiring, `SearchIndex` filters, store adapter methods with `TestServer` tests |
| Subtle concurrency, crypto, containment, or cross-OS bugs | Reasoning ("thinking") model or multi-model council | `irori-io` Tokio vs GPUI window lifecycle races, `parking_lot` lock ordering, `crates/runtime` Wine/Proton/umu/bubblewrap launch composition (load `.agents/skills/irori-runtime-launch/SKILL.md`), `crates/msixvc` XVD/AES-XTS math, `crates/scripts` confinement, `crates/core/src/fs_guard.rs`, Linux/macOS/Windows `#[cfg]` parity |

When stuck on a subtle bug or reviewing a high-risk security or concurrency change, run an independent second-model ("council") review: hand the exact diff and constraints to a second reasoning model in a clean session with zero prior bias and ask it to find failure modes.

## 4. Structured XML prompt templates

Delimit context, task scope, invariants, few-shot examples, and verification commands with explicit XML tags so the model separates hard constraints from background notes.

### Bug fix with offline regression test

```xml
<context>
Read AGENTS.md and crates/services/src/copies.rs.
Symptom: [exact reproduction steps, expected behavior, and observed behavior].
</context>

<task>
Identify the root cause and fix it with the smallest sound diff. Add an offline unit test that fails before the fix and passes with the fix.
</task>

<constraints>
- Zero `.unwrap()` or `.expect()` outside `#[cfg(test)]`.
- Filter `!game.is_dlc` if enumerating `store.games()`.
- Keep tests 100% offline using in-memory fixtures or `tempfile`.
</constraints>

<examples>
Follow existing offline unit tests in `crates/services/src/copies.rs` and the `<example>` patterns in `.agents/skills/irori-store-protocol/SKILL.md`.
</examples>

<verification>
Run `cargo clippy --locked -p irori-services --all-targets --profile ci -- -D warnings` and `cargo test --locked -p irori-services --profile ci`.
</verification>
```

### New GPUI view or dialog

```xml
<context>
Read AGENTS.md, docs/UI_CONVENTIONS.md, and .agents/skills/irori-gpui-ui/SKILL.md.
Target files: crates/feature-manage/src/wine/mod.rs and crates/feature-manage/locales/en.json.
</context>

<task>
Implement [specific UI control or sheet] wired to Backend via `irori_ui::run`.
</task>

<constraints>
- Grep `~/.cargo/registry/src/index.crates.io-*/gpui-component-0.8.0` to confirm component methods before use.
- Wrap dialog follow-up window updates in `cx.defer`, and pass `cx` as the second argument to `window.push_notification(note, cx)`.
- Use `cx.theme().*` tokens, `rem` sizing, `min_w_0()` + `truncate()` on text flex children, and `en.json` intent keys.
- Preserve keyboard and gamepad navigation (`cx.propagate()` at list edges; never bind `F24`).
</constraints>

<examples>
Follow the `<example>` patterns in `.agents/skills/irori-gpui-ui/SKILL.md` and `.agents/skills/irori-i18n/SKILL.md`.
</examples>

<verification>
Run `cargo clippy --locked -p irori-manage --all-targets --profile ci -- -D warnings` and `xvfb-run -a cargo test --locked -p irori-manage --profile ci`.
</verification>
```

### Splitting a large module into submodules

```xml
<context>
Read AGENTS.md, docs/GRAPHS.md (Graph 5), and .agents/skills/irori-submodule-refactor/SKILL.md.
Target file: crates/<crate>/src/<area>.rs.
</context>

<task>
Split `crates/<crate>/src/<area>.rs` into a thin `<area>.rs` facade and cohesive `crates/<crate>/src/<area>/*.rs` submodules with zero behavior changes and zero public API changes.
</task>

<constraints>
- Keep `crates/<crate>/src/<area>.rs` as the facade (`mod` declarations + `pub` / `pub(crate)` re-exports) instead of deleting `<area>.rs` for `mod.rs`.
- Preserve every `serde` attribute and every existing unit test (`#[cfg(test)] mod tests;`).
- Do not mix behavioral changes into this refactor PR.
</constraints>

<examples>
Follow the facade splits in `crates/core/src/settings.rs` + `crates/core/src/settings/`, `crates/feature-settings/src/form.rs` + `crates/feature-settings/src/form/`, and `.agents/skills/irori-submodule-refactor/SKILL.md`.
</examples>

<verification>
Run `cargo clippy --locked -p <package> --all-targets --profile ci -- -D warnings`, `xvfb-run -a cargo test --locked -p <package> --profile ci`, and `python3 .github/scripts/check_agent_docs.py`.
</verification>
```

### Adversarial diff review

```xml
<context>
Read AGENTS.md and inspect `git diff origin/main...HEAD`.
</context>

<task>
Review this diff like a skeptical maintainer trying to break it in production across Linux, macOS, and Windows. List concrete bugs, race conditions, UI-thread blocking calls, missing bounds, or stale doc references with file and line numbers.
</task>

<constraints>
- Check every item in Section 6 ("Junior-analyst diff audit") of docs/PROMPTING_PLAYBOOK.md.
- Report only verified defects or untested high-risk edge cases, ordered by severity.
</constraints>

<examples>
Format each finding as: `file:line`: input -> expected -> actual -> root cause, matching `.agents/skills/irori-pr-checks/SKILL.md`.
</examples>

<verification>
Run `python3 -m unittest discover -s .github/scripts -p "test_*.py"`, `python3 .github/scripts/check_agent_docs.py`, and targeted `cargo clippy` / `cargo test` commands on every touched crate.
</verification>
```

## 5. Multimodal screenshots and hybrid voice input

### Closing the blind-UI gap with `IRORI_HOME`

Automated tests verify widget trees without proving visual balance, contrast, or overflow. Whenever a desktop display is available and you touch UI layout or styling:

1. Build and launch against a scratch profile so your personal library and credentials stay untouched (or capture both themes in one headless Linux step with `website/scripts/screenshots/pr_shot.sh`):
   ```bash
   cargo build --locked --profile quick -p irori -p irori-sign-in
   IRORI_HOME="$(mktemp -d)" target/quick/irori
   # Headless Linux one-shot dark + light capture seeded with demo games:
   website/scripts/screenshots/pr_shot.sh target/quick/irori irori://library /tmp/pr-shots
   ```
2. Capture screenshots of the modified view in both Irori Night (dark) and Irori Day (light), plus narrow window widths for German or Russian text expansion.
3. Inspect screenshots with a multimodal model in two explicit steps:
   - First ask the model to transcribe every visible label, badge, button alignment, truncation ellipsis, and border boundary it sees in the image so you confirm it read the pixels accurately.
   - Then ask it to check against `docs/UI_CONVENTIONS.md` (WCAG contrast, neutral chrome, Linux CSD shadow margin, overflow clipping).

### Voice dictation paired with typed symbols

System-wide speech-to-text tools speed up architectural descriptions and PR plain-language summaries, while speech recognizers routinely mangle Rust identifiers. Dictate high-level intent and user flows by voice, then type exact crate names, file paths, and identifiers (`GameSettingsOverrides`, `enviromentOptions`, `irori_ui::run`, `fs_guard::ensure_deletable`). If you bind a desktop push-to-talk shortcut, never use `F24` while testing Irori (`crates/feature-bigpicture/src/gamepad/pointer.rs` synthesizes `F24` to hide the pointer).

## 6. Junior-analyst diff audit (9 silent LLM traps in Irori)

Coding agents routinely make silent assumptions to finish quickly. Audit every generated diff against these nine traps before committing:

1. Blocking work on the GPUI thread or inside `Render::render`: check that zero `std::fs`, SQLite, keyring, subprocess, `Regex::new`, or full-library sort/clone calls landed in UI callbacks or `render()`. Route async work through `irori_ui::run` or `backend.spawn_blocking`.
2. Re-entrant window updates and premature library events: check that dialog callbacks use `cx.defer` before updating the same window, and check that UI code listens for `Event::LibraryUpdated` rather than `Event::LibraryChanged`.
3. Hallucinated `gpui-kit` APIs or broken `Cargo.lock` features: check that `[dev-dependencies]` uses `gpui = { package = "gpui-pre", version = "=0.3.9", features = ["test-support"] }` and never enables `gpui-kit`'s `test-support` feature.
4. Unfiltered DLC records or single-release assumptions: check that callers of `store.games()` filter `!game.is_dlc` before counting or displaying games, and that Epic release logic inspects all `releaseInfo` entries.
5. Renamed settings keys or unguarded recursive deletes: check that Heroic-compatible camelCase keys in `crates/core/src/settings.rs` (including `enviromentOptions`) were never "corrected", and every recursive delete calls `fs_guard::ensure_deletable`.
6. Panics, lock guards across `.await`, or unbounded I/O: check for `.unwrap()`, `.expect()`, or slice indexing `[i]` outside `#[cfg(test)]`, `parking_lot` guards held across `.await`, missing HTTP timeouts, or missing `MAX_*_BYTES` caps.
7. Machine-translated locale churn or broken `{placeholder}` names: check that code PRs cleanly update `crates/<crate>/locales/en.json` with matching `{placeholder}` variables and zero orphan keys in other catalogs rather than spraying unreviewed text across 18 languages.
8. Cross-OS `#[cfg]` drift and stale doc paths: check that `#[cfg(target_os = "...")]` changes leave no unused imports or missing methods on Linux, macOS, or Windows, and run `python3 .github/scripts/check_agent_docs.py` whenever modules or crates move.
9. Click-and-Play UX regression or silent unsafe automation: check that new flows pass the 1-Click UX Litmus Test in `AGENTS.md` (safe smart defaults instead of manual technical fields or multi-step wizards, expert controls behind progressive disclosure, plain-language errors with a direct 1-click recovery button) and never run destructive actions, external scripts, or heavy downloads silently.
