//! COSMIC's desktop theme, turned into an Irori theme.
//!
//! COSMIC keeps a finished light and dark theme side by side, one small RON
//! file per field, and every app reads them, so Irori does too rather than
//! bundling lookalikes. Settings the user never changed exist only as the
//! system's defaults, which is why a field is looked for in both places.
//! Nothing happens outside a COSMIC session: its files can stay behind on a
//! machine that tried it once, so the session decides, not the files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use futures::channel::mpsc;
use gpui_kit::{Rgba, rgb};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::Deserialize;

use crate::desktop_theme::{DesktopThemes, Palette, mix};

const DARK_ID: &str = "com.system76.CosmicTheme.Dark";
const LIGHT_ID: &str = "com.system76.CosmicTheme.Light";
const MODE_ID: &str = "com.system76.CosmicTheme.Mode";

/// What the theme ids have in common, to tell their changes from those of the
/// many other settings under the same folder.
const THEME_PREFIX: &str = "com.system76.CosmicTheme.";

/// The theme's fields Irori uses, each a file of its own.
const FIELDS: &[&str] = &[
    "accent",
    "background",
    "corner_radii",
    "destructive",
    "frosted_windows",
    "palette",
    "primary",
    "secondary",
    "success",
    "warning",
];

/// Whether this is a COSMIC session.
pub(crate) fn running() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| names_cosmic(&desktops))
}

fn names_cosmic(desktops: &str) -> bool {
    desktops
        .split(':')
        .any(|desktop| desktop.eq_ignore_ascii_case("COSMIC"))
}

/// COSMIC's settings of the user, which win.
fn user_root() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("cosmic"))
}

/// Where COSMIC looks for settings: the user's, then the system's defaults.
fn roots() -> Vec<PathBuf> {
    let data = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    user_root()
        .into_iter()
        .chain(data.split(':').map(|dir| Path::new(dir).join("cosmic")))
        .collect()
}

/// The contents of the files making up COSMIC's theme, by field.
type ThemeFiles = BTreeMap<&'static str, String>;

/// Everything Irori reads, kept as text to tell cheaply whether it changed.
#[derive(PartialEq)]
pub(crate) struct Files {
    is_dark: Option<String>,
    light: ThemeFiles,
    dark: ThemeFiles,
}

/// The newest version of a setting. The schema moves with COSMIC, and only the
/// newest is kept up to date: an older folder may hold a theme long replaced.
fn latest_version(roots: &[PathBuf], id: &str) -> Option<u32> {
    roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root.join(id)).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_prefix('v')?.parse().ok())
        .max()
}

fn read_field(roots: &[PathBuf], id: &str, version: u32, field: &str) -> Option<String> {
    roots.iter().find_map(|root| {
        std::fs::read_to_string(root.join(id).join(format!("v{version}")).join(field)).ok()
    })
}

fn read_theme(roots: &[PathBuf], id: &str) -> ThemeFiles {
    let Some(version) = latest_version(roots, id) else {
        return ThemeFiles::new();
    };
    FIELDS
        .iter()
        .filter_map(|field| Some((*field, read_field(roots, id, version, field)?)))
        .collect()
}

fn read_from(roots: &[PathBuf]) -> Option<Files> {
    let files = Files {
        is_dark: read_field(roots, MODE_ID, 1, "is_dark"),
        light: read_theme(roots, LIGHT_ID),
        dark: read_theme(roots, DARK_ID),
    };
    (!files.light.is_empty() || !files.dark.is_empty()).then_some(files)
}

/// COSMIC's theme files, if there are any.
pub(crate) fn read() -> Option<Files> {
    read_from(&roots())
}

/// Watch for theme changes. The whole folder is watched because a theme's
/// folder only appears once the user first changes something.
pub(crate) fn watch() -> Option<(RecommendedWatcher, mpsc::UnboundedReceiver<()>)> {
    let root = user_root().filter(|root| root.is_dir())?;
    let (tx, rx) = mpsc::unbounded();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            if !event.kind.is_access() && event.paths.iter().any(|path| is_theme_path(path)) {
                let _ = tx.unbounded_send(());
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("watching COSMIC's theme: {e}");
                return None;
            }
        };
    if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
        tracing::warn!("watching {}: {e}", root.display());
    }
    Some((watcher, rx))
}

fn is_theme_path(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|part| part.starts_with(THEME_PREFIX))
    })
}

/// A colour as COSMIC writes it: hex since schema v2, and channels from 0 to 1
/// before it.
#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
enum Color {
    Hex(HexColor),
    Channels {
        red: f32,
        green: f32,
        blue: f32,
        #[serde(default = "opaque")]
        alpha: f32,
    },
}

fn opaque() -> f32 {
    1.0
}

/// `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy)]
struct HexColor(Rgba);

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_hex(&text)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("not a colour: {text}")))
    }
}

fn parse_hex(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(rgb(value)),
        8 => Some(Rgba {
            a: (value & 0xff) as f32 / 255.0,
            ..rgb(value >> 8)
        }),
        _ => None,
    }
}

impl Color {
    fn rgba(self) -> Rgba {
        match self {
            Self::Hex(HexColor(color)) => color,
            Self::Channels {
                red,
                green,
                blue,
                alpha,
            } => Rgba {
                r: red,
                g: green,
                b: blue,
                a: alpha,
            },
        }
    }

    /// The colour as it shows on `surface`: with frosted glass on, COSMIC's
    /// colours can be see-through, while Irori's translucency is its own.
    fn over(self, surface: Rgba) -> Rgba {
        let color = self.rgba();
        mix(surface, color, color.a)
    }
}

/// A surface and the text on it.
#[derive(Deserialize)]
struct Container {
    base: Color,
    on: Color,
}

/// A coloured element, such as the accent.
#[derive(Deserialize)]
struct Component {
    base: Color,
}

/// The colours of the theme's palette Irori has no field for.
#[derive(Deserialize)]
struct Accents {
    /// COSMIC's teal: its bluest accent is the indigo.
    accent_blue: Color,
    accent_indigo: Color,
    accent_purple: Color,
    accent_orange: Color,
}

/// How round the desktop's corners are, per corner.
#[derive(Deserialize)]
struct CornerRadii {
    radius_s: [f32; 4],
    radius_m: [f32; 4],
}

fn parse<'a, T: Deserialize<'a>>(files: &'a ThemeFiles, field: &str) -> Option<T> {
    let text = files.get(field)?;
    ron::from_str(text)
        .inspect_err(|e| tracing::warn!("COSMIC's theme, {field}: {e}"))
        .ok()
}

/// The palette for one of COSMIC's themes. `None` when even the background,
/// its text or the accent is missing, as they would be in a schema Irori does
/// not know yet. Other colours fall back to what suits them.
fn palette(files: &ThemeFiles, dark: bool) -> Option<Palette> {
    let window: Container = parse(files, "background")?;
    let accent: Component = parse(files, "accent")?;
    let background = window
        .base
        .over(rgb(if dark { 0x000000 } else { 0xffffff }));
    let foreground = window.on.over(background);
    let color = |field: &str, fallback: u32| {
        parse::<Component>(files, field).map_or(rgb(fallback), |c| c.base.over(background))
    };
    let surface = |field: &str, fallback: Rgba| {
        parse::<Container>(files, field).map_or(fallback, |c| c.base.over(background))
    };
    let tint = |amount: f32| mix(background, foreground, amount);
    let panel = surface("primary", tint(0.04));
    let accents = parse::<Accents>(files, "palette");
    let extra = |pick: fn(&Accents) -> Color, fallback: u32| {
        accents
            .as_ref()
            .map_or(rgb(fallback), |a| pick(a).over(background))
    };
    let radii = parse::<CornerRadii>(files, "corner_radii");
    let radius = |pick: fn(&CornerRadii) -> f32, fallback: u32| {
        radii.as_ref().map_or(fallback, |r| pick(r).round() as u32)
    };
    Some(Palette {
        name: if dark { "COSMIC Dark" } else { "COSMIC Light" },
        dark,
        accent: accent.base.over(background),
        background,
        sidebar: panel,
        panel,
        // Irori washes pages with this colour, which expects a small step from
        // the background. COSMIC's next surface up is too far in light themes,
        // where it is nearly white on grey, so it only goes to what floats.
        raised: panel,
        popover: surface("secondary", tint(0.08)),
        foreground,
        muted: mix(foreground, background, 0.3),
        bright_foreground: foreground,
        red: color("destructive", 0xf2555a),
        green: color("success", 0x4fd18b),
        yellow: color("warning", 0xf5c451),
        blue: extra(|a| a.accent_indigo, 0x5aa9ff),
        magenta: extra(|a| a.accent_purple, 0xc58af9),
        cyan: extra(|a| a.accent_blue, 0x4fd1d9),
        orange: extra(|a| a.accent_orange, 0xeb6834),
        radius: radius(|r| r.radius_s[0], 10),
        radius_lg: radius(|r| r.radius_m[0], 14),
    })
}

/// The Irori themes for COSMIC's files.
pub(crate) fn themes(files: &Files) -> Option<DesktopThemes> {
    let theme = |files: &ThemeFiles, dark| palette(files, dark)?.theme();
    let light = theme(&files.light, false);
    let dark = theme(&files.dark, true);
    let is_dark = files
        .is_dark
        .as_deref()
        .and_then(|text| ron::from_str(text).ok())
        // COSMIC starts dark until told otherwise.
        .unwrap_or(true);
    let current = if is_dark { &files.dark } else { &files.light };
    let frosted = current
        .get("frosted_windows")
        .and_then(|text| ron::from_str(text).ok());
    (light.is_some() || dark.is_some()).then_some(DesktopThemes {
        light,
        dark,
        is_dark,
        frosted,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use gpui_kit::component::ThemeMode;

    use super::*;

    fn container(base: &str, on: &str) -> String {
        format!(r#"(base: "{base}", component: (base: "{on}"), divider: "{on}", on: "{on}")"#)
    }

    fn component(base: &str) -> String {
        format!(r##"(base: "{base}", hover: "{base}", on: "#000000ff")"##)
    }

    /// One of COSMIC's themes, as the files look on a real install.
    fn theme_files(dark: bool) -> ThemeFiles {
        let (bg, panel, raised, text) = if dark {
            ("#1b1b1bff", "#272727ff", "#303030ff", "#c4c4c4ff")
        } else {
            ("#e4e4e4ff", "#f0f0f0ff", "#fafafaff", "#2b2b2bff")
        };
        let palette = r##"(
            name: "cosmic-dark",
            accent_blue: "#94ebebff",
            accent_indigo: "#95c4fcff",
            accent_purple: "#e79cfeff",
            accent_orange: "#fff173ff",
            neutral_0: "#000000ff",
        )"##;
        ThemeFiles::from([
            ("background", container(bg, text)),
            ("primary", container(panel, text)),
            ("secondary", container(raised, text)),
            ("accent", component("#63d0dfff")),
            ("success", component("#acf7d2ff")),
            ("destructive", component("#fda1a0ff")),
            ("warning", component("#f7e062ff")),
            ("palette", palette.to_string()),
            (
                "corner_radii",
                "(radius_0: (0.0, 0.0, 0.0, 0.0), radius_s: (8.0, 8.0, 8.0, 8.0), \
                 radius_m: (16.0, 16.0, 16.0, 16.0))"
                    .to_string(),
            ),
        ])
    }

    pub(crate) fn files(is_dark: bool) -> Files {
        Files {
            is_dark: Some(is_dark.to_string()),
            light: theme_files(false),
            dark: theme_files(true),
        }
    }

    #[test]
    fn only_a_cosmic_session_counts() {
        assert!(names_cosmic("COSMIC"));
        assert!(names_cosmic("pop:COSMIC"));
        assert!(!names_cosmic("GNOME"));
        assert!(!names_cosmic("ubuntu:GNOME"));
        assert!(!names_cosmic(""));
    }

    #[test]
    fn reads_both_themes_and_the_mode() {
        let found = themes(&files(false)).unwrap();
        assert!(!found.is_dark);
        assert_eq!(found.light.unwrap().mode, ThemeMode::Light);
        assert_eq!(found.dark.unwrap().mode, ThemeMode::Dark);
        assert!(themes(&files(true)).unwrap().is_dark);
    }

    #[test]
    fn surfaces_are_cosmics_own() {
        let colors = palette(&theme_files(true), true).unwrap().colors();
        assert_eq!(colors["background"], "#1b1b1b");
        assert_eq!(colors["panel.background"], "#272727");
        assert_eq!(colors["popover.background"], "#303030");
        assert_eq!(colors["primary.background"], "#63d0df");
        assert_eq!(colors["danger.background"], "#fda1a0");
    }

    #[test]
    fn corners_follow_the_desktop() {
        let square = palette(
            &ThemeFiles::from([
                (
                    "corner_radii",
                    "(radius_s: (2.0, 2.0, 2.0, 2.0), radius_m: (2.0, 2.0, 2.0, 2.0))".to_string(),
                ),
                ("background", theme_files(true)["background"].clone()),
                ("accent", theme_files(true)["accent"].clone()),
            ]),
            true,
        )
        .unwrap();
        assert_eq!((square.radius, square.radius_lg), (2, 2));
    }

    #[test]
    fn colours_written_in_capitals_read_too() {
        let mut files = theme_files(true);
        files.insert("accent", component("#63D0DFFF"));
        let colors = palette(&files, true).unwrap().colors();
        assert_eq!(colors["primary.background"], "#63d0df");
    }

    #[test]
    fn colours_written_as_channels_read_too() {
        let mut files = theme_files(true);
        files.insert(
            "accent",
            "(base: (red: 1.0, green: 0.5, blue: 0.0, alpha: 1.0))".to_string(),
        );
        let colors = palette(&files, true).unwrap().colors();
        assert_eq!(colors["primary.background"], "#ff8000");
    }

    #[test]
    fn a_see_through_surface_is_flattened() {
        let mut files = theme_files(true);
        files.insert("primary", container("#ffffff80", "#c4c4c4ff"));
        let colors = palette(&files, true).unwrap().colors();
        assert_eq!(colors["panel.background"], "#8d8d8d");
    }

    #[test]
    fn optional_fields_can_be_missing_or_broken() {
        let mut files = theme_files(true);
        files.remove("palette");
        files.insert("warning", "not ron at all".to_string());
        files.insert("corner_radii", "(radius_s: 4)".to_string());
        let palette = palette(&files, true).unwrap();
        assert_eq!((palette.radius, palette.radius_lg), (10, 14));
        assert!(palette.theme().is_some());
    }

    #[test]
    fn no_background_or_accent_gives_no_theme() {
        for field in ["background", "accent"] {
            let mut files = theme_files(true);
            files.remove(field);
            assert!(palette(&files, true).is_none(), "{field}");
        }
        let nothing = Files {
            is_dark: None,
            light: ThemeFiles::new(),
            dark: ThemeFiles::new(),
        };
        assert!(themes(&nothing).is_none());
    }

    #[test]
    fn frosted_glass_is_read_from_the_theme_showing() {
        let mut found = files(true);
        assert_eq!(themes(&found).unwrap().frosted, None);
        found.dark.insert("frosted_windows", "true".to_string());
        found.light.insert("frosted_windows", "false".to_string());
        assert_eq!(themes(&found).unwrap().frosted, Some(true));
        found.is_dark = Some("false".to_string());
        assert_eq!(themes(&found).unwrap().frosted, Some(false));
    }

    #[test]
    fn a_missing_mode_means_dark() {
        let mut found = files(false);
        found.is_dark = None;
        assert!(themes(&found).unwrap().is_dark);
    }

    #[test]
    fn the_session_mode_keeps_working_with_one_theme_left() {
        let mut found = files(true);
        found.light = ThemeFiles::new();
        let themes = themes(&found).unwrap();
        assert!(themes.light.is_none());
        assert!(themes.dark.is_some());
    }

    fn write(root: &Path, id: &str, version: &str, field: &str, text: &str) {
        let dir = root.join(id).join(version);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(field), text).unwrap();
    }

    #[test]
    fn the_user_wins_over_the_system_field_by_field() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("user"), dir.path().join("system"));
        write(&system, DARK_ID, "v2", "accent", "system accent");
        write(&system, DARK_ID, "v2", "background", "system background");
        write(&user, DARK_ID, "v2", "accent", "user accent");
        write(&user, MODE_ID, "v1", "is_dark", "false");

        let files = read_from(&[user, system]).unwrap();
        assert_eq!(files.dark["accent"], "user accent");
        assert_eq!(files.dark["background"], "system background");
        assert_eq!(files.is_dark.as_deref(), Some("false"));
        assert!(files.light.is_empty());
    }

    #[test]
    fn the_newest_version_is_the_only_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("user"), dir.path().join("system"));
        write(&user, DARK_ID, "v1", "accent", "old accent");
        write(&user, DARK_ID, "v1", "background", "old background");
        write(&system, DARK_ID, "v2", "accent", "new accent");

        let files = read_from(&[user, system]).unwrap();
        assert_eq!(files.dark["accent"], "new accent");
        assert!(!files.dark.contains_key("background"));
    }

    #[test]
    fn no_files_means_no_theme() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_from(&[dir.path().to_path_buf()]).is_none());
    }

    #[test]
    fn only_theme_files_wake_the_watcher() {
        let root = Path::new("/home/me/.config/cosmic");
        assert!(is_theme_path(&root.join(DARK_ID).join("v2/accent")));
        assert!(is_theme_path(&root.join(MODE_ID).join("v1/is_dark")));
        assert!(!is_theme_path(
            &root.join("com.system76.CosmicPanel/v1/size")
        ));
    }
}
