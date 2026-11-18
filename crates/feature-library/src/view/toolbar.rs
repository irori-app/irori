//! The library's toolbar: what narrows the shown games (search, store,
//! account, filters, views, sort, grouping) apart from how they are shown
//! (cover size, view, selection). With a top row ("Continue playing",
//! favourites), which no filter changes, the toolbar sits under that row in
//! the list and stays at the list's top once the row scrolls away.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Pixels, SharedString, Styled as _, Window, canvas, div, point, px, rems,
};
use irori_core::Runner;
use irori_core::settings::LibraryLayout;
use irori_services::StoreFilter;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::{Item, LibraryView, item_rem, layout_button, quick};
use crate::filter::{Grouping, ViewFilter};
use crate::{grid, layouts};

/// Height of the toolbar before it was first measured, in rem: one row of
/// small controls and its padding.
pub(super) const TOOLBAR_REM: f32 = 3.0;

impl LibraryView {
    /// The toolbar's controls, the narrowing ones first and the display ones
    /// apart at the end, each group wrapping on its own.
    pub(super) fn render_toolbar(
        &mut self,
        filter: &ViewFilter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = filter.active_count();
        let model = self.filters.clone();
        let sort = filter.sort;
        let descending = filter.descending;
        let searching = !filter.query.trim().is_empty();
        let panel = self.filters_panel.clone();
        let layout = self.layout;
        let group = filter.group;

        let store_menu = self.render_store_menu(filter, cx);
        let account_switcher = self.render_account_switcher(filter, cx);

        let order = crate::i18n::sort_label(sort);
        let sort_label = if searching {
            t("library.toolbar.sort_best_match")
        } else if descending {
            t_args("library.toolbar.sort_reversed", &[("order", &order)])
        } else {
            t_args("library.toolbar.sort", &[("order", &order)])
        };

        let narrowing = h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .flex_wrap()
            .child(
                div()
                    .id("library-search")
                    .w(rems(18.))
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
            .children(store_menu)
            .children(account_switcher)
            .child(
                Popover::new("library-filters-popover")
                    .trigger(
                        Button::new("library-filters")
                            .small()
                            .outline()
                            .icon(IconName::ListFilter)
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
            )
            .child(crate::dialogs::views::views_button(filter, cx))
            .child(
                Button::new("library-sort")
                    .small()
                    .outline()
                    .icon(IconName::ArrowDownUp)
                    .label(sort_label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        quick::sort_menu(menu, &model, sort, descending)
                    }),
            )
            .child(
                Button::new("library-group")
                    .small()
                    .outline()
                    .icon(IconName::Rows3)
                    .selected(group != Grouping::None)
                    .tooltip(t("library.toolbar.group_by"))
                    .dropdown_menu({
                        let model = self.filters.clone();
                        move |menu, _, _| {
                            let options = [
                                (Grouping::None, "library.toolbar.group.none"),
                                (Grouping::Status, "library.toolbar.group.status"),
                                (Grouping::Mode, "library.toolbar.group.mode"),
                                (Grouping::Genre, "library.toolbar.group.genre"),
                                (Grouping::Category, "library.toolbar.group.category"),
                            ];
                            options.into_iter().fold(menu, |menu, (option, key)| {
                                let model = model.clone();
                                menu.item(
                                    PopupMenuItem::new(t(key))
                                        .checked(option == group)
                                        .on_click(move |_, _, cx| {
                                            model.update(cx, |m, cx| {
                                                m.update(|f| f.group = option, cx)
                                            })
                                        }),
                                )
                            })
                        }
                    }),
            )
            .child(
                Button::new("library-random")
                    .small()
                    .outline()
                    .icon(IconName::Dices)
                    .tooltip(t("library.random.tooltip"))
                    .on_click(|_, window, cx| crate::random::open_random_game(window, cx)),
            );

        let display = h_flex()
            .flex_shrink_0()
            .gap_2()
            .pl_2()
            .border_l_1()
            .border_color(cx.theme().border)
            .when(layout == LibraryLayout::Grid, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Image)
                                .small()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            Slider::new(&self.tile_slider)
                                .accessibility_label(t("library.toolbar.tile_size"))
                                .w(rems(7.)),
                        ),
                )
            })
            .child(
                ButtonGroup::new("library-layout")
                    .small()
                    .outline()
                    .children(layouts::LAYOUTS.into_iter().map(|l| {
                        let (id, icon, label) = layout_button(l);
                        Button::new(id)
                            .icon(icon)
                            .label(label.clone())
                            .tooltip(label)
                            .selected(layout == l)
                    }))
                    .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                        if let Some(l) = clicked.first().and_then(|ix| layouts::LAYOUTS.get(*ix)) {
                            this.on_show_layout(*l, window, cx);
                        }
                    })),
            )
            .child(
                Button::new("library-select")
                    .small()
                    .outline()
                    .icon(IconName::ListChecks)
                    .label(if self.selection.is_active() {
                        t("library.bulk.done")
                    } else {
                        t("library.toolbar.select")
                    })
                    .selected(self.selection.is_active())
                    .tooltip(t("library.toolbar.select_tooltip"))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_selection_mode(cx))),
            );

        h_flex()
            .w_full()
            .gap_2()
            .items_start()
            .flex_wrap()
            .child(narrowing)
            .child(display)
            .into_any_element()
    }

    /// The store whose games are shown, as one menu button: a row of every
    /// store's button took the toolbar's room. Hidden with a single store.
    fn render_store_menu(&self, filter: &ViewFilter, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stores = self.frame.stores.clone();
        if stores.len() < 2 {
            return None;
        }
        let current = match filter.store {
            StoreFilter::Store(r) if stores.contains(&r) => Some(r),
            _ => None,
        };
        let label: SharedString = match current {
            Some(r) => r.store_name().into(),
            None => t("library.toolbar.all_stores"),
        };
        let view = cx.entity().downgrade();
        Some(
            Button::new("library-store")
                .small()
                .outline()
                .icon(IconName::Store)
                .label(label)
                .selected(current.is_some())
                .tooltip(t("library.list.store"))
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let options = std::iter::once(None).chain(stores.iter().copied().map(Some));
                    options.fold(menu, |menu, option: Option<Runner>| {
                        let view = view.clone();
                        let label: SharedString = match option {
                            Some(r) => r.store_name().into(),
                            None => t("library.toolbar.all_stores"),
                        };
                        menu.item(
                            PopupMenuItem::new(label)
                                .checked(option == current)
                                .on_click(move |_, _, cx| {
                                    let store = match option {
                                        Some(r) => StoreFilter::Store(r),
                                        None => StoreFilter::All,
                                    };
                                    view.update(cx, |this, cx| {
                                        this.update_filter(
                                            |f| {
                                                if f.store != store {
                                                    f.account = None;
                                                }
                                                f.store = store;
                                            },
                                            cx,
                                        )
                                    })
                                    .ok();
                                }),
                        )
                    })
                })
                .into_any_element(),
        )
    }

    /// The shown store's accounts, when it has several.
    fn render_account_switcher(
        &self,
        filter: &ViewFilter,
        cx: &mut Context<Self>,
    ) -> Option<ButtonGroup> {
        let runner = filter.shown_store(&self.frame.stores)?;
        let accounts: Vec<(String, String)> = models(cx)
            .accounts
            .read(cx)
            .accounts(runner)
            .iter()
            .filter(|a| a.signed_in)
            .map(|a| (a.account_id.clone(), a.display_name.clone()))
            .collect();
        if accounts.len() < 2 {
            return None;
        }
        let current = filter
            .account
            .as_ref()
            .filter(|a| a.runner == runner)
            .map(|a| a.account_id.clone());
        let options: Vec<Option<String>> = std::iter::once(None)
            .chain(accounts.iter().map(|(id, _)| Some(id.clone())))
            .collect();
        let clicks = options.clone();
        Some(
            ButtonGroup::new("library-account")
                .small()
                .outline()
                .children(options.iter().enumerate().map(|(ix, option)| {
                    let label: SharedString = match option {
                        Some(id) => accounts
                            .iter()
                            .find(|(a, _)| a == id)
                            .map(|(_, name)| name.clone())
                            .unwrap_or_default()
                            .into(),
                        None => t("library.toolbar.all_accounts"),
                    };
                    Button::new(SharedString::from(format!("account-{ix}")))
                        .label(label)
                        .selected(*option == current)
                }))
                .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                    if let Some(&ix) = clicked.first() {
                        let account = clicks[ix]
                            .clone()
                            .map(|account_id| crate::filter::AccountFilter { runner, account_id });
                        this.update_filter(|f| f.account = account, cx);
                    }
                })),
        )
    }

    /// The toolbar over the list, under the top row until that scrolls
    /// away, then held at the list's top. The list keeps an empty item of
    /// the toolbar's measured height where it sits.
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
        let toolbar = self.render_toolbar(filter, cx);
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
