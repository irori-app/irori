//! The game's "More actions" menu. The same builder serves the game page
//! dropdown and other features' context menus, so every entry point offers
//! the same commands with the same labels and enabled states.

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{App, Context, WeakEntity, Window};
use irori_core::{ClientAction, GameId, GameInfo, InstallPlatform, Runner};
use irori_services::LaunchParams;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::{GamePage, GamePageEvent};
use crate::state::{ActionInputs, PrimaryAction};
use crate::{install, manage, ops, redmod};

/// Append every game command for `id` to `menu`. Use it as the builder of a
/// `DropdownMenu` or `ContextMenu` anywhere a game is shown.
pub fn game_menu(menu: PopupMenu, id: &GameId, window: &mut Window, cx: &mut App) -> PopupMenu {
    build_menu(menu, id, None, None, None, window, cx)
}

/// One-off launch variants offered next to Play: label key, icon, params.
pub(super) const PLAY_VARIANTS: [(&str, IconName, LaunchParamsFn); 2] = [
    ("game.menu.play_verbose", IconName::ScrollText, verbose),
    (
        "game.menu.play_skip_update",
        IconName::FastForward,
        skip_update_check,
    ),
];

pub(super) type LaunchParamsFn = fn(LaunchParams) -> LaunchParams;

fn verbose(p: LaunchParams) -> LaunchParams {
    LaunchParams { verbose: true, ..p }
}

fn skip_update_check(p: LaunchParams) -> LaunchParams {
    LaunchParams {
        skip_version_check: true,
        ..p
    }
}

/// Launch through the page when there is one (so it shows "Launching…"),
/// else directly.
fn launch(
    page: Option<&WeakEntity<GamePage>>,
    id: &GameId,
    params: LaunchParams,
    window: &mut Window,
    cx: &mut App,
) {
    match page.and_then(|p| p.upgrade()) {
        Some(page) => page.update(cx, |p, cx| p.play_with(params, window, cx)),
        None => ops::play_with(id, params, window, cx).detach(),
    }
}

fn idle(game: &GameInfo, cx: &App) -> bool {
    let m = models(cx);
    let status = m.statuses.read(cx).kind(&game.id());
    let queue = &m.downloads.read(cx).queue;
    let inputs = ActionInputs::from_game(game, status, queue, false);
    PrimaryAction::from_inputs(&inputs).is_idle()
}

/// `page` is the game page the menu belongs to; page-only entries (edit,
/// categories) need it.
pub(super) fn build_menu(
    mut menu: PopupMenu,
    id: &GameId,
    shortcuts_exist: Option<bool>,
    store_url: Option<String>,
    page: Option<WeakEntity<GamePage>>,
    _: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let Some(game) = backend(cx).game(id) else {
        return menu;
    };
    let idle = idle(&game, cx);
    let sideload = id.runner == Runner::Sideload;
    let hidden = models(cx).library.read(cx).is_hidden(id);
    let item = |key: &str, icon: IconName| PopupMenuItem::new(t(key)).icon(icon);

    // Steam's client installs, verifies and removes its games.
    let client = id.runner == Runner::Steam;
    let in_client = |key: &str, icon: IconName| {
        PopupMenuItem::new(t_args(key, &[("store", id.runner.store_name())])).icon(icon)
    };
    if !game.is_installed && client {
        let i = id.clone();
        menu = menu.item(
            in_client("game.menu.install_in_client", IconName::Download).on_click(
                move |_, window, cx| ops::client_action(&i, ClientAction::Install, window, cx),
            ),
        );
    } else if !game.is_installed {
        if sideload && game.browser_url.is_none() {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.locate_program", IconName::FolderSearch)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::locate_program(&i, window, cx)),
            );
        }
        if !sideload {
            let i = id.clone();
            menu = menu.item(
                item("game.common.install_ellipsis", IconName::Download)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| {
                        install::open_install_sheet(i.clone(), window, cx)
                    }),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.import", IconName::FolderInput)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| {
                        manage::open_import_dialog(i.clone(), window, cx)
                    }),
            );
            // Steam starts Irori for the shortcut, which offers the install.
            if game.installable {
                let i = id.clone();
                menu = menu.separator().item(
                    item("game.menu.add_to_steam", IconName::Gamepad2).on_click(
                        move |_, window, cx| ops::add_shortcuts(&i, true, window, cx).detach(),
                    ),
                );
            }
        }
    } else {
        let playable = matches!(
            PrimaryAction::from_inputs(&ActionInputs::from_game(
                &game,
                models(cx).statuses.read(cx).kind(id),
                &models(cx).downloads.read(cx).queue,
                false,
            )),
            PrimaryAction::Play | PrimaryAction::Update
        );
        // Steam starts its games itself: no Irori logs or update check.
        let variants = if id.runner.manages_installs() {
            &PLAY_VARIANTS[..]
        } else {
            &[]
        };
        for &(label, icon, params) in variants {
            let i = id.clone();
            let page = page.clone();
            menu = menu.item(item(label, icon).disabled(!playable).on_click(
                move |_, window, cx| {
                    launch(
                        page.as_ref(),
                        &i,
                        params(LaunchParams::default()),
                        window,
                        cx,
                    )
                },
            ));
        }
        if id.runner == Runner::Emulation {
            let i = id.clone();
            menu = menu.item(item("game.play_with.menu", IconName::Joystick).on_click(
                move |_, window, cx| crate::open_play_with_dialog(i.clone(), window, cx),
            ));
        }
        menu = menu.separator();
        // ROMs are the user's files: nothing to verify, update or move.
        let store_managed = !sideload
            && id.runner != Runner::Emulation
            && game.third_party_managed_app.is_none()
            && id.runner.manages_installs();
        if store_managed {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.verify", IconName::ShieldCheck)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::verify(&i, window, cx).detach()),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.update_now", IconName::CloudDownload)
                    .disabled(!idle || id.runner == Runner::Zoom)
                    .on_click(move |_, _, cx| ops::queue_update(&i, cx)),
            );
            if matches!(id.runner, Runner::Legendary | Runner::Gog | Runner::Itch) {
                let i = id.clone();
                menu = menu.item(
                    item("game.menu.modify", IconName::Layers)
                        .disabled(!idle)
                        .on_click(move |_, window, cx| {
                            install::open_modify_sheet(i.clone(), window, cx)
                        }),
                );
            }
            if redmod::applies(&game, cx) {
                let i = id.clone();
                menu = menu.item(
                    item("game.menu.redmod", IconName::Puzzle)
                        .disabled(!idle)
                        .on_click(move |_, window, cx| {
                            redmod::open_redmod_sheet(i.clone(), window, cx)
                        }),
                );
            }
            let i = id.clone();
            menu = menu.item(
                item("game.menu.move", IconName::FolderInput)
                    .disabled(!idle || id.runner == Runner::Zoom)
                    .on_click(move |_, window, cx| manage::open_move_dialog(i.clone(), window, cx)),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.change_location", IconName::FolderSearch)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::change_install_location(&i, window, cx)),
            );
            menu = menu.separator();
        }
        if crate::mods::applies(&game) {
            let g = game.clone();
            menu = menu.item(item("game.menu.mods", IconName::Puzzle).on_click(
                move |_, window, cx| crate::mods::open_mods_sheet(g.clone(), window, cx),
            ));
        }
        let i = id.clone();
        menu = menu.item(
            item("game.menu.open_install_folder", IconName::FolderOpen)
                .disabled(game.install_path().is_none())
                .on_click(move |_, window, cx| ops::open_install_folder(&i, window, cx)),
        );
        if client {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.launch_options", IconName::SquareTerminal).on_click(
                    move |_, window, cx| crate::open_launch_options_dialog(i.clone(), window, cx),
                ),
            );
            let i = id.clone();
            menu = menu.item(
                in_client("game.menu.verify_in_client", IconName::ShieldCheck)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| {
                        ops::client_action(&i, ClientAction::Verify, window, cx)
                    }),
            );
        }
        if id.runner.manages_installs() {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.open_log", IconName::ScrollText)
                    .on_click(move |_, window, cx| crate::open_log_sheet(i.clone(), window, cx)),
            );
        }
        if irori_ui::save_backups::offers_backups(&game) {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.back_up_saves", IconName::DatabaseBackup).on_click(
                    move |_, window, cx| irori_ui::save_backups::back_up_now(&i, window, cx),
                ),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.save_backups", IconName::ArchiveRestore).on_click(
                    move |_, window, cx| {
                        irori_ui::save_backups::open_save_backups_sheet(i.clone(), window, cx)
                    },
                ),
            );
        }
        let uses_wine = game
            .installed_platform()
            .unwrap_or(InstallPlatform::Windows)
            .needs_wine();
        if uses_wine {
            menu = menu.separator();
            let i = id.clone();
            menu = menu.item(
                item("game.menu.open_prefix", IconName::Folder)
                    .on_click(move |_, window, cx| ops::open_prefix_folder(&i, window, cx)),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.run_exe", IconName::Terminal)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::run_exe_in_prefix(&i, window, cx)),
            );
            let i = id.clone();
            menu = menu.item(
                item("game.menu.winecfg", IconName::Wine)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::winecfg(&i, window, cx).detach()),
            );
            let i = id.clone();
            menu = menu.item(
                PopupMenuItem::new("Winetricks")
                    .icon(IconName::Wrench)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::winetricks(&i, window, cx).detach()),
            );
        }
        // protontricks finds Steam's own Proton prefixes by app ID.
        if cfg!(target_os = "linux") && id.runner == Runner::Steam {
            menu = menu.separator();
            let i = id.clone();
            menu = menu.item(
                PopupMenuItem::new("Protontricks")
                    .icon(IconName::Wrench)
                    .disabled(!idle)
                    .on_click(move |_, window, cx| ops::protontricks(&i, window, cx).detach()),
            );
        }
        menu = menu.separator();
        if shortcuts_exist != Some(true) {
            let i = id.clone();
            menu =
                menu.item(item("game.menu.add_shortcuts", IconName::Link).on_click(
                    move |_, window, cx| ops::add_shortcuts(&i, false, window, cx).detach(),
                ));
        }
        if shortcuts_exist != Some(false) {
            let i = id.clone();
            menu = menu.item(
                item("game.menu.remove_shortcuts", IconName::Link2Off)
                    .on_click(move |_, window, cx| ops::remove_shortcuts(&i, window, cx).detach()),
            );
        }
        if id.runner != Runner::Steam {
            let i = id.clone();
            menu =
                menu.item(item("game.menu.add_to_steam", IconName::Gamepad2).on_click(
                    move |_, window, cx| ops::add_shortcuts(&i, true, window, cx).detach(),
                ));
        }
    }

    if let Some(page) = page.clone() {
        menu = menu.separator();
        let i = id.clone();
        let p = page.clone();
        menu = menu.item(
            item("game.menu.edit", IconName::Pencil).on_click(move |_, _, cx| {
                let _ = p.update(cx, |_, cx| cx.emit(GamePageEvent::RequestEdit(i.clone())));
            }),
        );
        menu = menu.item(
            item("game.menu.categories", IconName::Tags).on_click(move |_, _, cx| {
                let _ = page.update(cx, |_, cx| cx.emit(GamePageEvent::RequestCategories));
            }),
        );
    }
    let i = id.clone();
    menu = menu.item(
        item(
            if hidden {
                "game.menu.show_in_library"
            } else {
                "game.menu.hide_from_library"
            },
            if hidden {
                IconName::Eye
            } else {
                IconName::EyeOff
            },
        )
        .on_click(move |_, _, cx| ops::set_hidden(&i, !hidden, cx)),
    );
    if client {
        let i = id.clone();
        menu = menu.item(
            in_client("game.menu.show_in_client", IconName::Joystick).on_click(
                move |_, window, cx| ops::client_action(&i, ClientAction::Show, window, cx),
            ),
        );
    }
    // Xbox app games (`xbox:<package family>`) open their Microsoft Store
    // page, where the Xbox app's install and remove commands are.
    if id.runner == Runner::Launchers && id.app_name.starts_with("xbox:") {
        let i = id.clone();
        menu = menu.item(
            PopupMenuItem::new(t_args(
                "game.menu.show_in_client",
                &[("store", "Microsoft Store")],
            ))
            .icon(IconName::ExternalLink)
            .on_click(move |_, window, cx| ops::client_action(&i, ClientAction::Show, window, cx)),
        );
    }
    if let Some(url) = store_url.filter(|u| !u.is_empty()) {
        let runner = id.runner;
        menu = menu.item(item("game.menu.store_page", IconName::Store).on_click(
            move |_, window, cx| irori_ui::hooks::open_store_page(runner, url.clone(), window, cx),
        ));
    }
    if game.is_installed && client {
        menu = menu.separator();
        let i = id.clone();
        menu = menu.item(
            in_client("game.menu.uninstall_in_client", IconName::Trash)
                .disabled(!idle)
                .on_click(move |_, window, cx| {
                    ops::client_action(&i, ClientAction::Uninstall, window, cx)
                }),
        );
    }
    // An added game whose program is gone can still leave the library.
    if (game.is_installed || sideload)
        && id.runner.manages_installs()
        && id.runner != Runner::Emulation
    {
        menu = menu.separator();
        let i = id.clone();
        menu = menu.item(
            item("game.menu.uninstall", IconName::Trash)
                .disabled(!idle)
                .on_click(move |_, window, cx| {
                    manage::open_uninstall_dialog(i.clone(), window, cx)
                }),
        );
    }
    menu
}

/// "Your rating" with the star counts and "Clear rating", for the page's own
/// menus (the rating's stars sit on the page too).
pub(super) fn rating_submenu(
    menu: PopupMenu,
    id: &GameId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let current = models(cx).library.read(cx).rating(id);
    let id = id.clone();
    menu.separator().submenu_with_icon(
        Some(IconName::Star.into()),
        t("game.rating.label"),
        window,
        cx,
        move |mut menu, _, _| {
            for stars in 1..=irori_services::userdata::MAX_RATING {
                let i = id.clone();
                menu = menu.item(
                    PopupMenuItem::new(super::hero::rate_label(stars))
                        .checked(current == Some(stars))
                        .on_click(move |_, _, cx| ops::set_rating(&i, Some(stars), cx)),
                );
            }
            let i = id.clone();
            menu.separator().item(
                PopupMenuItem::new(t("game.rating.clear"))
                    .disabled(current.is_none())
                    .on_click(move |_, _, cx| ops::set_rating(&i, None, cx)),
            )
        },
    )
}
