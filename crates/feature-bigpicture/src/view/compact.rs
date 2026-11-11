//! Home's Compact layout, for small screens such as the Steam Deck's: the
//! shelves as chips along the top, the focused shelf as a list of titles,
//! and the focused game's cover and facts beside it. Up and Down run along
//! the list; Left and Right change the shelf. Only the rows near the view
//! are built.

use gpui_kit::component::{StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_ui::components::{art_img, physical_width};
use irori_ui::motion::rise_in;

use super::home::{InfoStyle, SCROLL_SPRING};
use super::tile::CoverSlot;
use super::{BigPictureView, Colors};
use crate::layout::home::{LIST_ROW, SHELF_CHIPS};
use crate::layout::{Metrics, end_scroll, list_scroll, strip_start};

/// Shelf chips shown before the focused one when there are many.
const CHIPS_BEFORE: usize = 2;

impl BigPictureView {
    pub(super) fn render_compact(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = m.compact_layout();
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (row_ix, col) = focused.unwrap_or((0, 0));
        let shape = self.look.theme().covers.shape;
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();

        // Chips: the shelves, from a little before the focused one.
        let first_chip = row_ix.saturating_sub(CHIPS_BEFORE);
        let chips: Vec<AnyElement> = self
            .rows
            .iter()
            .enumerate()
            .skip(first_chip)
            .map(|(i, row)| {
                let current = i == row_ix;
                div()
                    .id(SharedString::from(format!(
                        "bp-compact-chip-{}",
                        row.kind.id()
                    )))
                    .role(Role::Tab)
                    .aria_selected(current)
                    .flex_shrink_0()
                    .px_4()
                    .py_1()
                    .rounded_full()
                    .cursor_pointer()
                    .text_lg()
                    .map(|d| {
                        if current {
                            d.bg(c.foreground).text_color(c.background).font_bold()
                        } else {
                            d.bg(c.foreground.opacity(0.1))
                                .text_color(c.foreground.opacity(0.8))
                        }
                    })
                    .child(row.kind.title())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        let lens = this.lens();
                        let col = this.grid.column(i);
                        this.grid.set(i, col, &lens);
                        this.after_focus_change(cx);
                        this.focus.focus(window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        // The list of the focused shelf.
        let mut rows: Vec<AnyElement> = Vec::new();
        let len = self.rows.get(row_ix).map_or(0, |r| r.games.len());
        let scroll = end_scroll(
            list_scroll(col, None, LIST_ROW, 0.0, layout.list_h),
            len as f32 * LIST_ROW,
            layout.list_h,
        );
        if let Some(row) = self.rows.get(row_ix) {
            let shown = (layout.list_h / LIST_ROW).ceil() as usize;
            let start = strip_start((scroll / LIST_ROW) as usize, row.games.len(), shown);
            let from = start.saturating_sub(3);
            let to = (start + shown + 3).min(row.games.len());
            let thumb_w = LIST_ROW * 0.8 / shape.ratio().max(0.75);
            for ix in from..to {
                let game = row.games[ix].clone();
                let current = ix == col && self.overlay.is_none() && self.look.editor.is_none();
                let state = Self::game_state(&game, cx);
                let thumb = shape
                    .art(&game)
                    .and_then(|url| art_img(url, physical_width(rems(thumb_w), window), false, cx));
                let slot = CoverSlot::Shelf {
                    row: row_ix,
                    col: ix,
                };
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!(
                            "bp-compact-{}-{}",
                            row.kind.id(),
                            game.id().key()
                        )))
                        .role(Role::ListBoxOption)
                        .aria_label(SharedString::from(game.title.clone()))
                        .aria_selected(current)
                        .when(current, |d| d.aria_active_descendant())
                        .absolute()
                        .top(rems(ix as f32 * LIST_ROW))
                        .left_0()
                        .right_0()
                        .h(rems(LIST_ROW))
                        .px_3()
                        .gap_3()
                        .items_center()
                        .rounded(c.radius_lg)
                        .cursor_pointer()
                        .map(|d| {
                            if current {
                                d.bg(c.foreground).text_color(c.background)
                            } else {
                                d.text_color(c.foreground)
                            }
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(rems(thumb_w))
                                .h(rems(LIST_ROW * 0.8))
                                .overflow_hidden()
                                .rounded(c.radius)
                                .bg(c.muted)
                                .children(thumb.map(|img| {
                                    img.image_cache(&self.image_cache)
                                        .size_full()
                                        .object_fit(ObjectFit::Cover)
                                })),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_lg()
                                        .font_semibold()
                                        .child(game.title.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .opacity(if current { 0.85 } else { 0.65 })
                                        .child(Self::meta_line(&game, state, true, cx)),
                                ),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            this.click_cover(slot, event.click_count(), cx);
                            this.focus.focus(window, cx);
                        }))
                        .into_any_element(),
                );
            }
        }

        let detail = game.as_ref().map(|game| {
            let art = shape.art(game).and_then(|url| {
                art_img(url, physical_width(rems(layout.cover_w), window), false, cx)
            });
            let style = InfoStyle {
                title: 2.0,
                logo: 0.0,
                lines: if m.compact { 2 } else { 4 },
                centered: false,
                max_w: m.width - layout.list_w - 3.0 * m.gutter,
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_4()
                .child(
                    div()
                        .flex_shrink_0()
                        .w(rems(layout.cover_w))
                        .h(rems(layout.cover_w * shape.ratio()))
                        .overflow_hidden()
                        .rounded(c.radius_lg)
                        .bg(c.muted)
                        .shadow_lg()
                        .children(art.map(|img| {
                            img.image_cache(&self.image_cache)
                                .size_full()
                                .object_fit(ObjectFit::Cover)
                        })),
                )
                .child(self.game_info(game, style, c, window, cx))
        });

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .id("bp-compact-shelves")
                    .role(Role::TabList)
                    .h(rems(SHELF_CHIPS))
                    .flex_shrink_0()
                    .px(rems(m.gutter))
                    .gap_2()
                    .items_center()
                    .overflow_hidden()
                    .children(chips),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .px(rems(m.gutter))
                    .gap(rems(m.gutter))
                    .items_start()
                    .child(
                        div()
                            .id("bp-compact-list")
                            .role(Role::ListBox)
                            .flex_shrink_0()
                            .w(rems(layout.list_w))
                            .h(rems(layout.list_h))
                            .relative()
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .children(rows)
                                    .with_spring(
                                        "bp-compact-scroll",
                                        SpringAnimation::new(SCROLL_SPRING).to(scroll),
                                        |el, v: f32| el.top(rems(-v)),
                                    ),
                            ),
                    )
                    .children(detail.map(|d| {
                        rise_in(
                            SharedString::from(format!("bp-compact-detail-{key}")),
                            d,
                            12.,
                        )
                    })),
            )
            .into_any_element()
    }
}
