//! Turning a desktop's colours into an Irori theme.
//!
//! Desktops that colour everything from one place (see [`crate::omarchy`],
//! [`crate::cosmic`] and [`crate::kde`]), and the tools that colour one from
//! its wallpaper ([`crate::wallpaper`]), don't describe the many surfaces Irori
//! paints. Each fills in a [`Palette`], and the rest is derived here, so they
//! all read the same way and text stays legible whatever the desktop's colours
//! are. Desktops that only have an accent colour tint one of Irori's themes
//! with [`accented`], and systems with shades of it with [`system_accented`].
//! A system in high contrast gets a theme of its own colours,
//! [`high_contrast`].

use gpui_kit::component::{ThemeConfig, try_parse_color};
use gpui_kit::{Hsla, Rgba, SharedString, rgb};
use serde_json::json;

use crate::desktop_accent::{Accent, ContrastColors};
use crate::style::{ICON_CONTRAST, TEXT_CONTRAST, contrast, legible_towards};

/// The contrast to aim for, a little over what is needed: colours are rounded
/// to 8 bits per channel afterwards, which can lose that much.
const TEXT: f32 = TEXT_CONTRAST + 0.1;
const ICON: f32 = ICON_CONTRAST + 0.1;

/// A desktop whose theme can be the default one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Omarchy,
    Cosmic,
    Kde,
    /// Not a desktop: the colours pywal, wallust or matugen made from the
    /// wallpaper, which win over a desktop's while the user has them on.
    Wallpaper,
}

/// What a desktop with its own setting says about see-through windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frost {
    pub(crate) on: bool,
    /// How opaque the desktop's own windows are, out of 255.
    pub(crate) opacity: Option<u8>,
}

/// The themes a desktop offers right now.
pub(crate) struct DesktopThemes {
    pub(crate) light: Option<ThemeConfig>,
    pub(crate) dark: Option<ThemeConfig>,
    /// The mode the desktop is in.
    pub(crate) is_dark: bool,
    /// See-through windows, on a desktop that has its own say in them.
    pub(crate) frost: Option<Frost>,
}

impl DesktopThemes {
    /// A desktop with one theme at a time, whose mode is that theme's.
    pub(crate) fn single(theme: ThemeConfig) -> Self {
        let is_dark = theme.mode.is_dark();
        let (light, dark) = if is_dark {
            (None, Some(theme))
        } else {
            (Some(theme), None)
        };
        Self {
            light,
            dark,
            is_dark,
            frost: None,
        }
    }
}

/// The colours a desktop gives Irori.
#[derive(Debug, PartialEq)]
pub(crate) struct Palette {
    /// The generated theme's name. Never shown or stored: the user picks it as
    /// "the default", which is what keeps it following the desktop.
    pub(crate) name: &'static str,
    pub(crate) dark: bool,
    pub(crate) accent: Rgba,
    pub(crate) background: Rgba,
    pub(crate) sidebar: Rgba,
    pub(crate) panel: Rgba,
    pub(crate) raised: Rgba,
    pub(crate) popover: Rgba,
    pub(crate) foreground: Rgba,
    pub(crate) muted: Rgba,
    pub(crate) bright_foreground: Rgba,
    pub(crate) red: Rgba,
    pub(crate) green: Rgba,
    pub(crate) yellow: Rgba,
    pub(crate) blue: Rgba,
    pub(crate) magenta: Rgba,
    pub(crate) cyan: Rgba,
    pub(crate) orange: Rgba,
    pub(crate) radius: u32,
    pub(crate) radius_lg: u32,
}

impl Palette {
    /// `color`, or the least change of it that reads at `min` on `surfaces`.
    /// Desktop palettes are made for other uses, where dim text can be a
    /// choice; here it has to stay readable.
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
    pub(crate) fn colors(&self) -> serde_json::Map<String, serde_json::Value> {
        let (bg, accent) = (self.background, self.accent);
        let tint = |amount: f32| mix(bg, self.foreground, amount);

        let (sidebar, panel, raised, popover) =
            (self.sidebar, self.panel, self.raised, self.popover);
        let surfaces = [bg, sidebar, panel, raised, popover];
        let fg = self.readable(self.foreground, &surfaces, TEXT);
        let muted = self.readable(self.muted, &surfaces, TEXT);
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

    pub(crate) fn theme(&self) -> Option<ThemeConfig> {
        self.theme_with(self.colors())
    }

    fn theme_with(
        &self,
        colors: serde_json::Map<String, serde_json::Value>,
    ) -> Option<ThemeConfig> {
        let theme = json!({
            "name": self.name,
            "mode": if self.dark { "dark" } else { "light" },
            "radius": self.radius,
            "radius.lg": self.radius_lg,
            "colors": colors,
        });
        serde_json::from_value(theme)
            .inspect_err(|e| tracing::warn!("building a theme from {}'s: {e}", self.name))
            .ok()
    }
}

/// `base`, in the desktop's accent colour. Only what Irori paints in its own
/// accent changes, so the theme stays recognisably Irori's.
///
/// A desktop's accent is made for its own surfaces, and can be anything: a
/// Plasma user's selection colour may be near black. So it is held against
/// these surfaces, as a colour that fills or marks things and as one that
/// writes on them. A colour that already reads stays exactly as it is.
pub(crate) fn accented(base: &ThemeConfig, accent: Rgba) -> Option<ThemeConfig> {
    let colors = &base.colors;
    let surfaces = surfaces(base)?;
    let background = surfaces[0].to_rgb();
    let dark = base.mode.is_dark();
    let towards = if dark {
        gpui_kit::white()
    } else {
        gpui_kit::black()
    };
    let hold = |color: Rgba, on: &[Hsla], min| legible_towards(color.into(), on, min, towards);
    let fill = hold(accent, &surfaces, ICON).to_rgb();
    let writing = hold(accent, &surfaces, TEXT).to_rgb();
    let foreground = parse(&colors.foreground)?.to_rgb();
    let tint = mix(background, fill, 0.15);

    // The text on a button stays on it as the pointer comes and goes.
    let on_fill = on(fill);
    let away = if on_fill == rgb(0x000000) {
        gpui_kit::white()
    } else {
        gpui_kit::black()
    };
    let state = |color: Rgba| legible_towards(color.into(), &[on_fill.into()], TEXT, away).to_rgb();

    let mut theme = base.clone();
    let colors = &mut theme.colors;
    let set = |color: Rgba| Some(SharedString::from(hex(color)));
    let translucent = |amount: u8| Some(SharedString::from(alpha(fill, amount)));
    colors.ring = set(fill);
    colors.caret = set(fill);
    colors.link = set(writing);
    colors.link_hover = set(hold(mix(accent, foreground, 0.25), &surfaces, TEXT).to_rgb());
    colors.link_active = set(writing);
    colors.accent = set(tint);
    colors.accent_foreground = set(hold(accent, &[tint.into()], TEXT).to_rgb());
    colors.primary = set(fill);
    colors.primary_foreground = set(on_fill);
    colors.primary_hover = set(state(mix(fill, rgb(0xffffff), 0.1)));
    colors.primary_active = set(state(mix(fill, rgb(0x000000), 0.1)));
    colors.list_active = translucent(if dark { 0x22 } else { 0x18 });
    colors.list_active_border = set(fill);
    colors.sidebar_primary = set(fill);
    colors.sidebar_primary_foreground = set(on_fill);
    colors.progress_bar = set(fill);
    colors.selection = translucent(if dark { 0x40 } else { 0x33 });
    Some(theme)
}

/// `base` in the system's accent for its colour mode: the first of the
/// system's shades of it that reads on Irori's surfaces, and the system's
/// highlight colour for selected text where Irori's text reads on it.
///
/// Windows makes its shades for this, lighter ones for dark surfaces and
/// darker ones for light, so a shade of the system's own is truer to it than
/// the accent [`accented`] would move until it reads.
pub(crate) fn system_accented(base: &ThemeConfig, accent: &Accent) -> Option<ThemeConfig> {
    let shade = pick_shade(&accent.shades, &surfaces(base)?)?;
    let mut theme = accented(base, shade)?;
    let foreground = parse(&base.colors.foreground)?;
    // Selections are painted under the text, so the system's opaque highlight
    // works as it does in its own apps, as long as Irori's text reads on it.
    if let Some(selection) = accent.selection
        && contrast(foreground, selection.into()) >= TEXT_CONTRAST
    {
        theme.colors.selection = Some(SharedString::from(hex(selection)));
    }
    Some(theme)
}

/// The first of `shades` that reads as text on every one of `surfaces`, or
/// failing that the one that comes closest, which [`accented`] then holds.
pub(crate) fn pick_shade(shades: &[Rgba], surfaces: &[Hsla]) -> Option<Rgba> {
    let least = |color: &Rgba| {
        surfaces
            .iter()
            .map(|surface| contrast((*color).into(), *surface))
            .fold(f32::INFINITY, f32::min)
    };
    shades
        .iter()
        .find(|shade| least(shade) >= TEXT)
        .or_else(|| shades.iter().max_by(|a, b| least(a).total_cmp(&least(b))))
        .copied()
}

/// A theme of the system's high-contrast colours, which stands in for every
/// theme while high contrast is on. Borders take the text colour and links the
/// link colour, as in the system's own apps; the highlight colour fills what
/// Irori fills with its accent.
pub(crate) fn high_contrast(system: &ContrastColors) -> Option<ThemeConfig> {
    let (black, white) = (rgb(0x000000), rgb(0xffffff));
    let window = system.window;
    let dark = contrast(window.into(), white.into()) > contrast(window.into(), black.into());
    let palette = Palette {
        name: HIGH_CONTRAST,
        dark,
        accent: system.highlight,
        background: window,
        sidebar: window,
        panel: window,
        raised: system.button_face,
        popover: window,
        foreground: system.window_text,
        muted: system.gray_text,
        bright_foreground: system.window_text,
        // Windows' console colours: high-contrast themes have no others.
        red: rgb(0xe74856),
        green: rgb(0x16c60c),
        yellow: rgb(0xf9f1a5),
        blue: system.hotlight,
        magenta: rgb(0xb4009e),
        cyan: rgb(0x61d6d6),
        orange: rgb(0xf7630c),
        radius: 6,
        radius_lg: 8,
    };
    let mut colors = palette.colors();
    let surfaces = [window, system.button_face];
    let mut set = |keys: &[&str], color: Rgba| {
        for key in keys {
            colors.insert(key.to_string(), hex(color).into());
        }
    };
    set(
        &[
            "border",
            "input.border",
            "sidebar.border",
            "title_bar.border",
            "status_bar.border",
            "window.border",
            "list.active.border",
        ],
        palette.readable(system.window_text, &surfaces, TEXT),
    );
    set(
        &["link", "link.hover", "link.active"],
        palette.readable(system.hotlight, &surfaces, TEXT),
    );
    let reads_on = |color: Rgba, fill: Rgba| contrast(color.into(), fill.into()) >= TEXT_CONTRAST;
    if reads_on(system.highlight_text, system.highlight) {
        set(
            &["primary.foreground", "sidebar.primary.foreground"],
            system.highlight_text,
        );
    }
    if reads_on(system.button_text, system.button_face) {
        set(&["secondary.foreground"], system.button_text);
    }
    if reads_on(system.window_text, system.highlight) {
        set(&["selection.background"], system.highlight);
    }
    palette.theme_with(colors)
}

/// The high-contrast theme's name. Never shown or stored.
pub(crate) const HIGH_CONTRAST: &str = "High contrast";

/// The surfaces Irori's accent has to read on, in `base`.
fn surfaces(base: &ThemeConfig) -> Option<[Hsla; 4]> {
    let colors = &base.colors;
    Some([
        parse(&colors.background)?,
        parse(&colors.sidebar)?,
        parse(&colors.secondary)?,
        parse(&colors.popover)?,
    ])
}

fn parse(color: &Option<SharedString>) -> Option<Hsla> {
    try_parse_color(color.as_ref()?).ok()
}

pub(crate) fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
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
pub(crate) fn on(fill: Rgba) -> Rgba {
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
mod tests {
    use super::*;

    /// Windows' default blue and its shades, as `UISettings` gives them.
    const WINDOWS_BLUE: u32 = 0x0078d4;
    const LIGHTER: [u32; 3] = [0x429ce3, 0x76b9ed, 0x99ebff];
    const DARKER: [u32; 3] = [0x005a9e, 0x004275, 0x001a68];

    fn shades(accent: u32, more: [u32; 3]) -> Vec<Rgba> {
        std::iter::once(accent).chain(more).map(rgb).collect()
    }

    fn irori(name: &str) -> ThemeConfig {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../../../assets/themes/irori.json")).unwrap();
        let theme = doc["themes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        serde_json::from_value(theme.clone()).unwrap()
    }

    fn least(color: Rgba, surfaces: &[Hsla]) -> f32 {
        surfaces
            .iter()
            .map(|s| contrast(color.into(), *s))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn the_first_shade_that_reads_is_the_one() {
        for (surface, more) in [(0x1c1c1c, LIGHTER), (0xf3f3f3, DARKER)] {
            let surfaces = [rgb(surface).into()];
            let shades = shades(WINDOWS_BLUE, more);
            let picked = pick_shade(&shades, &surfaces).unwrap();
            let at = shades.iter().position(|s| *s == picked).unwrap();
            assert!(least(picked, &surfaces) >= TEXT, "{surface:06x}");
            for earlier in &shades[..at] {
                assert!(least(*earlier, &surfaces) < TEXT, "{surface:06x}");
            }
        }
    }

    #[test]
    fn an_accent_that_reads_needs_no_shade() {
        let surfaces = [rgb(0xffffff).into()];
        let shades = shades(0x005a9e, LIGHTER);
        assert_eq!(pick_shade(&shades, &surfaces), Some(rgb(0x005a9e)));
    }

    #[test]
    fn with_no_shade_that_reads_the_closest_is_picked() {
        // Every shade lighter, on white: the darkest of them comes closest.
        let surfaces = [rgb(0xffffff).into()];
        let shades = shades(0x76b9ed, [0x99ebff, 0xc0f0ff, 0xe0f8ff]);
        assert_eq!(pick_shade(&shades, &surfaces), Some(rgb(0x76b9ed)));
        assert_eq!(pick_shade(&[], &surfaces), None);
    }

    #[test]
    fn a_dark_theme_takes_windows_lighter_shade() {
        let base = irori("Irori Night");
        let accent = Accent {
            shades: shades(WINDOWS_BLUE, LIGHTER),
            selection: None,
        };
        let theme = system_accented(&base, &accent).unwrap();
        let primary = parse(&theme.colors.primary).unwrap();
        assert_ne!(primary, rgb(WINDOWS_BLUE).into());
        let wanted: Vec<Hsla> = LIGHTER.map(|c| rgb(c).into()).to_vec();
        assert!(wanted.contains(&primary), "{primary:?}");
    }

    #[test]
    fn the_systems_highlight_is_used_where_text_reads_on_it() {
        let base = irori("Irori Day");
        let blue = shades(WINDOWS_BLUE, DARKER);
        // macOS's light highlight for its blue accent.
        let accent = Accent {
            shades: blue.clone(),
            selection: Some(rgb(0xb3d7ff)),
        };
        let theme = system_accented(&base, &accent).unwrap();
        assert_eq!(parse(&theme.colors.selection), Some(rgb(0xb3d7ff).into()));

        // Irori Day's dark text can't be read on a near-black highlight.
        let unreadable = Accent {
            shades: blue,
            selection: Some(rgb(0x101010)),
        };
        let theme = system_accented(&base, &unreadable).unwrap();
        assert_ne!(parse(&theme.colors.selection), Some(rgb(0x101010).into()));
    }

    /// Windows 11's "Night sky" and "Desert" contrast themes.
    fn night_sky() -> ContrastColors {
        ContrastColors {
            window: rgb(0x000000),
            window_text: rgb(0xffffff),
            highlight: rgb(0xd6b4fd),
            highlight_text: rgb(0x2b2b2b),
            hotlight: rgb(0x8080ff),
            gray_text: rgb(0xa6a6a6),
            button_face: rgb(0x000000),
            button_text: rgb(0xffffff),
        }
    }

    fn desert() -> ContrastColors {
        ContrastColors {
            window: rgb(0xfffaef),
            window_text: rgb(0x3d3d3d),
            highlight: rgb(0x903909),
            highlight_text: rgb(0xfff5e3),
            hotlight: rgb(0x1c5e75),
            gray_text: rgb(0x676767),
            button_face: rgb(0xfffaef),
            button_text: rgb(0x202020),
        }
    }

    #[test]
    fn high_contrast_is_the_systems_colours() {
        for (system, dark) in [(night_sky(), true), (desert(), false)] {
            let theme = high_contrast(&system).unwrap();
            let c = &theme.colors;
            assert_eq!(theme.mode.is_dark(), dark);
            assert_eq!(parse(&c.background), Some(system.window.into()));
            assert_eq!(parse(&c.foreground), Some(system.window_text.into()));
            assert_eq!(parse(&c.border), Some(system.window_text.into()));
            assert_eq!(parse(&c.input), Some(system.window_text.into()));
            assert_eq!(parse(&c.primary), Some(system.highlight.into()));
            assert_eq!(
                parse(&c.primary_foreground),
                Some(system.highlight_text.into())
            );
            assert_eq!(parse(&c.link), Some(system.hotlight.into()));
            // Selections are drawn under the page's text, so the highlight
            // is opaque only where that text reads on it.
            let selection = parse(&c.selection).unwrap();
            let opaque = contrast(system.window_text.into(), system.highlight.into());
            if opaque >= TEXT_CONTRAST {
                assert_eq!(selection, system.highlight.into());
            } else {
                assert_eq!(
                    Hsla {
                        a: 1.0,
                        ..selection
                    },
                    system.highlight.into()
                );
                assert!(selection.a < 1.0);
            }
        }
    }

    #[test]
    fn high_contrast_holds_what_the_system_leaves_unreadable() {
        // A home-made theme whose link colour vanishes on its window.
        let system = ContrastColors {
            hotlight: rgb(0x101010),
            highlight_text: rgb(0x000000),
            ..night_sky()
        };
        let theme = high_contrast(&system).unwrap();
        let c = &theme.colors;
        let background = rgb(0x000000).into();
        let link = parse(&c.link).unwrap();
        assert!(contrast(link, background) >= TEXT_CONTRAST, "{link:?}");
        let primary = parse(&c.primary).unwrap();
        let on_primary = parse(&c.primary_foreground).unwrap();
        assert!(contrast(on_primary, primary) >= TEXT_CONTRAST);
    }
}
