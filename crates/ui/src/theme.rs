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
//! it as the user switches. Elsewhere, macOS and Windows included, the default
//! is Irori's own in the system's accent colour, where the system has one (see
//! [`crate::desktop_accent`]). With `iroriUi.wallpaperColors` on, the colours
//! pywal, wallust or matugen made from the wallpaper (see [`crate::wallpaper`])
//! are the default instead, on any system. A theme the user picked wins.
//!
//! What the system asks for accessibility applies to every theme, a picked one
//! included: stronger borders for more contrast, a solid window for less
//! transparency, and on Windows the high-contrast colours instead of any theme.
//!
//! An accent colour the user picks replaces the accent of whichever theme
//! shows, the desktop's included, held legible on its surfaces the way the
//! system's accent is. The interface typeface is Manrope unless the user picks
//! another installed family.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Hsla, Rgba, SharedString, Task, Window, WindowAppearance, px};
use irori_core::settings::{ThemePreference, UiPrefs};
use tokio::runtime::Runtime;

use crate::desktop_accent::{Accent, ContrastColors, SystemColors};
pub use crate::desktop_theme::Desktop;
use crate::desktop_theme::{DesktopThemes, Frost, accented, high_contrast, system_accented};
use crate::style::{ICON_CONTRAST, TEXT_CONTRAST, contrast, legible_towards};
use crate::{cosmic, desktop_accent, kde, omarchy, wallpaper};

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

/// Irori's font files, for drawing outside the window (the shareable cards).
pub const FONTS: &[&[u8]] = &[
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

/// The wallpaper's colours, while the user has them on and the file has
/// them. They win over a desktop's theme, which stays for when they are off.
struct WallpaperTheme(DesktopTheme);

impl Global for WallpaperTheme {}

/// The colours file being followed, and the task following it, which stops
/// when dropped.
struct WallpaperFollow {
    file: Option<PathBuf>,
    _task: Option<Task<()>>,
}

impl Global for WallpaperFollow {}

/// The last revision the wallpaper's colours had.
struct WallpaperRevision(u64);

impl Global for WallpaperRevision {}

/// Irori's own themes in the system's accent colour. Like [`DesktopTheme`] they
/// stay out of the registry, and they keep their names: to the user they are
/// the default theme, and "Irori Night" in the list is the untinted one.
struct DesktopAccent {
    /// The system's accents for light and dark themes they were made from.
    from: (Option<Accent>, Option<Accent>),
    light: Option<Rc<ThemeConfig>>,
    dark: Option<Rc<ThemeConfig>>,
    revision: u64,
}

impl Global for DesktopAccent {}

/// What the system asks of every theme for accessibility.
#[derive(Default)]
struct SystemLook {
    more_contrast: bool,
    transparency: Option<bool>,
    /// The system's high-contrast colours, and the theme made of them.
    high_contrast: Option<(ContrastColors, Rc<ThemeConfig>)>,
    revision: u64,
}

impl Global for SystemLook {}

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
    /// So do the wallpaper's. Zero while they aren't there.
    wallpaper: u64,
    /// So does Irori's own when the accent changes, and tinted or not. Zero
    /// while the accent isn't showing, so a picked theme ignores its changes.
    accent: u64,
    /// What the system asks for accessibility, which every theme follows.
    system: u64,
    /// The accent the user picked, if any.
    picked_accent: Option<u32>,
    font_family: SharedString,
}

impl Global for Applied {}

/// The font families installed, read once: the system's list doesn't change
/// while Irori runs often enough to be worth watching.
struct FontNames(Rc<[SharedString]>);

impl Global for FontNames {}

/// Every font family the interface can use, sorted, Manrope included.
pub fn font_names(cx: &mut App) -> Rc<[SharedString]> {
    if let Some(names) = cx.try_global::<FontNames>() {
        return names.0.clone();
    }
    let mut names: Vec<SharedString> = cx
        .text_system()
        .all_font_names()
        .into_iter()
        // Families whose names start with a dot are the system's private ones.
        .filter(|name| !name.starts_with('.'))
        .map(SharedString::from)
        .collect();
    // Irori loads Manrope itself; not every platform lists added fonts.
    if !names.iter().any(|name| name.as_ref() == FONT_FAMILY) {
        names.push(FONT_FAMILY.into());
        names.sort();
    }
    let names: Rc<[SharedString]> = names.into();
    cx.set_global(FontNames(names.clone()));
    names
}

/// The typeface `prefs` ask for: the picked family while it is installed,
/// else Manrope.
fn font_family(prefs: &UiPrefs, cx: &mut App) -> SharedString {
    let picked = prefs.font_family.trim();
    if picked.is_empty() || picked == FONT_FAMILY {
        return FONT_FAMILY.into();
    }
    if font_names(cx).iter().any(|name| name.as_ref() == picked) {
        SharedString::from(picked.to_string())
    } else {
        FONT_FAMILY.into()
    }
}

/// Whether the system or a followed desktop asks windows not to be
/// see-through ("Reduce transparency", "Transparency effects" off, COSMIC's
/// frosted windows off). Pictures behind content stay off then too.
pub fn reduce_transparency(cx: &App) -> bool {
    desktop_frost(cx).is_some_and(|frost| !frost.on)
}

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

/// Tint the default theme with the system's accent colour, take what it asks
/// for accessibility, and keep up with both. The wait is on the UI thread for
/// the reason [`follow`]'s first read is, and it is bounded because the portal,
/// unlike a file, may take its time to start.
fn follow_accent(cx: &mut App) {
    let backend = crate::backend(cx);
    let Some(mut changes) = desktop_accent::watch(&backend) else {
        return;
    };
    if let Some(colors) = first_accent(&mut changes, backend.runtime()) {
        set_accent(&colors, cx);
        set_system(&colors, cx);
    }
    cx.spawn(async move |cx| {
        while let Some(mut colors) = changes.next().await {
            // A colour picker can send a change per step of a drag.
            cx.background_executor().timer(DESKTOP_SETTLE).await;
            while let Ok(newer) = changes.try_recv() {
                colors = newer;
            }
            cx.update(|cx| {
                let accent = set_accent(&colors, cx);
                let system = set_system(&colors, cx);
                if accent || system {
                    let prefs = crate::backend(cx).settings.app().ui.clone();
                    apply(&prefs, None, cx);
                }
                if system {
                    // The shell works out the window's translucency when
                    // settings change, and the system's say in it may have.
                    let settings = crate::models(cx).settings.clone();
                    settings.update(cx, |_, cx| cx.notify());
                }
            });
        }
    })
    .detach();
}

/// The colours, if they come within [`ACCENT_WAIT`]. Whatever is later stays in
/// `changes`. The timer needs the runtime, so it has to start inside it.
fn first_accent(changes: &mut desktop_accent::Changes, runtime: &Runtime) -> Option<SystemColors> {
    runtime
        .block_on(async { tokio::time::timeout(ACCENT_WAIT, changes.next()).await })
        .ok()
        .flatten()
}

/// Tint Irori's themes with the system's accents, or stop. False if they were
/// the accents already.
fn set_accent(colors: &SystemColors, cx: &mut App) -> bool {
    let from = (colors.light.clone(), colors.dark.clone());
    let current = cx.try_global::<DesktopAccent>();
    if current.map_or((None, None), |a| a.from.clone()) == from {
        return false;
    }
    let revision = current.map_or(0, |a| a.revision) + 1;
    let tinted = |name: &str, accent: &Option<Accent>| {
        let base = registered(name, cx)?;
        system_accented(&base, accent.as_ref()?).map(Rc::new)
    };
    let (light, dark) = (
        tinted(DEFAULT_LIGHT, &from.0),
        tinted(DEFAULT_DARK, &from.1),
    );
    cx.set_global(DesktopAccent {
        from,
        light,
        dark,
        revision,
    });
    true
}

/// Take what the system asks for accessibility. False if nothing changed.
fn set_system(colors: &SystemColors, cx: &mut App) -> bool {
    let current = cx.try_global::<SystemLook>();
    let had = current.map_or((false, None, None), |look| {
        let contrast = look.high_contrast.as_ref().map(|(c, _)| *c);
        (look.more_contrast, look.transparency, contrast)
    });
    if had
        == (
            colors.more_contrast,
            colors.transparency,
            colors.high_contrast,
        )
    {
        return false;
    }
    let revision = current.map_or(0, |look| look.revision) + 1;
    let high_contrast = colors
        .high_contrast
        .and_then(|system| Some((system, Rc::new(high_contrast(&system)?))));
    cx.set_global(SystemLook {
        more_contrast: colors.more_contrast,
        transparency: colors.transparency,
        high_contrast,
        revision,
    });
    true
}

/// The theme of the system's high-contrast colours, while it is in high
/// contrast.
fn high_contrast_theme(cx: &App) -> Option<Rc<ThemeConfig>> {
    let (_, theme) = cx.try_global::<SystemLook>()?.high_contrast.as_ref()?;
    Some(theme.clone())
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

/// What the desktop wants of see-through windows, on one that has a setting:
/// a followed desktop's own, else the system's ("Reduce transparency" on
/// macOS, "Transparency effects" on Windows).
pub(crate) fn desktop_frost(cx: &App) -> Option<Frost> {
    if let Some(frost) = cx.try_global::<DesktopTheme>().and_then(|d| d.frost) {
        return Some(frost);
    }
    let on = cx.try_global::<SystemLook>()?.transparency?;
    Some(Frost { on, opacity: None })
}

/// The desktop whose theme is the default one, if Irori follows one here:
/// [`Desktop::Wallpaper`] while the wallpaper's colours are.
pub fn followed_desktop(cx: &App) -> Option<Desktop> {
    shown_desktop(cx).map(|d| d.desktop)
}

/// The theme that is the default one in place of Irori's: the wallpaper's,
/// else the desktop's.
fn shown_desktop(cx: &App) -> Option<&DesktopTheme> {
    cx.try_global::<WallpaperTheme>()
        .map(|w| &w.0)
        .or_else(|| cx.try_global::<DesktopTheme>())
}

/// Use the wallpaper's colours as the default theme while
/// `iroriUi.wallpaperColors` is on, and follow the file it names as the tool
/// rewrites it and as the settings change. Call once models exist.
pub(crate) fn follow_wallpaper(cx: &mut App) {
    let prefs = crate::backend(cx).settings.app().ui.clone();
    sync_wallpaper(&prefs, true, cx);
    let settings = crate::models(cx).settings.clone();
    cx.observe(&settings, |settings, cx| {
        let prefs = settings.read(cx).app.ui.clone();
        sync_wallpaper(&prefs, false, cx);
    })
    .detach();
}

/// The colours file `prefs` ask to follow, if any.
fn wallpaper_file(prefs: &UiPrefs) -> Option<PathBuf> {
    prefs
        .wallpaper_colors
        .then(|| wallpaper::resolve(&prefs.wallpaper_colors_file))
        .flatten()
}

/// Start or stop following the colours file, or follow another, as `prefs`
/// say. Only Irori's start reads on the UI thread, for the reason [`follow`]'s
/// first read is; a switch in the settings reads in the background.
fn sync_wallpaper(prefs: &UiPrefs, first: bool, cx: &mut App) {
    let file = wallpaper_file(prefs);
    if cx
        .try_global::<WallpaperFollow>()
        .is_some_and(|f| f.file == file)
    {
        return;
    }
    // Dropping the old task stops its watch.
    if cx.has_global::<WallpaperFollow>() {
        cx.remove_global::<WallpaperFollow>();
    }
    let had = cx.has_global::<WallpaperTheme>();
    if had {
        cx.remove_global::<WallpaperTheme>();
    }
    let Some(path) = file.clone() else {
        cx.set_global(WallpaperFollow {
            file: None,
            _task: None,
        });
        if had {
            reapply(cx);
        }
        return;
    };
    let mut shown = None;
    if first {
        shown = wallpaper::read(&path).as_deref().and_then(wallpaper::parse);
        if let Some(themes) = shown.as_ref().and_then(wallpaper::Colors::themes) {
            set_wallpaper(themes, cx);
        }
    }
    let task = cx.spawn(async move |cx| {
        let load = |path: PathBuf| async move {
            wallpaper::read(&path).as_deref().and_then(wallpaper::parse)
        };
        if !first {
            let found = cx.background_executor().spawn(load(path.clone())).await;
            let themes = found.as_ref().and_then(wallpaper::Colors::themes);
            shown = found;
            cx.update(|cx| {
                if let Some(themes) = themes {
                    set_wallpaper(themes, cx);
                }
                if had || cx.has_global::<WallpaperTheme>() {
                    reapply(cx);
                }
            });
        }
        let Some((mut watch, mut changes)) = wallpaper::Watch::new(&path) else {
            return;
        };
        while changes.next().await.is_some() {
            cx.background_executor().timer(DESKTOP_SETTLE).await;
            while changes.try_recv().is_ok() {}
            watch.rewatch();
            let found = cx.background_executor().spawn(load(path.clone())).await;
            // A tool may remove the file before writing the next one; the
            // write wakes this up again. The same colours again need nothing.
            if found.is_none() || found == shown {
                continue;
            }
            let Some(themes) = found.as_ref().and_then(wallpaper::Colors::themes) else {
                tracing::warn!("the wallpaper's colours have no usable theme");
                continue;
            };
            shown = found;
            cx.update(|cx| {
                set_wallpaper(themes, cx);
                reapply(cx);
            });
        }
    });
    cx.set_global(WallpaperFollow {
        file,
        _task: Some(task),
    });
}

fn set_wallpaper(themes: DesktopThemes, cx: &mut App) {
    // Counted across files and switches, so another file's colours of the
    // same theme name are never taken for the ones applied.
    let revision = cx.try_global::<WallpaperRevision>().map_or(0, |r| r.0) + 1;
    cx.set_global(WallpaperRevision(revision));
    cx.set_global(WallpaperTheme(DesktopTheme {
        desktop: Desktop::Wallpaper,
        light: themes.light.map(Rc::new),
        dark: themes.dark.map(Rc::new),
        is_dark: themes.is_dark,
        frost: None,
        revision,
    }));
}

/// Apply the user's preferences again, after a theme they decide between
/// changed.
fn reapply(cx: &mut App) {
    let prefs = crate::backend(cx).settings.app().ui.clone();
    apply(&prefs, None, cx);
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
    if let Some(theme) = high_contrast_theme(cx) {
        return (Some(theme.clone()), Some(theme));
    }
    let find = |name: &str| registered(name, cx);
    let desktop = shown_desktop(cx).filter(|_| prefs.theme_name.is_empty());
    let accent = showing_accent(prefs, cx);
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
    let (light, dark) = (for_mode(false), for_mode(true));
    match prefs.accent_rgb() {
        Some(picked) => {
            let picked = gpui_kit::rgb(picked);
            let tint = |theme: Option<Rc<ThemeConfig>>| {
                let theme = theme?;
                Some(accented(&theme, picked).map(Rc::new).unwrap_or(theme))
            };
            (tint(light), tint(dark))
        }
        None => (light, dark),
    }
}

/// Irori's themes in the system's accent, while they are the ones to show.
fn showing_accent<'a>(prefs: &UiPrefs, cx: &'a App) -> Option<&'a DesktopAccent> {
    cx.try_global::<DesktopAccent>().filter(|_| {
        prefs.theme_name.is_empty()
            && prefs.system_accent
            && prefs.accent_rgb().is_none()
            && !cx.has_global::<WallpaperTheme>()
    })
}

/// Apply theme, mode and base font size (application zoom) from preferences.
///
/// Cheap to call often: it does nothing when the result would not change, and
/// only rescales when just the interface size did.
pub fn apply(prefs: &UiPrefs, window: Option<&mut Window>, cx: &mut App) {
    let high_contrast = high_contrast_theme(cx);
    let is_dark = match prefs.theme {
        // The system's high-contrast theme comes in one mode.
        _ if high_contrast.is_some() => high_contrast.as_ref().is_some_and(|t| t.mode.is_dark()),
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        // The desktop's theme is what decides, and it can be set more quickly
        // than the portal reports the change.
        ThemePreference::System if prefs.theme_name.is_empty() && shown_desktop(cx).is_some() => {
            shown_desktop(cx).is_some_and(|d| d.is_dark)
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
    let font_family = font_family(prefs, cx);
    let (light, dark) = pair(prefs, cx);
    let name = |t: &Option<Rc<ThemeConfig>>| t.as_ref().map(|t| t.name.clone()).unwrap_or_default();
    let next = Applied {
        light: name(&light),
        dark: name(&dark),
        is_dark,
        font_size: font_size.to_bits(),
        desktop: cx.try_global::<DesktopTheme>().map_or(0, |d| d.revision),
        wallpaper: cx
            .try_global::<WallpaperTheme>()
            .map_or(0, |w| w.0.revision),
        accent: showing_accent(prefs, cx).map_or(0, |a| a.revision),
        system: cx.try_global::<SystemLook>().map_or(0, |s| s.revision),
        // The system's high-contrast theme keeps its own colours.
        picked_accent: prefs
            .accent_rgb()
            .filter(|_| high_contrast_theme(cx).is_none()),
        font_family: font_family.clone(),
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
        // The typeface is part of Irori's identity, whatever the colours,
        // unless the user picked another.
        theme.font_family = font_family;
    });
    let mode = if is_dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, window, cx);
    Theme::update(cx, hold_status_colors);
    if cx
        .try_global::<SystemLook>()
        .is_some_and(|s| s.more_contrast)
    {
        Theme::update(cx, strengthen_borders);
    }
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

/// Borders drawn to be seen, at the 3:1 WCAG asks of what marks a control, for
/// a user who asked the system for more contrast. Themes draw them faint on
/// purpose; a border that already reads stays as the theme has it.
fn strengthen_borders(theme: &mut Theme) {
    let towards = if theme.mode.is_dark() {
        gpui_kit::white()
    } else {
        gpui_kit::black()
    };
    let surfaces = [
        theme.background,
        theme.secondary,
        theme.popover,
        theme.sidebar,
    ];
    let hold = |color: Hsla| legible_towards(color, &surfaces, ICON_CONTRAST + 0.1, towards);
    let t = &mut theme.colors;
    for border in [
        &mut t.border,
        &mut t.input,
        &mut t.sidebar_border,
        &mut t.title_bar_border,
        &mut t.status_bar_border,
        &mut t.window_border,
        &mut t.table_row_border,
    ] {
        *border = hold(*border);
    }
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
    use crate::desktop_theme;

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
    fn a_picked_accent_tints_any_theme_and_stays_legible(cx: &mut TestAppContext) {
        setup(cx);
        for (mode, name) in [
            (ThemePreference::Dark, ""),
            (ThemePreference::Light, ""),
            (ThemePreference::Dark, "Catppuccin Mocha"),
        ] {
            cx.update(|cx| apply(&prefs(mode, name), None, cx));
            let own = cx.read(|cx| cx.theme().primary);
            let picked = UiPrefs {
                accent_color: "#ff2d95".into(),
                ..prefs(mode, name)
            };
            cx.update(|cx| apply(&picked, None, cx));
            cx.read(|cx| {
                let theme = cx.theme();
                assert_ne!(theme.primary, own, "{name} kept its accent");
                let pink = theme.link.to_rgb();
                assert!(pink.r > pink.g && pink.b > pink.g, "{name}: {pink:?}");
                assert!(
                    contrast(theme.link, theme.background) >= TEXT_CONTRAST - 0.05,
                    "{name}: links don't read"
                );
            });
            // Clearing it brings the theme's own back.
            cx.update(|cx| apply(&prefs(mode, name), None, cx));
            assert_eq!(cx.read(|cx| cx.theme().primary), own);
        }
    }

    #[gpui_kit::test]
    fn a_missing_font_falls_back_to_manrope(cx: &mut TestAppContext) {
        setup(cx);
        let missing = UiPrefs {
            font_family: "No Such Typeface 2049".into(),
            ..UiPrefs::default()
        };
        cx.update(|cx| apply(&missing, None, cx));
        assert_eq!(cx.read(|cx| cx.theme().font_family.clone()), FONT_FAMILY);
        let names = cx.update(font_names);
        assert!(names.iter().any(|n| n.as_ref() == FONT_FAMILY), "{names:?}");
        if let Some(other) = names.iter().find(|n| n.as_ref() != FONT_FAMILY) {
            let picked = UiPrefs {
                font_family: other.to_string(),
                ..UiPrefs::default()
            };
            cx.update(|cx| apply(&picked, None, cx));
            assert_eq!(&cx.read(|cx| cx.theme().font_family.clone()), other);
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

    fn use_wallpaper(file: &str, cx: &mut TestAppContext) {
        let themes = wallpaper::parse(file).unwrap().themes().unwrap();
        cx.update(|cx| set_wallpaper(themes, cx));
    }

    #[gpui_kit::test]
    fn the_wallpapers_colours_beat_the_desktops(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::System, "");
        use_omarchy(omarchy::tests::LIGHT, cx);
        use_wallpaper(wallpaper::tests::PYWAL_DARK, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "Wallpaper Dark");
            assert!(cx.theme().mode.is_dark());
            assert_eq!(followed_desktop(cx), Some(Desktop::Wallpaper));
        });

        // Turned off, Omarchy's theme is back.
        cx.update(|cx| {
            cx.remove_global::<WallpaperTheme>();
            apply(&p, None, cx);
        });
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "Omarchy");
            assert_eq!(followed_desktop(cx), Some(Desktop::Omarchy));
        });
    }

    #[gpui_kit::test]
    fn new_wallpaper_colours_show_at_once(cx: &mut TestAppContext) {
        setup(cx);
        let p = prefs(ThemePreference::System, "");
        use_wallpaper(wallpaper::tests::PYWAL_DARK, cx);
        cx.update(|cx| apply(&p, None, cx));
        let before = cx.read(|cx| cx.theme().background);
        let recoloured = wallpaper::tests::PYWAL_DARK.replace("#0f1418", "#201010");
        use_wallpaper(&recoloured, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_ne!(cx.theme().background, before));
    }

    #[gpui_kit::test]
    fn a_theme_the_user_picked_beats_the_wallpaper(cx: &mut TestAppContext) {
        setup(cx);
        use_wallpaper(wallpaper::tests::PYWAL_DARK, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, "Tokyo Storm"), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Tokyo Storm"));
    }

    #[gpui_kit::test]
    fn a_template_in_one_mode_leaves_the_other_to_irori(cx: &mut TestAppContext) {
        setup(cx);
        use_wallpaper(wallpaper::tests::MATUGEN_TEMPLATE, cx);
        cx.update(|cx| apply(&prefs(ThemePreference::System, ""), None, cx));
        cx.read(|cx| {
            assert_eq!(cx.theme().theme_name().as_ref(), "Wallpaper Light");
            assert!(!cx.theme().mode.is_dark());
        });
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), DEFAULT_DARK));
    }

    #[test]
    fn the_wallpaper_is_followed_only_when_turned_on() {
        let off = UiPrefs {
            wallpaper_colors_file: "~/colours.json".into(),
            ..UiPrefs::default()
        };
        assert_eq!(wallpaper_file(&off), None);
        let on = UiPrefs {
            wallpaper_colors: true,
            ..UiPrefs::default()
        };
        assert_eq!(wallpaper_file(&on), wallpaper::default_file());
        let picked = UiPrefs {
            wallpaper_colors: true,
            ..off
        };
        assert_eq!(
            wallpaper_file(&picked),
            dirs::home_dir().map(|h| h.join("colours.json"))
        );
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

    fn portal(accent: u32) -> SystemColors {
        SystemColors::accent(Some(gpui_kit::rgb(accent)))
    }

    fn use_accent(accent: u32, cx: &mut TestAppContext) -> bool {
        cx.update(|cx| set_accent(&portal(accent), cx))
    }

    fn no_accent() -> SystemColors {
        SystemColors::accent(None)
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
        let blue = portal(0x3584e4);
        tx.unbounded_send(blue.clone()).unwrap();
        assert_eq!(first_accent(&mut changes, &runtime()), Some(blue));
    }

    #[test]
    fn a_slow_portal_only_delays_its_answer() {
        let (tx, mut changes) = mpsc::unbounded();
        assert_eq!(first_accent(&mut changes, &runtime()), None);

        let blue = portal(0x3584e4);
        tx.unbounded_send(blue.clone()).unwrap();
        assert_eq!(changes.try_recv(), Ok(blue));
    }

    #[test]
    fn no_portal_ends_the_wait_at_once() {
        let (tx, mut changes) = mpsc::unbounded::<SystemColors>();
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

        assert!(
            !cx.update(|cx| set_accent(&no_accent(), cx)),
            "nothing to change"
        );

        use_accent(0x3584e4, cx);
        cx.update(|cx| apply(&p, None, cx));
        cx.read(|cx| assert_ne!(cx.theme().primary, own));

        assert!(cx.update(|cx| set_accent(&no_accent(), cx)));
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
            set_accent(&portal(0xe62d42), cx);
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
                    set_accent(&SystemColors::accent(Some(accent)), cx);
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
                    set_accent(&SystemColors::accent(Some(accent)), cx);
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

    #[gpui_kit::test]
    fn turning_the_system_accent_off_brings_irori_back(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        let off = UiPrefs {
            system_accent: false,
            ..prefs(ThemePreference::Dark, "")
        };
        cx.update(|cx| apply(&off, None, cx));
        cx.read(|cx| {
            let own = file_color(DEFAULT_DARK, "primary.background");
            assert_eq!(cx.theme().primary, own);
        });
        cx.update(|cx| apply(&prefs(ThemePreference::Dark, ""), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().primary, gpui_kit::rgb(0x3584e4).into()));
    }

    /// Windows' default blue, with its shade for each mode.
    fn windows_blue() -> SystemColors {
        let accent = |shades: [u32; 4]| desktop_accent::Accent {
            shades: shades.map(gpui_kit::rgb).to_vec(),
            selection: None,
        };
        SystemColors {
            light: Some(accent([0x0078d4, 0x005a9e, 0x004275, 0x001a68])),
            dark: Some(accent([0x0078d4, 0x429ce3, 0x76b9ed, 0x99ebff])),
            transparency: Some(true),
            ..SystemColors::default()
        }
    }

    #[gpui_kit::test]
    fn each_mode_takes_the_systems_shade_for_it(cx: &mut TestAppContext) {
        setup(cx);
        let colors = windows_blue();
        cx.update(|cx| set_accent(&colors, cx));
        for (mode, accent) in [
            (ThemePreference::Light, &colors.light),
            (ThemePreference::Dark, &colors.dark),
        ] {
            cx.update(|cx| apply(&prefs(mode, ""), None, cx));
            let shades: Vec<gpui_kit::Hsla> = accent
                .as_ref()
                .unwrap()
                .shades
                .iter()
                .map(|c| (*c).into())
                .collect();
            cx.read(|cx| {
                let primary = cx.theme().primary;
                assert!(shades.contains(&primary), "{mode:?} {primary:?}");
            });
        }
    }

    fn use_system(colors: SystemColors, cx: &mut TestAppContext) -> bool {
        cx.update(|cx| set_system(&colors, cx))
    }

    #[gpui_kit::test]
    fn more_contrast_draws_borders_that_are_seen(cx: &mut TestAppContext) {
        setup(cx);
        for mode in [ThemePreference::Dark, ThemePreference::Light] {
            for name in ["", "Catppuccin Latte", "Tokyo Storm"] {
                use_system(SystemColors::default(), cx);
                let p = prefs(mode, name);
                cx.update(|cx| apply(&p, None, cx));
                let faint = cx.read(|cx| cx.theme().border);
                use_system(
                    SystemColors {
                        more_contrast: true,
                        ..SystemColors::default()
                    },
                    cx,
                );
                cx.update(|cx| apply(&p, None, cx));
                cx.read(|cx| {
                    let t = cx.theme();
                    for (what, border) in [
                        ("border", t.border),
                        ("input", t.input),
                        ("sidebar", t.sidebar_border),
                        ("title bar", t.title_bar_border),
                    ] {
                        for surface in [t.background, t.secondary, t.popover, t.sidebar] {
                            let ratio = contrast(border, surface);
                            assert!(ratio >= ICON_CONTRAST, "{name} {mode:?} {what}: {ratio}");
                        }
                    }
                    assert_ne!(t.border, faint, "{name} {mode:?}");
                });
            }
        }
    }

    fn night_sky() -> ContrastColors {
        let c = gpui_kit::rgb;
        ContrastColors {
            window: c(0x000000),
            window_text: c(0xffffff),
            highlight: c(0xd6b4fd),
            highlight_text: c(0x2b2b2b),
            hotlight: c(0x8080ff),
            gray_text: c(0xa6a6a6),
            button_face: c(0x000000),
            button_text: c(0xffffff),
        }
    }

    #[gpui_kit::test]
    fn high_contrast_beats_every_theme_and_decides_the_mode(cx: &mut TestAppContext) {
        setup(cx);
        use_accent(0x3584e4, cx);
        let on = SystemColors {
            more_contrast: true,
            transparency: Some(false),
            high_contrast: Some(night_sky()),
            ..SystemColors::default()
        };
        assert!(use_system(on.clone(), cx));
        assert!(!use_system(on, cx), "nothing to change");
        for p in [
            prefs(ThemePreference::Light, ""),
            prefs(ThemePreference::Light, "Catppuccin Latte"),
        ] {
            cx.update(|cx| apply(&p, None, cx));
            cx.read(|cx| {
                let t = cx.theme();
                assert_eq!(t.theme_name().as_ref(), desktop_theme::HIGH_CONTRAST);
                assert!(t.mode.is_dark());
                assert_eq!(t.background, gpui_kit::rgb(0x000000).into());
                assert_eq!(t.primary, gpui_kit::rgb(0xd6b4fd).into());
                assert_eq!(t.border, gpui_kit::rgb(0xffffff).into());
            });
        }
        assert!(use_system(SystemColors::default(), cx));
        cx.update(|cx| apply(&prefs(ThemePreference::Light, "Catppuccin Latte"), None, cx));
        cx.read(|cx| assert_eq!(cx.theme().theme_name().as_ref(), "Catppuccin Latte"));
    }

    #[gpui_kit::test]
    fn the_systems_transparency_switch_is_its_frost(cx: &mut TestAppContext) {
        setup(cx);
        assert!(!use_system(portal(0x3584e4), cx), "an accent asks nothing");
        cx.read(|cx| assert_eq!(desktop_frost(cx), None));
        for on in [false, true] {
            use_system(
                SystemColors {
                    transparency: Some(on),
                    ..SystemColors::default()
                },
                cx,
            );
            cx.read(|cx| assert_eq!(desktop_frost(cx), Some(Frost { on, opacity: None })));
        }
        // A followed desktop's own setting comes first.
        let frosted = Frost {
            on: true,
            opacity: Some(0xc2),
        };
        cx.update(|cx| {
            let mut themes =
                DesktopThemes::single(omarchy::tests::theme(omarchy::tests::DARK).unwrap());
            themes.frost = Some(frosted);
            set_desktop(Desktop::Omarchy, themes, cx);
        });
        cx.read(|cx| assert_eq!(desktop_frost(cx), Some(frosted)));
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
