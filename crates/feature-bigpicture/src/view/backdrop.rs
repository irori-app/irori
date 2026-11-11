//! What fills the screen behind everything, as the theme says: the focused
//! game's art (blurred, tinted with the art's own colour, crossfading when
//! the focus moves on), a gradient, the user's picture, or the background
//! colour alone. Each is darkened toward the bottom and the left, where text
//! sits. The fade is a one-shot animation, so nothing is drawn while Big
//! Picture sits idle.
//!
//! Game art is held legible the way the library's backdrop is
//! ([`irori_ui::components::strongest_legible_weight`]): it shows as strongly
//! as the top bar's text still reads over its lightest and darkest tones,
//! never more than [`ART_OPACITY`]. Nothing here brightens, saturates or adds
//! contrast to the art: every layer is the art or the page colour laid over
//! it, and the tint ([`art_tint`]) is held to a middle lightness with no more
//! saturation than the art's.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, Hsla, IntoElement, ObjectFit,
    ParentElement as _, Styled as _, StyledImage as _, Window, div, linear_color_stop,
    linear_gradient,
};
use irori_core::GameInfo;
use irori_ui::components::{art_img, strongest_legible_weight};
use irori_ui::motion::STANDARD;
use irori_ui::style::{art_tint, reduce_motion};

use super::{BigPictureView, Colors};
use crate::theme::Background;

/// Width in physical pixels of the stand-in shown until the blurred copy is
/// ready: stretched this far, linear filtering blurs it too.
const STAND_IN_WIDTH: u32 = 48;
/// The most the art shows, before the veil over everything.
const ART_OPACITY: f32 = 0.75;
/// The page colour laid over everything, art included.
const VEIL: f32 = 0.25;

/// Opacity of the art layer for art whose blurred copy runs from the first
/// to the second of `tones` (darkest, lightest; `None` for art not measured
/// yet, taken as black to white): the strongest, up to [`ART_OPACITY`], at
/// which `inks` still read on `bg` where only the veil shades the art (the
/// top bar). The veil keeps `1 - VEIL` of whatever lies under it.
fn art_opacity(bg: Hsla, inks: &[Hsla], tones: Option<[[u8; 3]; 2]>) -> f32 {
    let tones = tones.unwrap_or([[0; 3], [255; 3]]);
    strongest_legible_weight(bg, inks, tones, ART_OPACITY * (1. - VEIL)) / (1. - VEIL)
}

/// One layer of art: the blurred backdrop of `game` (or its stand-in) on the
/// page colour, so a layer without art still covers the one below it.
fn layer(game: Option<&GameInfo>, c: Colors, cx: &mut App) -> gpui_kit::Div {
    let url = game.and_then(|g| g.hero_art().map(str::to_owned));
    let ambient = url
        .as_deref()
        .and_then(|url| irori_ui::art::ambient(url, cx));
    let image = match &ambient {
        Some(a) => Some(gpui_kit::img(a.backdrop.clone())),
        None => url.and_then(|url| art_img(&url, STAND_IN_WIDTH, false, cx)),
    };
    let opacity = art_opacity(
        c.background,
        &[c.foreground, c.muted_foreground],
        ambient.as_ref().map(|a| [a.darkest, a.lightest]),
    );
    let tint = ambient.map(|a| art_tint(a.rgb, cx));
    div()
        .absolute()
        .inset_0()
        .size_full()
        .bg(c.background)
        .when_some(image, |d, img| {
            d.child(
                img.absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .opacity(opacity),
            )
        })
        // The art's colour rising from the bottom, behind the covers.
        .when_some(tint, |d, tint| {
            d.child(div().absolute().inset_0().bg(linear_gradient(
                0.,
                linear_color_stop(tint.opacity(0.28), 0.),
                linear_color_stop(tint.opacity(0.), 0.7),
            )))
        })
}

impl BigPictureView {
    pub(super) fn render_backdrop(
        &self,
        c: Colors,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let veiled = |d: gpui_kit::Div| {
            d
                // Legibility: a light veil overall, deep shade along the
                // bottom (shelves and hints) and the left (titles and lists).
                .child(div().absolute().inset_0().bg(c.background.opacity(VEIL)))
                .child(div().absolute().inset_0().bg(linear_gradient(
                    180.,
                    linear_color_stop(c.background.opacity(0.), 0.3),
                    linear_color_stop(c.background.opacity(0.92), 1.),
                )))
                .child(div().absolute().inset_0().bg(linear_gradient(
                    90.,
                    linear_color_stop(c.background.opacity(0.85), 0.),
                    linear_color_stop(c.background.opacity(0.), 0.6),
                )))
        };
        let base = div().absolute().inset_0().size_full().bg(c.background);
        match self.look.theme().background.clone() {
            Background::Art => {}
            Background::Solid => return base.into_any_element(),
            Background::Gradient { from, to, angle } => {
                return veiled(base.child(div().absolute().inset_0().bg(linear_gradient(
                    angle as f32,
                    linear_color_stop(from.hsla(), 0.),
                    linear_color_stop(to.hsla(), 1.),
                ))))
                .into_any_element();
            }
            Background::Picture(picture) => {
                return veiled(
                    base.child(
                        gpui_kit::img(picture.image)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    ),
                )
                .into_any_element();
            }
        }
        let animate = !reduce_motion(cx) && self.hero_gen > 0;
        let current = layer(self.hero.as_deref(), c, cx);
        let current: AnyElement = if animate {
            current
                .with_animation(
                    ElementId::NamedInteger("bp-hero".into(), self.hero_gen),
                    Animation::new(STANDARD * 2).with_easing(gpui_kit::ease_in_out),
                    |el, t| el.opacity(t),
                )
                .into_any_element()
        } else {
            current.into_any_element()
        };
        veiled(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .when(animate, |d| {
                    d.when_some(self.hero_prev.as_deref(), |d, prev| {
                        d.child(layer(Some(prev), c, cx))
                    })
                })
                .child(current),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Rgba;
    use irori_ui::style::{TEXT_CONTRAST, contrast};

    fn srgb([r, g, b]: [u8; 3]) -> Hsla {
        Rgba {
            r: r as f32 / 255.,
            g: g as f32 / 255.,
            b: b as f32 / 255.,
            a: 1.,
        }
        .into()
    }

    /// The colour at the top of the screen: `tone` at `opacity` over `bg`,
    /// then the veil.
    fn top(tone: [u8; 3], bg: Hsla, opacity: f32) -> Hsla {
        let mix = |a: f32, b: f32, w: f32| a * w + b * (1. - w);
        let (t, b) = (srgb(tone).to_rgb(), bg.to_rgb());
        let w = opacity * (1. - VEIL);
        Rgba {
            r: mix(t.r, b.r, w),
            g: mix(t.g, b.g, w),
            b: mix(t.b, b.b, w),
            a: 1.,
        }
        .into()
    }

    #[test]
    fn light_art_is_held_back_until_the_top_bar_reads() {
        // A dark Big Picture theme over white art.
        let (bg, inks) = (srgb([15; 3]), [srgb([250; 3]), srgb([161; 3])]);
        let tones = Some([[220; 3], [255; 3]]);
        let opacity = art_opacity(bg, &inks, tones);
        assert!(opacity > 0. && opacity < ART_OPACITY, "{opacity}");
        for ink in inks {
            assert!(contrast(ink, top([255; 3], bg, opacity)) >= TEXT_CONTRAST);
        }
        // Shown at the full opacity, the clock wouldn't read.
        assert!(contrast(inks[0], top([255; 3], bg, ART_OPACITY)) < TEXT_CONTRAST);
    }

    #[test]
    fn dark_art_shows_fully_and_nothing_shows_more() {
        let (bg, inks) = (srgb([15; 3]), [srgb([250; 3]), srgb([161; 3])]);
        let dark = art_opacity(bg, &inks, Some([[0; 3], [60, 50, 70]]));
        assert!((dark - ART_OPACITY).abs() < 1e-4, "{dark}");
        for tones in [None, Some([[0; 3], [255; 3]]), Some([[128; 3], [200; 3]])] {
            let o = art_opacity(bg, &inks, tones);
            assert!((0. ..=ART_OPACITY + 1e-4).contains(&o), "{tones:?}: {o}");
        }
        // Art not measured yet is taken at its worst.
        assert!(
            art_opacity(bg, &inks, None)
                <= art_opacity(bg, &inks, Some([[220; 3], [255; 3]])) + 1e-4
        );
    }

    #[test]
    fn a_light_theme_holds_dark_art_back() {
        let (bg, inks) = (srgb([250; 3]), [srgb([20; 3]), srgb([100; 3])]);
        let opacity = art_opacity(bg, &inks, Some([[0; 3], [30; 3]]));
        assert!(opacity < ART_OPACITY);
        for ink in inks {
            assert!(contrast(ink, top([0; 3], bg, opacity)) >= TEXT_CONTRAST);
        }
    }
}
