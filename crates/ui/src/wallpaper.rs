//! Colours pywal, wallust or matugen made from the wallpaper, turned into an
//! Irori theme.
//!
//! These tools theme a whole desktop from its wallpaper by writing colour
//! files the other programs read, so Irori reads one too:
//!
//! - pywal's `colors.json`, in `$PYWAL_CACHE_DIR` or `~/.cache/wal`: the
//!   terminal's `special` background, foreground and cursor, and its sixteen
//!   `colors`. wallust writes the same file when its templates include
//!   pywal's (`pywal = true`, target `~/.cache/wal`); its own cache is named
//!   after each wallpaper and says nothing of which is current.
//! - matugen writes nothing of its own: it renders the user's templates. So
//!   Irori reads a file the user names, holding Material You colour roles
//!   (`primary`, `surface`, `on_surface`…): either `matugen … --json hex`'s
//!   output, old (`colors.dark.primary`) or new (`colors.primary.dark`), or a
//!   template of flat keys (see `docs/PLATFORMS.md`).
//!
//! The files exist without the user wanting Irori themed, and stay behind once
//! they stop using the tool, so this is off unless `iroriUi.wallpaperColors`
//! turns it on. The tools write by renaming, so the folder is watched, not the
//! file.

use std::path::{Path, PathBuf};

use futures::channel::mpsc;
use gpui_kit::{Rgba, rgb};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use serde_json::Value;

use crate::desktop_theme::{DesktopThemes, Palette, mix, on};

/// pywal's colours file, the one the setting means when it names none.
pub(crate) fn default_file() -> Option<PathBuf> {
    let dir = match std::env::var_os("PYWAL_CACHE_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => dirs::home_dir()?.join(".cache").join("wal"),
    };
    Some(dir.join("colors.json"))
}

/// The file `setting` (`iroriUi.wallpaperColorsFile`) names: pywal's when it
/// is empty, with a leading `~` meaning the home folder, as in the tools' own
/// configuration.
pub(crate) fn resolve(setting: &str) -> Option<PathBuf> {
    let setting = setting.trim();
    if setting.is_empty() {
        return default_file();
    }
    if setting == "~" {
        return dirs::home_dir();
    }
    if let Some(rest) = setting
        .strip_prefix("~/")
        .or_else(|| setting.strip_prefix("~\\"))
    {
        return Some(dirs::home_dir()?.join(rest));
    }
    Some(PathBuf::from(setting))
}

/// The colours file's contents, if it can be read.
pub(crate) fn read(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok()
}

/// What a colours file holds: a theme for one mode or both.
#[derive(Debug, PartialEq)]
pub(crate) struct Colors {
    light: Option<Palette>,
    dark: Option<Palette>,
    /// The mode the tool was run in, where both are there.
    is_dark: bool,
}

impl Colors {
    /// The themes these colours make. `None` when neither builds.
    pub(crate) fn themes(&self) -> Option<DesktopThemes> {
        let light = self.light.as_ref().and_then(Palette::theme);
        let dark = self.dark.as_ref().and_then(Palette::theme);
        let is_dark = match (&light, &dark) {
            (None, None) => return None,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (Some(_), Some(_)) => self.is_dark,
        };
        Some(DesktopThemes {
            light,
            dark,
            is_dark,
            frost: None,
        })
    }
}

/// Read a colours file, whichever tool wrote it. `None` when it holds no
/// background, text and accent colour.
pub(crate) fn parse(text: &str) -> Option<Colors> {
    let doc: Value = serde_json::from_str(text).ok()?;
    if doc.get("special").is_some_and(Value::is_object) {
        return pywal(&doc).map(|palette| {
            let is_dark = palette.dark;
            let (light, dark) = if is_dark {
                (None, Some(palette))
            } else {
                (Some(palette), None)
            };
            Colors {
                light,
                dark,
                is_dark,
            }
        });
    }
    material(&doc)
}

fn parse_color(text: &str) -> Option<Rgba> {
    let digits = text.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    // An alpha channel (`#rrggbbaa`) is the terminal's; Irori's surfaces stay
    // opaque and its own translucency decides.
    let digits = match digits.len() {
        6 => digits,
        8 => digits.get(..6)?,
        _ => return None,
    };
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Whether text on `background` would be white: a dark theme.
fn is_dark(background: Rgba) -> bool {
    on(background) == rgb(0xffffff)
}

fn name(dark: bool) -> &'static str {
    if dark {
        "Wallpaper Dark"
    } else {
        "Wallpaper Light"
    }
}

/// pywal's file. Its sixteen colours are hues of the wallpaper in terminal
/// order, not a red that means danger, so warnings and errors keep Irori's
/// own colours and only the accent and chart colours come from it.
fn pywal(doc: &Value) -> Option<Palette> {
    let special = doc.get("special");
    let colors = doc.get("colors");
    let get = |group: Option<&Value>, key: &str| parse_color(group?.get(key)?.as_str()?);
    let ansi = |n: u8| get(colors, &format!("color{n}"));

    let background = get(special, "background").or_else(|| ansi(0))?;
    let foreground = get(special, "foreground").or_else(|| ansi(15))?;
    let accent = ansi(4)
        .or_else(|| ansi(1))
        .or_else(|| get(special, "cursor"))?;
    let dark = is_dark(background);
    let tint = |amount: f32| mix(background, foreground, amount);
    Some(Palette {
        name: name(dark),
        dark,
        accent,
        background,
        // Darkening a light wallpaper's background towards black could leave
        // no text colour that reads on it, so light themes only tint it.
        sidebar: if dark {
            mix(background, rgb(0x000000), 0.15)
        } else {
            tint(0.04)
        },
        panel: tint(0.03),
        raised: tint(0.07),
        popover: if dark {
            tint(0.09)
        } else {
            mix(background, rgb(0xffffff), 0.6)
        },
        foreground,
        // The bright black terminal themes use for comments and hints.
        muted: ansi(8).unwrap_or_else(|| mix(foreground, background, 0.3)),
        bright_foreground: ansi(15).unwrap_or(foreground),
        red: rgb(0xf2555a),
        green: rgb(0x4fd18b),
        yellow: rgb(0xf5c451),
        blue: rgb(0x5aa9ff),
        magenta: ansi(5).unwrap_or(rgb(0xc58af9)),
        cyan: ansi(6).unwrap_or(rgb(0x4fd1d9)),
        orange: rgb(0xeb6834),
        radius: 10,
        radius_lg: 14,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Dark,
    Light,
}

impl Mode {
    fn key(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
}

/// A colour as matugen writes it: a string, or `{"color": "#…"}` in its
/// newer JSON output.
fn color_value(value: &Value) -> Option<Rgba> {
    match value {
        Value::String(text) => parse_color(text),
        Value::Object(map) => parse_color(map.get("color")?.as_str()?),
        _ => None,
    }
}

/// Material You roles, in any of the shapes matugen's JSON takes: name then
/// mode (`colors.primary.dark`, with `default` for the mode it was run in),
/// mode then name (`colors.dark.primary`, before matugen 3), or flat keys a
/// template wrote, the same for either mode.
fn material(doc: &Value) -> Option<Colors> {
    let colors = doc.get("colors").filter(|c| c.is_object()).unwrap_or(doc);
    let role = |names: &[&str], mode: Mode| {
        names.iter().find_map(|name| {
            if let Some(entry) = colors.get(*name) {
                if let Some(color) = color_value(entry) {
                    return Some(color);
                }
                if let Some(color) = entry
                    .get(mode.key())
                    .or_else(|| entry.get("default"))
                    .and_then(color_value)
                {
                    return Some(color);
                }
            }
            colors.get(mode.key())?.get(*name).and_then(color_value)
        })
    };
    let said = match doc.get("mode").and_then(Value::as_str) {
        Some(mode) if mode.eq_ignore_ascii_case("dark") => Some(true),
        Some(mode) if mode.eq_ignore_ascii_case("light") => Some(false),
        _ => doc.get("is_dark_mode").and_then(Value::as_bool),
    };

    let mut found = Colors {
        light: None,
        dark: None,
        is_dark: said.unwrap_or(true),
    };
    for mode in [Mode::Dark, Mode::Light] {
        let Some(palette) = material_palette(&|names| role(names, mode)) else {
            continue;
        };
        // A template's flat keys make one palette, whose mode goes by its
        // background; each mode's slot takes only the palette for it.
        let slot = if palette.dark {
            &mut found.dark
        } else {
            &mut found.light
        };
        if slot.is_none() {
            *slot = Some(palette);
        }
    }
    if found.dark.is_none() && found.light.is_none() {
        return None;
    }
    if said.is_none() && (found.dark.is_none() || found.light.is_none()) {
        found.is_dark = found.dark.is_some();
    }
    Some(found)
}

fn material_palette(role: &dyn Fn(&[&str]) -> Option<Rgba>) -> Option<Palette> {
    let background = role(&["surface", "background"])?;
    let foreground = role(&["on_surface", "on_background"])?;
    let accent = role(&["primary"])?;
    let dark = is_dark(background);
    let tint = |amount: f32| mix(background, foreground, amount);
    let named = |names: &[&str], fallback: u32| role(names).unwrap_or(rgb(fallback));
    Some(Palette {
        name: name(dark),
        dark,
        accent,
        background,
        // Material's navigation surfaces, then its containers by elevation.
        sidebar: role(&["surface_container_low", "surface_container"])
            .unwrap_or_else(|| mix(background, rgb(0x000000), 0.15)),
        panel: role(&["surface_container"]).unwrap_or_else(|| tint(0.03)),
        raised: role(&["surface_container_high"]).unwrap_or_else(|| tint(0.07)),
        popover: role(&["surface_container_highest", "surface_container_high"])
            .unwrap_or_else(|| tint(0.09)),
        foreground,
        muted: role(&["on_surface_variant", "outline"])
            .unwrap_or_else(|| mix(foreground, background, 0.3)),
        bright_foreground: foreground,
        // Material has an error role and no other status colours.
        red: named(&["error"], 0xf2555a),
        green: rgb(0x4fd18b),
        yellow: rgb(0xf5c451),
        blue: rgb(0x5aa9ff),
        magenta: named(&["tertiary"], 0xc58af9),
        cyan: named(&["secondary"], 0x4fd1d9),
        orange: rgb(0xeb6834),
        radius: 10,
        radius_lg: 14,
    })
}

/// A watch on a colours file's folder, or on the folder above while the tool
/// hasn't made it yet.
pub(crate) struct Watch {
    watcher: RecommendedWatcher,
    dir: PathBuf,
    watching_dir: bool,
}

impl Watch {
    /// Watch `file`. `None` when neither its folder nor the one above exists,
    /// or the system won't watch them.
    pub(crate) fn new(file: &Path) -> Option<(Self, mpsc::UnboundedReceiver<()>)> {
        let dir = file.parent()?.to_path_buf();
        let names = [file.file_name(), dir.file_name()]
            .into_iter()
            .flatten()
            .map(|name| name.to_os_string())
            .collect::<Vec<_>>();
        let (tx, rx) = mpsc::unbounded();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            // pywal writes a few dozen files at once; only its colours matter.
            let ours = event.paths.iter().any(|path| {
                path.file_name()
                    .is_some_and(|n| names.iter().any(|m| m == n))
            });
            if !event.kind.is_access() && ours {
                let _ = tx.unbounded_send(());
            }
        })
        .inspect_err(|e| tracing::warn!("watching the wallpaper's colours: {e}"))
        .ok()?;
        let mut watch = Self {
            watcher,
            dir,
            watching_dir: false,
        };
        watch.rewatch();
        if !watch.watching_dir {
            let parent = watch.dir.parent()?.to_path_buf();
            watch
                .watcher
                .watch(&parent, RecursiveMode::NonRecursive)
                .inspect_err(|e| tracing::debug!("watching {}: {e}", parent.display()))
                .ok()?;
        }
        Some((watch, rx))
    }

    /// Watch the file's folder once it exists, as after pywal's first run.
    pub(crate) fn rewatch(&mut self) {
        if self.watching_dir || !self.dir.is_dir() {
            return;
        }
        match self.watcher.watch(&self.dir, RecursiveMode::NonRecursive) {
            Ok(()) => self.watching_dir = true,
            Err(e) => tracing::warn!("watching {}: {e}", self.dir.display()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use gpui_kit::Hsla;
    use gpui_kit::component::{ThemeMode, try_parse_color};

    use super::*;
    use crate::style::{TEXT_CONTRAST, contrast};

    /// pywal 3's `colors.json` for a dark wallpaper.
    pub(crate) const PYWAL_DARK: &str = r##"{
        "wallpaper": "/home/user/Pictures/forest.jpg",
        "alpha": "100",
        "special": {
            "background": "#0f1418",
            "foreground": "#c3c5c6",
            "cursor": "#c3c5c6"
        },
        "colors": {
            "color0": "#0f1418", "color1": "#4E6B57", "color2": "#5C7A61",
            "color3": "#6B8B6A", "color4": "#5E7F8C", "color5": "#7A8E92",
            "color6": "#8BA19B", "color7": "#c3c5c6", "color8": "#5f6a72",
            "color9": "#4E6B57", "color10": "#5C7A61", "color11": "#6B8B6A",
            "color12": "#5E7F8C", "color13": "#7A8E92", "color14": "#8BA19B",
            "color15": "#e7e8e8"
        }
    }"##;

    /// The same with `wal -l`, a light background.
    const PYWAL_LIGHT: &str = r##"{
        "special": {"background": "#eeeae6", "foreground": "#272522", "cursor": "#272522"},
        "colors": {
            "color0": "#eeeae6", "color1": "#9A6B4E", "color4": "#6F7E8C",
            "color7": "#272522", "color8": "#a6a39f", "color15": "#272522"
        }
    }"##;

    /// `matugen image … --json hex` from matugen 3 and later.
    const MATUGEN_NEW: &str = r##"{
        "image": "/home/user/Pictures/forest.jpg",
        "mode": "dark",
        "is_dark_mode": true,
        "scheme": "scheme-tonal-spot",
        "colors": {
            "primary": {"dark": {"color": "#a6d0b4"}, "light": {"color": "#3d6750"}, "default": {"color": "#a6d0b4"}},
            "on_primary": {"dark": {"color": "#0e3724"}, "light": {"color": "#ffffff"}, "default": {"color": "#0e3724"}},
            "secondary": {"dark": {"color": "#b5ccbb"}, "light": {"color": "#4e6355"}, "default": {"color": "#b5ccbb"}},
            "tertiary": {"dark": {"color": "#a4cddd"}, "light": {"color": "#3c6472"}, "default": {"color": "#a4cddd"}},
            "error": {"dark": {"color": "#ffb4ab"}, "light": {"color": "#ba1a1a"}, "default": {"color": "#ffb4ab"}},
            "surface": {"dark": {"color": "#0f1511"}, "light": {"color": "#f6fbf5"}, "default": {"color": "#0f1511"}},
            "on_surface": {"dark": {"color": "#dfe4dd"}, "light": {"color": "#171d19"}, "default": {"color": "#dfe4dd"}},
            "on_surface_variant": {"dark": {"color": "#c0c9c1"}, "light": {"color": "#404943"}, "default": {"color": "#c0c9c1"}},
            "surface_container_low": {"dark": {"color": "#171d19"}, "light": {"color": "#f0f5ef"}, "default": {"color": "#171d19"}},
            "surface_container": {"dark": {"color": "#1b211d"}, "light": {"color": "#eaefe9"}, "default": {"color": "#1b211d"}},
            "surface_container_high": {"dark": {"color": "#262b27"}, "light": {"color": "#e4eae4"}, "default": {"color": "#262b27"}},
            "surface_container_highest": {"dark": {"color": "#303632"}, "light": {"color": "#dfe4de"}, "default": {"color": "#303632"}}
        },
        "palettes": {"primary": {"0": {"color": "#000000"}}}
    }"##;

    /// The same from matugen 2, mode first and plain strings.
    const MATUGEN_OLD: &str = r##"{
        "colors": {
            "dark": {
                "primary": "#a6d0b4", "surface": "#0f1511", "on_surface": "#dfe4dd",
                "on_surface_variant": "#c0c9c1", "surface_container": "#1b211d",
                "error": "#ffb4ab"
            },
            "light": {
                "primary": "#3d6750", "surface": "#f6fbf5", "on_surface": "#171d19",
                "on_surface_variant": "#404943", "surface_container": "#eaefe9",
                "error": "#ba1a1a"
            }
        }
    }"##;

    /// What the template in `docs/PLATFORMS.md` renders.
    pub(crate) const MATUGEN_TEMPLATE: &str = r##"{
        "mode": "light",
        "primary": "#3d6750",
        "secondary": "#4e6355",
        "tertiary": "#3c6472",
        "error": "#ba1a1a",
        "surface": "#f6fbf5",
        "on_surface": "#171d19",
        "on_surface_variant": "#404943",
        "surface_container_low": "#f0f5ef",
        "surface_container": "#eaefe9",
        "surface_container_high": "#e4eae4",
        "surface_container_highest": "#dfe4de"
    }"##;

    type Map = serde_json::Map<String, serde_json::Value>;

    fn color(colors: &Map, key: &str) -> Hsla {
        try_parse_color(colors[key].as_str().unwrap()).unwrap()
    }

    fn hex(colors: &Map, key: &str) -> String {
        colors[key].as_str().unwrap().to_string()
    }

    fn every_palette(text: &str) -> Vec<&'static str> {
        let colors = parse(text).unwrap();
        [colors.dark.map(|p| p.name), colors.light.map(|p| p.name)]
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn reads_pywals_colors() {
        let colors = parse(PYWAL_DARK).unwrap();
        assert!(colors.light.is_none());
        let dark = colors.dark.as_ref().unwrap().colors();
        assert_eq!(hex(&dark, "background"), "#0f1418");
        assert_eq!(hex(&dark, "primary.background"), "#5e7f8c");
        let themes = colors.themes().unwrap();
        assert!(themes.is_dark);
        assert_eq!(themes.dark.unwrap().mode, ThemeMode::Dark);

        let light = parse(PYWAL_LIGHT).unwrap();
        assert!(light.dark.is_none());
        let themes = light.themes().unwrap();
        assert!(!themes.is_dark);
        assert_eq!(themes.light.unwrap().mode, ThemeMode::Light);
    }

    #[test]
    fn pywals_hues_never_stand_for_danger() {
        let colors = parse(PYWAL_DARK).unwrap().dark.unwrap().colors();
        // color1 is a green here, from the wallpaper.
        assert_ne!(hex(&colors, "danger.background"), "#4e6b57");
    }

    #[test]
    fn reads_matugens_json_old_and_new() {
        for text in [MATUGEN_NEW, MATUGEN_OLD] {
            let colors = parse(text).unwrap();
            assert!(colors.is_dark);
            let dark = colors.dark.as_ref().unwrap().colors();
            let light = colors.light.as_ref().unwrap().colors();
            assert_eq!(hex(&dark, "background"), "#0f1511");
            assert_eq!(hex(&dark, "primary.background"), "#a6d0b4");
            assert_eq!(hex(&dark, "panel.background"), "#1b211d");
            assert_eq!(hex(&light, "background"), "#f6fbf5");
            assert_eq!(hex(&light, "danger.background"), "#ba1a1a");
            let themes = colors.themes().unwrap();
            assert!(themes.is_dark && themes.light.is_some() && themes.dark.is_some());
        }
    }

    #[test]
    fn a_template_gives_one_theme_in_the_mode_it_says() {
        let colors = parse(MATUGEN_TEMPLATE).unwrap();
        assert!(colors.dark.is_none());
        assert!(!colors.is_dark);
        let light = colors.light.as_ref().unwrap().colors();
        assert_eq!(hex(&light, "sidebar.background"), "#f0f5ef");
        assert_eq!(hex(&light, "popover.background"), "#dfe4de");

        // Without a mode, the background decides.
        let unsaid = MATUGEN_TEMPLATE.replace(r#""mode": "light","#, "");
        assert!(!parse(&unsaid).unwrap().is_dark);
    }

    #[test]
    fn a_new_wallpaper_is_a_change_and_rewriting_the_same_one_is_not() {
        let other = PYWAL_DARK.replace("forest.jpg", "forest-copy.jpg");
        assert_eq!(parse(PYWAL_DARK), parse(&other));
        let recoloured = PYWAL_DARK.replace("#0f1418", "#181010");
        assert_ne!(parse(PYWAL_DARK), parse(&recoloured));
    }

    #[test]
    fn text_is_held_readable_whatever_the_wallpaper() {
        // A washed-out wallpaper whose text barely shows.
        let dim = r##"{
            "special": {"background": "#808080", "foreground": "#8a8a8a"},
            "colors": {"color4": "#858585", "color8": "#7a7a7a"}
        }"##;
        for text in [PYWAL_DARK, PYWAL_LIGHT, MATUGEN_NEW, MATUGEN_TEMPLATE, dim] {
            let found = parse(text).unwrap();
            for palette in [found.dark, found.light].into_iter().flatten() {
                let colors = palette.colors();
                for fg in ["foreground", "muted.foreground", "link"] {
                    for bg in ["background", "sidebar.background", "popover.background"] {
                        let ratio = contrast(color(&colors, fg), color(&colors, bg));
                        assert!(ratio >= TEXT_CONTRAST - 0.01, "{fg} on {bg}: {ratio}");
                    }
                }
                let ratio = contrast(
                    color(&colors, "primary.background"),
                    color(&colors, "primary.foreground"),
                );
                assert!(ratio >= TEXT_CONTRAST - 0.01, "primary: {ratio}");
            }
        }
    }

    #[test]
    fn dark_and_light_palettes_have_their_own_names() {
        assert_eq!(
            every_palette(MATUGEN_NEW),
            ["Wallpaper Dark", "Wallpaper Light"]
        );
    }

    #[test]
    fn a_file_without_colours_gives_nothing() {
        for text in [
            "",
            "not json",
            "{}",
            "[]",
            r##"{"special": {"background": "#000000"}}"##,
            r##"{"colors": {"primary": "#ffffff"}}"##,
            r##"{"surface": "#000000", "on_surface": "#ffffff", "primary": "teal"}"##,
        ] {
            assert!(parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn colours_are_read_with_or_without_hash_and_alpha() {
        assert_eq!(parse_color("#102030"), Some(rgb(0x102030)));
        assert_eq!(parse_color("102030"), Some(rgb(0x102030)));
        assert_eq!(parse_color("#102030cc"), Some(rgb(0x102030)));
        assert_eq!(parse_color("#123"), None);
        assert_eq!(parse_color("rgb(1, 2, 3)"), None);
    }

    #[test]
    fn the_setting_names_pywals_file_or_the_users() {
        let pywal = default_file().unwrap();
        assert!(pywal.ends_with("colors.json"), "{}", pywal.display());
        assert_eq!(resolve(""), Some(pywal.clone()));
        assert_eq!(resolve("  "), Some(pywal));
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            resolve("~/.cache/matugen/irori.json"),
            Some(home.join(".cache/matugen/irori.json"))
        );
        assert_eq!(
            resolve("/tmp/colours.json"),
            Some(PathBuf::from("/tmp/colours.json"))
        );
    }

    /// Waits for a change, up to a few seconds: file watches report a moment
    /// after the write.
    fn changed(rx: &mut mpsc::UnboundedReceiver<()>) -> bool {
        for _ in 0..100 {
            if rx.try_recv().is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_folder_made_after_the_watch_starts_is_followed() {
        let tmp = tempfile::tempdir().unwrap();
        // Canonical, as macOS reports `/private/var` for `/var`.
        let root = tmp.path().canonicalize().unwrap();
        let file = root.join("wal").join("colors.json");
        let (mut watch, mut rx) = Watch::new(&file).unwrap();
        assert!(!watch.watching_dir);

        std::fs::create_dir(root.join("wal")).unwrap();
        assert!(changed(&mut rx), "the folder's creation was missed");
        watch.rewatch();
        assert!(watch.watching_dir);
        while rx.try_recv().is_ok() {}

        // The tools write a temporary file and rename it over the old one.
        let partial = root.join("wal").join("colors.json.tmp");
        std::fs::write(&partial, PYWAL_DARK).unwrap();
        std::fs::rename(&partial, &file).unwrap();
        assert!(changed(&mut rx), "the rename was missed");
    }
}
