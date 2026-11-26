---
name: irori-i18n
description: Add or update English translation keys in crates/<crate>/locales/en.json with intent-based namespaces, matching {placeholder} variables, runtime fallback, and Weblate compatibility.
---

# Localization and Translation Catalogs (en.json-first)

Use this skill whenever adding, changing, or removing user-facing UI strings across any of Irori's seven localized crates (`docs/TRANSLATING.md` and `crates/ui/src/i18n.rs`).

## Catalog ownership

| Crate directory | Cargo package | Namespace prefix | Source catalog |
| --- | --- | --- | --- |
| `crates/ui` | `irori-ui` | `ui.` | `crates/ui/locales/en.json` |
| `crates/irori` | `irori` | `shell.` | `crates/irori/locales/en.json` |
| `crates/feature-library` | `irori-library` | `library.` | `crates/feature-library/locales/en.json` |
| `crates/feature-game` | `irori-game` | `game.` | `crates/feature-game/locales/en.json` |
| `crates/feature-settings` | `irori-settings` | `settings.` | `crates/feature-settings/locales/en.json` |
| `crates/feature-manage` | `irori-manage` | `manage.` | `crates/feature-manage/locales/en.json` |
| `crates/feature-bigpicture` | `irori-bigpicture` | `bigpicture.` | `crates/feature-bigpicture/locales/en.json` |

## Core rules

- Update `crates/<crate>/locales/en.json` in every code PR that adds, rewords, or deletes user-facing copy. Updating the other 18 `<lang>.json` catalogs in code PRs is optional because `crates/ui/src/i18n.rs` falls back to `en.json` per key at runtime and non-English translations flow through Hosted Weblate or dedicated `i18n` PRs.
- If you remove or rename a key in `en.json`, also remove that obsolete key from any `<lang>.json` files that still contain it! Every crate's catalog tests reject orphan keys present in a non-English catalog when absent from `en.json`.
- If you change the `{placeholder}` names of an existing key in `en.json`, either remove the outdated entry from non-English catalogs (so they cleanly fall back to English until re-translated) or update its `{placeholder}` variables across those catalogs so placeholder parity tests pass.
- Name keys by UI intent (`game.uninstall.confirm_action`, `manage.downloads.queued_count_one`, `manage.downloads.queued_count_other`), never by raw English wording.
- Write complete sentences with lowercase snake_case `{placeholder}` variables (`[a-z_]+`) via `t("...")` and `t_args("...", &[("title", &title)])`. Never assemble sentences by concatenating translated fragments.
- Use sentence case, active verb labels on buttons ("Verify files", "Remove from library"), and no trailing periods on titles or button labels. Never translate brand or tool names (Steam, GOG, Epic Games, Wine, Proton, DXVK, RetroArch).
- Keep every catalog a flat UTF-8 JSON object with 2-space indentation and a trailing newline (compatible with Weblate's `crates/(?P<component>[^/]+)/locales/(?P<language>[^/]+)\.json` discovery mask in `docs/TRANSLATING.md`). Never nest JSON objects or add unregistered `<lang>.json` files without updating `LANGUAGES` in `crates/ui/src/i18n.rs`.
- In both `crates/feature-manage` (`crates/feature-manage/src/i18n.rs`) and `crates/feature-bigpicture` (`crates/feature-bigpicture/src/lib.rs`), catalog tests also reject unused keys in `en.json`, so delete unreferenced `manage.*` and `bigpicture.*` keys immediately.

## Verification

Run the touched crate's catalog tests (`-- i18n` in `irori-ui`, `irori-library`, `irori-game`, `irori-settings`, and `irori-manage`; `-- strings` in `irori`; `-- catalog_tests` in `irori-bigpicture`):

```bash
cargo test --locked -p <package> --profile ci
```

## Few-shot examples

<example>
<input>
Task: Add a confirmation dialog title, description, and pluralized file count to `crates/feature-game` when clearing shader cache files for a game.
</input>
<output>
1. Add flat intent-based entries to `crates/feature-game/locales/en.json`:
   ```json
   "game.shader_cache.clear_action": "Clear cache",
   "game.shader_cache.clear_body_one": "Remove {count} cached shader file for \"{title}\"? The game will rebuild shaders on next launch.",
   "game.shader_cache.clear_body_other": "Remove {count} cached shader files for \"{title}\"? The game will rebuild shaders on next launch.",
   "game.shader_cache.clear_title": "Clear shader cache for \"{title}\"?"
   ```
2. Reference the keys in Rust without fragment concatenation:
   ```rust
   let count_str = file_count.to_string();
   let body_key = if file_count == 1 {
       "game.shader_cache.clear_body_one"
   } else {
       "game.shader_cache.clear_body_other"
   };
   let body = t_args(body_key, &[("count", &count_str), ("title", &game.title)]);
   ```
3. Run `cargo test --locked -p irori-game --profile ci -- i18n` to verify JSON syntax, namespace prefix, key usage, and placeholder parity.
</output>
</example>

<example>
<input>
Task: Rename an existing key `manage.updates.empty` to `manage.downloads.no_updates` in `crates/feature-manage` after unifying Updates into Downloads.
</input>
<output>
1. Replace `"manage.updates.empty"` with `"manage.downloads.no_updates"` in `crates/feature-manage/locales/en.json` and in `crates/feature-manage/src/`.
2. Remove `"manage.updates.empty"` from the 18 non-English `crates/feature-manage/locales/*.json` files (or rename the key if the wording and placeholders are unchanged) so no non-English catalog retains an orphan key missing from `en.json`.
3. Run `cargo test --locked -p irori-manage --profile ci -- i18n` to confirm zero orphan or unknown keys.
</output>
</example>
