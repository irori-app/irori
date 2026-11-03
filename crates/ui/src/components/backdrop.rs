//! A game's art filling a page behind its content: the copy blurred once on
//! disk by the art pipeline ([`crate::art::ambient`]), shown as strongly as
//! the theme's text allows over it and fading towards the page background
//! further down.
//!
//! Legibility is measured, not guessed: the art pipeline records the darkest
//! and lightest tones of the blurred copy, and the art is shown at the
//! strongest weight at which the theme's text and muted text still read at
//! WCAG AA over both, down to not at all. So everything drawn in the theme's
//! colours stays legible over it, and [`BackdropInk`] holds the colours of
//! panels over it to the same standard.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Entity, Hsla, IntoElement, ObjectFit, ParentElement as _, RenderOnce, Rgba, SharedString,
    Styled, StyledImage as _, Window, div, img, linear_color_stop, linear_gradient,
};

use crate::art::{Ambient, LruImageCache, ambient};
use crate::glass::over;
use crate::motion::fade_in;
use crate::style::{TEXT_CONTRAST, contrast, legible};

/// The most the art shows at the top of the page, in dark and light themes.
/// Light themes take less: dark art over a light page reads as dirt sooner
/// than light art over a dark one reads as glare.
const MAX_WEIGHT: [f32; 2] = [0.5, 0.36];
/// Step between the weights tried, from the most down to none.
const WEIGHT_STEP: f32 = 0.02;
/// How much of the top weight remains at the bottom of the page.
const BOTTOM_SHARE: f32 = 0.4;
/// Opacity of [`BackdropInk::panel`]: enough body for dense text, little
/// enough that the art still shows through.
const PANEL_ALPHA: f32 = 0.7;

/// Text colours for content drawn over an [`ArtBackdrop`] of the same art.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackdropInk {
    /// Body text and titles over the backdrop.
    pub text: Hsla,
    /// Secondary text over the backdrop.
    pub muted: Hsla,
    /// Fill of a panel over the backdrop (translucent).
    pub panel: Hsla,
    /// Secondary text on such a panel.
    pub panel_muted: Hsla,
    /// The darkest and the lightest the backdrop gets behind content, to hold
    /// other colours to with [`crate::style::legible`].
    pub surfaces: [Hsla; 2],
}

impl BackdropInk {
    /// The colours for content over the backdrop of `url` (`None`, or art
    /// not ready yet, is the plain page).
    pub fn of(url: Option<&str>, cx: &mut App) -> Self {
        let art = url.and_then(|u| ambient(u, cx));
        Self::for_ambient(art.as_ref(), cx)
    }

    fn for_ambient(art: Option<&Ambient>, cx: &App) -> Self {
        let theme = cx.theme();
        let (bg, fg, muted) = (theme.background, theme.foreground, theme.muted_foreground);
        let panel = bg.opacity(PANEL_ALPHA);
        let surfaces = match art {
            Some(a) => {
                let weight = weight(a, cx);
                [a.darkest, a.lightest].map(|tone| over(srgb(tone).opacity(weight), bg))
            }
            None => [bg, bg],
        };
        let on_panel = surfaces.map(|s| over(panel, s));
        Self {
            text: legible(fg, &surfaces, TEXT_CONTRAST, cx),
            muted: legible(muted, &surfaces, TEXT_CONTRAST, cx),
            panel,
            panel_muted: legible(muted, &on_panel, TEXT_CONTRAST, cx),
            surfaces,
        }
    }
}

/// How strongly `art` shows at the top of the page in the current theme.
fn weight(art: &Ambient, cx: &App) -> f32 {
    let theme = cx.theme();
    let max = MAX_WEIGHT[usize::from(!theme.mode.is_dark())];
    strongest_legible_weight(
        theme.background,
        &[theme.foreground, theme.muted_foreground],
        [art.darkest, art.lightest],
        max,
    )
}

/// The largest weight from `max` down to none at which every colour of
/// `inks` reads at [`TEXT_CONTRAST`] over both `tones` laid on `bg` at that
/// weight. Inks that don't read on `bg` alone get no art.
fn strongest_legible_weight(bg: Hsla, inks: &[Hsla], tones: [[u8; 3]; 2], max: f32) -> f32 {
    let reads = |w: f32| {
        tones.iter().all(|tone| {
            let surface = over(srgb(*tone).opacity(w), bg);
            inks.iter()
                .all(|ink| contrast(*ink, surface) >= TEXT_CONTRAST)
        })
    };
    let steps = (max / WEIGHT_STEP).round() as u32;
    (0..steps)
        .map(|i| max - i as f32 * WEIGHT_STEP)
        .find(|w| reads(*w))
        .unwrap_or(0.)
}

fn srgb([r, g, b]: [u8; 3]) -> Hsla {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// A game's blurred art filling its parent (which should be `relative`),
/// over the theme's background. Place it first, so content paints over it,
/// and colour that content with [`BackdropInk::of`] for the same art.
///
/// Costs nothing per frame beyond drawing one small image: the blur is made
/// once and kept on disk. Until it exists the page shows its plain background.
#[derive(IntoElement)]
pub struct ArtBackdrop {
    url: Option<SharedString>,
    cache: Option<Entity<LruImageCache>>,
}

impl ArtBackdrop {
    /// The backdrop of `url`, typically the game's hero art.
    pub fn new(url: Option<impl Into<SharedString>>) -> Self {
        Self {
            url: url.map(Into::into),
            cache: None,
        }
    }

    /// Keep the decoded image in `cache` rather than the window's own.
    pub fn image_cache(mut self, cache: &Entity<LruImageCache>) -> Self {
        self.cache = Some(cache.clone());
        self
    }
}

impl RenderOnce for ArtBackdrop {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let art = self.url.as_deref().and_then(|u| ambient(u, cx));
        let bg = cx.theme().background;
        let base = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            .bg(bg);
        let Some(art) = art else {
            return base;
        };
        let weight = weight(&art, cx);
        if weight <= 0. {
            return base;
        }
        let image = img(art.backdrop.clone())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .object_fit(ObjectFit::Cover)
            .opacity(weight)
            .when_some(self.cache, |i, cache| i.image_cache(&cache));
        // Fade the art out down the page, so long content ends on the theme's
        // own background rather than on the bottom of the picture.
        let scrim = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(linear_gradient(
                180.,
                linear_color_stop(bg.opacity(0.), 0.2),
                linear_color_stop(bg.opacity(1. - BOTTOM_SHARE), 1.),
            ));
        let layers = div().size_full().child(image).child(scrim);
        let key = SharedString::from(format!("art-backdrop-{}", art.backdrop.display()));
        base.child(if crate::style::reduce_motion(cx) {
            layers.into_any_element()
        } else {
            fade_in(key, layers)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(v: u8) -> Hsla {
        srgb([v, v, v])
    }

    #[test]
    fn dark_art_on_a_dark_theme_shows_fully() {
        let w = strongest_legible_weight(
            grey(15),
            &[grey(250), grey(161)],
            [[0, 0, 0], [40, 30, 50]],
            0.5,
        );
        assert_eq!(w, 0.5);
    }

    #[test]
    fn bright_art_is_held_back_until_muted_text_reads() {
        let (bg, inks) = (grey(15), [grey(250), grey(161)]);
        let tones = [[10, 10, 10], [255, 255, 255]];
        let w = strongest_legible_weight(bg, &inks, tones, 0.5);
        assert!(w > 0. && w < 0.5, "{w}");
        let surface = over(srgb(tones[1]).opacity(w), bg);
        assert!(contrast(inks[1], surface) >= TEXT_CONTRAST);
        let stronger = over(srgb(tones[1]).opacity(w + WEIGHT_STEP), bg);
        assert!(contrast(inks[1], stronger) < TEXT_CONTRAST);
    }

    #[test]
    fn art_that_no_weight_keeps_legible_is_left_out() {
        // Muted text barely reads on the bare page; any black over it fails.
        let w = strongest_legible_weight(grey(255), &[grey(118)], [[0, 0, 0], [0, 0, 0]], 0.36);
        assert_eq!(w, 0.);
    }
}
