//! Per-game context menus, category/copy submenus, and the "…" action button.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
};
use irori_core::{GameId, GameInfo};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::LibraryView;
use crate::commands::{self, GameCommand, GameState};

/// Build a game's menu (used by the context menu and the "…" button).
pub(super) fn game_menu(
    menu: PopupMenu,
    view: &WeakEntity<LibraryView>,
    game: &Arc<GameInfo>,
    in_recent: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(state) = view.upgrade().map(|v| {
        let id = game.id();
        v.update(cx, |this, cx| this.check_shortcut(&id, cx));
        v.read(cx).state_for(game, in_recent, cx)
    }) else {
        return menu;
    };
    let id = game.id();
    let mut menu = menu;
    for (ix, group) in commands::menu(&state).into_iter().enumerate() {
        if ix > 0 {
            menu = menu.separator();
        }
        for command in group {
            let after_hidden = command == GameCommand::ToggleHidden;
            menu = menu.item(command_item(view, &id, command, &state));
            if after_hidden {
                let copies_view = view.clone();
                let view = view.clone();
                let id = id.clone();
                menu = menu.submenu_with_icon(
                    Some(Icon::new(IconName::Tags)),
                    t("library.menu.add_to"),
                    window,
                    cx,
                    move |sub, _, cx| categories_menu(sub, &view, &id, cx),
                );
                // Smart categories follow their rules: named, never ticked.
                let smart = smart_categories_of(&game.id(), cx);
                if !smart.is_empty() {
                    let names = smart.join(&t("library.filter.list_separator"));
                    menu = menu.label(t_args("library.menu.in_smart", &[("names", &names)]));
                }
                let status_id = game.id();
                menu = menu.submenu_with_icon(
                    Some(Icon::new(IconName::Flag)),
                    irori_ui::play_status::title(),
                    window,
                    cx,
                    move |sub, _, cx| irori_ui::play_status::menu(sub, &status_id, cx),
                );
                menu = copies_menu(menu, &copies_view, game, window, cx);
                if game.is_installed && !irori_ui::user_actions::list(&game.id(), cx).is_empty() {
                    let actions_id = game.id();
                    menu = menu.submenu_with_icon(
                        Some(Icon::new(IconName::SquareTerminal)),
                        t("library.menu.actions"),
                        window,
                        cx,
                        move |sub, _, cx| {
                            irori_ui::user_actions::menu_items(sub, &actions_id, false, cx)
                        },
                    );
                }
            }
        }
    }
    // A picked game's menu removes every picked game it can.
    let picked_removable = view.upgrade().and_then(|v| {
        let v = v.read(cx);
        (v.selection.is_active() && v.selection.contains(&id))
            .then(|| v.removal_plan(cx).remove.len())
            .filter(|n| *n > 0)
    });
    if let Some(count) = picked_removable {
        let view = view.clone();
        menu = menu.separator().item(
            PopupMenuItem::new(t_args(
                "library.bulk.remove",
                &[("count", &count.to_string())],
            ))
            .icon(Icon::new(IconName::ListX))
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| this.remove_picked(window, cx))
                    .ok();
            }),
        );
    }
    menu
}

/// The game's copies on other stores: a submenu to open, prefer or
/// separate them, or, for a game of one copy, combining it with others
/// (picked in selection mode, then combined from the bar).
fn copies_menu(
    menu: PopupMenu,
    view: &WeakEntity<LibraryView>,
    game: &Arc<GameInfo>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = game.id();
    if irori_ui::copies::of(&id, cx).len() > 1 {
        return menu.submenu_with_icon(
            Some(Icon::new(IconName::Layers)),
            t("ui.copies.title"),
            window,
            cx,
            move |sub, _, cx| irori_ui::copies::menu_items(sub, &id, cx),
        );
    }
    let (view, title) = (view.clone(), game.title.clone());
    menu.item(
        PopupMenuItem::new(t("library.menu.combine"))
            .icon(Icon::new(IconName::Combine))
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.start_combining(&id, cx));
                window.push_notification(
                    gpui_kit::component::notification::Notification::info(t_args(
                        "library.combine.hint",
                        &[("title", &title)],
                    )),
                    cx,
                );
            }),
    )
}

/// The names of the smart categories a game is in, pinned first.
fn smart_categories_of(id: &GameId, cx: &App) -> Vec<String> {
    let lib = models(cx).library.read(cx);
    lib.user
        .categories_in_order()
        .into_iter()
        .filter(|(c, _)| {
            matches!(c, irori_services::CategoryRef::Smart(_))
                && irori_ui::categories::contains(c, id, &lib.user, &lib.categories)
        })
        .map(|(_, name)| name)
        .collect()
}

fn categories_menu(
    menu: PopupMenu,
    view: &WeakEntity<LibraryView>,
    id: &GameId,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let key = id.key();
    let entries: Vec<(String, bool)> = models(cx)
        .library
        .read(cx)
        .user
        .categories
        .iter()
        .map(|(name, set)| (name.clone(), set.contains(&key)))
        .collect();
    let mut menu = menu;
    if entries.is_empty() {
        menu = menu.label(t("library.menu.no_categories"));
    }
    let state = GameState::default();
    for (name, member) in entries {
        menu = menu.item(
            command_item(view, id, GameCommand::ToggleCategory(name), &state).checked(member),
        );
    }
    menu.separator()
        .item(command_item(view, id, GameCommand::NewCategory, &state))
}

fn command_item(
    view: &WeakEntity<LibraryView>,
    id: &GameId,
    command: GameCommand,
    state: &GameState,
) -> PopupMenuItem {
    let view = view.clone();
    let id = id.clone();
    let item = PopupMenuItem::new(command.label(state));
    let item = if matches!(command, GameCommand::ToggleCategory(_)) {
        item
    } else {
        item.icon(Icon::new(command.icon(state)))
    };
    item.on_click(move |_, window, cx| {
        let command = command.clone();
        view.update(cx, |this, cx| this.run_command(&id, command, window, cx))
            .ok();
    })
}

impl LibraryView {
    /// The visible "…" button opening the same menu as a right-click.
    pub(super) fn more_button(
        &self,
        game: &Arc<GameInfo>,
        in_recent: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = game.id().key();
        let view = cx.entity().downgrade();
        let open_view = view.clone();
        let menu_game = game.clone();
        let open_id = game.id();
        div()
            .id(ElementId::Name(format!("tile-more-wrap:{key}").into()))
            .flex_shrink_0()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new(ElementId::Name(format!("tile-more:{key}").into()))
                    .small()
                    .icon(IconName::Ellipsis)
                    .tooltip(t("library.tile.more_actions"))
                    .dropdown_menu(move |menu, window, cx| {
                        game_menu(menu, &view, &menu_game, in_recent, window, cx)
                    })
                    .on_open_change(move |open, _, cx| {
                        let id = open_id.clone();
                        let open = *open;
                        open_view
                            .update(cx, |this, cx| {
                                this.menu_open = open.then_some(id);
                                cx.notify();
                            })
                            .ok();
                    }),
            )
    }
}
