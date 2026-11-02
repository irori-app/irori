//! Themes: Irori's own light/dark pair plus a few bundled community themes,
//! applied from the user's [`UiPrefs`].
//!
//! Each bundled file is a family (Catppuccin, Gruvbox...). The user picks one
//! theme; the other colour mode uses its sibling in the same family (Latte ↔
//! Mocha), or Irori's own theme when the family has none, so following the
//! system never drops the user back to an unrelated look.
//!
//! On COSMIC, KDE Plasma and Omarchy the default theme is the desktop's own (see
//! [`crate::cosmic`], [`crate::kde`] and [`crate::omarchy`]), and Irori follows
//! it as the user switches. Elsewhere on Linux the default is Irori's own in the
//! desktop's accent colour, where the desktop has one (see
//! [`crate::desktop_accent`]). A theme the user picked wins.

use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Hsla, Rgba, SharedString, Window, WindowAppearance, px};
use irori_core::settings::{ThemePreference, UiPrefs};
use tokio::runtime::Runtime;

pub use crate::desktop_theme::Desktop;
use crate::desktop_theme::{DesktopThemes, Frost, accented};
use crate::style::{TEXT_CONTRAST, contrast, legible_towards};
use crate::{cosmic, desktop_accent, kde, omarchy};

pub const DEFAULT_DARK: &str = "Irori Night";
pub const DEFAULT_LIGHT: &str = "Irori Day";

const BUNDLED: &[&str] = &[
    include_str!("../../../assets/themes/irori.json"),
    include_str!("../../../assets/themes/catppuccin.json"),
    include_str!("../../../assets/themes/tokyonight.json"),
    include_str!("../../../assets/themes/gruvbox.json"),
    include_str!("../../../assets/themes/everforest.json"),
    include_str!("../../../assets/themes/solarized.json"),
    include_str!("../../../assets/themes/ayu.json"),
];

/// How long a desktop's theme files have to stay still before Irori reads
/// them: a switch writes them in several steps.
const DESKTOP_SETTLE: Duration = Duration::from_millis(200);

/// The longest the first window waits for the desktop's accent colour. A running
/// portal answers in well under this, and a slow one mustn't hold Irori back:
/// its answer then arrives while Irori runs, like any change.
const ACCENT_WAIT: Duration = Duration::from_millis(100);

/// Irori's typeface (Manrope, SIL Open Font License), used by every theme.
pub const FONT_FAMILY: &str = "Manrope";

const FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/Manrope-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-Bold.ttf"),
    include_bytes!("../../../assets/fonts/Manrope-ExtraBold.ttf"),
];

/// The bundled themes by family, in file order.
#[derive(Default)]
struct Families(Vec<Vec<(SharedString, bool)>>);

impl Global for Families {}

/// The desktop's theme, while Irori runs on a desktop it follows. It lives here
/// rather than in the theme registry because the registry never replaces a
/// theme it has.
struct DesktopTheme {
    desktop: Desktop,
    light: Option<Rc<ThemeConfig>>,
    dark: Option<Rc<ThemeConfig>>,
    is_dark: bool,
    frost: Option<Frost>,
    revision: u64,
}

impl Global for DesktopTheme {}

/// Irori's own themes in the desktop's accent colour. Like [`DesktopTheme`] they
/// stay out of the registry, and they keep their names: to the user they are
/// the default theme, and "Irori Night" in the list is the untinted one.
struct DesktopAccent {
    color: Option<Rgba>,
    light: Option<Rc<ThemeConfig>>,
    dark: Option<Rc<ThemeConfig>>,
    revision: u64,
}

impl Global for DesktopAccent {}

/// What [`apply`] last applied, so a call that changes nothing costs nothing
/// and one that only resizes the interface skips re-theming.
#[derive(Clone, PartialEq)]
struct Applied {
    light: SharedString,
    dark: SharedString,
    is_dark: bool,
    font_size: u32,
    /// The desktop's theme keeps its name when its colours change.
    desktop: u64,
    /// So does Irori's own when the accent changes, and tinted or not. Zero
    /// while the accent isn't showing, so a picked theme ignores its changes.
    accent: u64,
}

impl Global for Applied {}

/// Extra zoom on top of the interface size while a screen asks for it
/// (Big Picture on a TV). Absent means 1.
#[derive(Clone, Copy, PartialEq)]
struct Zoom(f32);

impl Global for Zoom {}

fn zoom(cx: &App) -> f32 {
    cx.try_global::<Zoom>().map_or(1.0, |z| z.0)
}

/// Scale the whole interface by `zoom` on top of the interface size the user
/// picked (1 = none). Big Picture sets its TV size with this while it shows
/// and resets it to 1 when it closes.
pub fn set_zoom(zoom: f32, cx: &mut App) {
    let zoom = zoom.clamp(0.5, 3.0);
    if self::zoom(cx) == zoom {
        return;
    }
    cx.set_global(Zoom(zoom));
    let Some(base) = cx
        .try_global::<Applied>()
        .map(|a| f32::from_bits(a.font_size))
    else {
        return;
    };
    Theme::update(cx, |theme| theme.font_size = px(base * zoom));
    cx.refresh_windows();
}

pub(crate) fn install(cx: &mut App) {
    let fonts = FONTS
        .iter()
        .map(|f| std::borrow::Cow::Borrowed(*f))
        .collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("loading Irori's fonts: {e:#}");
    }
    let mut families = Families::default();
    let registry = ThemeRegistry::global_mut(cx);
    for json in BUNDLED {
        if let Err(e) = registry.load_themes_from_str(json) {
            tracing::warn!("loading bundled theme: {e:#}");
        }
        families.0.push(family_members(json));
    }
    cx.set_global(families);
}

/// Use the desktop's theme as the default on a desktop Irori follows, and keep
/// up with it. COSMIC and Plasma go first because they are the session running
/// now, while Omarchy's files stay on disk whatever session is.
pub(crate) fn follow_desktop(cx: &mut App) {
    if cosmic::running() {
        follow(
            Desktop::Cosmic,
            cosmic::read,
            cosmic::themes,
            cosmic::watch(),
            cx,
        );
    } else if kde::running() {
        follow(Desktop::Kde, kde::read, kde::themes, kde::watch(), cx);
    } else {
        follow(
            Desktop::Omarchy,
            omarchy::read,
            |text: &String| omarchy::themes(text),
            omarchy::watch(),
            cx,
        );
    }
    // A desktop Irori follows has its accent in its theme already.
    if !cx.has_global::<DesktopTheme>() {
        follow_accent(cx);
    }
}

/// Tint the default theme with the desktop's accent colour, and keep up with it.
/// The wait is on the UI thread for the reason [`follow`]'s first read is, and
/// it is bounded because the portal, unlike a file, may take its time to start.
fn follow_accent(cx: &mut App) {
    let backend = crate::backend(cx);
    let Some(mut changes) = desktop_accent::watch(&backend) else {
        return;
    };
    if let Some(accent) = first_accent(&mut changes, backend.runtime()) {
        set_accent(accent, cx);
    }
    cx.spawn(async move |cx| {
        while let Some(mut accent) = changes.next().await {
            // A colour picker can send a change per step of a drag.
            cx.background_executor().timer(DESKTOP_SETTLE).await;
            while let Ok(newer) = changes.try_recv() {
                accent = newer;
            }
            cx.update(|cx| {
                if set_accent(accent, cx) {
                    let prefs = crate::backend(cx).settings.app().ui.clone();
                    apply(&prefs, None, cx);
                }
            });
        }
    })
    .detach();
}

/// The accent, if it comes within [`ACCENT_WAIT`]. Whatever is later stays in
/// `changes`. The timer needs the runtime, so it has to start inside it.
fn first_accent(changes: &mut desktop_accent::Changes, runtime: &Runtime) -> Option<Option<Rgba>> {
    runtime
        .block_on(async { tokio::time::timeout(ACCENT_WAIT, changes.next()).await })
        .ok()
        .flatten()
}

/// Tint Irori's themes with `color`, or stop. False if it was the accent already.
fn set_accent(color: Option<Rgba>, cx: &mut App) -> bool {
    let current = cx.try_global::<DesktopAccent>();
    if current.and_then(|a| a.color) == color {
        return false;
    }
    let revision = current.map_or(0, |a| a.revision) + 1;
    let tinted = |name: &str| {
        let base = registered(name, cx)?;
        accented(&base, color?).map(Rc::new)
    };
    let (light, dark) = (tinted(DEFAULT_LIGHT), tinted(DEFAULT_DARK));
    cx.set_global(DesktopAccent {
        color,
        light,
        dark,
        revision,
    });
    true
}

/// The first read is on the UI thread, before any window exists to wait for
/// it, so the first frame already has the right colours.
fn follow<T: PartialEq + Send + 'static>(
    desktop: Desktop,
    read: fn() -> Option<T>,
    themes: impl Fn(&T) -> Option<DesktopThemes> + 'static,
    watch: Option<(notify::RecommendedWatcher, mpsc::UnboundedReceiver<()>)>,
    cx: &mut App,
) {
    let mut shown = read();
    if let Some(found) = shown.as_ref().and_then(&themes) {
        set_desktop(desktop, found, cx);
    }
    let Some((watcher, mut changes)) = watch else {
        return;
    };
    cx.spawn(async move |cx| {
        let _watcher = watcher;
        while changes.next().await.is_some() {
            cx.background_executor().timer(DESKTOP_SETTLE).await;
            while changes.try_recv().is_ok() {}
            let files = cx.background_executor().spawn(async move { read() }).await;
            // Between two steps of a switch there may be no theme at all; the
            // next step wakes this up again.
            if files.is_none() || files == shown {
                continue;
            }
            let Some(found) = files.as_ref().and_then(&themes) else {
                tracing::warn!("the desktop's theme has no usable colours");
                continue;
            };
            shown = files;
            cx.update(|cx| {
                set_desktop(desktop, found, cx);
                let prefs = crate::backend(cx).settings.app().ui.clone();
                apply(&prefs, None, cx);
                // The shell works out the window's translucency when settings
                // change, and the desktop's say in that may have changed.
                let settings = crate::models(cx).settings.clone();
                settings.update(cx, |_, cx| cx.notify());
            });
        }
    })
    .detach();
}

fn set_desktop(desktop: Desktop, themes: DesktopThemes, cx: &mut App) {
    let revision = cx.try_global::<DesktopTheme>().map_or(0, |d| d.revision) + 1;
    cx.set_global(DesktopTheme {
        desktop,
        light: themes.light.map(Rc::new),
        dark: themes.dark.map(Rc::new),
        is_dark: themes.is_dark,
        frost: themes.frost,
        revision,
    });
}

/// What the desktop wants of see-through windows, on one that has a setting.
pub(crate) fn desktop_frost(cx: &App) -> Option<Frost> {
    cx.try_global::<DesktopTheme>()?.frost
}

/// The desktop whose theme is the default one, if Irori follows one here.
pub fn followed_desktop(cx: &App) -> Option<Desktop> {
    cx.try_global::<DesktopTheme>().map(|d| d.desktop)
}

/// Names and modes of the themes in one bundled file.
fn family_members(json: &str) -> Vec<(SharedString, bool)> {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    doc["themes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t["name"].as_str()?;
            Some((SharedString::from(name.to_string()), t["mode"] == "dark"))
        })
        .collect()
}

/// The sibling of `name` in the other colour mode: the last theme of that mode
/// in its family (the family's flagship, e.g. Catppuccin Mocha).
fn sibling(name: &str, dark: bool, cx: &App) -> Option<SharedString> {
    let families = cx.try_global::<Families>()?;
    families
        .0
        .iter()
        .find(|f| f.iter().any(|(n, _)| n.as_ref() == name))?
        .iter()
        .rev()
        .find(|(_, d)| *d == dark)
        .map(|(n, _)| n.clone())
}

/// Names of the themes the user can pick, dark ones first. gpui-kit's own
/// unstyled defaults are left out: they are not Irori themes.
pub fn theme_names(cx: &App) -> Vec<(SharedString, bool)> {
    let mut names: Vec<(SharedString, bool)> = ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .filter(|t| !t.is_default)
        .map(|t| (t.name.clone(), t.mode.is_dark()))
        .collect();
    names.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    names
}

/// Whether the theme called `name` is a dark one, if it exists.
pub fn is_dark_theme(name: &str, cx: &App) -> Option<bool> {
    ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(name.to_string()))
        .map(|t| t.mode.is_dark())
}

/// The theme called `name`.
fn registered(name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(name.to_string()))
        .cloned()
}

/// The light and dark themes `prefs` call for.
fn pair(prefs: &UiPrefs, cx: &App) -> (Option<Rc<ThemeConfig>>, Option<Rc<ThemeConfig>>) {
    let find = |name: &str| registered(name, cx);
    let desktop = cx
        .try_global::<DesktopTheme>()
        .filter(|_| prefs.theme_name.is_empty());
    let accent = cx
        .try_global::<DesktopAccent>()
        .filter(|_| prefs.theme_name.is_empty());
    let chosen = find(&prefs.theme_name);
    let for_mode = |dark: bool| {
        let fallback = if dark { DEFAULT_DARK } else { DEFAULT_LIGHT };
        if let Some(desktop) = desktop {
            let theme = if dark { &desktop.dark } else { &desktop.light };
            return theme.clone().or_else(|| find(fallback));
        }
        match &chosen {
            Some(t) if t.mode.is_dark() == dark => Some(t.clone()),
            Some(t) => sibling(&t.name, dark, cx)
                .and_then(|n| find(&n))
                .or_else(|| find(fallback)),
            None => accent
                .and_then(|a| if dark { &a.dark } else { &a.light }.clone())
                .or_else(|| find(fallback)),
        }
    };
    (for_mode(false), for_mode(true))
}

/// Apply theme, mode and base font size (application zoom) from preferences.
///
/// Cheap to call often: it does nothing when the result would not change, and
/// only rescales when just the interface size did.
pub fn apply(prefs: &UiPrefs, window: Option<&mut Window>, cx: &mut App) {
    let is_dark = match prefs.theme {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        // The desktop's theme is what decides, and it can be set more quickly
        // than the portal reports the change.
        ThemePreference::System
            if prefs.theme_name.is_empty() && cx.has_global::<DesktopTheme>() =>
        {
            cx.global::<DesktopTheme>().is_dark
        }
        ThemePreference::System => {
            let appearance = window
                .as_deref()
                .map(Window::appearance)
                .unwrap_or_else(|| cx.window_appearance());
            matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            )
        }
    };
    let font_size = prefs.font_size.clamp(12.0, 24.0);
    let (light, dark) = pair(prefs, cx);
    let name = |t: &Option<Rc<ThemeConfig>>| t.as_ref().map(|t| t.name.clone()).unwrap_or_default();
    let next = Applied {
        light: name(&light),
        dark: name(&dark),
        is_dark,
        font_size: font_size.to_bits(),
        desktop: cx.try_global::<DesktopTheme>().map_or(0, |d| d.revision),
        accent: cx
            .try_global::<DesktopAccent>()
            .filter(|_| prefs.theme_name.is_empty())
            .map_or(0, |a| a.revision),
    };
    let previous = cx.try_global::<Applied>().cloned();
    if previous.as_ref() == Some(&next) {
        return;
    }
    let only_size = previous.is_some_and(|p| {
        Applied {
            font_size: next.font_size,
            ..p
        } == next
    });
    cx.set_global(next);
    let zoomed = font_size * zoom(cx);
    if only_size {
        Theme::update(cx, |theme| theme.font_size = px(zoomed));
        return;
    }

    Theme::update(cx, |theme| {
        if let Some(l) = &light {
            theme.apply_config(l);
        }
        if let Some(d) = &dark {
            theme.apply_config(d);
        }
        theme.font_size = px(zoomed);
        // The typeface is part of Irori's identity, whatever the colours.
        theme.font_family = FONT_FAMILY.into();
    });
    let mode = if is_dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, window, cx);
    Theme::update(cx, hold_status_colors);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    lift_modal_dim(cx);
}

/// The theme's dim behind dialogs and sheets, kept out of its reach.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct ModalDim(Hsla);

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Global for ModalDim {}

/// gpui-kit paints this dim across the whole window surface, and on Linux that
/// includes the transparent margin client-side decorations keep for their
/// shadow, which shows as a dark band around the window. Clearing its colour
/// is the one hook that reaches every dialog and sheet, so [`modal_dim`] hands
/// the colour to the shell, which paints it inside the visible window.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn lift_modal_dim(cx: &mut App) {
    let dim = Theme::global(cx).overlay;
    if dim.a > 0. {
        cx.set_global(ModalDim(dim));
    }
    Theme::update(cx, |theme| theme.overlay = gpui_kit::transparent_black());
}

/// The dim the shell should paint over itself right now, if a dialog or sheet
/// is open and gpui-kit is not already painting it.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn modal_dim(window: &mut Window, cx: &mut App) -> Option<Hsla> {
    use gpui_kit::component::WindowExt as _;

    if !window.has_active_dialog(cx) && !window.has_active_sheet(cx) {
        return None;
    }
    cx.try_global::<ModalDim>().map(|dim| dim.0)
}

/// gpui-kit paints the dim itself here: these windows have no shadow margin.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn modal_dim(_window: &mut Window, _cx: &mut App) -> Option<Hsla> {
    None
}

/// What status colours aim for, a little over WCAG's 4.5:1: alerts tint their
/// own background with the colour they write in.
const STATUS_CONTRAST: f32 = 4.8;
/// How much of its colour an alert mixes into its background.
const ALERT_TINT: f32 = 0.04;

/// Themes choose warning, danger, info and success by eye, and many light
/// ones read at 2:1 as text on their own page. Every alert and status line
/// writes in these colours, so hold them here instead of at each. A colour
/// that already reads stays exactly as the theme has it.
fn hold_status_colors(theme: &mut Theme) {
    let towards = if theme.mode.is_dark() {
        gpui_kit::white()
    } else {
        gpui_kit::black()
    };
    let surfaces = [theme.background, theme.popover];
    let fix = |color: Hsla| {
        let tinted = surfaces.map(|s| tint(s, color));
        legible_towards(color, &tinted, STATUS_CONTRAST, towards)
    };
    // What sits on the colour when it fills a button or a badge.
    let on = |foreground: Hsla, fill: Hsla| {
        let best = if contrast(gpui_kit::white(), fill) > contrast(gpui_kit::black(), fill) {
            gpui_kit::white()
        } else {
            gpui_kit::black()
        };
        legible_towards(foreground, &[fill], TEXT_CONTRAST, best)
    };
    let t = &mut *theme;
    (t.warning, t.warning_hover, t.warning_active) =
        (fix(t.warning), fix(t.warning_hover), fix(t.warning_active));
    t.warning_foreground = on(t.warning_foreground, t.warning);
    (t.danger, t.danger_hover, t.danger_active) =
        (fix(t.danger), fix(t.danger_hover), fix(t.danger_active));
    t.danger_foreground = on(t.danger_foreground, t.danger);
    (t.info, t.info_hover, t.info_active) = (fix(t.info), fix(t.info_hover), fix(t.info_active));
    t.info_foreground = on(t.info_foreground, t.info);
    (t.success, t.success_hover, t.success_active) =
        (fix(t.success), fix(t.success_hover), fix(t.success_active));
    t.success_foreground = on(t.success_foreground, t.success);
}

/// `color` at [`ALERT_TINT`] over `surface`, in sRGB.
fn tint(surface: Hsla, color: Hsla) -> Hsla {
    let (s, c) = (surface.to_rgb(), color.to_rgb());
    let mix = |a: f32, b: f32| a + (b - a) * ALERT_TINT;
    Rgba {
        r: mix(s.r, c.r),
        g: mix(s.g, c.g),
        b: mix(s.b, c.b),
        a: 1.0,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::TestAppContext;
    use gpui_kit::component::ActiveTheme as _;

    use super::*;

    fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            install(cx);
        });
    }

    fn prefs(theme: ThemePreference, name: &str) -> UiPrefs {
        UiPrefs {
            theme,
            theme_name: name.to_string(),
            ..UiPrefs::default()
        }
    }

    /// The colour a theme file gives `key`, as GPUI parses it.
    fn file_color(name: &str, key: &str) -> gpui_kit::Hsla {
        for json in BUNDLED {
            let doc: serde_json::Value = serde_json::from_str(json).unwrap();
            for t in doc["themes"].as_array().unwrap() {
                if t["name"] == name {
                    let raw = t["colors"][key].as_str().unwrap();
                    return gpui_kit::component::try_parse_color(raw).unwrap();
                }
            }
        }
        panic!("no theme {name}");
    }

    #[test]
    fn every_bundled_theme_colors_everything_irori_paints() {
        let irori: serde_json::Value = serde_json::from_str(BUNDLED[0]).unwrap();
        let wanted: BTreeSet<&str> = irori["themes"][0]["colors"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for json in BUNDLED {
            let doc: serde_json::Value = serde_json::from_str(json).unwrap();
            for t in doc["themes"].as_array().unwrap() {
                let has: BTreeSet<&str> = t["colors"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                let missing: Vec<_> = wanted.difference(&has).collect();
                assert!(missing.is_empty(), "{} lacks {missing:?}", t["name"]);
            }
        }
    }

    #[gpui_kit::test]
    fn every_theme_shows_when_its_mode_is_current(cx: &mut TestAppContext) {
        setup(cx);
        let names = cx.read(theme_names);
        assert!(names.len() >= 17, "{names:?}");
        for (name, dark) in names {
            let mode = if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            };
            cx.update(|cx| apply(&prefs(mode, &name), None, cx));
            cx.read(|cx| {
                let theme = cx.theme();
                assert_eq!(theme.theme_name(), &name);
                assert_eq!(theme.mode.is_dark(), dark, "{name}");
                assert_eq!(theme.background, file_color(&name, "background"), "{name}");
                assert_eq!(
                    theme.sidebar,
                    file_color(&name, "sidebar.background"),
                    "{name}"
                );
                assert_eq!(theme.font_family.as_ref(), FONT_FAMILY);
            });
        }
    }

    #[gpui_kit::test]
    fn the_other_mode_uses_the_family_sibling(cx: &mut TestAppContext) {
        setup(cx);
        let shown = |cx: &mut TestAppContext, mode, name: &str| {
            cx.update(|cx| apply(&prefs(mode, name), None, cx));
            cx.read(|cx| cx.theme().theme_name().to_string())
        };
        assert_eq!(
            shown(cx, ThemePreference::Dark, "Catppuccin Latte"),
            "Catppuccin Mocha"
        );
        assert_eq!(
            shown(cx, ThemePreference::Light, "Gruvbox Dark"),
            "Gruvbox Light"
        );
        // Tokyo Night has no light theme: Irori's own stands in.
        assert_eq!(
            shown(cx, ThemePreference::Light, "Tokyo Moon"),
            DEFAULT_LIGHT
        );
        assert_eq!(shown(cx, ThemePreference::Dark, ""), DEFAULT_DARK);
    }

    fn use_omarchy(palette: &str, cx: &mut TestAppContext) {
        cx.update(|cx| set_omarchy(palette, cx));
    }

    fn set_omarchy(palette: &str, cx: &mut App) {
        let themes = DesktopThemes::single(omarchy::tests::theme(palette).unwrap());
        set_desktop(Desktop::Omarchy, themes, cx);
    }

    #[gpui_kit::test]
    fn omarchy_is_the_default_and_decides_the_mode(cx: &mut TestAppContext) {
        setup(cx);
        use_omarchy(omarchy::tests::LIGHT, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "Omarchy");
            assert!(!cx.theme().mode.is_dark());
        });
    }

    #[gpui_kit::test]
    fn a_new_omarchy_theme_shows_at_once(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::System, "");
        use_omarchy(omarchy::tests::DARK, cx);
        cx.update(|cx| apply(&p, None, cx));
        let before = cx.read(|cx| cx.theme().background);

        let recoloured = omarchy::tests::DARK.replace("#1a1b26", "#201010");
        use_omarchy(&recoloured, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_ne!(cx.theme().background, before));

        use_omarchy(omarchy::tests::LIGHT, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert!(!cx.theme().mode.is_dark()));
    }

    struct Probe(Rc<std::cell::Cell<Option<gpui_kit::Hsla>>>);

    impl gpui_kit::Render for Probe {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            self.0.set(Some(cx.theme().background));
            gpui_kit::div()
        }
    }

    #[gpui_kit::test]
    fn an_open_window_redraws_when_omarchy_changes(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::System, "");
        use_omarchy(omarchy::tests::DARK, cx);
        cx.update(|cx| apply(&p, None, cx));

        let drawn = Rc::new(std::cell::Cell::new(None));
        let probe = drawn.clone();
        let (_view, cx) = cx.add_window_view(|_, _| Probe(probe));
        cx.run_until_parked();
        assert_eq!(drawn.get(), cx.read(|cx| Some(cx.theme().background)));

        use_omarchy_in_window(omarchy::tests::LIGHT, cx);
        cx.update(|_, cx| apply(&p, None, cx));
        cx.run_until_parked();
        let light = omarchy::tests::theme(omarchy::tests::LIGHT).unwrap();
        let wanted =
            gpui_kit::component::try_parse_color(light.colors.background.as_ref().unwrap());
        assert_eq!(drawn.get(), wanted.ok());
    }

    fn use_omarchy_in_window(palette: &str, cx: &mut gpui_kit::VisualTestContext) {
        cx.update(|_, cx| set_omarchy(palette, cx));
    }

    #[gpui_kit::test]
    fn a_theme_the_user_picked_beats_omarchy(cx: &mut TestAppContext) {
        setup(cx);
        use_omarchy(omarchy::tests::LIGHT, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, "Tokyo Storm"), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Tokyo Storm"));
    }

    #[gpui_kit::test]
    fn a_forced_mode_omarchy_is_not_in_gets_irori(cx: &mut TestAppContext) {
        setup(cx);
        use_omarchy(omarchy::tests::LIGHT, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), DEFAULT_DARK));
    }

    #[gpui_kit::test]
    fn omarchy_is_not_offered_as_a_theme(cx: &mut TestAppContext) {
        setup(cx);
        use_omarchy(omarchy::tests::DARK, cx);
        let names = cx.read(theme_names);
        assert!(names.iter().all(|(name, _)| name != "Omarchy"), "{names:?}");
    }

    #[gpui_kit::test]
    fn sidebar_rows_read_on_omarchy(cx: &mut TestAppContext) {
        use crate::style::{SidebarPalette, TEXT_CONTRAST, contrast};

        setup(cx);
        for palette in [omarchy::tests::DARK, omarchy::tests::LIGHT] {
            use_omarchy(palette, cx);
            cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
            cx.read(|cx| {
                let theme = cx.theme();
                let p = SidebarPalette::new(cx);
                assert!(contrast(p.text, theme.sidebar) >= TEXT_CONTRAST - 0.01);
                assert!(contrast(p.selected_text, theme.sidebar_accent) >= TEXT_CONTRAST - 0.01);
            });
        }
    }

    /// GNOME's accent colours, as libadwaita gives them to the portal.
    const GNOME_ACCENTS: [u32; 9] = [
        0x3584e4, 0x2190a4, 0x3a944a, 0xc88800, 0xed5b00, 0xe62d42, 0xd56199, 0x9141ac, 0x6f8396,
    ];

    /// A Plasma selection colour or a COSMIC custom accent can be any of these,
    /// and some land right on Irori's surfaces.
    const HARD_ACCENTS: [u32; 9] = [
        0x000000, 0xffffff, 0x808080, 0x777777, 0x100e0d, 0xfaf7f3, 0xffff00, 0x0000ff, 0x1c1916,
    ];

    fn accents() -> impl Iterator<Item = Rgba> {
        GNOME_ACCENTS
            .into_iter()
            .chain(HARD_ACCENTS)
            .map(gpui_kit::rgb)
    }

    fn use_accent(accent: u32, cx: &mut TestAppContext) -> bool {
        cx.update(|cx| set_accent(Some(gpui_kit::rgb(accent)), cx))
    }

    fn runtime() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }

    #[test]
    fn the_first_accent_is_waited_for_on_the_ui_thread() {
        let (tx, mut changes) = mpsc::unbounded();
        let blue = Some(gpui_kit::rgb(0x3584e4));
        tx.unbounded_send(blue).unwrap();
        assert_eq!(first_accent(&mut changes, &runtime()), Some(blue));
    }

    #[test]
    fn a_slow_portal_only_delays_its_answer() {
        let (tx, mut changes) = mpsc::unbounded();
        assert_eq!(first_accent(&mut changes, &runtime()), None);

        let blue = Some(gpui_kit::rgb(0x3584e4));
        tx.unbounded_send(blue).unwrap();
        assert_eq!(changes.try_recv(), Ok(blue));
    }

    #[test]
    fn no_portal_ends_the_wait_at_once() {
        let (tx, mut changes) = mpsc::unbounded::<Option<Rgba>>();
        drop(tx);
        let started = std::time::Instant::now();
        assert_eq!(first_accent(&mut changes, &runtime()), None);
        assert!(started.elapsed() < ACCENT_WAIT);
    }

    #[gpui_kit::test]
    fn the_default_theme_takes_the_desktops_accent(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        for (mode, name) in [
            (ThemePreference::Dark, DEFAULT_DARK),
            (ThemePreference::Light, DEFAULT_LIGHT),
        ] {
            cx.update(|cx| apply(&prefs(mode, ""), None, cx));
            cx.read(|cx| {
                let theme = cx.theme();
                assert_eq!(theme.primary, gpui_kit::rgb(0x3584e4).into(), "{name}");
                assert_eq!(theme.ring, theme.primary, "{name}");
                assert_eq!(theme.theme_name().as_ref(), name);
                assert_eq!(theme.background, file_color(name, "background"));
                assert_eq!(theme.sidebar, file_color(name, "sidebar.background"));
                assert_eq!(followed_desktop(cx), None);
            });
        }
    }

    #[gpui_kit::test]
    fn no_accent_leaves_irori_as_it_is(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::Dark, "");
        cx.update(|cx| apply(&p, None, cx));
        let own = cx.read(|cx| cx.theme().primary);
        assert_eq!(own, file_color(DEFAULT_DARK, "primary.background"));

        assert!(!cx.update(|cx| set_accent(None, cx)), "nothing to change");

        use_accent(0x3584e4, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_ne!(cx.theme().primary, own));

        assert!(cx.update(|cx| set_accent(None, cx)));
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_eq!(cx.theme().primary, own));
    }

    #[gpui_kit::test]
    fn the_same_accent_again_changes_nothing(cx: &mut TestAppContext) {
        setup(cx);
        assert!(use_accent(0x3584e4, cx));
        assert!(!use_accent(0x3584e4, cx));
        assert!(use_accent(0xe62d42, cx));
    }

    #[gpui_kit::test]
    fn an_open_window_redraws_when_the_accent_changes(cx: &mut TestAppContext) {
        struct PrimaryProbe(Rc<std::cell::Cell<Option<gpui_kit::Hsla>>>);

        impl gpui_kit::Render for PrimaryProbe {
            fn render(
                &mut self,
                _: &mut Window,
                cx: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                self.0.set(Some(cx.theme().primary));
                gpui_kit::div()
            }
        }

        setup(cx);
        let p = prefs(ThemePreference::Dark, "");
        use_accent(0x3584e4, cx);
        cx.update(|cx| apply(&p, None, cx));

        let drawn = Rc::new(std::cell::Cell::new(None));
        let probe = drawn.clone();
        let (_view, cx) = cx.add_window_view(|_, _| PrimaryProbe(probe));
        cx.run_until_parked();
        assert_eq!(drawn.get(), Some(gpui_kit::rgb(0x3584e4).into()));

        cx.update(|_, cx| {
            set_accent(Some(gpui_kit::rgb(0xe62d42)), cx);
            apply(&p, None, cx);
        });
        cx.run_until_parked();
        assert_eq!(drawn.get(), Some(gpui_kit::rgb(0xe62d42).into()));
    }

    #[gpui_kit::test]
    fn a_theme_the_user_picked_beats_the_accent(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        // Picking Irori's own by name is how to get its orange back.
        for name in [DEFAULT_DARK, "Tokyo Storm"] {
            cx.update(|cx| apply(&prefs(ThemePreference::Dark, name), None, cx));
            cx.read(|cx| {
                assert_eq!(cx.theme().theme_name().as_ref(), name);
                assert_eq!(cx.theme().primary, file_color(name, "primary.background"));
            });
        }
    }

    #[gpui_kit::test]
    fn picking_the_default_again_brings_the_accent_back(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, DEFAULT_DARK), None, cx));
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().primary, gpui_kit::rgb(0x3584e4).into()));
    }

    #[gpui_kit::test]
    fn a_followed_desktops_theme_beats_the_accent(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        use_omarchy(omarchy::tests::DARK, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Omarchy"));
    }

    #[gpui_kit::test]
    fn the_accent_keeps_the_system_mode(cx: &mut TestAppContext) {
        setup(cx);
        let without = cx.update(|cx| {
            apply(&prefs(ThemePreference::System, ""), None, cx);
            cx.theme().mode
        });
        use_accent(0x3584e4, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().mode, without));
    }

    #[gpui_kit::test]
    fn an_accent_that_reads_stays_exactly_as_the_desktop_has_it(cx: &mut TestAppContext) {
        setup(cx);
        for accent in GNOME_ACCENTS {
            use_accent(accent, cx);
            cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
            let wanted = gpui_kit::rgb(accent).into();
            let surfaces = cx.read(|cx| {
                let t = cx.theme();
                [t.background, t.sidebar, t.secondary, t.popover]
            });
            if surfaces.iter().all(|s| contrast(wanted, *s) >= 3.1) {
                cx.read(|cx| assert_eq!(cx.theme().primary, wanted, "{accent:06x}"));
            }
        }
    }

    #[gpui_kit::test]
    fn every_accent_reads_on_every_surface(cx: &mut TestAppContext) {
        use crate::style::ICON_CONTRAST;

        setup(cx);
        for accent in accents() {
            for mode in [ThemePreference::Dark, ThemePreference::Light] {
                cx.update(|cx| {
                    set_accent(Some(accent), cx);
                    apply(&prefs(mode, ""), None, cx);
                    let t = cx.theme();
                    let label = format!("{accent:?} {:?}", t.mode);
                    let check = |what: &str, fg, on: &[gpui_kit::Hsla], min: f32| {
                        for bg in on {
                            let ratio = contrast(fg, *bg);
                            assert!(ratio >= min - 0.01, "{label} {what}: {ratio:.2}");
                        }
                    };
                    let surfaces = [t.background, t.sidebar, t.secondary, t.popover];
                    check("filled button", t.primary, &surfaces, ICON_CONTRAST);
                    check("focus ring", t.ring, &surfaces, ICON_CONTRAST);
                    check("caret", t.caret, &surfaces, ICON_CONTRAST);
                    check("progress", t.progress_bar, &surfaces, ICON_CONTRAST);
                    check("link", t.link, &surfaces, TEXT_CONTRAST);
                    check("link hover", t.link_hover, &surfaces, TEXT_CONTRAST);
                    check("link active", t.link_active, &surfaces, TEXT_CONTRAST);
                    let fills = [t.primary, t.primary_hover, t.primary_active];
                    check("button text", t.primary_foreground, &fills, TEXT_CONTRAST);
                    check(
                        "sidebar badge",
                        t.sidebar_primary_foreground,
                        &[t.sidebar_primary],
                        TEXT_CONTRAST,
                    );
                    check(
                        "hover text",
                        t.accent_foreground,
                        &[t.accent],
                        TEXT_CONTRAST,
                    );
                    check(
                        "page text on hover",
                        t.foreground,
                        &[t.accent],
                        TEXT_CONTRAST,
                    );
                });
            }
        }
    }

    #[gpui_kit::test]
    fn sidebar_rows_read_with_every_accent_and_glass(cx: &mut TestAppContext) {
        use crate::glass::{Glass, chrome_surfaces};
        use crate::style::{ICON_CONTRAST, SidebarPalette};

        setup(cx);
        for accent in accents() {
            for mode in [ThemePreference::Dark, ThemePreference::Light] {
                cx.update(|cx| {
                    set_accent(Some(accent), cx);
                    apply(&prefs(mode, ""), None, cx);
                });
                for glass in [
                    Glass::Opaque,
                    Glass::Subtle { blurred: true },
                    Glass::Strong { blurred: false },
                ] {
                    cx.update(|cx| cx.set_global(glass));
                    cx.read(|cx| {
                        let theme = cx.theme();
                        let p = SidebarPalette::new(cx);
                        let lit = chrome_surfaces(&[theme.sidebar, theme.sidebar_accent], cx);
                        for bg in lit {
                            let ratio = contrast(p.selected_icon, bg);
                            assert!(
                                ratio >= ICON_CONTRAST - 0.01,
                                "{accent:?} {glass:?}: {ratio:.2}"
                            );
                        }
                    });
                }
            }
        }
    }

    fn use_cosmic(is_dark: bool, cx: &mut TestAppContext) {
        let themes = cosmic::themes(&cosmic::tests::files(is_dark)).unwrap();
        cx.update(|cx| set_desktop(Desktop::Cosmic, themes, cx));
    }

    #[gpui_kit::test]
    fn cosmic_is_the_default_and_decides_the_mode(cx: &mut TestAppContext) {
        setup(cx);
        use_cosmic(false, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "COSMIC Light");
            assert_eq!(followed_desktop(cx), Some(Desktop::Cosmic));
        });

        use_cosmic(true, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "COSMIC Dark"));
    }

    #[gpui_kit::test]
    fn a_forced_mode_uses_cosmics_theme_for_it(cx: &mut TestAppContext) {
        setup(cx);
        use_cosmic(true, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Light, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "COSMIC Light"));
    }

    #[gpui_kit::test]
    fn a_theme_the_user_picked_beats_cosmic(cx: &mut TestAppContext) {
        setup(cx);
        use_cosmic(true, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, "Tokyo Storm"), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Tokyo Storm"));
    }

    fn use_kde(globals: &str, cx: &mut TestAppContext) {
        let themes = kde::themes(&kde::tests::files(globals)).unwrap();
        cx.update(|cx| set_desktop(Desktop::Kde, themes, cx));
    }

    #[gpui_kit::test]
    fn kde_is_the_default_and_decides_the_mode(cx: &mut TestAppContext) {
        setup(cx);
        use_kde(kde::tests::LIGHT, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "KDE Plasma");
            assert!(!cx.theme().mode.is_dark());
            assert_eq!(followed_desktop(cx), Some(Desktop::Kde));
        });

        use_kde(kde::tests::DARK, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| assert!(cx.theme().mode.is_dark()));
    }

    #[gpui_kit::test]
    fn a_new_kde_scheme_of_the_same_mode_shows_at_once(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::System, "");
        use_kde(kde::tests::DARK, cx);
        cx.update(|cx| apply(&p, None, cx));
        let before = cx.read(|cx| cx.theme().background);

        use_kde(&kde::tests::DARK.replace("20,22,24", "40,20,20"), cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_ne!(cx.theme().background, before));
    }

    #[gpui_kit::test]
    fn a_forced_mode_kde_is_not_in_gets_irori(cx: &mut TestAppContext) {
        setup(cx);
        use_kde(kde::tests::LIGHT, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), DEFAULT_DARK));
    }

    #[gpui_kit::test]
    fn a_theme_the_user_picked_beats_kde(cx: &mut TestAppContext) {
        setup(cx);
        use_kde(kde::tests::DARK, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, "Tokyo Storm"), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Tokyo Storm"));
    }

    #[gpui_kit::test]
    fn sidebar_rows_read_on_kde(cx: &mut TestAppContext) {
        use crate::style::{SidebarPalette, TEXT_CONTRAST, contrast};

        setup(cx);
        for scheme in [kde::tests::DARK, kde::tests::LIGHT] {
            use_kde(scheme, cx);
            cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
            cx.read(|cx| {
                let theme = cx.theme();
                let p = SidebarPalette::new(cx);
                assert!(contrast(p.text, theme.sidebar) >= TEXT_CONTRAST - 0.01);
                assert!(contrast(p.selected_text, theme.sidebar_accent) >= TEXT_CONTRAST - 0.01);
            });
        }
    }

    #[gpui_kit::test]
    fn resizing_keeps_the_theme(cx: &mut TestAppContext) {
        setup(cx);
        let mut p = prefs(ThemePreference::Dark, "Tokyo Storm");
        cx.update(|cx| apply(&p, None, cx));
        p.font_size = 20.0;
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "Tokyo Storm");
            assert_eq!(cx.theme().font_size, px(20.0));
        });
    }

    #[gpui_kit::test]
    fn zoom_scales_the_interface_size(cx: &mut TestAppContext) {
        setup(cx);
        let mut p = prefs(ThemePreference::Dark, "");
        cx.update(|cx| apply(&p, None, cx));
        cx.update(|cx| set_zoom(1.5, cx));
        cx.read(|cx| assert_eq!(cx.theme().font_size, px(24.0)));
        p.font_size = 20.0;
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_eq!(cx.theme().font_size, px(30.0)));
        cx.update(|cx| set_zoom(1.0, cx));
        cx.read(|cx| assert_eq!(cx.theme().font_size, px(20.0)));
    }

    #[gpui_kit::test]
    fn sidebar_rows_read_in_every_theme_and_glass(cx: &mut TestAppContext) {
        use crate::glass::{Glass, chrome_surfaces};
        use crate::style::{ICON_CONTRAST, SidebarPalette, TEXT_CONTRAST, contrast};

        setup(cx);
        for (name, dark) in cx.read(theme_names) {
            let mode = if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            };
            cx.update(|cx| apply(&prefs(mode, &name), None, cx));
            for glass in [
                Glass::Opaque,
                Glass::Subtle { blurred: true },
                Glass::Strong { blurred: true },
                Glass::Subtle { blurred: false },
                Glass::Strong { blurred: false },
                Glass::Desktop { opacity: 0x60 },
                Glass::Desktop { opacity: 0xe6 },
            ] {
                cx.update(|cx| cx.set_global(glass));
                cx.read(|cx| {
                    let theme = cx.theme();
                    let p = SidebarPalette::new(cx);
                    let idle = chrome_surfaces(&[theme.sidebar], cx);
                    let lit = chrome_surfaces(&[theme.sidebar, theme.sidebar_accent], cx);
                    let check = |what: &str, fg, on: &[gpui_kit::Hsla], min| {
                        for bg in on {
                            let ratio = contrast(fg, *bg);
                            assert!(ratio >= min - 0.01, "{name} {glass:?} {what}: {ratio:.2}");
                        }
                    };
                    check("text", p.text, &idle, TEXT_CONTRAST);
                    check("icon", p.icon, &idle, ICON_CONTRAST);
                    check("hovered icon", p.icon, &lit, ICON_CONTRAST);
                    check("caption", p.caption, &idle, TEXT_CONTRAST);
                    check("selected text", p.selected_text, &lit, TEXT_CONTRAST);
                    check("selected icon", p.selected_icon, &lit, ICON_CONTRAST);
                });
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[gpui_kit::test]
    fn the_modal_dim_never_reaches_gpui_kit(cx: &mut TestAppContext) {
        setup(cx);
        for (name, dark) in cx.read(theme_names) {
            let mode = if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            };
            cx.update(|cx| apply(&prefs(mode, &name), None, cx));
            // Anything left here dims the shadow margin around the window.
            cx.read(|cx| assert_eq!(cx.theme().overlay.a, 0., "{name}"));
            let dim = cx.read(|cx| cx.global::<ModalDim>().0);
            assert!(dim.a > 0., "{name} lost its dim");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[gpui_kit::test]
    fn the_modal_dim_lasts_as_long_as_a_dialog_or_sheet(cx: &mut TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::WindowExt as _;

        setup(cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        let (handle, _) = cx.update(|cx| {
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| gpui_kit::Empty)
            })
            .expect("open test window")
        });
        let dim = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| modal_dim(window, cx))
                .expect("window is open")
        };
        assert_eq!(dim(cx), None);

        cx.update_window(handle, |_, window, cx| {
            window.open_dialog(cx, |dialog, _, _| dialog);
        })
        .expect("window is open");
        cx.run_until_parked();
        assert!(dim(cx).is_some(), "a dialog is open");
        cx.update_window(handle, |_, window, cx| window.close_dialog(cx))
            .expect("window is open");
        cx.run_until_parked();
        assert_eq!(dim(cx), None);

        cx.update_window(handle, |_, window, cx| {
            window.open_sheet(cx, |sheet, _, _| sheet);
        })
        .expect("window is open");
        cx.run_until_parked();
        assert!(dim(cx).is_some(), "a sheet is open");
    }

    #[gpui_kit::test]
    fn status_colours_read_as_text_on_every_bundled_theme(cx: &mut TestAppContext) {
        setup(cx);
        for json in BUNDLED {
            for (name, dark) in family_members(json) {
                cx.update(|cx| {
                    let theme = if dark {
                        ThemePreference::Dark
                    } else {
                        ThemePreference::Light
                    };
                    apply(&prefs(theme, &name), None, cx);
                    let t = cx.theme();
                    for (what, color, on_it) in [
                        ("warning", t.warning, t.warning_foreground),
                        ("danger", t.danger, t.danger_foreground),
                        ("info", t.info, t.info_foreground),
                        ("success", t.success, t.success_foreground),
                    ] {
                        for surface in [t.background, t.popover] {
                            let alert = contrast(color, tint(surface, color));
                            assert!(alert >= 4.5, "{name} {what}: {alert}");
                        }
                        let filled = contrast(on_it, color);
                        assert!(filled >= 4.5, "{name} {what} filled: {filled}");
                    }
                });
            }
        }
    }

    #[gpui_kit::test]
    fn status_colours_that_read_stay_as_the_theme_has_them(cx: &mut TestAppContext) {
        setup(cx);
        cx.update(|cx| {
            apply(&prefs(ThemePreference::Dark, DEFAULT_DARK), None, cx);
            assert_eq!(
                cx.theme().warning,
                file_color(DEFAULT_DARK, "warning.background")
            );
        });
    }

    #[test]
    fn gpui_kits_unstyled_defaults_are_not_offered() {
        // Covered by `theme_names` filtering `is_default`; the bundled files
        // must not reuse those names either.
        for json in BUNDLED {
            for (name, _) in family_members(json) {
                assert!(!name.starts_with("Default "), "{name}");
            }
        }
    }
}
