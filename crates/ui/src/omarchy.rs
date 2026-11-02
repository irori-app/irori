//! Omarchy's desktop theme, turned into an Irori theme.
//!
//! Omarchy colours the whole desktop from one palette file and rewrites it
//! whenever the user switches theme, so Irori reads that palette instead of
//! bundling Omarchy's themes, and third-party themes work too. The palette only
//! holds a few dozen colours, so the surfaces Irori paints are derived from
//! them. Nothing happens on machines without Omarchy: the files are not there
//! and no folder is watched.

use std::path::PathBuf;

use futures::channel::mpsc;
use gpui_kit::{Rgba, rgb};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::desktop_theme::{DesktopThemes, Palette, mix, on};

/// Where Omarchy keeps the active theme, newest first. Omarchy always uses
/// these paths under the home folder, whatever `XDG_STATE_HOME` says.
const CURRENT: &[&str] = &[".local/state/omarchy/current", ".config/omarchy/current"];

fn current_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    CURRENT.iter().map(|dir| home.join(dir)).collect()
}

/// The active theme's palette file, if this machine runs Omarchy.
pub(crate) fn read() -> Option<String> {
    current_dirs()
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join("theme/colors.toml")).ok())
}

/// Watch for theme switches. Omarchy swaps the whole theme folder, so the
/// folder holding it is what can be watched: a watch inside would die with the
/// old theme.
pub(crate) fn watch() -> Option<(RecommendedWatcher, mpsc::UnboundedReceiver<()>)> {
    let dirs: Vec<PathBuf> = current_dirs().into_iter().filter(|d| d.is_dir()).collect();
    if dirs.is_empty() {
        return None;
    }
    let (tx, rx) = mpsc::unbounded();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| !e.kind.is_access()) {
                let _ = tx.unbounded_send(());
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("watching Omarchy's theme: {e}");
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

fn parse_color(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#').filter(|d| d.len() == 6)?;
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Read a palette, accepting every spelling Omarchy does: the canonical keys,
/// their legacy short names, and ANSI numbers for the named colours. `None`
/// when even the background, text or accent colour is missing.
fn parse(text: &str) -> Option<Palette> {
    let keys: toml::Table = text.parse().ok()?;
    let color = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| parse_color(keys.get(*name)?.as_str()?))
    };
    let named = |names: &[&str], fallback: u32| color(names).unwrap_or(rgb(fallback));

    let background = color(&["background", "bg"])?;
    let foreground = color(&["foreground", "fg"])?;
    let accent = color(&["accent", "color4"])?;
    let dark = match keys.get("mode").and_then(|mode| mode.as_str()) {
        Some("dark") => true,
        Some("light") => false,
        // Themes older than `mode` only have their colours to go by.
        _ => on(background) == rgb(0xffffff),
    };
    let tint = |amount: f32| mix(background, foreground, amount);
    Some(Palette {
        name: "Omarchy",
        dark,
        accent,
        background,
        sidebar: color(&["dark_background", "dark_bg"])
            .unwrap_or_else(|| mix(background, rgb(0x000000), 0.15)),
        panel: tint(0.03),
        raised: tint(0.07),
        popover: if dark {
            tint(0.09)
        } else {
            mix(background, rgb(0xffffff), 0.6)
        },
        foreground,
        muted: color(&["dark_foreground", "dark_fg"])
            .unwrap_or_else(|| mix(foreground, background, 0.3)),
        bright_foreground: color(&["bright_foreground", "bright_fg"]).unwrap_or(foreground),
        red: named(&["red", "color1"], 0xf2555a),
        green: named(&["green", "color2"], 0x4fd18b),
        yellow: named(&["yellow", "color3"], 0xf5c451),
        blue: named(&["blue", "color4"], 0x5aa9ff),
        magenta: named(&["magenta", "color5"], 0xc58af9),
        cyan: named(&["cyan", "color6"], 0x4fd1d9),
        orange: named(&["orange"], 0xeb6834),
        radius: 10,
        radius_lg: 14,
    })
}

/// The Irori theme for the contents of a palette file.
pub(crate) fn themes(text: &str) -> Option<DesktopThemes> {
    parse(text)?.theme().map(DesktopThemes::single)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::Hsla;
    use gpui_kit::component::{ThemeConfig, ThemeMode, try_parse_color};

    use super::*;
    use crate::style::{TEXT_CONTRAST, contrast};

    pub(crate) const DARK: &str = r##"
        mode = "dark"
        accent = "#7aa2f7"
        background = "#1a1b26"
        dark_background = "#13141c"
        foreground = "#a9b1d6"
        dark_foreground = "#565f89"
        bright_foreground = "#c0caf5"
        red = "#f7768e"
        green = "#9ece6a"
        yellow = "#e0af68"
        blue = "#7aa2f7"
        magenta = "#ad8ee6"
        cyan = "#449dab"
        orange = "#eb927b"
    "##;

    pub(crate) const LIGHT: &str = r##"
        mode = "light"
        accent = "#6e6e6e"
        background = "#ffffff"
        dark_background = "#f0f0f0"
        foreground = "#000000"
        dark_foreground = "#c0c0c0"
        bright_foreground = "#000000"
        red = "#ff0000"
        green = "#00ff00"
        yellow = "#ffff00"
        blue = "#0000ff"
        magenta = "#ff00ff"
        cyan = "#00ffff"
    "##;

    type Colors = serde_json::Map<String, serde_json::Value>;

    fn color(colors: &Colors, key: &str) -> Hsla {
        try_parse_color(colors[key].as_str().unwrap()).unwrap()
    }

    fn generated(palette: &str) -> Colors {
        parse(palette).unwrap().colors()
    }

    pub(crate) fn theme(text: &str) -> Option<ThemeConfig> {
        parse(text)?.theme()
    }

    #[test]
    fn colors_everything_a_bundled_theme_does() {
        let irori: serde_json::Value =
            serde_json::from_str(include_str!("../../../assets/themes/irori.json")).unwrap();
        let wanted = irori["themes"][0]["colors"].as_object().unwrap();
        let wanted: BTreeSet<&String> = wanted.keys().collect();
        for palette in [DARK, LIGHT] {
            let colors = generated(palette);
            let has: BTreeSet<&String> = colors.keys().collect();
            let missing: Vec<_> = wanted.difference(&has).collect();
            assert!(missing.is_empty(), "{missing:?}");
        }
    }

    #[test]
    fn dim_palette_text_is_held_readable() {
        for palette in [DARK, LIGHT] {
            let colors = generated(palette);
            let surfaces = [
                "background",
                "sidebar.background",
                "panel.background",
                "secondary.background",
                "popover.background",
            ];
            for text in ["foreground", "muted.foreground", "link"] {
                for surface in surfaces {
                    let ratio = contrast(color(&colors, text), color(&colors, surface));
                    assert!(
                        ratio >= TEXT_CONTRAST - 0.01,
                        "{text} on {surface}: {ratio}"
                    );
                }
            }
            for fill in ["primary", "success", "danger", "warning", "info"] {
                let ratio = contrast(
                    color(&colors, &format!("{fill}.background")),
                    color(&colors, &format!("{fill}.foreground")),
                );
                assert!(ratio >= TEXT_CONTRAST - 0.01, "{fill}: {ratio}");
            }
        }
    }

    #[test]
    fn keeps_the_palettes_mode() {
        assert_eq!(theme(DARK).unwrap().mode, ThemeMode::Dark);
        assert_eq!(theme(LIGHT).unwrap().mode, ThemeMode::Light);
    }

    #[test]
    fn a_palette_without_a_mode_goes_by_its_background() {
        let without = |palette: &str| palette.replace("mode = ", "style = ");
        assert_eq!(theme(&without(DARK)).unwrap().mode, ThemeMode::Dark);
        assert_eq!(theme(&without(LIGHT)).unwrap().mode, ThemeMode::Light);
    }

    #[test]
    fn accepts_omarchys_legacy_names_and_ansi_numbers() {
        let palette = r##"
            bg = "#101010"
            fg = "#e0e0e0"
            color4 = "#3366cc"
            color1 = "#cc2222"
        "##;
        let colors = generated(palette);
        assert_eq!(colors["background"], "#101010");
        assert_eq!(colors["primary.background"], "#3366cc");
        assert_eq!(colors["danger.background"], "#cc2222");
    }

    #[test]
    fn a_broken_palette_gives_no_theme() {
        assert!(theme("").is_none());
        assert!(theme("not toml at all").is_none());
        assert!(theme(r##"background = "#000000""##).is_none());
        let bad = DARK.replace("#7aa2f7", "blue");
        assert!(parse(&bad).is_none());
    }
}
