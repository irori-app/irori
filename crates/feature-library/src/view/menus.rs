//! Per-game context menus, collection/copy/manage submenus, and the "…" action button.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div, rems,
};
use irori_core::{GameId, GameInfo};
use irori_services::CategoryRef;
use irori_services::smart_categories::matches_rules_only;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::LibraryView;
use crate::commands::{self, GameCommand, GameState};
use crate::dialogs::collection::open_collection_dialog_with_games;

/// Structured Steam-style game menu groups derived from [`commands::menu`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuLayout {
    pub(crate) primary: Vec<GameCommand>,
    pub(crate) favourite: Option<GameCommand>,
    pub(crate) manage_edit: Vec<GameCommand>,
    pub(crate) manage_shortcuts: Vec<GameCommand>,
    pub(crate) manage_files: Vec<GameCommand>,
    pub(crate) danger: Vec<GameCommand>,
}

/// Partition `commands::menu(state)` into the 3-block + `Manage >` + footer layout
/// so every valid command stays reachable while keeping the top level concise.
pub(crate) fn layout_commands(state: &GameState) -> MenuLayout {
    use GameCommand::*;
    let mut out = MenuLayout::default();
    for cmd in commands::menu(state).into_iter().flatten() {
        match cmd {
            Play | Stop | BackToGame | Install | Update | RemoveFromQueue | OpenPage | Settings => {
                out.primary.push(cmd);
            }
            ToggleFavourite => out.favourite = Some(cmd),
            Edit | EditMetadata | PlayAsPcGame | RemoveFromRecent | ToggleHidden => {
                out.manage_edit.push(cmd);
            }
            AddShortcut | RemoveShortcut | AddToSteam | RemoveFromSteam => {
                out.manage_shortcuts.push(cmd);
            }
            OpenFolder | Verify | Logs => out.manage_files.push(cmd),
            Uninstall => out.danger.push(cmd),
            ToggleCategory(_) | NewCategory | Locate => {}
        }
    }
    out
}

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
    let plan = layout_commands(&state);
    let mut menu = menu;

    // 1. Primary actions: Play/Install, Open game page, Game settings…, user actions.
    for command in plan.primary {
        menu = menu.item(command_item(view, &id, command, &state));
    }
    if game.is_installed && !irori_ui::user_actions::list(&id, cx).is_empty() {
        let actions_id = id.clone();
        menu = menu.submenu_with_icon(
            Some(Icon::new(IconName::SquareTerminal)),
            t("library.menu.actions"),
            window,
            cx,
            move |sub, _, cx| irori_ui::user_actions::menu_items(sub, &actions_id, false, cx),
        );
    }

    // 2. Organize: Favorites, Collections > (hybrid + manual), quick remove from active collection, Play status >.
    menu = menu.separator();
    if let Some(fav) = plan.favourite {
        menu = menu.item(command_item(view, &id, fav, &state));
    }
    {
        let col_view = view.clone();
        let col_game = game.clone();
        menu = menu.submenu_with_icon(
            Some(Icon::new(IconName::Layers)),
            t("library.menu.collections"),
            window,
            cx,
            move |sub, _, cx| collections_menu(sub, &col_view, &col_game, cx),
        );
    }
    let active_smart = view
        .upgrade()
        .map(|v| {
            v.read(cx)
                .filters
                .read(cx)
                .filter()
                .smart_categories
                .clone()
        })
        .unwrap_or_default();
    if let Some((sid, sname)) = smart_categories_of(&id, cx)
        .into_iter()
        .find(|(sid, _)| active_smart.contains(sid))
    {
        menu = menu.item(toggle_collection_item(
            sid,
            sname,
            game.clone(),
            true,
            true,
            None,
        ));
    }
    {
        let status_id = id.clone();
        menu = menu.submenu_with_icon(
            Some(Icon::new(IconName::Flag)),
            irori_ui::play_status::title(),
            window,
            cx,
            move |sub, _, cx| irori_ui::play_status::menu(sub, &status_id, cx),
        );
    }

    // 3. Manage > submenu grouping edit, metadata, copies, hide, shortcuts/Steam, folder, verify, logs.
    let manage_view = view.clone();
    let manage_game = game.clone();
    let manage_state = state.clone();
    let manage_edit = plan.manage_edit;
    let manage_shortcuts = plan.manage_shortcuts;
    let manage_files = plan.manage_files;
    menu = menu.separator().submenu_with_icon(
        Some(Icon::new(IconName::Settings2)),
        t("library.menu.manage"),
        window,
        cx,
        move |mut sub, window, cx| {
            let gid = manage_game.id();
            for cmd in &manage_edit {
                if *cmd == GameCommand::ToggleHidden {
                    continue;
                }
                sub = sub.item(manage_command_item(
                    &manage_view,
                    &gid,
                    cmd.clone(),
                    &manage_state,
                ));
            }
            sub = copies_menu(sub, &manage_view, &manage_game, window, cx);
            if let Some(hide_cmd) = manage_edit
                .iter()
                .find(|c| **c == GameCommand::ToggleHidden)
            {
                sub = sub.item(manage_command_item(
                    &manage_view,
                    &gid,
                    hide_cmd.clone(),
                    &manage_state,
                ));
            }
            if !manage_shortcuts.is_empty() {
                sub = sub.separator();
                let sc_view = manage_view.clone();
                let sc_id = gid.clone();
                let sc_state = manage_state.clone();
                let sc_cmds = manage_shortcuts.clone();
                sub = sub.submenu_with_icon(
                    Some(Icon::new(IconName::FolderSymlink)),
                    t("library.menu.shortcuts"),
                    window,
                    cx,
                    move |mut sc_sub, _, _| {
                        for cmd in &sc_cmds {
                            sc_sub =
                                sc_sub.item(command_item(&sc_view, &sc_id, cmd.clone(), &sc_state));
                        }
                        sc_sub
                    },
                );
            }
            if !manage_files.is_empty() {
                sub = sub.separator();
                for cmd in &manage_files {
                    sub = sub.item(command_item(&manage_view, &gid, cmd.clone(), &manage_state));
                }
            }
            sub
        },
    );

    // 4. Footer: Uninstall / Remove from library (+ bulk removal when picked).
    if !plan.danger.is_empty() {
        menu = menu.separator();
        for cmd in plan.danger {
            menu = menu.item(command_item(view, &id, cmd, &state));
        }
    }
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
                    Notification::info(t_args("library.combine.hint", &[("title", &title)])),
                    cx,
                );
            }),
    )
}

/// Toggle `game` in or out of hybrid collection `smart_id` with atomic Undo.
fn toggle_collection_item(
    smart_id: String,
    collection_name: String,
    game: Arc<GameInfo>,
    is_member: bool,
    as_quick_remove: bool,
    icon: Option<IconName>,
) -> PopupMenuItem {
    let label = if as_quick_remove {
        t_args(
            "library.menu.remove_from_named_collection",
            &[("name", &collection_name)],
        )
    } else {
        collection_name.clone().into()
    };
    let mut item = PopupMenuItem::new(label);
    if as_quick_remove {
        item = item.icon(Icon::new(IconName::ListX));
    } else if is_member {
        item = item.checked(true);
    } else if let Some(glyph) = icon {
        item = item.icon(Icon::new(glyph));
    }
    let game_id = game.id();
    let game_title = game.title.clone();
    item.on_click(move |_, window, cx| {
        let b = backend(cx);
        let lib = models(cx).library.read(cx);
        let inputs = b.category_inputs();
        let today = chrono::Local::now().date_naive();
        let by_rules = lib.user.smart_categories.get(&smart_id).is_some_and(|cat| {
            matches_rules_only(
                cat,
                &lib.snapshot,
                &lib.user,
                &inputs,
                &game_id,
                today,
                &chrono::Local,
            )
        });
        let ids = vec![game_id.clone()];
        let before = b.userdata.capture(&ids);
        let sid = smart_id.clone();
        let b_run = b.clone();
        irori_ui::state::run_infallible(cx, async move {
            if is_member {
                b_run.remove_from_smart_category_many(&sid, &ids, |_| by_rules);
            } else {
                b_run.add_to_smart_category_many(&sid, &ids, |_| !by_rules);
            }
        })
        .detach();
        let msg = if is_member {
            t_args(
                "library.collection.game_removed",
                &[("title", &game_title), ("name", &collection_name)],
            )
        } else {
            t_args(
                "library.collection.game_added",
                &[("title", &game_title), ("name", &collection_name)],
            )
        };
        window.push_notification(
            Notification::info(msg)
                .action(move |_, _, cx| {
                    let note = cx.entity().downgrade();
                    let before = before.clone();
                    Button::new("undo-toggle-collection")
                        .label(t("library.bulk.undo"))
                        .small()
                        .on_click(move |_, window, cx| {
                            let (b, before) = (backend(cx), before.clone());
                            irori_ui::state::run_infallible(cx, async move {
                                b.userdata.restore(&before);
                            })
                            .detach();
                            let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                        })
                })
                .autohide(true),
            cx,
        );
    })
}

/// The `(id, name)` of the smart categories a game is in, pinned first.
fn smart_categories_of(id: &GameId, cx: &App) -> Vec<(String, String)> {
    let lib = models(cx).library.read(cx);
    lib.user
        .categories_in_order()
        .into_iter()
        .filter_map(|(c, name)| match &c {
            CategoryRef::Smart(sid)
                if irori_ui::categories::contains(&c, id, &lib.user, &lib.categories) =>
            {
                Some((sid.clone(), name))
            }
            _ => None,
        })
        .collect()
}

struct SmartMenuEntry {
    id: String,
    name: String,
    icon: IconName,
    member: bool,
}

/// Unified `Collections >` submenu: all top-bar hybrid collections with
/// checkmarks + "+ New collection…" pre-filling this game + any legacy manual
/// categories below a separator.
fn collections_menu(
    menu: PopupMenu,
    view: &WeakEntity<LibraryView>,
    game: &Arc<GameInfo>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = game.id();
    let key = id.key();
    let (smart_entries, manual_entries): (Vec<SmartMenuEntry>, Vec<(String, bool)>) = {
        let lib = models(cx).library.read(cx);
        let smart = lib
            .user
            .categories_in_order()
            .into_iter()
            .filter_map(|(c, name)| {
                let CategoryRef::Smart(sid) = &c else {
                    return None;
                };
                let cat = lib.user.smart_categories.get(sid)?;
                if cat.is_unreadable() {
                    return None;
                }
                let member = irori_ui::categories::contains(&c, &id, &lib.user, &lib.categories);
                let icon = irori_ui::categories::icon_for(&c, cat.look.icon.as_deref());
                Some(SmartMenuEntry {
                    id: sid.clone(),
                    name,
                    icon,
                    member,
                })
            })
            .collect();
        let manual = lib
            .user
            .categories
            .iter()
            .map(|(name, set)| (name.clone(), set.contains(&key)))
            .collect();
        (smart, manual)
    };

    let mut menu = menu;
    if smart_entries.is_empty() && manual_entries.is_empty() {
        menu = menu.label(t("library.menu.no_collections"));
    }
    for entry in &smart_entries {
        menu = menu.item(toggle_collection_item(
            entry.id.clone(),
            entry.name.clone(),
            game.clone(),
            entry.member,
            false,
            Some(entry.icon),
        ));
    }
    if !smart_entries.is_empty() {
        menu = menu.separator();
    }
    let initial_key = key.clone();
    menu = menu.item(
        PopupMenuItem::new(t("library.collection.new_menu"))
            .icon(Icon::new(IconName::Plus))
            .on_click(move |_, window, cx| {
                open_collection_dialog_with_games(None, vec![initial_key.clone()], window, cx);
            }),
    );

    if !manual_entries.is_empty() {
        let state = GameState::default();
        menu = menu.separator().label(t("library.menu.manual_categories"));
        for (name, member) in manual_entries {
            menu = menu.item(
                command_item(view, &id, GameCommand::ToggleCategory(name), &state).checked(member),
            );
        }
    }
    menu
}

fn manage_command_item(
    view: &WeakEntity<LibraryView>,
    id: &GameId,
    command: GameCommand,
    state: &GameState,
) -> PopupMenuItem {
    if command == GameCommand::EditMetadata {
        let view = view.clone();
        let id = id.clone();
        return PopupMenuItem::new(t("library.menu.edit_metadata_short"))
            .icon(Icon::new(command.icon(state)))
            .on_click(move |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.run_command(&id, GameCommand::EditMetadata, window, cx)
                })
                .ok();
            });
    }
    command_item(view, id, command, state)
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

    /// Translucent circular "…" button paired with the hover action capsule on
    /// grid cover tiles (height matched to `tile-primary`).
    pub(super) fn cover_more_button(
        &self,
        game: &Arc<GameInfo>,
        in_recent: bool,
        large: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = game.id().key();
        let more_key = key.clone();
        let view = cx.entity().downgrade();
        let open_view = view.clone();
        let menu_game = game.clone();
        let open_id = game.id();
        let theme = cx.theme();
        let bg = theme.background.opacity(0.76);
        let border = theme.foreground.opacity(0.2);
        let button = Button::new(ElementId::Name(format!("tile-more:{key}").into()))
            .ghost()
            .rounded_full()
            .tooltip(t("library.tile.more_actions"));
        let button = if large {
            button
                .size(rems(2.5))
                .p_0()
                .accessibility_label(t("library.tile.more_actions"))
                .child(Icon::new(IconName::Ellipsis).size_5())
        } else {
            button.icon(IconName::Ellipsis)
        };
        div()
            .id(ElementId::Name(format!("tile-more-wrap:{key}").into()))
            .debug_selector(move || format!("tile-more-wrap:{more_key}"))
            .flex_shrink_0()
            .rounded_full()
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow_md()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                button
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_style_menu_layout_keeps_top_level_concise_and_preserves_every_command() {
        let uninstalled = GameState {
            installable: true,
            ..Default::default()
        };
        let layout = layout_commands(&uninstalled);
        assert_eq!(
            layout.primary,
            vec![GameCommand::Install, GameCommand::OpenPage]
        );
        assert_eq!(layout.favourite, Some(GameCommand::ToggleFavourite));
        assert_eq!(
            layout.manage_edit,
            vec![
                GameCommand::ToggleHidden,
                GameCommand::Edit,
                GameCommand::EditMetadata,
            ]
        );
        assert_eq!(
            layout.manage_shortcuts,
            vec![GameCommand::AddToSteam, GameCommand::RemoveFromSteam]
        );
        assert!(layout.manage_files.is_empty());
        assert!(layout.danger.is_empty());

        let installed = GameState {
            installed: true,
            has_install_path: true,
            ..Default::default()
        };
        let layout = layout_commands(&installed);
        assert_eq!(
            layout.primary,
            vec![
                GameCommand::Play,
                GameCommand::OpenPage,
                GameCommand::Settings,
            ]
        );
        assert_eq!(
            layout.manage_files,
            vec![
                GameCommand::Logs,
                GameCommand::OpenFolder,
                GameCommand::Verify,
            ]
        );
        assert_eq!(layout.danger, vec![GameCommand::Uninstall]);
    }
}
