//! The focused game's art behind everything: blurred, darkened toward the
//! bottom and the left where text sits, tinted with the art's own colour, and
//! crossfading when the focus moves on. The fade is a one-shot animation, so
//! nothing is drawn while Big Picture sits idle.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, ElementId, IntoElement, ObjectFit,
    ParentElement as _, Styled as _, StyledImage as _, Window, div, linear_color_stop,
    linear_gradient,
};
use irori_core::GameInfo;
use irori_ui::components::art_img;
use irori_ui::motion::STANDARD;
use irori_ui::style::{art_tint, reduce_motion};

use super::{BigPictureView, Colors};

/// Width in physical pixels of the stand-in shown until the blurred copy is
/// ready: stretched this far, linear filtering blurs it too.
const STAND_IN_WIDTH: u32 = 48;

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
                    .opacity(0.75),
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
    ) -> impl IntoElement {
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
        div()
            .absolute()
            .inset_0()
            .size_full()
            .when(animate, |d| {
                d.when_some(self.hero_prev.as_deref(), |d, prev| {
                    d.child(layer(Some(prev), c, cx))
                })
            })
            .child(current)
            // Legibility: a light veil overall, deep shade along the bottom
            // (shelves and hints) and the left (titles and lists).
            .child(div().absolute().inset_0().bg(c.background.opacity(0.25)))
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
    }
}
