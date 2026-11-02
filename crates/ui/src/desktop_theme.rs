//! Turning a desktop's colours into an Irori theme.
//!
//! Desktops that colour everything from one place (see [`crate::omarchy`] and
//! [`crate::cosmic`]) don't describe the many surfaces Irori paints. Each fills
//! in a [`Palette`], and the rest is derived here, so both read the same way
//! and text stays legible whatever the desktop's colours are.

use gpui_kit::component::ThemeConfig;
use gpui_kit::{Hsla, Rgba, rgb};
use serde_json::json;

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
}

/// The themes a desktop offers right now.
pub(crate) struct DesktopThemes {
    pub(crate) light: Option<ThemeConfig>,
    pub(crate) dark: Option<ThemeConfig>,
    /// The mode the desktop is in.
    pub(crate) is_dark: bool,
    /// Whether the desktop's windows are see-through, on a desktop that has
    /// its own say in that.
    pub(crate) frosted: Option<bool>,
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
            frosted: None,
        }
    }
}

/// The colours a desktop gives Irori.
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
        let theme = json!({
            "name": self.name,
            "mode": if self.dark { "dark" } else { "light" },
            "radius": self.radius,
            "radius.lg": self.radius_lg,
            "colors": self.colors(),
        });
        serde_json::from_value(theme)
            .inspect_err(|e| tracing::warn!("building a theme from {}'s: {e}", self.name))
            .ok()
    }
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
