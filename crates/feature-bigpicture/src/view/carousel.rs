//! Home's Carousel layout: one shelf on a wheel seen from a little above,
//! the focused cover in front and its neighbours turning away and shrinking
//! along the arc, under the focused game's title. Left and Right turn the
//! wheel; Up and Down change the shelf.
//!
//! Every cover springs toward its own place on the wheel (its distance from
//! the focus), so they all glide together when the focus moves and stop
//! drawing frames once they arrive. The covers furthest round fade out
//! before they leave, so none pops.

use gpui_kit::assets::IconName;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, IntoElement, ParentElement as _, SharedString,
    SpringAnimation, Styled as _, Window, div, rems,
};
use irori_ui::motion::{fade_in, rise_in};

use super::home::{InfoStyle, SCROLL_SPRING};
use super::tile::{Cover, CoverSlot, focus_frame};
use super::{BigPictureView, Colors};
use crate::layout::Metrics;
use crate::layout::home::WHEEL_SIDE;

impl BigPictureView {
    pub(super) fn render_carousel(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wheel = m.wheel();
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (row_ix, col) = focused.unwrap_or((0, 0));
        let shape = self.look.theme().covers.shape;
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();

        let info = game.as_ref().map(|game| {
            let style = InfoStyle {
                title: if m.compact { 2.0 } else { 3.0 },
                logo: if m.compact { 0.0 } else { 4.5 },
                lines: if m.compact { 0 } else { 1 },
                centered: true,
                max_w: m.width * 0.7,
            };
            self.game_info(game, style, c, window, cx)
        });

        let mut covers: Vec<(usize, AnyElement)> = Vec::new();
        if let Some(row) = self.rows.get(row_ix) {
            let from = col.saturating_sub(WHEEL_SIDE);
            let to = (col + WHEEL_SIDE + 1).min(row.games.len());
            let (w, h) = (wheel.tile, wheel.tile * shape.ratio());
            let width = m.width;
            for ix in from..to {
                let game = row.games[ix].clone();
                let id =
                    SharedString::from(format!("bp-wheel-{}-{}", row.kind.id(), game.id().key()));
                let cover = Cover {
                    game,
                    id: id.clone(),
                    width: w,
                    scale: 1.0,
                    shape,
                    gap: 0.0,
                    focused: ix == col && self.overlay.is_none(),
                    slot: CoverSlot::Shelf {
                        row: row_ix,
                        col: ix,
                    },
                };
                let tile = self.cover_tile(&cover, c, window, cx);
                let d = ix as f32 - col as f32;
                let element = tile
                    .with_spring(
                        SharedString::from(format!("{id}-turn")),
                        SpringAnimation::new(SCROLL_SPRING).to(d),
                        move |el, v: f32| {
                            let p = wheel.place(v, width);
                            let front = (1.0 - v.abs()).clamp(0.0, 1.0);
                            focus_frame(
                                el.absolute()
                                    .left(rems(p.x - w * p.scale / 2.0))
                                    .top(rems(p.y - h * p.scale / 2.0))
                                    .w(rems(w * p.scale))
                                    .h(rems(h * p.scale))
                                    .opacity(p.opacity),
                                c,
                                front,
                            )
                        },
                    )
                    .into_any_element();
                covers.push((ix.abs_diff(col), element));
            }
        }
        // Covers further round are drawn first, so nearer ones overlap them.
        covers.sort_by_key(|(distance, _)| std::cmp::Reverse(*distance));
        let shelf_id = self
            .rows
            .get(row_ix)
            .map(|r| r.kind.id())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .w_full()
                    .h(rems(wheel.info))
                    .flex_shrink_0()
                    .px(rems(m.gutter))
                    .justify_end()
                    .items_center()
                    .children(info.map(|info| {
                        rise_in(
                            SharedString::from(format!("bp-wheel-info-{key}")),
                            info,
                            12.,
                        )
                    })),
            )
            .child(
                div()
                    .px(rems(m.gutter))
                    .flex_shrink_0()
                    .child(self.shelf_switcher(
                        row_ix,
                        (IconName::ChevronUp, IconName::ChevronDown),
                        true,
                        c,
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    // A new shelf's wheel fades in rather than turning in.
                    .child(fade_in(
                        SharedString::from(format!("bp-wheel-{shelf_id}")),
                        div()
                            .absolute()
                            .inset_0()
                            .children(covers.into_iter().map(|(_, e)| e)),
                    )),
            )
            .into_any_element()
    }
}
