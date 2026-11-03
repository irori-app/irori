//! A game's cover as Big Picture shows it on shelves and in the grid. The
//! focused cover grows, lifts and glows on a spring; uninstalled games are
//! dimmed; running operations show their progress across the bottom.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, BoxShadow, ClickEvent, Context, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, point, px, rems,
};
use irori_core::{GameInfo, StatusKind};
use irori_ui::components::{ArtImage, art_image, art_loading, missing_art_fill, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::motion::HOVER_SPRING;

use super::{BigPictureView, Colors};
use crate::layout::{TILE_GAP, TILE_RATIO};
use crate::text::status_with_percent;

/// How far the focused cover rises, in rem.
const LIFT: f32 = 0.5;

/// A cover to draw.
pub(super) struct Cover {
    pub game: Arc<GameInfo>,
    /// Unique among the covers on screen (shelf or grid, and the game).
    pub id: SharedString,
    /// Width at rest, in rem.
    pub width: f32,
    /// Scale when focused.
    pub scale: f32,
    pub focused: bool,
}

impl BigPictureView {
    /// The cover in a slot sized for its focused scale, so neighbours never
    /// move when it grows.
    pub(super) fn render_cover(
        &self,
        cover: Cover,
        c: Colors,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Cover {
            game,
            id,
            width,
            scale,
            focused,
        } = cover;
        let state = Self::game_state(&game, cx);
        let status = models(cx).statuses.read(cx).get(&game.id()).cloned();
        let image = match game.cover_art() {
            Some(url) => art_image(url, physical_width(rems(width * scale), window), false, cx),
            None => ArtImage::Missing,
        };

        let badge = if state.playing {
            Some(Tag::primary().small().child(t("bigpicture.badge.playing")))
        } else if state.has_update {
            Some(Tag::secondary().small().child(t("bigpicture.badge.update")))
        } else {
            None
        };
        // Uninstalled games are dimmed instead of badged, so installed ones
        // stand out; the focused one shows in full.
        let dim = !state.installed && !state.in_queue && !focused && state.busy.is_none();
        let progress = status
            .filter(|s| s.kind.is_busy() && s.kind != StatusKind::Playing)
            .map(|s| {
                let pct = s.progress.and_then(|p| p.percent);
                (status_with_percent(s.kind, pct), pct)
            });
        let label = if state.installed {
            SharedString::from(game.title.clone())
        } else {
            t_args(
                "bigpicture.tile.not_installed",
                &[("title", game.title.as_str())],
            )
        };

        let tile = div()
            .id(id.clone())
            .role(Role::Button)
            .aria_label(label)
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .relative()
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(c.radius_lg)
            .bg(c.muted)
            .border_2()
            .cursor_pointer()
            .map(|d| match image {
                ArtImage::Ready(img) => d.child(
                    img.size_full()
                        .rounded(c.radius_lg)
                        .object_fit(ObjectFit::Cover)
                        .when(dim, |img| img.opacity(0.5)),
                ),
                ArtImage::Loading => d.child(art_loading()),
                ArtImage::Missing => d.child(
                    v_flex()
                        .size_full()
                        .justify_between()
                        .p_3()
                        .bg(missing_art_fill(cx))
                        .child(Icon::new(IconName::Gamepad2).text_color(c.muted_foreground))
                        .child(
                            div()
                                .text_lg()
                                .font_bold()
                                .line_clamp(4)
                                .text_color(c.foreground)
                                .child(game.title.clone()),
                        ),
                ),
            })
            .when_some(badge, |d, badge| {
                d.child(
                    h_flex()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .min_w_0()
                        .overflow_hidden()
                        .child(badge),
                )
            })
            .when_some(progress, |d, (label, pct)| {
                d.child(
                    v_flex()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .p_2()
                        .gap_1()
                        .bg(c.background.opacity(0.85))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(c.foreground)
                                .child(label),
                        )
                        .child(irori_ui::motion::gliding_progress(
                            SharedString::from(format!("{id}-progress")),
                            pct,
                        )),
                )
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                on_click(this, window, cx);
                this.focus.focus(window, cx);
            }));

        let (ring, edge) = (c.ring, c.foreground);
        let lifted = tile.with_spring(
            SharedString::from(format!("{id}-focus")),
            SpringAnimation::new(HOVER_SPRING).to(if focused { 1.0 } else { 0.0 }),
            move |el, f: f32| {
                let w = width * (1.0 + (scale - 1.0) * f);
                el.w(rems(w))
                    .h(rems(w * TILE_RATIO))
                    .top(rems(-LIFT * f))
                    .border_color(edge.opacity(0.9 * f))
                    // Covers at rest cast no glow, so none is drawn.
                    .when(f > 0.01, |el| {
                        el.shadow(vec![BoxShadow {
                            color: ring.opacity(0.55 * f),
                            offset: point(px(0.), px(8.)),
                            blur_radius: px(36.),
                            spread_radius: px(2.),
                            inset: false,
                        }])
                    })
            },
        );
        div()
            .w(rems(width * scale + TILE_GAP))
            .h(rems(width * TILE_RATIO * scale + TILE_GAP))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(lifted)
            .into_any_element()
    }
}
