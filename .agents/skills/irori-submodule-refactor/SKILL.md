---
name: irori-submodule-refactor
description: Split large Rust modules (over 600 to 800 lines) into a thin <module>.rs facade and focused <module>/*.rs submodules without changing crate APIs or breaking doc paths.
---

# Splitting Large Rust Modules into Submodules

Use this skill whenever refactoring a growing `.rs` file (above roughly 600 to 800 lines) or decomposing a module ahead of adding new behavior.

## Core rules

- Prefer the Rust 2018+ sibling facade layout: keep `crates/<crate>/src/<name>.rs` as a thin entry point and place focused child modules inside `crates/<crate>/src/<name>/*.rs` (as done in `crates/core/src/settings.rs` + `crates/core/src/settings/`, `crates/feature-settings/src/form.rs` + `crates/feature-settings/src/form/`, `crates/feature-library/src/view.rs` + `crates/feature-library/src/view/`, and `crates/feature-settings/src/hud/panel.rs` + `crates/feature-settings/src/hud/panel/`).
- Avoid deleting `<name>.rs` to create `<name>/mod.rs` unless the folder already uses `mod.rs` (such as `crates/feature-settings/src/registry/mod.rs` or `crates/extras/src/discounts/mod.rs`). Preserving `<name>.rs` keeps git blame intact and keeps existing documentation paths valid.
- Preserve the exact `pub` and `pub(crate)` surface re-exported by `<name>.rs` so callers in sibling modules and downstream crates require zero edits.
- Use `pub(super)` (or `pub(crate)` only when needed outside `<name>/`) for helpers and fields shared across `<name>/*.rs`.
- Move inline `#[cfg(test)] mod tests { ... }` into `crates/<crate>/src/<name>/tests.rs` and declare `#[cfg(test)] mod tests;` in `<name>.rs`. Never delete, weaken, or skip existing assertions during a structural split.
- Keep pure structural splits in their own pull request (`refactor(<scope>): split <file> into submodules`) without mixing behavior changes. When a single file exceeds 1,500 lines, split it incrementally or coordinate the `large-pr-approved` label required by `.github/scripts/pr_rules.py`.
- Run `python3 .github/scripts/check_agent_docs.py` after moving or renaming any file so stale paths in `AGENTS.md`, `docs/`, or `.agents/skills/` fail fast locally instead of in CI.

## Recommended slice boundaries

### GPUI views and dialogs

- `<name>.rs` holds module docs, `mod` declarations, re-exports, the view struct definition, and `EventEmitter` / top-level `Render` wiring.
- `<name>/actions.rs` holds command handlers, async `irori_ui::run` tasks, and mutations.
- `<name>/render.rs` or `<name>/sections.rs` holds layout helpers returning `impl IntoElement` or `AnyElement`.
- `<name>/helpers.rs` holds pure state calculations, formatting, and width/layout math.
- `<name>/tests.rs` holds unit tests over the pure helpers.

### Domain schemas and backend services

- `<name>.rs` holds `mod` declarations and `pub use` re-exports of the public types and functions.
- Group submodules by domain concern (for example `types.rs`, `options.rs`, `overrides.rs`, `tests.rs`) rather than arbitrary line counts.
- Keep `serde` attributes (`#[serde(rename_all = "camelCase")]`, `#[serde(default)]`, and Heroic-compatible field spellings such as `enviromentOptions`) byte-for-byte identical across the move.

## Few-shot examples

<example>
<input>
Task: Split a 1,900-line GPUI panel `crates/feature-settings/src/hud/panel.rs` into cohesive submodules without changing callers in `crates/feature-settings/src/hud/mod.rs` or `crates/feature-settings/src/view.rs`.
</input>
<output>
1. Keep `crates/feature-settings/src/hud/panel.rs` as the entry facade declaring:
   ```rust
   mod helpers;
   mod labels;
   mod sections;
   mod sections_layout;
   mod sections_metrics;
   #[cfg(test)]
   mod tests;
   ```
2. Keep `pub enum HudPanelScope` and `pub struct HudPanel` exported from `panel.rs`; mark fields accessed by `sections*.rs` as `pub(super)`.
3. Move pure preset/config helpers into `panel/helpers.rs`, cached `SharedString` label builders into `panel/labels.rs`, section renderers into `panel/sections*.rs`, and existing unit tests into `panel/tests.rs`.
4. Verify with:
   - `cargo clippy --locked -p irori-settings --all-targets --profile ci -- -D warnings`
   - `xvfb-run -a cargo test --locked -p irori-settings --profile ci`
   - `python3 .github/scripts/check_agent_docs.py`
   - `python3 .github/scripts/pr_rules.py --title "refactor(settings): split hud/panel.rs into submodules" --head "refactor/split-hud-panel"`
</output>
</example>

<example>
<input>
Task: Split `crates/core/src/settings.rs` into domain submodules while keeping every `irori_core::settings::*` re-export, Heroic camelCase `serde` key, and JSON round-trip test intact.
</input>
<output>
1. Keep `crates/core/src/settings.rs` as the thin facade with shared private `serde` default helpers, `mod` declarations (`app`, `game`, `overrides`, `prefs`, `ui`, and `#[cfg(test)] mod tests`), and `pub use` re-exports.
2. Place cohesive domain schemas into `crates/core/src/settings/app.rs` (`AppSettings`), `crates/core/src/settings/game.rs` (`GameSettings`), `crates/core/src/settings/overrides.rs` (`GameSettingsOverrides`), `crates/core/src/settings/prefs.rs`, `crates/core/src/settings/ui.rs`, and `crates/core/src/settings/tests.rs`.
3. Keep every `#[serde(rename = "...")]`, `#[serde(default)]`, and Heroic spelling (`enviromentOptions`) untouched, then run `cargo clippy --locked -p irori-core --all-targets --profile ci -- -D warnings`, `cargo test --locked -p irori-core --profile ci`, and `python3 .github/scripts/check_agent_docs.py`.
</output>
</example>
