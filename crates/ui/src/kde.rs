//! KDE Plasma's colour scheme, turned into an Irori theme.
//!
//! Plasma colours every app from one colour scheme at a time, so Irori reads it
//! too rather than bundling Breeze lookalikes. Applying a scheme copies it into
//! `kdeglobals`, but a desktop whose scheme was never changed may only name it
//! there, or not even that, which is why a colour is looked for in `kdeglobals`
//! and then in the scheme's own file. Nothing happens outside a Plasma session:
//! any KDE app leaves a `kdeglobals` behind on whatever desktop it ran, so the
//! session decides, not the files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures::channel::mpsc;
use gpui_kit::{Rgba, rgb};
use irori_core::paths::sanitize;
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::desktop_theme::{DesktopThemes, Palette, mix, on};

const GLOBALS: &str = "kdeglobals";

/// Holds the defaults of the Global Theme in use. Plasma puts it on
/// `XDG_CONFIG_DIRS` for its session, which a program started some other way
/// may not inherit.
const DEFAULTS_DIR: &str = "kdedefaults";

/// What Plasma shows when nothing names a scheme.
const DEFAULT_SCHEME: &str = "BreezeLight";

const VIEW: &str = "Colors:View";
const WINDOW: &str = "Colors:Window";
const BUTTON: &str = "Colors:Button";

/// Whether this is a Plasma session.
pub(crate) fn running() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| names_kde(&desktops))
}

fn names_kde(desktops: &str) -> bool {
    desktops
        .split(':')
        .any(|desktop| desktop.eq_ignore_ascii_case("KDE"))
}

/// The user's own folder, and the same folder under the home folder. A Flatpak
/// has a folder of its own, but the desktop's stays where it always is, which
/// Irori's Flatpak can read.
fn user_dirs(own: Option<PathBuf>, in_home: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = own
        .into_iter()
        .chain(dirs::home_dir().map(|home| home.join(in_home)))
        .collect();
    dirs.dedup();
    dirs
}

fn user_config_dirs() -> Vec<PathBuf> {
    user_dirs(dirs::config_dir(), ".config")
}

fn list_from_env(name: &str, fallback: &str) -> Vec<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| fallback.to_string())
        .split(':')
        .map(PathBuf::from)
        .collect()
}

/// Where `kdeglobals` can be, the one that wins first.
fn config_dirs() -> Vec<PathBuf> {
    let user = user_config_dirs();
    let defaults: Vec<PathBuf> = user.iter().map(|dir| dir.join(DEFAULTS_DIR)).collect();
    user.into_iter()
        .chain(defaults)
        .chain(list_from_env("XDG_CONFIG_DIRS", "/etc/xdg"))
        .collect()
}

/// Where colour schemes are kept, the one that wins first.
fn data_dirs() -> Vec<PathBuf> {
    user_dirs(dirs::data_dir(), ".local/share")
        .into_iter()
        .chain(list_from_env(
            "XDG_DATA_DIRS",
            "/usr/local/share:/usr/share",
        ))
        .collect()
}

/// Everything Irori reads, kept as text to tell cheaply whether it changed.
#[derive(PartialEq)]
pub(crate) struct Files {
    /// Every `kdeglobals` there is, the one that wins first.
    globals: Vec<String>,
    /// The active colour scheme.
    scheme: Option<String>,
}

fn read_from(config_dirs: &[PathBuf], data_dirs: &[PathBuf]) -> Option<Files> {
    let globals: Vec<String> = config_dirs
        .iter()
        .filter_map(|dir| std::fs::read_to_string(dir.join(GLOBALS)).ok())
        .collect();
    let configs: Vec<Config> = globals.iter().map(|text| parse_config(text)).collect();
    let name = configs
        .iter()
        .find_map(|config| config.get("General")?.get("ColorScheme"))
        .map_or_else(|| DEFAULT_SCHEME.to_string(), |name| sanitize(name));
    let file = Path::new("color-schemes").join(format!("{name}.colors"));
    let scheme = data_dirs
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(&file)).ok());
    (!globals.is_empty() || scheme.is_some()).then_some(Files { globals, scheme })
}

/// Plasma's colour settings, if there are any.
pub(crate) fn read() -> Option<Files> {
    read_from(&config_dirs(), &data_dirs())
}

/// Watch for colour scheme changes. KDE replaces `kdeglobals` instead of
/// writing to it, so what can be watched is the folder holding it. That folder
/// is a busy one, but its other files stop at the check here.
pub(crate) fn watch() -> Option<(RecommendedWatcher, mpsc::UnboundedReceiver<()>)> {
    let user = user_config_dirs();
    let defaults: Vec<PathBuf> = user.iter().map(|dir| dir.join(DEFAULTS_DIR)).collect();
    let dirs: Vec<PathBuf> = user
        .into_iter()
        .chain(defaults)
        .filter(|dir| dir.is_dir())
        .collect();
    if dirs.is_empty() {
        return None;
    }
    let (tx, rx) = mpsc::unbounded();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            if !event.kind.is_access() && event.paths.iter().any(|path| is_globals(path)) {
                let _ = tx.unbounded_send(());
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("watching KDE's colours: {e}");
                return None;
            }
        };
    for dir in &dirs {
        if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
            tracing::warn!("watching {}: {e}", dir.display());
        }
    }
    Some((watcher, rx))
}

fn is_globals(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == GLOBALS)
}

/// The groups of a KConfig file by name, each with its keys.
type Config = HashMap<String, HashMap<String, String>>;

/// `name` without KConfig's flags, as in `BackgroundNormal[$e]`.
fn without_flags(name: &str) -> &str {
    name.split_once("[$").map_or(name, |(name, _)| name)
}

/// Read a KConfig file. A group nested in another keeps its full name, as in
/// `Colors:Header][Inactive`, so its keys don't reach the outer group.
fn parse_config(text: &str) -> Config {
    let mut config = Config::new();
    let mut group = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            group = header
                .strip_suffix(']')
                .map(|name| without_flags(name).trim_end_matches(']').to_string());
        } else if let (Some(group), Some((key, value))) = (&group, line.split_once('=')) {
            config.entry(group.clone()).or_default().insert(
                without_flags(key).trim().to_string(),
                value.trim().to_string(),
            );
        }
    }
    config
}

/// A colour as KConfig writes it, `r,g,b` from 0 to 255 with an alpha that
/// colour schemes have no use for, or as it also takes it, `#rrggbb`.
fn parse_color(text: &str) -> Option<Rgba> {
    if let Some(digits) = text.strip_prefix('#') {
        return u32::from_str_radix(digits, 16)
            .ok()
            .filter(|_| digits.len() == 6)
            .map(rgb);
    }
    let channels = text
        .split(',')
        .map(|channel| channel.trim().parse().ok())
        .collect::<Option<Vec<u8>>>()?;
    match channels[..] {
        [r, g, b] | [r, g, b, _] => Some(rgb(u32::from_be_bytes([0, r, g, b]))),
        _ => None,
    }
}

/// The colour for `key` of `group` in the first file that has a usable one.
fn find_color(configs: &[Config], group: &str, key: &str) -> Option<Rgba> {
    configs
        .iter()
        .find_map(|config| parse_color(config.get(group)?.get(key)?))
}

/// The palette for Plasma's colours, the first of `configs` winning. `None`
/// when even the background, its text or the accent is missing. Other colours
/// fall back to what suits them.
fn palette(configs: &[Config]) -> Option<Palette> {
    let color = |group: &str, key: &str| find_color(configs, group, key);
    let view_or_window = |key: &str| color(VIEW, key).or_else(|| color(WINDOW, key));

    let background = view_or_window("BackgroundNormal")?;
    let foreground = view_or_window("ForegroundNormal")?;
    // Not the selection first: Plasma darkens or lightens it from the accent to
    // keep selected text readable, so it can be far from what the user picked.
    let accent = color("General", "AccentColor")
        .or_else(|| color(VIEW, "DecorationFocus"))
        .or_else(|| color("Colors:Selection", "BackgroundNormal"))?;
    let tint = |amount: f32| mix(background, foreground, amount);
    let status = |key: &str, fallback: u32| view_or_window(key).unwrap_or(rgb(fallback));
    let panel = color(WINDOW, "BackgroundNormal").unwrap_or_else(|| tint(0.04));
    let red = status("ForegroundNegative", 0xda4453);
    let yellow = status("ForegroundNeutral", 0xf67400);
    Some(Palette {
        name: "KDE Plasma",
        // Colour schemes don't say, and a Plasma session has no other mode.
        dark: on(background) == rgb(0xffffff),
        accent,
        background,
        // As in KDE's own apps: content on the view's colour, chrome and the
        // cards inside content on the window's.
        sidebar: panel,
        panel,
        raised: panel,
        // The only surface schemes keep lifted off both of those.
        popover: color(BUTTON, "BackgroundNormal").unwrap_or_else(|| tint(0.08)),
        foreground,
        muted: view_or_window("ForegroundInactive")
            .unwrap_or_else(|| mix(foreground, background, 0.3)),
        bright_foreground: foreground,
        red,
        green: status("ForegroundPositive", 0x27ae60),
        yellow,
        blue: status("ForegroundLink", 0x2980b9),
        magenta: status("ForegroundVisited", 0x9b59b6),
        cyan: status("ForegroundActive", 0x3daee9),
        // Schemes have no orange, and their neutral text is one already, which
        // charts need told apart from this.
        orange: mix(yellow, red, 0.5),
        // Schemes don't hold corners. These sit near Breeze's.
        radius: 6,
        radius_lg: 10,
    })
}

/// The Irori theme for Plasma's colours.
pub(crate) fn themes(files: &Files) -> Option<DesktopThemes> {
    let configs: Vec<Config> = files
        .globals
        .iter()
        .chain(&files.scheme)
        .map(|text| parse_config(text))
        .collect();
    palette(&configs)?.theme().map(DesktopThemes::single)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use gpui_kit::component::ThemeMode;

    use super::*;

    /// A dark colour scheme, as `kdeglobals` holds it once one was applied.
    pub(crate) const DARK: &str = "\
[ColorEffects:Disabled]
Color=56,56,56

[Colors:Button]
BackgroundNormal=41,44,48
ForegroundNormal=252,252,252

[Colors:Header]
BackgroundNormal=46,50,54

[Colors:Header][Inactive]
BackgroundNormal=32,35,38

[Colors:Selection]
BackgroundNormal=61,174,233
ForegroundNormal=252,252,252

[Colors:View]
BackgroundAlternate=29,31,34
BackgroundNormal=20,22,24
DecorationFocus=61,174,233
ForegroundActive=61,174,233
ForegroundInactive=161,169,177
ForegroundLink=29,153,243
ForegroundNegative=218,68,83
ForegroundNeutral=246,116,0
ForegroundNormal=252,252,252
ForegroundPositive=39,174,96
ForegroundVisited=155,89,182

[Colors:Window]
BackgroundNormal=32,35,38
ForegroundNormal=252,252,252

[General]
ColorScheme=BreezeDark

[KDE]
LookAndFeelPackage=org.kde.breezedark.desktop
";

    /// A light colour scheme, as its file in `color-schemes` looks.
    pub(crate) const LIGHT: &str = "\
[Colors:Button]
BackgroundNormal=252,252,252

[Colors:Selection]
BackgroundNormal=61,174,233

[Colors:View]
BackgroundNormal=252,252,252
DecorationFocus=61,174,233
ForegroundInactive=112,125,138
ForegroundLink=41,128,185
ForegroundNegative=218,68,83
ForegroundNeutral=246,116,0
ForegroundNormal=35,38,41
ForegroundPositive=39,174,96
ForegroundVisited=155,89,182

[Colors:Window]
BackgroundNormal=239,240,241
ForegroundNormal=35,38,41

[General]
Name=Breeze Light
";

    pub(crate) fn files(globals: &str) -> Files {
        Files {
            globals: vec![globals.to_string()],
            scheme: None,
        }
    }

    fn generated(texts: &[&str]) -> serde_json::Map<String, serde_json::Value> {
        let configs: Vec<Config> = texts.iter().map(|text| parse_config(text)).collect();
        palette(&configs).unwrap().colors()
    }

    #[test]
    fn only_a_plasma_session_counts() {
        assert!(names_kde("KDE"));
        assert!(names_kde("Bazzite:KDE"));
        assert!(!names_kde("GNOME"));
        assert!(!names_kde("ubuntu:GNOME"));
        assert!(!names_kde("gamescope"));
        assert!(!names_kde(""));
    }

    #[test]
    fn surfaces_are_the_schemes_own() {
        let colors = generated(&[DARK]);
        assert_eq!(colors["background"], "#141618");
        assert_eq!(colors["sidebar.background"], "#202326");
        assert_eq!(colors["panel.background"], "#202326");
        assert_eq!(colors["popover.background"], "#292c30");
        assert_eq!(colors["primary.background"], "#3daee9");
        assert_eq!(colors["danger.background"], "#da4453");
        assert_eq!(colors["success.background"], "#27ae60");
    }

    #[test]
    fn the_scheme_decides_the_mode() {
        let mode = |text: &str| themes(&files(text)).unwrap().is_dark;
        assert!(mode(DARK));
        assert!(!mode(LIGHT));
        let theme = themes(&files(LIGHT)).unwrap().light.unwrap();
        assert_eq!(theme.mode, ThemeMode::Light);
    }

    #[test]
    fn the_accent_the_user_picked_wins() {
        let picked = format!("{DARK}\n[General]\nAccentColor=200,30,100\n");
        assert_eq!(generated(&[&picked])["primary.background"], "#c81e64");
    }

    #[test]
    fn the_accent_is_the_focus_colour_not_the_darkened_selection() {
        let text = DARK
            .replace(
                "[Colors:Selection]\nBackgroundNormal=61,174,233",
                "[Colors:Selection]\nBackgroundNormal=184,6,78",
            )
            .replace("DecorationFocus=61,174,233", "DecorationFocus=255,0,102");
        assert_eq!(generated(&[&text])["primary.background"], "#ff0066");
    }

    #[test]
    fn the_selection_stands_in_for_a_missing_focus_colour() {
        let text = DARK.replace("DecorationFocus=61,174,233", "DecorationFocus=none");
        let text = text.replace(
            "[Colors:Selection]\nBackgroundNormal=61,174,233",
            "[Colors:Selection]\nBackgroundNormal=1,2,3",
        );
        assert_eq!(generated(&[&text])["primary.background"], "#010203");
    }

    #[test]
    fn the_window_stands_in_for_a_missing_view() {
        let text = DARK.replace("[Colors:View]", "[Colors:Elsewhere]");
        assert_eq!(generated(&[&text])["background"], "#202326");
    }

    #[test]
    fn kdeglobals_wins_over_the_scheme_key_by_key() {
        let globals = "[Colors:View]\nBackgroundNormal=1,2,3\n";
        let colors = generated(&[globals, LIGHT]);
        assert_eq!(colors["background"], "#010203");
        assert_eq!(colors["panel.background"], "#eff0f1");
        assert_eq!(colors["danger.background"], "#da4453");
    }

    #[test]
    fn a_broken_colour_falls_through_to_the_next_file() {
        let globals = "[Colors:View]\nBackgroundNormal=not a colour\n";
        assert_eq!(generated(&[globals, LIGHT])["background"], "#fcfcfc");
    }

    #[test]
    fn a_nested_group_stays_out_of_the_one_around_it() {
        let text = format!("{DARK}\n[Colors:View][Inactive]\nBackgroundNormal=9,9,9\n");
        assert_eq!(generated(&[&text])["background"], "#141618");
    }

    #[test]
    fn flags_comments_and_line_ends_are_understood() {
        let config = parse_config(
            "# a comment\r\n\
             stray=1\r\n\
             [Colors:View][$i]\r\n\
             BackgroundNormal[$e] = 1,2,3\r\n\
             \r\n\
             not a key\r\n\
             [broken\r\n\
             BackgroundNormal=4,5,6\r\n",
        );
        assert_eq!(
            parse_color(&config["Colors:View"]["BackgroundNormal"]),
            parse_color("1,2,3")
        );
        assert_eq!(config.len(), 1);
    }

    #[test]
    fn colours_are_read_as_kconfig_writes_them() {
        assert_eq!(parse_color("61,174,233"), Some(rgb(0x3daee9)));
        assert_eq!(parse_color("61, 174, 233"), Some(rgb(0x3daee9)));
        assert_eq!(parse_color("61,174,233,128"), Some(rgb(0x3daee9)));
        assert_eq!(parse_color("#3daee9"), Some(rgb(0x3daee9)));
        for broken in ["", "61,174", "1,2,3,4,5", "256,0,0", "#3dae", "#gggggg"] {
            assert_eq!(parse_color(broken), None, "{broken}");
        }
    }

    #[test]
    fn no_background_text_or_accent_gives_no_theme() {
        let without = |key: &str| {
            let kept: Vec<&str> = DARK.lines().filter(|l| !l.starts_with(key)).collect();
            kept.join("\n")
        };
        for key in ["BackgroundNormal", "ForegroundNormal"] {
            assert!(themes(&files(&without(key))).is_none(), "{key}");
        }
        let text = without("DecorationFocus").replace("[Colors:Selection]", "[Colors:Elsewhere]");
        assert!(themes(&files(&text)).is_none());
        assert!(themes(&files("")).is_none());
    }

    #[test]
    fn optional_colours_can_be_missing() {
        let palette = palette(&[parse_config(
            "[Colors:View]\n\
             BackgroundNormal=20,22,24\n\
             ForegroundNormal=252,252,252\n\
             [Colors:Selection]\n\
             BackgroundNormal=61,174,233\n",
        )])
        .unwrap();
        assert_eq!(palette.red, rgb(0xda4453));
        assert!(palette.theme().is_some());
    }

    #[test]
    fn dim_scheme_text_is_held_readable() {
        use gpui_kit::component::try_parse_color;

        use crate::style::{TEXT_CONTRAST, contrast};

        let dim = LIGHT.replace(
            "ForegroundInactive=112,125,138",
            "ForegroundInactive=210,210,210",
        );
        let colors = generated(&[&dim]);
        let color = |key: &str| try_parse_color(colors[key].as_str().unwrap()).unwrap();
        for surface in ["background", "panel.background", "popover.background"] {
            let ratio = contrast(color("muted.foreground"), color(surface));
            assert!(ratio >= TEXT_CONTRAST - 0.01, "{surface}: {ratio}");
        }
    }

    #[test]
    fn charts_tell_orange_from_the_neutral_colour() {
        let colors = generated(&[DARK]);
        assert_ne!(colors["chart.2"], colors["chart.4"]);
    }

    fn write(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_scheme_named_in_kdeglobals_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let (user, data) = (dir.path().join("config"), dir.path().join("data"));
        write(&user, GLOBALS, "[General]\nColorScheme=Mine\n");
        write(&data, "color-schemes/Mine.colors", DARK);
        write(&data, "color-schemes/BreezeLight.colors", LIGHT);

        let files = read_from(&[user], &[data]).unwrap();
        assert_eq!(files.scheme.as_deref(), Some(DARK));
    }

    #[test]
    fn the_first_folder_naming_a_scheme_wins() {
        let dir = tempfile::tempdir().unwrap();
        let (user, defaults) = (dir.path().join("config"), dir.path().join("defaults"));
        let data = dir.path().join("data");
        write(&user, GLOBALS, "[KDE]\nSingleClick=false\n");
        write(&defaults, GLOBALS, "[General]\nColorScheme=Mine\n");
        write(&data, "color-schemes/Mine.colors", DARK);

        let files = read_from(&[user, defaults], &[data]).unwrap();
        assert_eq!(files.globals.len(), 2);
        assert_eq!(files.scheme.as_deref(), Some(DARK));
    }

    #[test]
    fn a_scheme_in_the_users_data_wins_over_the_systems() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("user"), dir.path().join("system"));
        write(&user, "color-schemes/BreezeLight.colors", DARK);
        write(&system, "color-schemes/BreezeLight.colors", LIGHT);

        let files = read_from(&[], &[user, system]).unwrap();
        assert_eq!(files.scheme.as_deref(), Some(DARK));
    }

    #[test]
    fn a_desktop_that_names_no_scheme_shows_breeze_light() {
        let dir = tempfile::tempdir().unwrap();
        let (user, data) = (dir.path().join("config"), dir.path().join("data"));
        write(&user, GLOBALS, "[KDE]\nSingleClick=false\n");
        write(&data, "color-schemes/BreezeLight.colors", LIGHT);

        let found = themes(&read_from(&[user], &[data]).unwrap()).unwrap();
        assert!(!found.is_dark);
    }

    #[test]
    fn a_scheme_name_cannot_leave_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (user, data) = (dir.path().join("config"), dir.path().join("data"));
        write(&user, GLOBALS, "[General]\nColorScheme=../secret\n");
        write(&data, "secret.colors", DARK);
        fs::create_dir_all(data.join("color-schemes")).unwrap();

        let files = read_from(&[user], &[data]).unwrap();
        assert_eq!(files.scheme, None);
    }

    #[test]
    fn no_files_means_no_theme() {
        let dir = tempfile::tempdir().unwrap();
        let empty = [dir.path().to_path_buf()];
        assert!(read_from(&empty, &empty).is_none());
    }

    #[test]
    fn only_kdeglobals_wakes_the_watcher() {
        let root = Path::new("/home/me/.config");
        assert!(is_globals(&root.join(GLOBALS)));
        assert!(is_globals(&root.join("kdedefaults").join(GLOBALS)));
        assert!(!is_globals(&root.join("kdeglobals.lock")));
        assert!(!is_globals(&root.join("dolphinrc")));
    }
}
