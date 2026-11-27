# UI Conventions

Reference for `irori-ui`, the five feature crates (`irori-library`, `irori-game`, `irori-manage`, `irori-settings`, `irori-bigpicture`), and the application shell (`crates/irori`).

## UX north star: Click-and-Play first

Every screen should pass the 1-Click UX Litmus Test in `AGENTS.md` and uphold Irori's tie-breaker order (correctness and user-data safety > lightweight speed > 1-click simplicity > maximum features under progressive disclosure):

- Put one clear 1-click primary action front and center ("Play", "Install", "Fix", "Sign in"). Avoid multi-step modal wizards whenever a single smart-default sheet or inline action solves the task.
- Pre-fill safe defaults automatically (best already-installed runtime, default prefix path, detected emulators/controllers, and a drive with free space) so most users never have to open advanced settings.
- Keep power-user controls under progressive disclosure (collapsible sections, settings search, and per-game overrides with 1-click reset) so broad features never clutter the main surface.
- Respect the automation boundary in UI flows: never trigger destructive actions, third-party scripts (`Approvals`), or heavy downloads silently; present an explicit, clear 1-click confirmation or install button instead.
- Pair every failure or empty state with plain-language guidance and a direct 1-click recovery button (retry, sign in again, install missing runtime, open the exact setting) rather than raw technical dumps or passive toasts.
- Keep surfaces clean, modern, and responsive in both light and dark themes, and preserve the same 1-click simplicity across desktop windows and Big Picture with controllers.

## GPUI and GPUI Kit APIs

- Use `gpui-kit = "0.8.0"` as the sole UI library: `use gpui_kit::*` for GPUI, `gpui_kit::component::*` for components, and `gpui_kit::assets::IconName` for icons (`Icon::new(IconName::...)`).
- Never invent a GPUI or GPUI Kit method or component. Grep the exact crate sources compiled into the workspace at `~/.cargo/registry/src/index.crates.io-*/{gpui-kit-0.8.0,gpui-component-0.8.0,gpui-base-0.8.0,gpui-pre-0.3.9}`.
- Initialize `irori_ui::init(backend, cx)` only after `gpui_kit::init`; it installs i18n, `AppState`, theme, art, navigation, and models.

### Calling the backend from views

Never block the UI thread and never await a backend future directly outside `irori_ui::run` (backend futures require the `irori-io` Tokio runtime):

```rust
let task = irori_ui::run(cx, async move { backend.launch(&id, None, vec![]).await });
cx.spawn_in(window, async move |this, cx| {
    let result = task.await;
    let _ = this.update_in(cx, |this, window, cx| {
        // Update view state; report errors with a notification or an inline Alert.
        cx.notify();
    });
})
.detach();
```

- Use `irori_ui::run_infallible` for `Task<Option<T>>` and `irori_ui::backend(cx)` to clone `Backend`.
- `WindowHandle::update` fails if invoked re-entrantly inside that window's own update, and discarding the `Result` hides the failure. Dialog callbacks run inside the window update, so defer updates targeting that window with `cx.defer`.

## Reactive models and navigation

### Models (`crates/ui/src/models.rs`)

- Global models accessed through `models(cx)`: `LibraryModel`, `StatusModel`, `DownloadsModel`, `ComponentsModel`, `AccountsModel`, `SettingsModel`, `NoticeModel`, and `SessionModel`.
- One background event pump drains `EventBus` in batches and notifies each affected model once.
- Wait for `LibraryUpdated` (after snapshot rebuild), never `LibraryChanged` (raw store signal before rebuild), in UI code.
- Observe only the models a view actually renders and store returned `Subscription` handles in `_subscriptions: Vec<Subscription>`.

### Navigation and cross-feature hooks (`crates/ui/src/nav.rs`, `crates/ui/src/hooks.rs`)

- `Route` variants: `Library`, `Game(id)` (`crates/feature-game/src/page/`), `Downloads` (unified queue, app self-update banner, game updates, component updates, and history in `crates/feature-manage/src/downloads/mod.rs` and `crates/feature-manage/src/downloads/updates.rs`), `Settings(section)`, `GameSettings(id, section)`, `WineManager` (`crates/feature-manage/src/wine/`), `Emulators` (`crates/feature-manage/src/emulators/` and `crates/feature-manage/src/emulators/cores.rs`), `Accounts`, `Stores` (store cards and ProtonDB/GG.deals views in `crates/feature-manage/src/stores.rs` and `crates/feature-manage/src/deals.rs`), `Stats`, `YearInReview(year)`, `BigPicture`, and `Onboarding`.
- Navigate with `navigate(route, cx)`, `go_back`, and `go_forward`. The shell decides which pages to cache.
- Feature crates stay decoupled from each other (except `irori-settings` reading `irori_bigpicture::gamepad` and `irori_bigpicture::overlay::support` for controller mapping and overlay support tables) and depend on concrete store crates only where a dedicated screen requires a store's own API. The shell routes between features via events (`LibraryEvent`, `GamePageEvent`) and registers hooks in `irori_ui::hooks` (`game_panel` for `GamePage::embedded`, `game_settings` for `GameSettingsView::embedded`, `game_setup` for `irori_settings::setup_card`, `uninstall`, `play_game`, `install_game`, `open_store_page`, `show_unlocks`, `found_sources`) plus `irori_game::install_hooks`.
- A game's settings open on the "Changed" section (`changed`) listing only fields that differ from global defaults, each with an inline reset. Settings field specs live in `crates/feature-settings/src/registry/`, form controls and actions in `crates/feature-settings/src/form.rs` + `crates/feature-settings/src/form/`, HUD controls in `crates/feature-settings/src/hud/panel.rs` + `crates/feature-settings/src/hud/panel/`, app update controls in `crates/feature-settings/src/app_update.rs`, and crash review sheets in `crates/feature-settings/src/crash_report.rs`.

### Feature crate contract

- Each feature crate exposes `pub fn init(cx)`, `*View` entities (`impl Render`, `pub fn new(..., window, cx)`), `open_*_dialog` and `open_*_sheet` helpers, `*Event` enums, and `pub use i18n::CATALOGS`.
- Declare key contexts as `*_CONTEXT` constants and bind actions once per crate.

## Visual language, themes, and accessibility

### Tokens and sizing

- Use theme tokens (`cx.theme().*`) and `rem` helpers (`p_4()`, `gap_3()`, `w_64()`) exclusively. Never use raw color literals, and never use `px(...)` except for hairlines and art dimensions.
- Cover art supplies the color; chrome stays neutral. Reserve the primary accent color for the main action of a region.
- Maintain WCAG contrast across every bundled light and dark theme (`style::legible`: at least 4.5:1 for text, 3:1 for icons). `theme::apply` enforces readable contrast on `warning`, `danger`, `info`, and `success` without altering themes whose tokens already meet contrast.

### Themes, desktop integration, and translucency

- Bundled themes live in `assets/themes`; Irori Night and Irori Day are the defaults, applied through `theme::apply`.
- Desktop theme sync: on Omarchy (`omarchy.rs`), COSMIC (`cosmic.rs`), and KDE Plasma (`kde.rs`), the default theme mirrors the desktop palette (`desktop_theme::Palette`) and tracks live theme switches. These dynamic themes stay outside the theme registry. COSMIC's `frosted_windows` and opacity also govern Automatic translucency (`glass::resolve`, `Glass::Desktop`).
- Wallpaper colors (`iroriUi.wallpaperColors`, opt-in on all OSes): overrides followed desktop themes by reading pywal/wallust `colors.json` or the matugen JSON named in `iroriUi.wallpaperColorsFile` (`wallpaper.rs`). `theme::follow_wallpaper` watches the file with `notify` and re-themes only when parsed colors change.
- System accent (`desktop_accent.rs`, `macos.rs`, `windows.rs`, `desktop_theme::system_accented`): outside followed desktops, tints Irori's default theme with the OS accent (Linux settings portal, macOS `controlAccentColor`, Windows `UISettings`) via event-driven OS notifications with zero polling. Followed desktops, user-picked themes, and `iroriUi.systemAccent` off take precedence.
- User overrides: `iroriUi.accentColor` tints the active theme legibly; `iroriUi.fontFamily` replaces Manrope when installed (`theme::font_names`). `<config>/appearance.json` (`irori_ui::appearance`) overlays theme keys on top (never over Windows high contrast), watched via `notify` with a 200 ms debounce; invalid files keep the current look and raise `ui.appearance.invalid` once per error.
- Accessibility flags apply to every theme: macOS "Increase contrast" and Windows high contrast strengthen borders, Windows high contrast substitutes `desktop_theme::high_contrast`, and "Reduce transparency" or "Transparency effects" off forces Automatic translucency opaque.
- Translucency surfaces must pull colors from `glass::chrome(..)`, `glass::page(..)`, and `solid_pages()`. Never add alpha manually.
- Backdrops (`components::library_backdrop`, Big Picture `backdrop`): `ArtBackdrop` enforces `strongest_legible_weight` against foreground text, stays off under `theme::reduce_transparency`, and never brightens, saturates, or adds contrast (`style::art_tint` only lowers saturation; pinned by unit tests).

### Linux window shadow margin and modal dim

- On Linux, `cx.theme().overlay` is intentionally transparent so `gpui-kit` does not dim the client-side shadow margin. Read the dim color from `theme::modal_dim` instead, which the shell paints inside visible bounds.
- `Window::viewport_size` includes the Linux CSD shadow margin (20 px per side unless tiled). Size window-covering overlays and layouts using `style::visible_bounds` and `style::visible_size` so nothing renders over window decorations.

## Layout, components, tooltips, and feedback

### Layout discipline

- Add `min_w_0()` (and `min_h_0()` vertically) plus `truncate()` on every flex child containing text that can overflow. Keep one scroll owner per region.
- Leave 30% to 40% horizontal room for longer languages such as German and Russian. Never trap translated text inside a fixed-width box.
- Adaptive controls (such as the library header's `iroriUi.libraryQuickControls`) must compute visibility from measured widths using a `canvas` and pure tested functions (`crates/feature-library/src/quick.rs`, rendered by `crates/feature-library/src/view.rs` and `crates/feature-library/src/view/quick.rs`), never hardcoded pixel breakpoints.
- Focus view column counts (`libraryFocusColumns`, `layouts::focus_columns_for`), cover sizes (`libraryFocusCoverSize`, `layouts::focus_tile_rem`), and automatic Grid/Focus switching (`libraryAutoLayout`, `layouts::wide_enough_for_focus`) use measured widths and switch only on threshold crossings without overwriting manual picks.

### Shared components (`crates/ui/src/components/`)

- Reuse `GameCover`, `ArtImage`, `EmptyState`, `PageHeader`, `SectionTitle`, `ProgressSummary`, `StoreBadge`, `StoreLogo`, and `clip_mark`.
- Stores, launchers, and emulators with Simple Icons marks in `assets/brand/stores` (CC0; see `NOTICE.md`) render through `StoreLogo::icon` in the current text color; others render their text name.
- `GameCover` honors `coverCorners`, `coverCaption` (full leading with `format::sort_value`, title-only, or none), `coverShape` (boxes, squares, or 16:9 hero banners via `components::cover_art_for`; Focus and favorites stay boxes, and only boxes render with `coverFit = "whole"`), `coverHover`, and `coverFavourite`. Games without cover art whose Windows executable exposes an icon render `components::program_icon_cover` centered on the placeholder wash.
- Respect `iroriUi.hiddenDetailRows` (`crates/feature-game/src/detail_rows.rs` IDs are persisted user data) on the game page Details card.
- Guided tour (`tour.rs`, triggered manually via "Show me around" in Settings, Getting Started, or the palette): anchors regions with `tour::mark`, navigable by arrows, Enter, Escape, and controller.

### Tooltips, feedback, and destructive confirmations

- Every async action must visibly represent loading, success, and failure states.
- Surface failures to users via `window.push_notification(Notification::error(format!("{e:#}")).title(t(..)), cx)` or an inline `Alert`; never log errors silently without user feedback. Explain offline state clearly.
- Add concise hover tooltips where icon-only buttons or compact controls benefit from explanation, and keep copy clear and direct without hover-only actions.
- Confirm only destructive actions using `window.open_alert_dialog` with a danger button. Name the specific object in the title and action (for example: Uninstall "Hades"? / Uninstall), never generic "Are you sure?" prompts.
- Reachability: every user action must be accessible from a visible button, context menu, command palette, and keyboard, all invoking the same action or handler. Never build hover-only workflows, and keep focus rings visible (`keyboard_focusable`).
- Motion: use `motion::{QUICK, STANDARD}` and `fade_in`/`rise_in`, always honoring `style::reduce_motion(cx)`.

## Copy and translations (i18n)

- Use sentence case, verb labels on buttons ("Install", "Verify files"), curly quotes where appropriate, and no trailing periods on titles or buttons.
- Never translate or alter store, runtime, or tool brand names (Epic Games, GOG, Steam, Proton, Wine, DXVK, etc.).
- Irori ships 19 locale catalogs in `irori_ui::i18n::LANGUAGES`: `en, cs, de, es, fr, it, nl, pl, pt, pt_BR, ro, sv, tr, ru, uk, ja, ko, zh_Hans, zh_Hant`.
- Each crate owns flat JSON catalogs under `crates/<crate>/locales/<code>.json` and a key namespace: `ui.`, `library.`, `game.`, `manage.`, `settings.`, `bigpicture.`, and `shell.` (`crates/irori/src/strings.rs`).
- `en.json` is the source of truth. Always use `t("ns.area.intent")` and `t_args("ns.area.intent", &[("name", value)])` with intent-based keys and full sentences carrying `{placeholders}` (never concatenate sentence fragments). Use `_one` / `_other` keys for plurals. Do not translate log messages, identifiers, file paths, key names, or brand names.
- Register `CATALOGS` once (`std::sync::Once`) from each crate's `init`. Language changes apply live via `set_language` and window refresh in `shell.rs`.
- Put every new or changed UI string into `crates/<crate>/locales/en.json` with matching `{placeholder}` names. English is the runtime fallback (`crates/ui/src/i18n.rs`), so updating the other 18 locale files in code PRs is optional and translations flow through Hosted Weblate or `i18n` PRs (see [docs/TRANSLATING.md](TRANSLATING.md)). Localization test suites across all seven crates validate JSON parsing across all 19 catalogs, verify namespace prefixes, reject blank values, reject orphan keys absent from `en.json`, require every key referenced in `src/` to exist in `en.json`, and enforce exact `{placeholder}` parity on translated keys (`irori-manage` and `irori-bigpicture` also fail on unused `en.json` keys).

## Big Picture, controllers, and in-game overlay

### Controller input and focus (`crates/feature-bigpicture/src/gamepad/`)

- `reader.rs` runs on a dedicated `gilrs` thread blocking on OS events (zero idle CPU). On Windows it uses patched `crates/gilrs-core` (never panics, tolerates `NonRoamableId` failures; PlayStation and Switch Pro pads in full HID mode set by Steam/SDL still read nothing there). On Linux `xpad`, `reader::swaps_north_west` un-swaps `BTN_NORTH`/`BTN_WEST` for unlisted pads.
- `input.rs` handles repeat and deduplication (unit-tested). `mod.rs` dispatches `irori_gamepad` actions or synthesizes keystrokes (arrows, Enter, Escape, Ctrl+Enter, Ctrl+K, Page Up/Down). Unhandled arrows and shoulders move between Tab stops, so grids and lists that bind arrows MUST call `cx.propagate()` when moving past an edge so controllers never get trapped.
- Pointer hiding (`gamepad/pointer.rs`, `iroriController.hidePointer`): GPUI has no public hide-cursor API, so controller input synthesizes an unbound `F24` key press followed by a global action to trigger `CursorHideMode::OnTypingAndAction`. Never bind `F24`.
- Button mapping (`gamepad/bindings.rs`, `iroriController`): a single app-wide map (`confirmButton`, `glyphs`, `buttons`) feeds the reader thread, overlay evdev reader, and button hints (`ControllerKind::glyph`). Reject maps that leave confirm or back unbound. Guide, D-pad, and sticks retain fixed roles.
- Guide tap and hold-to-open: `input::GuideTap` releases a stuck Guide hold after 5 s, disconnect, or focus change (`HoldHintView`). Holding Start or Select (`iroriUi.bigPictureHoldButton`, `bigPictureHoldMs` 300 to 3000, default 1000) opens Big Picture when active and no game is running (`input::HoldTimer`).
- Every desktop and Big Picture screen must remain navigable with arrows, Enter, and Escape. Keep search inputs and embedded game settings outside the `BigPicture` key context (`SEARCH_CONTEXT`, `view/settings.rs`) so typed letters reach controls.

### Big Picture UI (`crates/feature-bigpicture/src/`)

- Pure state and layout math live in `model.rs` (shelves, filters, page rows) and `layout/mod.rs` / `layout/home.rs` / `layout/wheel.rs` (window-relative `rem` sizes, scrolling, mouse wheel conversion) and must be unit-tested there.
- Six Home layouts (`Shelves`, `Spotlight`, `Cinema`, `Wall`, `Carousel`, `Compact`) share one `FocusGrid`.
- Themes (`theme/`, `<config>/bigpicture/themes/<id>.irori-theme.json`): versioned, field-validated, with bounded embedded PNG/JPEG/WebP images (never arbitrary paths or URLs) and Irori-generated IDs. The live editor (`crates/feature-bigpicture/src/view/editor/mod.rs`) supports 100-step undo/redo via Page Up/Down (LB/RB).
- Navigation sounds (`nav_sound.rs`, opt-in): synthesized WAV files written once under `<cache>/bigpicture-sounds` and played via OS helpers (`PlaySoundW`, `afplay`, `pw-play`/`paplay`/`aplay`) with zero linked audio libraries.
- Idle screen (`view/idle.rs`, `iroriBigPicture.idleMinutes`): single timer task sleeping until due; dimmed mode uses no timer. Any input wakes Big Picture and is consumed.
- Shelf kinds (`theme::ShelfKind`, `model::build_rows_on`) and pinned category shelves respect hidden games and the hidden-games PIN (`LibrarySnapshot::hidden_locked`).

### In-game overlay and achievement toasts (`crates/feature-bigpicture/src/overlay/`)

- Second GPUI window inside Irori's process (`overlay/window.rs`), created only while open: Wayland `wlr` layer-shell surface on the overlay layer with on-demand keyboard where supported, else a GPUI pop-up window (non-activating panel on macOS, topmost tool window on Windows; does not render over exclusive fullscreen games on Windows).
- Platform differences are centralized in `overlay/support.rs`. Linux grabs controllers via `EVIOCGRAB` and reads raw `evdev` while open; macOS and Windows register `Ctrl+Shift+O` (and `Ctrl+Shift+S` for screenshots) only while a game runs; Windows also supports `Back + Start` via XInput while a game runs.
- Guide chords (`gamepad::Chord`): X saves a replay clip, RB takes a screenshot, Up/Down adjust volume (showing a transient `overlay/level.rs` surface), Y marks a journal moment. Each chord is configurable.
- Achievement unlock toasts (`overlay/notice.rs`, `iroriAchievementToast`): non-focusing top-right surface (layer-shell without keyboard on Wayland, pop-up elsewhere) sharing one queue with corner notices (`overlay::toasts`). Falls back to regular notices when Irori is foregrounded, Wayland lacks layer-shell, or Windows runs exclusive fullscreen.

## UI testing

- Write GPUI UI tests with `#[gpui_kit::test]` and `TestAppContext` (see `crates/feature-library/src/ui_tests.rs`): isolate `Paths`, run `Backend::start_for_tests` over an in-memory `Store`, call `run_until_parked`, then open and draw a window.
- Never enable `gpui-kit`'s `test-support` feature (it pulls conflicting GPUI Fast crates into `Cargo.lock`). Use `gpui = { package = "gpui-pre", version = "=0.3.9", features = ["test-support"] }` as a dev-dependency instead.
- On Linux without a display, wrap UI tests in `xvfb-run -a`.
- Whenever a change affects layout, visual appearance, or interaction, run Irori and verify both light and dark themes (attaching screenshots to the PR), or state explicitly in the PR that you did not render the screen.
