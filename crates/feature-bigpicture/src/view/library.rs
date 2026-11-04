//! The Library tab: filter chips (Show, Sort, Status, Search), the focused
//! game's title with its place in the list, and a grid of covers whose
//! focused row leads and glides into place. Only rows near the view are
//! built, so a library of thousands stays cheap.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, SpringAnimation, StatefulInteractiveElement as _,
    Styled as _, Window, div, rems,
};
use irori_ui::i18n::{t, t_args};

use super::home::SCROLL_SPRING;
use super::tile::{Cover, CoverSlot};
use super::{BigPictureView, Colors, LibraryFocus};
use crate::layout::{
    CHIPS, GRID_FOCUS_SCALE, GRID_HEADER, Metrics, TILE_GAP, strip_range, strip_start,
};
use crate::model::Chip;

impl BigPictureView {
    fn chip_value(&self, chip: Chip) -> SharedString {
        let filter = &self.library.filter;
        match chip {
            Chip::Show => filter.show.label(),
            Chip::Sort => filter.sort.label(),
            Chip::Status => filter.status.label(),
            Chip::Search if filter.query.is_empty() => SharedString::new_static("—"),
            Chip::Search => SharedString::from(filter.query.clone()),
        }
    }

    fn render_chip(&self, ix: usize, chip: Chip, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.library.focus == LibraryFocus::Chips(ix)
            && self.overlay.is_none()
            && !self.library.searching;
        let value = self.chip_value(chip);
        h_flex()
            .id(chip.id())
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{}: {value}", chip.label())))
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .flex_shrink_0()
            .max_w(rems(22.))
            .min_w_0()
            .h(rems(2.75))
            .px_4()
            .gap_3()
            .items_center()
            .rounded(c.radius_lg)
            .cursor_pointer()
            .map(|d| {
                if focused {
                    d.bg(c.foreground).text_color(c.background)
                } else {
                    d.bg(c.foreground.opacity(0.1)).text_color(c.foreground)
                }
            })
            .when(chip == Chip::Search, |d| {
                d.child(Icon::new(IconName::Search).small())
            })
            .child(div().flex_shrink_0().opacity(0.75).child(chip.label()))
            .child(div().min_w_0().truncate().font_bold().child(value))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.activate_chip(chip, window, cx);
                if chip != Chip::Search {
                    this.focus.focus(window, cx);
                }
            }))
            .into_any_element()
    }

    /// The search field, drawn over the Search chip's row outside the
    /// `BigPicture` key context, so letters bound there (M for the menu,
    /// Space for play) reach the field.
    pub(super) fn render_search(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("bp-search")
            .key_context(crate::SEARCH_CONTEXT)
            .on_action(
                cx.listener(|this, _: &crate::CloseSearch, window, cx| {
                    this.close_search(window, cx)
                }),
            )
            .absolute()
            .left_0()
            .right_0()
            .top(rems(m.top_bar))
            .h(rems(CHIPS))
            .px(rems(m.gutter))
            .items_center()
            .bg(c.background.opacity(0.9))
            .child(
                div()
                    .w_full()
                    .max_w(rems(40.))
                    .text_xl()
                    .child(Input::new(&self.library.search).large().cleanable(true)),
            )
    }

    pub(super) fn render_library(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chips: Vec<AnyElement> = Chip::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, chip)| self.render_chip(ix, chip, c, cx))
            .collect();
        let len = self.library.games.len();
        let ix = self.library.ix.min(len.saturating_sub(1));
        let in_grid = self.library.focus == LibraryFocus::Grid;
        let title = self.library.games.get(ix).map(|g| g.title.clone());

        let cols = m.grid_cols;
        let rows = len.div_ceil(cols);
        let shown = m.grid_rows_shown();
        let start = strip_start(ix / cols, rows, shown);
        let range = strip_range(start, rows, shown + 1, 1);
        let first = range.start;
        let row_h = m.grid_row();
        let mut grid_rows = Vec::with_capacity(range.len());
        for row in range {
            let mut tiles = Vec::with_capacity(cols);
            for i in row * cols..((row + 1) * cols).min(len) {
                let game = self.library.games[i].clone();
                let key = game.id().key();
                let cover = Cover {
                    game,
                    id: SharedString::from(format!("bp-grid-{key}")),
                    width: m.grid_tile,
                    scale: GRID_FOCUS_SCALE,
                    shape: self.look.theme().covers.shape,
                    gap: TILE_GAP,
                    focused: in_grid && i == ix && self.overlay.is_none(),
                    slot: CoverSlot::Grid(i),
                };
                tiles.push(self.render_cover(cover, c, window, cx));
            }
            grid_rows.push(h_flex().h(rems(row_h)).items_center().children(tiles));
        }

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .id("bp-chips")
                    .role(Role::Toolbar)
                    .h(rems(CHIPS))
                    .flex_shrink_0()
                    .px(rems(m.gutter))
                    .gap_3()
                    .items_center()
                    .overflow_hidden()
                    .children(chips),
            )
            .child(
                h_flex()
                    .h(rems(GRID_HEADER))
                    .flex_shrink_0()
                    .px(rems(m.gutter))
                    .gap_4()
                    .items_center()
                    .min_w_0()
                    .when_some(title, |d, title| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(rems(if m.compact { 1.5 } else { 2.0 }))
                                .font_extrabold()
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_lg()
                                .text_color(c.foreground.opacity(0.65))
                                .child(t_args(
                                    "bigpicture.library.position",
                                    &[
                                        ("position", (ix + 1).to_string().as_str()),
                                        ("total", len.to_string().as_str()),
                                    ],
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .id("bp-grid")
                    .role(Role::Grid)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .map(|d| {
                        if len == 0 {
                            d.child(
                                div()
                                    .px(rems(m.gutter))
                                    .pt_6()
                                    .text_xl()
                                    .text_color(c.foreground.opacity(0.7))
                                    .child(t("bigpicture.library.empty")),
                            )
                        } else {
                            d.child(
                                v_flex()
                                    .absolute()
                                    .left(rems(m.gutter - (m.grid_slot() - m.grid_tile) / 2.0))
                                    .right_0()
                                    .children(grid_rows)
                                    .with_spring(
                                        "bp-grid-scroll",
                                        SpringAnimation::new(SCROLL_SPRING).to(start as f32),
                                        move |el, v: f32| el.top(rems((first as f32 - v) * row_h)),
                                    ),
                            )
                        }
                    }),
            )
            .into_any_element()
    }
}
