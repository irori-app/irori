//! The library's unified tab strip: built-in presets (`All`, `Installed`,
//! `Favorites`, `Recently added`), dynamic tag collections (smart categories),
//! a 1-click `+ New collection` button, and the right-aligned `Edit tabs`
//! popover.
//!
//! Each tab automatically remembers its own layout, cover shape, size, sort,
//! and grouping in [`crate::views::TabViewPrefs`]. Turning
//! `iroriUi.libraryQuickControls` off hides the tab strip.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, rems, transparent_black,
};
use irori_ui::i18n::t;
use irori_ui::models;
use irori_ui::style::KeyboardFocusable as _;

use super::LibraryView;
use crate::chips::{self, PresetChip};
use crate::dialogs::collection::{
    delete_collection_with_undo, is_simple_tag_collection, open_collection_dialog,
};
use crate::filter::ViewFilter;
use crate::views::{self, TabKey};

/// Context and "⋯" menu for a dynamic collection tab.
fn collection_tab_menu(
    mut menu: PopupMenu,
    id: String,
    is_simple: bool,
    smart_ids: Vec<String>,
) -> PopupMenu {
    let tab_key = TabKey::Collection(id.clone());
    let edit_id = id.clone();
    let rules_id = id.clone();
    let del_id = id;
    let left_key = tab_key.clone();
    let left_smart = smart_ids.clone();
    let right_key = tab_key.clone();
    let right_smart = smart_ids;
    let hide_key = tab_key;

    if is_simple {
        menu = menu.item(
            PopupMenuItem::new(t("library.collection.edit"))
                .icon(Icon::new(IconName::Pencil))
                .on_click(move |_, window, cx| {
                    open_collection_dialog(Some(edit_id.clone()), window, cx)
                }),
        );
    }
    menu.item(
        PopupMenuItem::new(t("library.categories.edit_rules"))
            .icon(Icon::new(IconName::Settings2))
            .on_click(move |_, window, cx| {
                crate::dialogs::smart::open_smart_category_editor(
                    Some(rules_id.clone()),
                    None,
                    window,
                    cx,
                )
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new(t("library.views.move_left"))
            .icon(Icon::new(IconName::ChevronLeft))
            .on_click(move |_, _, cx| {
                views::saved_views(cx).update(cx, |m, cx| {
                    m.update(|v| v.shift_visible_tab(&left_key, -1, &left_smart), cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new(t("library.views.move_right"))
            .icon(Icon::new(IconName::ChevronRight))
            .on_click(move |_, _, cx| {
                views::saved_views(cx).update(cx, |m, cx| {
                    m.update(|v| v.shift_visible_tab(&right_key, 1, &right_smart), cx)
                });
            }),
    )
    .item(
        PopupMenuItem::new(t("library.tabs.hide"))
            .icon(Icon::new(IconName::EyeOff))
            .on_click(move |_, _, cx| {
                views::saved_views(cx).update(cx, |m, cx| {
                    m.update(|v| v.set_tab_hidden(&hide_key, true), cx)
                });
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new(t("library.views.delete"))
            .icon(Icon::new(IconName::Trash))
            .on_click(move |_, window, cx| delete_collection_with_undo(del_id.clone(), window, cx)),
    )
}

impl LibraryView {
    /// Apply a dynamic collection tab, keeping the current search text and
    /// restoring that collection's saved visualization settings.
    pub(crate) fn apply_collection_tab(&mut self, id: String, cx: &mut Context<Self>) {
        self.apply_tab(TabKey::Collection(id), None, cx);
    }

    /// The horizontal strip of underlined tabs (presets, dynamic collections,
    /// `+ New collection` button, and right-aligned `Edit tabs` button).
    /// Returns `None` while `libraryQuickControls` is off.
    pub(super) fn render_view_tabs(
        &mut self,
        filter: &ViewFilter,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.ui_prefs.library_quick_controls {
            return None;
        }
        let saved = views::saved_views(cx).read(cx).views().clone();
        let user = models(cx).library.read(cx).user.clone();
        let smart_ids = views::smart_collection_ids(&user);
        let visible = saved.visible_tabs(&smart_ids);
        let active = chips::active_tab_for(filter, self.active_tab.as_ref(), &smart_ids);
        let counts = self.frame.chip_counts.clone();
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let primary = theme.primary;
        let border = theme.border;

        let tabs: Vec<AnyElement> =
            visible
                .into_iter()
                .filter_map(|tab_key| match tab_key {
                    TabKey::All | TabKey::Installed | TabKey::Favorites | TabKey::RecentlyAdded => {
                        let preset = match tab_key {
                            TabKey::All => PresetChip::All,
                            TabKey::Installed => PresetChip::Installed,
                            TabKey::Favorites => PresetChip::Favourites,
                            TabKey::RecentlyAdded => PresetChip::RecentlyAdded,
                            _ => return None,
                        };
                        let selected = active.as_ref() == Some(&tab_key);
                        let count = counts.preset(preset);
                        let hideable = !tab_key.is_locked();
                        let click_key = tab_key.clone();
                        let hide_key = tab_key;
                        let tab = h_flex()
                            .id(preset.id())
                            .debug_selector(move || preset.id().to_string())
                            .keyboard_focusable(cx)
                            .flex_shrink_0()
                            .h(rems(2.35))
                            .px_0p5()
                            .gap_1p5()
                            .items_center()
                            .cursor_pointer()
                            .text_sm()
                            .border_b_2()
                            .border_color(if selected {
                                primary
                            } else {
                                transparent_black()
                            })
                            .text_color(if selected { fg } else { muted })
                            .when(selected, |el| el.font_semibold())
                            .when(!selected, |el| {
                                el.font_medium().hover(move |s| s.text_color(fg))
                            })
                            .child(Icon::new(preset.icon()).xsmall())
                            .child(preset.label())
                            .when_some(count, |el, n| {
                                el.child(div().text_xs().text_color(muted).child(n.to_string()))
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_tab(click_key.clone(), Some(window), cx)
                            }));
                        Some(if hideable {
                            tab.context_menu(move |menu, _, _| {
                                let key = hide_key.clone();
                                menu.item(
                                    PopupMenuItem::new(t("library.tabs.hide"))
                                        .icon(Icon::new(IconName::EyeOff))
                                        .on_click(move |_, _, cx| {
                                            views::saved_views(cx).update(cx, |m, cx| {
                                                m.update(|v| v.set_tab_hidden(&key, true), cx)
                                            });
                                        }),
                                )
                            })
                            .into_any_element()
                        } else {
                            tab.into_any_element()
                        })
                    }
                    TabKey::Collection(id) => {
                        let cat = user.smart_categories.get(&id)?;
                        let is_simple = is_simple_tag_collection(cat);
                        let label: SharedString = cat.name.clone().into();
                        let tint = irori_ui::categories::colour(cat.look.colour.as_deref(), cx);
                        let icon = irori_ui::categories::icon_for(
                            &irori_services::CategoryRef::Smart(id.clone()),
                            cat.look.icon.as_deref(),
                        );
                        let selected = active == Some(TabKey::Collection(id.clone()));
                        let count = counts.collection(&id);
                        let group = SharedString::from(format!("collection-tab-group:{id}"));
                        let click_key = TabKey::Collection(id.clone());
                        let menu_id = id.clone();
                        let ctx_id = id.clone();
                        let menu_smart = smart_ids.clone();
                        let ctx_smart = smart_ids.clone();
                        Some(
                            h_flex()
                                .id(SharedString::from(format!("collection-tab:{id}")))
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("collection-tab:{id}")
                                })
                                .group(group.clone())
                                .relative()
                                .keyboard_focusable(cx)
                                .flex_shrink_0()
                                .h(rems(2.35))
                                .px_0p5()
                                .gap_1p5()
                                .items_center()
                                .cursor_pointer()
                                .text_sm()
                                .border_b_2()
                                .border_color(if selected {
                                    tint.unwrap_or(primary)
                                } else {
                                    transparent_black()
                                })
                                .text_color(if selected { fg } else { muted })
                                .when(selected, |el| el.font_semibold())
                                .when(!selected, |el| {
                                    el.font_medium().hover(move |s| s.text_color(fg))
                                })
                                .child(
                                    Icon::new(icon)
                                        .xsmall()
                                        .when_some(tint, |el, colour| el.text_color(colour)),
                                )
                                .child(label)
                                .child(div().text_xs().text_color(muted).child(count.to_string()))
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "collection-tab-more-wrap:{id}"
                                        )))
                                        .flex_shrink_0()
                                        .on_click(|_, _, cx| cx.stop_propagation())
                                        .when(!selected, |el| {
                                            el.opacity(0.).group_hover(group, |s| s.opacity(1.))
                                        })
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "collection-tab-more:{id}"
                                            )))
                                            .xsmall()
                                            .ghost()
                                            .icon(IconName::Ellipsis)
                                            .tooltip(t("library.views.tab_menu"))
                                            .dropdown_menu(move |menu, _, _| {
                                                collection_tab_menu(
                                                    menu,
                                                    menu_id.clone(),
                                                    is_simple,
                                                    menu_smart.clone(),
                                                )
                                            }),
                                        ),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_tab(click_key.clone(), Some(window), cx)
                                }))
                                .context_menu(move |menu, _, _| {
                                    collection_tab_menu(
                                        menu,
                                        ctx_id.clone(),
                                        is_simple,
                                        ctx_smart.clone(),
                                    )
                                })
                                .into_any_element(),
                        )
                    }
                    TabKey::View(_) => None,
                })
                .collect();

        let add_button = Button::new("library-new-collection-tab")
            .debug_selector(|| "library-new-collection-tab".into())
            .xsmall()
            .ghost()
            .icon(IconName::Plus)
            .label(t("library.collection.new_button"))
            .text_color(muted)
            .on_click(|_, window, cx| open_collection_dialog(None, window, cx));

        let edit_tabs = self.render_tabs_manager_popover(cx);
        let divider = div()
            .w(px(1.))
            .h(rems(1.))
            .bg(border)
            .flex_shrink_0()
            .into_any_element();

        Some(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .id("library-view-tabs")
                        .flex_1()
                        .min_w_0()
                        .overflow_x_scroll()
                        .items_center()
                        .gap_4()
                        .children(tabs)
                        .child(divider)
                        .child(div().flex_shrink_0().pb_0p5().child(add_button)),
                )
                .child(div().relative().flex_shrink_0().pb_0p5().child(edit_tabs))
                .into_any_element(),
        )
    }
}
