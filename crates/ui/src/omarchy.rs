//! Omarchy's desktop theme, turned into an Irori theme.
//!
//! Omarchy colours the whole desktop from one palette file and rewrites it
//! whenever the user switches theme, so Irori reads that palette instead of
//! bundling Omarchy's themes, and third-party themes work too. The palette only
//! holds a few dozen colours, so the many surfaces Irori paints are derived
//! from them here. Nothing happens on machines without Omarchy: the files are
//! not there and no folder is watched.

use std::path::PathBuf;

use futures::channel::mpsc;
use gpui_kit::component::ThemeConfig;
use gpui_kit::{Hsla, Rgba, rgb};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use serde_json::json;

use crate::style::{ICON_CONTRAST, TEXT_CONTRAST, contrast, legible_towards};

/// The name of the generated theme. Never shown or stored: the user picks it
/// as "the default", which is what keeps it following Omarchy.
const THEME_NAME: &str = "Omarchy";

/// The contrast to aim for, a little over what is needed: colours are rounded
/// to 8 bits per channel afterwards, which can lose that much.
const TEXT: f32 = TEXT_CONTRAST + 0.1;
const ICON: f32 = ICON_CONTRAST + 0.1;

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

/// The colours Irori uses from a palette file.
struct Palette {
    dark: bool,
    accent: Rgba,
    background: Rgba,
    dark_background: Rgba,
    foreground: Rgba,
    dark_foreground: Rgba,
    bright_foreground: Rgba,
    red: Rgba,
    green: Rgba,
    yellow: Rgba,
    blue: Rgba,
    magenta: Rgba,
    cyan: Rgba,
    orange: Rgba,
}

fn parse_color(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#').filter(|d| d.len() == 6)?;
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

impl Palette {
    /// Read a palette, accepting every spelling Omarchy does: the canonical
    /// keys, their legacy short names, and ANSI numbers for the named colours.
    /// `None` when even the background, text or accent colour is missing.
    fn parse(text: &str) -> Option<Self> {
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
        let dark_foreground = color(&["dark_foreground", "dark_fg"]);
        Some(Self {
            dark,
            accent,
            background,
            dark_background: color(&["dark_background", "dark_bg"])
                .unwrap_or_else(|| mix(background, rgb(0x000000), 0.15)),
            foreground,
            dark_foreground: dark_foreground.unwrap_or_else(|| mix(foreground, background, 0.3)),
            bright_foreground: color(&["bright_foreground", "bright_fg"]).unwrap_or(foreground),
            red: named(&["red", "color1"], 0xf2555a),
            green: named(&["green", "color2"], 0x4fd18b),
            yellow: named(&["yellow", "color3"], 0xf5c451),
            blue: named(&["blue", "color4"], 0x5aa9ff),
            magenta: named(&["magenta", "color5"], 0xc58af9),
            cyan: named(&["cyan", "color6"], 0x4fd1d9),
            orange: named(&["orange"], 0xeb6834),
        })
    }

    /// `color`, or the least change of it that reads at `min` on `surfaces`.
    /// Omarchy's palettes are made for terminals, where dim text is a choice;
    /// here it has to stay readable.
    fn readable(&self, color: Rgba, surfaces: &[Rgba], min: f32) -> Rgba {
        let surfaces: Vec<Hsla> = surfaces.iter().map(|c| (*c).into()).collect();
        let towards = if self.dark {
            gpui_kit::white()
        } else {
            gpui_kit::black()
        };
        legible_towards(color.into(), &surfaces, min, towards).to_rgb()
    }

    /// The theme colours for this palette, one for all that a bundled theme
    /// colours. Colours fall between the same anchors in dark and light themes
    /// (background to text), so they keep working whichever way the ramp runs.
    fn colors(&self) -> serde_json::Map<String, serde_json::Value> {
        let (bg, accent) = (self.background, self.accent);
        let tint = |amount: f32| mix(bg, self.foreground, amount);

        let sidebar = self.dark_background;
        let panel = tint(0.03);
        let raised = tint(0.07);
        let popover = if self.dark {
            tint(0.09)
        } else {
            mix(bg, rgb(0xffffff), 0.6)
        };
        let surfaces = [bg, sidebar, panel, raised, popover];
        let fg = self.readable(self.foreground, &surfaces, TEXT);
        let muted = self.readable(self.dark_foreground, &surfaces, TEXT);
        let link = self.readable(accent, &surfaces, TEXT);
        let accent_bg = mix(bg, accent, 0.15);
        let sidebar_accent = mix(sidebar, self.foreground, 0.1);
        let sidebar_fg = self.readable(self.foreground, &[sidebar, sidebar_accent], TEXT);
        let sidebar_border = mix(sidebar, self.foreground, 0.08);
        let border = tint(0.12);
        let input_border = tint(0.2);
        let status = |color: Rgba| self.readable(color, &[bg], ICON);

        let colors = [
            ("background", hex(bg)),
            ("foreground", hex(fg)),
            ("border", hex(border)),
            ("ring", hex(accent)),
            ("caret", hex(accent)),
            ("link", hex(link)),
            (
                "link.hover",
                hex(self.readable(mix(accent, fg, 0.25), &surfaces, TEXT)),
            ),
            ("link.active", hex(link)),
            ("accent.background", hex(accent_bg)),
            (
                "accent.foreground",
                hex(self.readable(accent, &[accent_bg], TEXT)),
            ),
            ("primary.background", hex(accent)),
            ("primary.foreground", hex(on(accent))),
            (
                "primary.hover.background",
                hex(mix(accent, on(accent), 0.1)),
            ),
            ("primary.active.background", hex(mix(accent, bg, 0.1))),
            ("secondary.background", hex(raised)),
            ("secondary.foreground", hex(fg)),
            ("secondary.hover.background", hex(tint(0.1))),
            ("secondary.active.background", hex(tint(0.13))),
            ("muted.background", hex(raised)),
            ("muted.foreground", hex(muted)),
            ("panel.background", hex(panel)),
            ("popover.background", hex(popover)),
            ("popover.foreground", hex(fg)),
            ("list.active.background", alpha(accent, 0x22)),
            ("list.active.border", hex(accent)),
            ("list.even.background", alpha(self.foreground, 0x04)),
            ("list.hover.background", alpha(self.foreground, 0x0a)),
            ("sidebar.background", hex(sidebar)),
            ("sidebar.foreground", hex(sidebar_fg)),
            ("sidebar.border", hex(sidebar_border)),
            ("sidebar.accent.background", hex(sidebar_accent)),
            (
                "sidebar.accent.foreground",
                hex(self.readable(self.bright_foreground, &[sidebar_accent], TEXT)),
            ),
            ("sidebar.primary.background", hex(accent)),
            ("sidebar.primary.foreground", hex(on(accent))),
            ("title_bar.background", hex(sidebar)),
            ("title_bar.border", hex(sidebar_border)),
            ("status_bar.background", hex(sidebar)),
            ("status_bar.border", hex(sidebar_border)),
            ("window.border", hex(border)),
            ("scrollbar.background", "#00000000".to_string()),
            ("scrollbar.thumb.background", hex(tint(0.2))),
            ("scrollbar.thumb.hover.background", hex(tint(0.3))),
            ("chart.grid", hex(tint(0.08))),
            ("chart.1", hex(status(accent))),
            ("chart.2", hex(status(self.orange))),
            ("chart.3", hex(status(self.green))),
            ("chart.4", hex(status(self.yellow))),
            ("chart.5", hex(status(self.magenta))),
            ("base.red", hex(status(self.red))),
            ("base.green", hex(status(self.green))),
            ("base.yellow", hex(status(self.yellow))),
            ("base.blue", hex(status(self.blue))),
            ("base.magenta", hex(status(self.magenta))),
            ("base.cyan", hex(status(self.cyan))),
            ("success.background", hex(self.green)),
            ("success.foreground", hex(on(self.green))),
            ("danger.background", hex(self.red)),
            ("danger.foreground", hex(on(self.red))),
            ("warning.background", hex(self.yellow)),
            ("warning.foreground", hex(on(self.yellow))),
            ("info.background", hex(self.blue)),
            ("info.foreground", hex(on(self.blue))),
            ("progress.bar.background", hex(accent)),
            ("skeleton.background", hex(tint(0.1))),
            ("switch.background", hex(input_border)),
            ("selection.background", alpha(accent, 0x40)),
            (
                "overlay",
                alpha(rgb(0x000000), if self.dark { 0xa6 } else { 0x66 }),
            ),
            ("input.border", hex(input_border)),
            ("tab_bar.background", "#00000000".to_string()),
            ("tab.active.background", hex(raised)),
            ("tab_bar.segmented.background", hex(tint(0.05))),
            ("table.head.background", hex(panel)),
        ];
        colors
            .into_iter()
            .map(|(key, color)| (key.to_string(), color.into()))
            .collect()
    }

    fn theme(&self) -> Option<ThemeConfig> {
        let theme = json!({
            "name": THEME_NAME,
            "mode": if self.dark { "dark" } else { "light" },
            "radius": 10,
            "radius.lg": 14,
            "colors": self.colors(),
        });
        serde_json::from_value(theme)
            .inspect_err(|e| tracing::warn!("building a theme from Omarchy's: {e}"))
            .ok()
    }
}

/// The Irori theme for the contents of a palette file.
pub(crate) fn theme(text: &str) -> Option<ThemeConfig> {
    Palette::parse(text)?.theme()
}

fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let channel = |a: f32, b: f32| a + (b - a) * amount;
    Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: 1.0,
    }
}

/// Black or white, whichever reads better on `fill`. One of them always clears
/// the text contrast, however middling the fill.
fn on(fill: Rgba) -> Rgba {
    let (black, white) = (rgb(0x000000), rgb(0xffffff));
    if contrast(black.into(), fill.into()) > contrast(white.into(), fill.into()) {
        black
    } else {
        white
    }
}

fn hex(color: Rgba) -> String {
    let [r, g, b] = [color.r, color.g, color.b].map(|c| (c * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn alpha(color: Rgba, alpha: u8) -> String {
    format!("{}{alpha:02x}", hex(color))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use gpui_kit::component::{ThemeMode, try_parse_color};

    use super::*;

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
        Palette::parse(palette).unwrap().colors()
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
        assert!(Palette::parse(&bad).is_none());
    }
}
