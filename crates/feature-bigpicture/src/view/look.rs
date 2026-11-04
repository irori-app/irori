//! What Big Picture looks like now: the colours it paints with (the
//! interface theme's, with a Big Picture theme's laid over them and held
//! legible), the theme and layout in use, the user's own themes, and the
//! editor's draft while it is open.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, Pixels, Rgba, px};
use irori_core::UiPrefs;
use irori_ui::style::{ICON_CONTRAST, TEXT_CONTRAST, contrast, legible_towards};

use super::editor::Editor;
use crate::layout::HomeLayout;
use crate::theme::store::{ThemeRef, UserTheme};
use crate::theme::{Corners, Palette, Theme, gallery};

/// Theme colours Big Picture paints with, read once per frame: the
/// interface theme's, with the Big Picture theme's laid over them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Colors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub ring: Hsla,
    pub border: Hsla,
    pub popover: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub danger: Hsla,
    pub red: Hsla,
    pub green: Hsla,
    pub blue: Hsla,
    pub yellow: Hsla,
    pub magenta: Hsla,
    pub radius: Pixels,
    pub radius_lg: Pixels,
}

/// `a` mixed `t` of the way to `b`, opaque.
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
    .into()
}

/// White on dark colours, black on light ones: where text must move to
/// read on `bg`.
fn away_from(bg: Hsla) -> Hsla {
    if contrast(gpui_kit::white(), bg) >= contrast(gpui_kit::black(), bg) {
        gpui_kit::white()
    } else {
        gpui_kit::black()
    }
}

impl Colors {
    /// The interface theme's colours.
    pub fn base(cx: &App) -> Self {
        let t = cx.theme();
        Self {
            background: t.background,
            foreground: t.foreground,
            muted: t.muted,
            muted_foreground: t.muted_foreground,
            ring: t.ring,
            border: t.border,
            popover: t.popover,
            primary: t.primary,
            primary_foreground: t.primary_foreground,
            danger: t.danger,
            red: t.red,
            green: t.green,
            blue: t.blue,
            yellow: t.yellow,
            magenta: t.magenta,
            radius: t.radius,
            radius_lg: t.radius_lg,
        }
    }

    /// The interface theme's colours under `theme`.
    pub fn of(cx: &App, theme: &Theme) -> Self {
        Self::base(cx).themed(&theme.palette, theme.covers.corners)
    }

    /// These colours with `palette` laid over them and corners of `corners`.
    /// Text, rings and buttons are held to WCAG contrast on the surfaces
    /// they sit on, so a theme can't make Big Picture unreadable.
    pub fn themed(mut self, palette: &Palette, corners: Corners) -> Self {
        match corners {
            Corners::Sharp => {
                self.radius = px(0.);
                self.radius_lg = px(0.);
            }
            Corners::Rounded => {}
            Corners::Round => {
                self.radius = px(10.);
                self.radius_lg = px(20.);
            }
        }
        if *palette == Palette::default() {
            return self;
        }
        if let Some(bg) = palette.background {
            self.background = bg.hsla();
            let toward = away_from(self.background);
            self.muted = mix(self.background, toward, 0.1);
            self.popover = mix(self.background, toward, 0.06);
            self.border = mix(self.background, toward, 0.2);
        }
        if let Some(surface) = palette.surface {
            self.popover = surface.hsla();
            self.muted = mix(self.popover, away_from(self.popover), 0.06);
        }
        if let Some(fg) = palette.foreground {
            self.foreground = fg.hsla();
        }
        if let Some(accent) = palette.accent {
            self.primary = accent.hsla();
            self.ring = accent.hsla();
        }
        let bg = self.background;
        let surfaces = [self.background, self.popover, self.muted];
        let toward = away_from(bg);
        self.foreground = legible_towards(self.foreground, &surfaces, TEXT_CONTRAST, toward);
        self.muted_foreground = legible_towards(
            mix(self.foreground, bg, 0.35),
            &surfaces,
            TEXT_CONTRAST,
            toward,
        );
        self.ring = legible_towards(self.ring, &[bg], ICON_CONTRAST, toward);
        self.primary_foreground = legible_towards(
            self.primary_foreground,
            &[self.primary],
            TEXT_CONTRAST,
            away_from(self.primary),
        );
        for color in [
            &mut self.danger,
            &mut self.red,
            &mut self.green,
            &mut self.blue,
            &mut self.yellow,
            &mut self.magenta,
        ] {
            *color = legible_towards(*color, &[bg], ICON_CONTRAST, toward);
        }
        self
    }
}

/// The theme and layout in use, the user's own themes, and the editor.
pub(super) struct Look {
    /// The layout setting.
    pub layout: HomeLayout,
    /// The theme setting.
    pub active: ThemeRef,
    /// The active theme, resolved.
    pub theme: Theme,
    /// The user's themes, once read from disk.
    pub user: Vec<UserTheme>,
    pub loaded: bool,
    pub editor: Option<Editor>,
}

impl Look {
    pub fn new(ui: &UiPrefs) -> Self {
        let mut look = Self {
            layout: HomeLayout::default(),
            active: ThemeRef::BuiltIn(gallery::DEFAULT_ID.into()),
            theme: Theme::default(),
            user: Vec::new(),
            loaded: false,
            editor: None,
        };
        look.follow_settings(ui);
        look
    }

    /// The theme to draw: the editor's draft while it is open.
    pub fn theme(&self) -> &Theme {
        match &self.editor {
            Some(editor) => &editor.draft,
            None => &self.theme,
        }
    }

    /// The layout to draw: the editor's while it is open.
    pub fn layout(&self) -> HomeLayout {
        match &self.editor {
            Some(editor) => editor.layout,
            None => self.layout,
        }
    }

    /// Take up changed settings. Returns whether anything changed.
    pub fn follow_settings(&mut self, ui: &UiPrefs) -> bool {
        let layout = HomeLayout::from_setting(&ui.big_picture_layout);
        let active = ThemeRef::parse(&ui.big_picture_theme);
        if layout == self.layout && active == self.active {
            return false;
        }
        self.layout = layout;
        self.active = active;
        self.resolve();
        true
    }

    /// Find the active theme; Irori's own while the user's themes load or
    /// when the one named is gone.
    pub fn resolve(&mut self) {
        self.theme = self
            .find(&self.active)
            .unwrap_or_else(|| gallery::all()[0].theme.clone());
    }

    pub fn find(&self, r: &ThemeRef) -> Option<Theme> {
        match r {
            ThemeRef::BuiltIn(id) => gallery::get(id).map(|b| b.theme.clone()),
            ThemeRef::User(id) => self
                .user
                .iter()
                .find(|u| &u.id == id)
                .map(|u| u.theme.clone()),
        }
    }

    /// Every theme to choose from: the gallery, then the user's.
    pub fn choices(&self) -> Vec<ThemeRef> {
        gallery::all()
            .iter()
            .map(|b| ThemeRef::BuiltIn(b.id.into()))
            .chain(self.user.iter().map(|u| ThemeRef::User(u.id.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Rgb;

    fn dark() -> Colors {
        let gray = |l: f32| Hsla {
            h: 0.0,
            s: 0.0,
            l,
            a: 1.0,
        };
        Colors {
            background: gray(0.08),
            foreground: gray(0.95),
            muted: gray(0.15),
            muted_foreground: gray(0.7),
            ring: gray(0.8),
            border: gray(0.2),
            popover: gray(0.12),
            primary: gray(0.9),
            primary_foreground: gray(0.1),
            danger: gray(0.6),
            red: gray(0.6),
            green: gray(0.6),
            blue: gray(0.6),
            yellow: gray(0.6),
            magenta: gray(0.6),
            radius: px(6.),
            radius_lg: px(10.),
        }
    }

    #[test]
    fn no_palette_keeps_the_interface_colours() {
        assert_eq!(dark().themed(&Palette::default(), Corners::Rounded), dark());
        let sharp = dark().themed(&Palette::default(), Corners::Sharp);
        assert_eq!(sharp.radius_lg, px(0.));
    }

    #[test]
    fn themed_text_stays_legible_whatever_the_palette() {
        let palettes = [
            // Dark text on a dark background.
            Palette {
                background: Some(Rgb([10, 10, 20])),
                foreground: Some(Rgb([40, 40, 50])),
                accent: Some(Rgb([20, 20, 60])),
                surface: None,
            },
            // A light theme over a dark interface theme.
            Palette {
                background: Some(Rgb([243, 237, 226])),
                foreground: None,
                accent: Some(Rgb([255, 230, 0])),
                surface: Some(Rgb([251, 248, 242])),
            },
            Palette {
                accent: Some(Rgb([255, 255, 255])),
                ..Default::default()
            },
        ];
        for palette in palettes {
            let c = dark().themed(&palette, Corners::Rounded);
            for surface in [c.background, c.popover, c.muted] {
                assert!(contrast(c.foreground, surface) >= TEXT_CONTRAST - 0.01);
                assert!(contrast(c.muted_foreground, surface) >= TEXT_CONTRAST - 0.01);
            }
            assert!(contrast(c.ring, c.background) >= ICON_CONTRAST - 0.01);
            assert!(contrast(c.primary_foreground, c.primary) >= TEXT_CONTRAST - 0.01);
            assert!(contrast(c.green, c.background) >= ICON_CONTRAST - 0.01);
        }
    }
}
