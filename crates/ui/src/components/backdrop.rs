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

use irori_core::GameInfo;
use irori_core::settings::{LibraryBackground, UiPrefs};

use crate::art::{Ambient, LruImageCache, ambient};
use crate::glass::over;
use crate::motion::fade_in;
use crate::style::{TEXT_CONTRAST, contrast, legible};

/// The most the art shows at the top of the page, in dark and light themes.
/// Light themes take less: dark art over a light page reads as dirt sooner
/// than light art over a dark one reads as glare.
const MAX_WEIGHT: [f32; 2] = [0.62, 0.42];
/// Step between the weights tried, from the most down to none.
const WEIGHT_STEP: f32 = 0.02;
/// How much of the top weight remains at the bottom of the page.
const BOTTOM_SHARE: f32 = 0.6;
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
    weight_at(art, 1., cx)
}

/// [`weight`] with the most it may reach scaled by `strength` (0 to 1).
fn weight_at(art: &Ambient, strength: f32, cx: &App) -> f32 {
    let theme = cx.theme();
    legible_weight_at(
        theme.background,
        &[theme.foreground, theme.muted_foreground],
        [art.darkest, art.lightest],
        MAX_WEIGHT[usize::from(!theme.mode.is_dark())],
        strength,
    )
}

/// The legible weight of art up to `max`, capped at `strength` of `max`.
/// The search runs over the full range and strength only caps its answer:
/// searching from a scaled-down `max` steps through other weights and
/// could land a little above the full-strength answer.
fn legible_weight_at(bg: Hsla, inks: &[Hsla], tones: [[u8; 3]; 2], max: f32, strength: f32) -> f32 {
    strongest_legible_weight(bg, inks, tones, max).min(max * strength.clamp(0., 1.))
}

/// The opacity the art is drawn at: the legible weight at `strength`, then
/// `dim` of the way to none. Both only ever take weight away.
fn shown_weight(legible_at_strength: f32, dim: f32) -> f32 {
    legible_at_strength * (1. - dim.clamp(0., 1.))
}

/// The largest weight from `max` down to none at which every colour of
/// `inks` reads at [`TEXT_CONTRAST`] over both `tones` laid on `bg` at that
/// weight. Inks that don't read on `bg` alone get no art. Big Picture holds
/// its backdrop's art to it too.
pub fn strongest_legible_weight(bg: Hsla, inks: &[Hsla], tones: [[u8; 3]; 2], max: f32) -> f32 {
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

/// The art the library shows behind its covers, as `prefs` ask: `game`'s (the
/// focused, else last played, else first shown) or the user's picture, as an
/// art-store URL.
/// None for no background, and while the system asks windows not to be
/// see-through, which a picture behind text is as much as glass.
pub fn library_backdrop_url(prefs: &UiPrefs, game: Option<&GameInfo>, cx: &App) -> Option<String> {
    if crate::theme::reduce_transparency(cx) || prefs.library_background_strength == 0 {
        return None;
    }
    match prefs.library_background {
        LibraryBackground::None => None,
        LibraryBackground::GameArt => game?.hero_art().map(str::to_owned),
        LibraryBackground::Picture => {
            let picture = prefs.library_background_picture.trim();
            (!picture.is_empty()).then(|| format!("file://{picture}"))
        }
    }
}

/// The library's [`ArtBackdrop`] of `url`, at the strength and dim in `prefs`.
pub fn library_backdrop(prefs: &UiPrefs, url: Option<String>) -> ArtBackdrop {
    ArtBackdrop::new(url)
        .strength(prefs.library_background_strength.min(100) as f32 / 100.)
        .dim(prefs.library_background_dim.min(100) as f32 / 100.)
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
    strength: f32,
    dim: f32,
}

impl ArtBackdrop {
    /// The backdrop of `url`, typically the game's hero art.
    pub fn new(url: Option<impl Into<SharedString>>) -> Self {
        Self {
            url: url.map(Into::into),
            cache: None,
            strength: 1.,
            dim: 0.,
        }
    }

    /// Show the art at `strength` (0 to 1) of the most the theme's text
    /// stays legible over. Weaker art only reads better, so this never
    /// costs legibility.
    pub fn strength(mut self, strength: f32) -> Self {
        self.strength = strength.clamp(0., 1.);
        self
    }

    /// Lay the theme's background over the art at `dim` (0 to 1).
    pub fn dim(mut self, dim: f32) -> Self {
        self.dim = dim.clamp(0., 1.);
        self
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
        let weight = shown_weight(weight_at(&art, self.strength, cx), self.dim);
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

    /// Every channel of `a` laid over `bg` at `w` lies between the two: an
    /// opacity blend never brightens, saturates or adds contrast beyond its
    /// inputs.
    fn between(art: [u8; 3], bg: Hsla, w: f32) -> bool {
        let (a, b, s) = (
            srgb(art).to_rgb(),
            bg.to_rgb(),
            over(srgb(art).opacity(w), bg).to_rgb(),
        );
        [(a.r, b.r, s.r), (a.g, b.g, s.g), (a.b, b.b, s.b)]
            .iter()
            .all(|(a, b, s)| *s >= a.min(*b) - 1e-3 && *s <= a.max(*b) + 1e-3)
    }

    #[test]
    fn strength_and_dim_only_weaken_light_art() {
        // White art over Irori Night's page: strength and dim take weight
        // away, and nothing shows light art brighter than the art itself.
        let (bg, inks) = (grey(15), [grey(250), grey(161)]);
        let tones = [[200, 200, 200], [255, 255, 255]];
        let full = legible_weight_at(bg, &inks, tones, MAX_WEIGHT[0], 1.);
        let mut last = full;
        for strength in [1., 0.9, 0.75, 0.6, 0.5, 0.25, 0.1, 0.] {
            let legible = legible_weight_at(bg, &inks, tones, MAX_WEIGHT[0], strength);
            assert!(legible <= last + 1e-6, "strength {strength}");
            last = legible;
            let mut dimmed = legible;
            for dim in [0., 0.25, 0.5, 1.] {
                let w = shown_weight(legible, dim);
                assert!(w <= dimmed + 1e-6 && w <= full, "dim {dim}");
                assert!(between(tones[1], bg, w));
                for ink in inks {
                    let surface = over(srgb(tones[1]).opacity(w), bg);
                    assert!(contrast(ink, surface) >= TEXT_CONTRAST, "{strength} {dim}");
                }
                dimmed = w;
            }
        }
        assert_eq!(shown_weight(0.5, 1.), 0.);
        assert_eq!(shown_weight(0.5, 2.), 0., "dim past 100 % clamps");
        assert_eq!(shown_weight(0.5, -1.), 0.5, "no dim never adds");
    }

    #[test]
    fn art_that_no_weight_keeps_legible_is_left_out() {
        // Muted text barely reads on the bare page; any black over it fails.
        let w = strongest_legible_weight(grey(255), &[grey(118)], [[0, 0, 0], [0, 0, 0]], 0.36);
        assert_eq!(w, 0.);
    }
}
