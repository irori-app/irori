//! Home's Wall layout: every shelf as a section of dense rows of small
//! covers under its heading. The arrows move across rows and on into the
//! next section at the same column ([`crate::layout::home::wall_step`]);
//! the wall glides to keep the focused row two fifths of the way down, and
//! only the rows near the view are built.

use gpui_kit::component::{StyledExt as _, h_flex};
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, SpringAnimation, StatefulInteractiveElement as _,
    Styled as _, Window, div, rems,
};

use super::home::SCROLL_SPRING;
use super::tile::{Cover, CoverSlot};
use super::{BigPictureView, Colors};
use crate::layout::Metrics;
use crate::layout::home::{WALL_FOCUS_SCALE, WALL_GAP};

impl BigPictureView {
    pub(super) fn render_wall(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wall = m.wall();
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (tops, total) = wall.sections(&lens);
        let view = m.area();
        let scroll = focused.map_or(0.0, |f| wall.scroll(&tops, total, f, view));
        // Rows within half a screen of the view are built, so the wall has
        // its neighbours ready while it glides there.
        let (from, to) = (
            scroll - view * 0.5 - wall.row,
            scroll + view * 1.5 + wall.row,
        );
        let shape = self.look.theme().covers.shape;
        let slot = wall.tile * WALL_FOCUS_SCALE + WALL_GAP;
        let left = m.gutter - WALL_GAP / 2.0;

        let mut children: Vec<AnyElement> = Vec::new();
        for (section, row) in self.rows.iter().enumerate() {
            let len = row.games.len();
            if len == 0 {
                continue;
            }
            let top = tops[section];
            let bottom = top + wall.heading + wall.rows(len) as f32 * wall.row;
            if bottom < from || top > to {
                continue;
            }
            let current = focused.is_some_and(|(r, _)| r == section);
            children.push(
                h_flex()
                    .absolute()
                    .top(rems(top))
                    .left(rems(m.gutter))
                    .right(rems(m.gutter))
                    .h(rems(wall.heading))
                    .gap_2()
                    .items_center()
                    .text_xl()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_bold()
                            .text_color(c.foreground.opacity(if current { 1.0 } else { 0.6 }))
                            .child(row.kind.title()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(c.foreground.opacity(0.5))
                            .child(SharedString::from(format!("· {len}"))),
                    )
                    .into_any_element(),
            );
            for r in 0..wall.rows(len) {
                let y = top + wall.heading + r as f32 * wall.row;
                if y + wall.row < from || y > to {
                    continue;
                }
                let start = r * wall.cols;
                let end = (start + wall.cols).min(len);
                let tiles: Vec<AnyElement> = (start..end)
                    .map(|ix| {
                        let game = row.games[ix].clone();
                        let key = game.id().key();
                        let cover = Cover {
                            game,
                            id: SharedString::from(format!("bp-wall-{}-{key}", row.kind.id())),
                            width: wall.tile,
                            scale: WALL_FOCUS_SCALE,
                            shape,
                            gap: WALL_GAP,
                            focused: focused == Some((section, ix)) && self.overlay.is_none(),
                            slot: CoverSlot::Shelf {
                                row: section,
                                col: ix,
                            },
                        };
                        self.render_cover(cover, c, window, cx)
                    })
                    .collect();
                children.push(
                    h_flex()
                        .absolute()
                        .top(rems(y))
                        .left(rems(left))
                        .w(rems(slot * wall.cols as f32))
                        .h(rems(wall.row))
                        .items_center()
                        .children(tiles)
                        .into_any_element(),
                );
            }
        }

        div()
            .id("bp-wall")
            .role(Role::Grid)
            .size_full()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(rems(total))
                    .children(children)
                    .with_spring(
                        "bp-wall-scroll",
                        SpringAnimation::new(SCROLL_SPRING).to(scroll),
                        |el, v: f32| el.top(rems(-v)),
                    ),
            )
            .into_any_element()
    }
}
