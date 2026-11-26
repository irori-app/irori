# Translating Irori

Irori stores interface strings as flat JSON objects embedded into the binary at build time with `include_str!`. English (`en.json`) is the source catalog and the runtime fallback whenever another locale lacks a key.

Irori ships 19 interface languages registered in `crates/ui/src/i18n.rs`: `en`, `cs`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `nl`, `pl`, `pt`, `pt_BR`, `ro`, `ru`, `sv`, `tr`, `uk`, `zh_Hans`, and `zh_Hant`.

## Translation catalogs

Each UI crate owns its own `locales/` directory and key namespace so catalogs never collide:

| Component | Key namespace | Source file | Translation mask |
| --- | --- | --- | --- |
| `ui` | `ui.` | `crates/ui/locales/en.json` | `crates/ui/locales/*.json` |
| `irori` | `shell.` | `crates/irori/locales/en.json` | `crates/irori/locales/*.json` |
| `feature-library` | `library.` | `crates/feature-library/locales/en.json` | `crates/feature-library/locales/*.json` |
| `feature-game` | `game.` | `crates/feature-game/locales/en.json` | `crates/feature-game/locales/*.json` |
| `feature-settings` | `settings.` | `crates/feature-settings/locales/en.json` | `crates/feature-settings/locales/*.json` |
| `feature-manage` | `manage.` | `crates/feature-manage/locales/en.json` | `crates/feature-manage/locales/*.json` |
| `feature-bigpicture` | `bigpicture.` | `crates/feature-bigpicture/locales/en.json` | `crates/feature-bigpicture/locales/*.json` |

## Guidelines for translators

- Preserve placeholders inside curly braces (such as `{title}`, `{count}`, `{store}`, or `{prefix}`) verbatim without translating or renaming the identifier.
- Partial translations are safe. Missing keys fall back to the English string automatically at runtime instead of failing builds or showing raw key names.
- Keep each file as a flat UTF-8 JSON object with 2-space indentation, sorted or source-aligned keys, and a trailing newline. Never nest JSON objects inside translation catalogs.
- Translate complete natural sentences rather than assuming fragment concatenation, and keep button and label wording concise so controls fit without wrapping or clipping.
- Adding a brand-new language requires code changes in Rust: the language must be added to `LANGUAGES` in `crates/ui/src/i18n.rs`, and its `<lang>.json` file must be embedded in all seven crates above. Open an issue before creating a new locale file.

## Hosted Weblate setup (maintainers)

Once Irori completes its relicensing to `GPL-3.0-or-later` (`#214`), the repository qualifies for the Hosted Weblate Libre Plan (`hosted.weblate.org`).

### Component Discovery addon

Configure one parent component (or use Weblate's Component Discovery addon) with these exact settings so all seven crates stay synced automatically:

- File format: `json` (JSON flat key-value format)
- Match regular expression: `crates/(?P<component>[^/]+)/locales/(?P<language>[^/]+)\.json`
- Base file template: `crates/{{ component }}/locales/en.json`
- Component name template: `{{ component }}`
- Component slug template: `{{ component }}`

### Language mapping and new languages

- Configure Weblate's language code mappings so regional and script codes keep underscores instead of hyphens: `pt_BR` (not `pt-BR`), `zh_Hans` (not `zh-CN` or `zh-Hans` with hyphen), and `zh_Hant` (not `zh-TW` or `zh-Hant` with hyphen).
- Set `new_lang` to `contact` ("Contact maintainers") so Weblate does not open pull requests adding `.json` files that are not yet registered in `crates/ui/src/i18n.rs`.

### Pull request and quality checks

Irori's `.github/scripts/pr_rules.py` check validates branch prefixes, pull request titles, and length limits. Configure Weblate's GitHub VCS / Pull Request settings to match:

- Push branch: `i18n/weblate-translations`
- Commit message and PR title: `i18n: update translations from Weblate`
- Enable commit squashing per pull request so Weblate produces a single clean commit.
- Enable the brace placeholder check (`{placeholder}`) so missing or renamed variables block broken strings before merge.
