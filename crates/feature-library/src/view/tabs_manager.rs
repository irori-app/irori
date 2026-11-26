//! The "Edit tabs" popover in the library's top tab bar.
//!
//! Lists every tab (built-in presets and dynamic collections) in their unified
//! order with GPUI native drag-and-drop reordering, accessible ↑/↓ controls,
//! visibility toggles (`All` and `Installed` stay locked), and quick
//! edit/delete actions for collections.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, rems,
};
use irori_ui::i18n::t;
use irori_ui::models;

use super::LibraryView;
use super::look::separator;
use crate::chips::PresetChip;
use crate::dialogs::collection::{delete_collection_with_undo, open_collection_dialog};
use crate::views::{self, TabKey};

/// Floating drag preview while reordering tabs inside the "Edit tabs" popover.
#[derive(Clone)]
struct DraggedTab {
    key: TabKey,
    label: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_2()
            .px_3()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .text_sm()
            .child(Icon::new(IconName::GripVertical).small())
            .child(self.label.clone())
    }
}

fn type_pill(label: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(theme.secondary.opacity(0.65))
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(label)
        .into_any_element()
}

fn tab_row_key(key: &TabKey) -> String {
    match key {
        TabKey::All => "all".into(),
        TabKey::Installed => "installed".into(),
        TabKey::Favorites => "favorites".into(),
        TabKey::RecentlyAdded => "recently-added".into(),
        TabKey::View(id) => format!("view-{id}"),
        TabKey::Collection(id) => format!("col-{id}"),
    }
}

impl LibraryView {
    /// The "Edit tabs" button and popover aligned at the right end of the tab strip.
    pub(super) fn render_tabs_manager_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let view: WeakEntity<LibraryView> = cx.entity().downgrade();
        let counts = self.frame.chip_counts.clone();
        let open = self.tabs_manager_open;

        Popover::new("library-edit-tabs-popover")
            .anchor(Anchor::TopRight)
            .open(open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                if this.tabs_manager_open != *open {
                    this.tabs_manager_open = *open;
                    cx.notify();
                }
            }))
            .trigger(
                Button::new("library-edit-tabs")
                    .debug_selector(|| "library-edit-tabs-trigger".into())
                    .xsmall()
                    .ghost()
                    .icon(IconName::SlidersHorizontal)
                    .label(t("library.tabs.edit"))
                    .tooltip(t("library.tabs.edit_tooltip")),
            )
            .content(move |_, window, cx| {
                let saved_model = views::saved_views(cx);
                let saved = saved_model.read(cx).views().clone();
                let user = models(cx).library.read(cx).user.clone();
                let smart_ids = views::smart_collection_ids(&user);
                let resolved = saved.resolved_tabs(&smart_ids);
                let total = resolved.len();

                let theme = cx.theme();
                let fg = theme.foreground;
                let muted = theme.muted_foreground;
                let border = theme.border;
                let hover_bg = theme.secondary.opacity(0.55);
                let radius = theme.radius;
                let visible = irori_ui::style::visible_bounds(window);
                let rem = window.rem_size();
                let max_h = (visible.size.height - rem * 14.).max(rem * 10.);

                let rows: Vec<AnyElement> = resolved
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, key)| {
                        let hidden = saved.is_tab_hidden(key);
                        let row_slug = tab_row_key(key);

                        let (icon, icon_color, label, kind_label, count): (
                            IconName,
                            Option<gpui_kit::Hsla>,
                            SharedString,
                            SharedString,
                            Option<usize>,
                        ) = match key {
                            TabKey::All => (
                                PresetChip::All.icon(),
                                None,
                                PresetChip::All.label(),
                                t("library.tabs.kind.preset"),
                                counts.preset(PresetChip::All),
                            ),
                            TabKey::Installed => (
                                PresetChip::Installed.icon(),
                                None,
                                PresetChip::Installed.label(),
                                t("library.tabs.kind.preset"),
                                counts.preset(PresetChip::Installed),
                            ),
                            TabKey::Favorites => (
                                PresetChip::Favourites.icon(),
                                None,
                                PresetChip::Favourites.label(),
                                t("library.tabs.kind.preset"),
                                counts.preset(PresetChip::Favourites),
                            ),
                            TabKey::RecentlyAdded => (
                                PresetChip::RecentlyAdded.icon(),
                                None,
                                PresetChip::RecentlyAdded.label(),
                                t("library.tabs.kind.preset"),
                                counts.preset(PresetChip::RecentlyAdded),
                            ),
                            TabKey::Collection(id) => {
                                let cat = user.smart_categories.get(id)?;
                                let tint =
                                    irori_ui::categories::colour(cat.look.colour.as_deref(), cx);
                                let icon = irori_ui::categories::icon_for(
                                    &irori_services::CategoryRef::Smart(id.clone()),
                                    cat.look.icon.as_deref(),
                                );
                                (
                                    icon,
                                    tint,
                                    cat.name.clone().into(),
                                    t("library.tabs.kind.collection"),
                                    Some(counts.collection(id)),
                                )
                            }
                            TabKey::View(_) => return None,
                        };

                        let dragged = DraggedTab {
                            key: key.clone(),
                            label: label.clone(),
                        };
                        let drop_target = key.clone();
                        let drop_smart = smart_ids.clone();
                        let up_key = key.clone();
                        let up_smart = smart_ids.clone();
                        let down_key = key.clone();
                        let down_smart = smart_ids.clone();

                        let mut actions: Vec<AnyElement> = Vec::new();
                        actions.push(
                            Button::new(SharedString::from(format!("tab-up-{row_slug}")))
                                .xsmall()
                                .ghost()
                                .icon(IconName::ChevronUp)
                                .disabled(ix == 0)
                                .tooltip(t("library.tabs.move_up"))
                                .on_click(move |_, _, cx| {
                                    let smart = up_smart.clone();
                                    let k = up_key.clone();
                                    views::saved_views(cx).update(cx, |m, cx| {
                                        m.update(|v| v.shift_tab(&k, -1, &smart), cx)
                                    });
                                })
                                .into_any_element(),
                        );
                        actions.push(
                            Button::new(SharedString::from(format!("tab-down-{row_slug}")))
                                .xsmall()
                                .ghost()
                                .icon(IconName::ChevronDown)
                                .disabled(ix + 1 >= total)
                                .tooltip(t("library.tabs.move_down"))
                                .on_click(move |_, _, cx| {
                                    let smart = down_smart.clone();
                                    let k = down_key.clone();
                                    views::saved_views(cx).update(cx, |m, cx| {
                                        m.update(|v| v.shift_tab(&k, 1, &smart), cx)
                                    });
                                })
                                .into_any_element(),
                        );

                        if key.is_locked() {
                            actions.push(
                                div()
                                    .id(SharedString::from(format!("tab-locked-{row_slug}")))
                                    .debug_selector({
                                        let slug = row_slug.clone();
                                        move || format!("tab-locked-{slug}")
                                    })
                                    .px_1()
                                    .flex_none()
                                    .child(Icon::new(IconName::Lock).xsmall().text_color(muted))
                                    .into_any_element(),
                            );
                        } else {
                            let vis_key = key.clone();
                            actions.push(
                                Button::new(SharedString::from(format!("tab-vis-{row_slug}")))
                                    .debug_selector({
                                        let slug = row_slug.clone();
                                        move || format!("tab-vis-{slug}")
                                    })
                                    .xsmall()
                                    .ghost()
                                    .icon(if hidden {
                                        IconName::EyeOff
                                    } else {
                                        IconName::Eye
                                    })
                                    .tooltip(if hidden {
                                        t("library.tabs.show")
                                    } else {
                                        t("library.tabs.hide")
                                    })
                                    .on_click(move |_, _, cx| {
                                        let k = vis_key.clone();
                                        views::saved_views(cx).update(cx, |m, cx| {
                                            m.update(|v| v.set_tab_hidden(&k, !hidden), cx)
                                        });
                                    })
                                    .into_any_element(),
                            );
                        }

                        if let TabKey::Collection(id) = key {
                            let edit_id = id.clone();
                            let del_id = id.clone();
                            let edit_view = view.clone();
                            actions.push(
                                Button::new(SharedString::from(format!("tab-edit-{row_slug}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Pencil)
                                    .tooltip(t("library.collection.edit"))
                                    .on_click(move |_, window, cx| {
                                        edit_view
                                            .update(cx, |this, cx| {
                                                this.tabs_manager_open = false;
                                                cx.notify();
                                            })
                                            .ok();
                                        open_collection_dialog(Some(edit_id.clone()), window, cx);
                                    })
                                    .into_any_element(),
                            );
                            actions.push(
                                Button::new(SharedString::from(format!("tab-del-{row_slug}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Trash)
                                    .tooltip(t("library.views.delete"))
                                    .on_click(move |_, window, cx| {
                                        delete_collection_with_undo(del_id.clone(), window, cx);
                                    })
                                    .into_any_element(),
                            );
                        }

                        Some(
                            h_flex()
                                .id(SharedString::from(format!("tab-mgr-row-{row_slug}")))
                                .debug_selector(move || format!("tab-mgr-row-{row_slug}"))
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .px_1p5()
                                .py_1()
                                .rounded(radius)
                                .when(ix > 0, |el| {
                                    el.border_t_1().border_color(border.opacity(0.4))
                                })
                                .hover(move |s| s.bg(hover_bg))
                                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                                .drag_over::<DraggedTab>(|style, _, _, cx| {
                                    style.bg(cx.theme().drop_target)
                                })
                                .on_drop(move |d: &DraggedTab, _, cx| {
                                    let from = d.key.clone();
                                    let to = drop_target.clone();
                                    let smart = drop_smart.clone();
                                    views::saved_views(cx).update(cx, |m, cx| {
                                        m.update(|v| v.move_tab(&from, &to, &smart), cx)
                                    });
                                })
                                .child(
                                    Icon::new(IconName::GripVertical)
                                        .xsmall()
                                        .flex_shrink_0()
                                        .text_color(muted),
                                )
                                .child(Icon::new(icon).xsmall().flex_shrink_0().text_color(
                                    if hidden {
                                        muted
                                    } else {
                                        icon_color.unwrap_or(fg)
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .font_medium()
                                        .truncate()
                                        .text_color(if hidden { muted } else { fg })
                                        .when(hidden, |el| el.line_through())
                                        .child(label),
                                )
                                .child(type_pill(kind_label, cx))
                                .when_some(count, |el, n| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(n.to_string()),
                                    )
                                })
                                .child(
                                    h_flex()
                                        .flex_none()
                                        .items_center()
                                        .gap_0p5()
                                        .children(actions),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect();

                let new_col_view = view.clone();
                v_flex()
                    .id("library-edit-tabs-panel")
                    .debug_selector(|| "library-edit-tabs-panel".into())
                    .w(rems(23.))
                    .gap_2p5()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().text_sm().font_bold().child(t("library.tabs.edit")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("library.tabs.edit_hint")),
                            ),
                    )
                    .child(
                        v_flex().w_full().max_h(max_h).child(
                            v_flex().flex_1().overflow_hidden().child(
                                div().flex_1().overflow_hidden().child(
                                    v_flex()
                                        .id("library-edit-tabs-list")
                                        .size_full()
                                        .overflow_y_scrollbar()
                                        .pr_1()
                                        .children(rows),
                                ),
                            ),
                        ),
                    )
                    .child(separator(cx))
                    .child(
                        h_flex().w_full().items_center().justify_end().child(
                            Button::new("edit-tabs-new-collection")
                                .xsmall()
                                .outline()
                                .icon(IconName::Sparkles)
                                .label(t("library.collection.new_button"))
                                .on_click(move |_, window, cx| {
                                    new_col_view
                                        .update(cx, |this, cx| {
                                            this.tabs_manager_open = false;
                                            cx.notify();
                                        })
                                        .ok();
                                    open_collection_dialog(None, window, cx);
                                }),
                        ),
                    )
            })
            .into_any_element()
    }
}
