//! The library's view tabs and toolbar: search, filters, sort (with grouping),
//! surprise me, layout switch, and the layout-aware Look popover. With a top
//! row ("Continue playing", favourites), which no filter changes, the tabs and
//! toolbar sit together under that row in the list and stay at the list's top
//! once the row scrolls away.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Pixels, Styled as _, Window, canvas, div, point, px, rems,
};
use irori_ui::i18n::{t, t_args};

use super::{Item, LibraryView, item_rem, layout_button};
use crate::filter::ViewFilter;
use crate::{grid, layouts};

/// Height of the tabs + toolbar block before it was first measured, in rem.
pub(super) const TOOLBAR_REM: f32 = 5.75;

impl LibraryView {
    /// The view tabs (when enabled) stacked above the slimmed toolbar.
    pub(super) fn render_toolbar_block(
        &mut self,
        filter: &ViewFilter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = self.render_view_tabs(filter, cx);
        let toolbar = self.render_toolbar(filter, cx);
        v_flex()
            .w_full()
            .gap_2()
            .children(tabs)
            .child(toolbar)
            .into_any_element()
    }

    /// The toolbar's controls: narrowing controls first (search, filters,
    /// sort/group, surprise me), display controls at the end (layout switch,
    /// Look popover).
    pub(super) fn render_toolbar(
        &mut self,
        filter: &ViewFilter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = filter.active_count();
        let sort = filter.sort;
        let descending = filter.descending;
        let group = filter.group;
        let searching = !filter.query.trim().is_empty();
        let panel = self.filters_panel.clone();
        let layout = self.layout;
        let sort_popover = self.render_sort_popover(sort, descending, group, searching, cx);

        let anchor_bottom = panel.read(cx).anchor_bottom();
        let panel_for_anchor = panel.clone();
        let filters_open = self.filters_open;
        let anchor_measure = canvas(
            move |bounds, _, cx| {
                let bottom = bounds.bottom();
                let moved = anchor_bottom
                    .get()
                    .is_none_or(|prev| (prev - bottom).abs() > px(0.5));
                if moved {
                    anchor_bottom.set(Some(bottom));
                    if filters_open {
                        panel_for_anchor.update(cx, |_, cx| cx.notify());
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let narrowing = h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .flex_wrap()
            .child(
                div()
                    .id("library-search")
                    .flex_1()
                    .min_w(rems(13.))
                    .max_w(rems(26.))
                    .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                        if e.keystroke.key == "escape" {
                            this.escape_search(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Input::new(&self.search)
                            .small()
                            .prefix(Icon::new(IconName::Search).small())
                            .cleanable(true),
                    ),
            )
            .child(
                div()
                    .relative()
                    .debug_selector(|| "library-filters-trigger".into())
                    .child(anchor_measure)
                    .child(
                        Popover::new("library-filters-popover")
                            .open(self.filters_open)
                            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                                if this.filters_open != *open {
                                    this.filters_open = *open;
                                    if !*open {
                                        this.filters_panel
                                            .update(cx, |panel, _| panel.reset_more_open());
                                    }
                                    cx.notify();
                                }
                            }))
                            .trigger(
                                Button::new("library-filters")
                                    .small()
                                    .outline()
                                    .icon(IconName::ListFilter)
                                    .selected(active > 0)
                                    .label(if active > 0 {
                                        t_args(
                                            "library.toolbar.filters_active",
                                            &[("count", &active.to_string())],
                                        )
                                    } else {
                                        t("library.toolbar.filters")
                                    }),
                            )
                            .content(move |_, _, _| panel.clone()),
                    ),
            )
            .child(sort_popover)
            .child(
                Button::new("library-random")
                    .small()
                    .outline()
                    .icon(IconName::Dices)
                    .label(t("library.random.button"))
                    .tooltip(t("library.random.tooltip"))
                    .on_click(|_, window, cx| crate::random::open_random_game(window, cx)),
            );

        let display = h_flex()
            .flex_shrink_0()
            .gap_2()
            .pl_2()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                ButtonGroup::new("library-layout")
                    .small()
                    .outline()
                    .children(layouts::LAYOUTS.into_iter().map(|l| {
                        let (id, icon, label) = layout_button(l);
                        Button::new(id)
                            .icon(icon)
                            .tooltip(label)
                            .selected(layout == l)
                    }))
                    .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                        if let Some(l) = clicked.first().and_then(|ix| layouts::LAYOUTS.get(*ix)) {
                            this.on_show_layout(*l, window, cx);
                        }
                    })),
            )
            .child(self.render_look_popover(cx));

        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(narrowing)
            .child(display)
            .into_any_element()
    }

    /// The tabs + toolbar block over the list, under the top row until that
    /// scrolls away, then held at the list's top. The list keeps an empty item
    /// of the block's measured height where it sits.
    pub(super) fn render_list_toolbar(
        &mut self,
        filter: &ViewFilter,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rem = window.rem_size();
        let offset = -self.scroll.offset().y;
        let top_kind = self.frame.top.as_ref().map(|(kind, _)| *kind);
        let row = rem * item_rem(Item::Top, top_kind, &self.frame.metrics);
        let y = (row - offset).max(px(0.));
        let stuck = offset > row;
        self.toolbar.offset = offset;
        let toolbar = self.render_toolbar_block(filter, cx);
        let view = cx.entity().downgrade();
        // Its height sizes the list's empty item, and a scroll the list
        // applied while laying out (a key moving the selection) moves it.
        let follow = canvas(
            move |bounds, _, cx| {
                view.update(cx, |this, cx| {
                    let height = bounds.size.height;
                    let offset = -this.scroll.offset().y;
                    if (this.toolbar.height - height).abs() > px(0.5)
                        || (this.toolbar.offset - offset).abs() > px(0.5)
                    {
                        this.toolbar.height = height;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let surface = irori_ui::glass::page(cx.theme().background, cx);
        let border = cx.theme().border;
        div()
            .id("library-list-toolbar")
            .absolute()
            .left_0()
            .right_0()
            .top(y)
            .px(rems(grid::EDGE_REM))
            .py_2()
            // Covers scroll under it: a click between its controls is not a
            // click on them, while the wheel still scrolls the list.
            .block_mouse_except_scroll()
            // The border is always there, so holding the toolbar doesn't
            // change its height (and the list's room for it).
            .border_b_1()
            .border_color(gpui_kit::transparent_black())
            .when(stuck, |el| el.bg(surface).border_color(border))
            .child(follow)
            .child(toolbar)
            .into_any_element()
    }

    /// Height of the list's empty item under the toolbar.
    pub(super) fn toolbar_height(&self, rem: Pixels) -> Pixels {
        if self.toolbar.height > px(0.) {
            self.toolbar.height
        } else {
            rem * TOOLBAR_REM
        }
    }

    /// Scroll `item` into view, as little as needed, clear of the toolbar
    /// held over the list's top.
    pub(super) fn reveal_item(&mut self, item: usize) {
        let sizes = self.frame.sizes.clone();
        let item_top = sizes[..item.min(sizes.len())]
            .iter()
            .fold(px(0.), |sum, s| sum + s.height);
        let item_height = sizes.get(item).map_or(px(0.), |s| s.height);
        let inset = if self.frame.toolbar_in_list {
            self.toolbar.height
        } else {
            px(0.)
        };
        let offset = -self.scroll.offset().y;
        let viewport = self.scroll.bounds().size.height;
        let next = nearest_offset(item_top, item_height, offset, viewport, inset);
        if (next - offset).abs() > px(0.5) {
            self.scroll.set_offset(point(px(0.), -next));
        }
    }
}

/// Measured toolbar over the list.
#[derive(Debug, Default)]
pub(super) struct ListToolbar {
    /// Its height, zero until first painted.
    pub height: Pixels,
    /// The list's scroll offset it was placed for.
    pub offset: Pixels,
}

/// The scroll offset that shows an item at `top` of `height`, moving
/// `offset` as little as possible, with the viewport's first `inset` covered.
pub(super) fn nearest_offset(
    top: Pixels,
    height: Pixels,
    offset: Pixels,
    viewport: Pixels,
    inset: Pixels,
) -> Pixels {
    if top < offset + inset {
        (top - inset).max(px(0.))
    } else if top + height > offset + viewport {
        // Taller than the room: its top stays in view.
        (top + height - viewport).min(top - inset).max(px(0.))
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::look::offers_background;
    use irori_core::settings::{LibraryBackground, UiPrefs};

    #[test]
    fn the_look_menu_offers_a_picture_only_once_one_is_chosen() {
        let mut ui = UiPrefs::default();
        assert!(offers_background(&ui, LibraryBackground::None));
        assert!(offers_background(&ui, LibraryBackground::GameArt));
        assert!(!offers_background(&ui, LibraryBackground::Picture));
        ui.library_background_picture = "/pictures/sky.png".into();
        assert!(offers_background(&ui, LibraryBackground::Picture));
        let ui = UiPrefs {
            library_background: LibraryBackground::Picture,
            ..UiPrefs::default()
        };
        assert!(offers_background(&ui, LibraryBackground::Picture));
    }

    #[test]
    fn rows_scroll_clear_of_the_held_toolbar() {
        let (viewport, inset) = (px(500.), px(50.));
        // Already in view below the toolbar: nothing moves.
        assert_eq!(
            nearest_offset(px(300.), px(100.), px(200.), viewport, inset),
            px(200.)
        );
        // Under the toolbar: moved down to just below it.
        assert_eq!(
            nearest_offset(px(220.), px(100.), px(200.), viewport, inset),
            px(170.)
        );
        // Past the foot: its foot meets the viewport's.
        assert_eq!(
            nearest_offset(px(700.), px(100.), px(200.), viewport, inset),
            px(300.)
        );
        // Near the list's top the offset never goes negative.
        assert_eq!(
            nearest_offset(px(10.), px(100.), px(40.), viewport, inset),
            px(0.)
        );
        // Without a held toolbar, a row's top meets the viewport's.
        assert_eq!(
            nearest_offset(px(220.), px(100.), px(300.), viewport, px(0.)),
            px(220.)
        );
    }
}
