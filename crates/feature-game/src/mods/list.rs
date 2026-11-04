//! The load order: one row per mod, dragged (or moved with the arrows) into
//! place, with its switch and what it wins or loses against other mods.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Window, div, img,
    px, rems, size,
};
use irori_services::ModsView;
use irori_ui::i18n::{t, t_args};

use super::{ModsPanel, source_label};

/// Height of one row, in rems.
const ROW_REMS: f32 = 3.5;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: usize = 8;

/// A mod being dragged to a new place.
#[derive(Clone)]
pub(crate) struct DraggedMod {
    id: String,
    name: SharedString,
}

impl Render for DraggedMod {
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
            .child(self.name.clone())
    }
}

impl ModsPanel {
    pub(super) fn render_list(
        &mut self,
        view: &Arc<ModsView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = view.list.mods.len();
        let row_h = window.rem_size() * ROW_REMS;
        if self.sized_for != (count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (count, row_h);
        }
        let rows = v_virtual_list(
            cx.entity(),
            "mods-rows",
            self.sizes.clone(),
            |this, range, _, cx| this.render_rows(range, cx),
        )
        .size_full();
        let theme = cx.theme();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().min_w_0().child(t("game.mods.load_order")))
                    .child(div().flex_shrink_0().child(t("game.mods.later_wins"))),
            )
            .child(
                div()
                    .id("mods-list")
                    .w_full()
                    .h(rems(ROW_REMS * count.min(VISIBLE_ROWS) as f32))
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(rows),
            )
            .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(view) = self.view().cloned() else {
            return Vec::new();
        };
        let locked = self.running(cx) || self.is_busy();
        let count = view.list.mods.len();
        let selected = self.selected.clone();
        range
            .filter_map(|ix| {
                let m = view.list.mods.get(ix)?.clone();
                let theme = cx.theme();
                let is_selected = selected.as_deref() == Some(m.id.as_str());
                let conflicts = view.conflicts.of(&m.id);
                let tag: Option<AnyElement> = match conflicts {
                    _ if !m.enabled => None,
                    Some(c) if c.hidden => Some(
                        Tag::warning()
                            .xsmall()
                            .child(t("game.mods.overridden"))
                            .into_any_element(),
                    ),
                    Some(c) if !c.loses_to.is_empty() || !c.wins_over.is_empty() => {
                        let lost: usize = c.loses_to.values().sum();
                        let won: usize = c.wins_over.values().sum();
                        Some(
                            Tag::secondary()
                                .xsmall()
                                .child(t_args(
                                    "game.mods.conflict_counts",
                                    &[("won", &won.to_string()), ("lost", &lost.to_string())],
                                ))
                                .into_any_element(),
                        )
                    }
                    _ => None,
                };
                let meta = [
                    m.version.clone().map(|v| format!("v{v}")),
                    m.author.clone(),
                    Some(source_label(&m.source).to_string()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let icon: AnyElement = match view.icons.get(&m.id) {
                    Some(path) => img(path.clone())
                        .size(rems(2.))
                        .rounded(theme.radius)
                        .flex_shrink_0()
                        .into_any_element(),
                    None => div()
                        .size(rems(2.))
                        .flex_shrink_0()
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Puzzle)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .into_any_element(),
                };
                let dragged = DraggedMod {
                    id: m.id.clone(),
                    name: m.name.clone().into(),
                };
                let (select_id, drop_target, toggle_id, menu_id) =
                    (m.id.clone(), ix, m.id.clone(), m.id.clone());
                let (up_id, down_id) = (m.id.clone(), m.id.clone());
                let entity = cx.entity();
                let name = m.name.clone();
                Some(
                    h_flex()
                        .id(ElementId::Name(format!("mod-row-{}", m.id).into()))
                        .w_full()
                        .h(rems(ROW_REMS))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .when(ix > 0, |r| r.border_t_1().border_color(theme.border))
                        .when(is_selected, |r| r.bg(theme.accent))
                        .hover(|s| s.bg(theme.muted))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = if this.selected.as_deref() == Some(select_id.as_str())
                            {
                                None
                            } else {
                                Some(select_id.clone())
                            };
                            cx.notify();
                        }))
                        .when(!locked, |r| {
                            r.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                                .drag_over::<DraggedMod>(|style, _, _, cx| {
                                    style.bg(cx.theme().drop_target)
                                })
                                .on_drop(cx.listener(move |this, d: &DraggedMod, window, cx| {
                                    this.move_mod(&d.id, drop_target, window, cx)
                                }))
                        })
                        .child(
                            Icon::new(IconName::GripVertical)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .w(rems(1.5))
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}", ix + 1)),
                        )
                        .child(icon)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_medium()
                                        .when(!m.enabled, |d| d.text_color(theme.muted_foreground))
                                        .child(m.name.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(meta),
                                ),
                        )
                        .children(tag)
                        .child(
                            Button::new(("mod-up", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::ArrowUp)
                                .tooltip(t("game.mods.load_earlier"))
                                .disabled(locked || ix == 0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.move_mod(&up_id, ix.saturating_sub(1), window, cx)
                                })),
                        )
                        .child(
                            Button::new(("mod-down", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::ArrowDown)
                                .tooltip(t("game.mods.load_later"))
                                .disabled(locked || ix + 1 >= count)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.move_mod(&down_id, ix + 1, window, cx)
                                })),
                        )
                        .child(
                            Switch::new(("mod-enabled", ix))
                                .small()
                                .checked(m.enabled)
                                .accessibility_label(t_args(
                                    "game.mods.enable_mod",
                                    &[("name", &name)],
                                ))
                                .disabled(locked)
                                .on_change(cx.listener(move |this, on: &bool, window, cx| {
                                    this.set_enabled(toggle_id.clone(), *on, window, cx)
                                })),
                        )
                        .child(
                            Button::new(("mod-more", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::EllipsisVertical)
                                .tooltip(t("game.mods.more"))
                                .dropdown_menu(move |menu, _, _| {
                                    let (top, bottom, remove, page) = (
                                        entity.clone(),
                                        entity.clone(),
                                        entity.clone(),
                                        m.source.page_url(),
                                    );
                                    let (top_id, bottom_id, remove_id) =
                                        (menu_id.clone(), menu_id.clone(), menu_id.clone());
                                    let mut menu = menu
                                        .item(
                                            PopupMenuItem::new(t("game.mods.move_first"))
                                                .icon(IconName::ChevronsUp)
                                                .disabled(locked || ix == 0)
                                                .on_click(move |_, window, cx| {
                                                    top.update(cx, |p, cx| {
                                                        p.move_mod(&top_id, 0, window, cx)
                                                    })
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new(t("game.mods.move_last"))
                                                .icon(IconName::ChevronsDown)
                                                .disabled(locked || ix + 1 >= count)
                                                .on_click(move |_, window, cx| {
                                                    bottom.update(cx, |p, cx| {
                                                        p.move_mod(&bottom_id, count, window, cx)
                                                    })
                                                }),
                                        );
                                    if let Some(url) = page.and_then(|u| irori_ui::web_url(&u)) {
                                        menu = menu.item(
                                            PopupMenuItem::new(t("game.mods.open_page"))
                                                .icon(IconName::ExternalLink)
                                                .on_click(move |_, _, cx| cx.open_url(&url)),
                                        );
                                    }
                                    menu.separator().item(
                                        PopupMenuItem::new(t("game.mods.remove"))
                                            .icon(IconName::Trash)
                                            .disabled(locked)
                                            .on_click(move |_, window, cx| {
                                                remove.update(cx, |p, cx| {
                                                    p.confirm_remove(remove_id.clone(), window, cx)
                                                })
                                            }),
                                    )
                                }),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}
