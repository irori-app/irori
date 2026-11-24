//! The actions the user added beside a game's Play button (`iroriActions`)
//! as every screen offers them: the Play button's menu, the game's menu, the
//! library's context menu, the command palette and Big Picture all list them
//! with [`list`] and start them with [`run`], so each says the same and
//! reports the same way.

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Icon, WindowExt as _};
use gpui_kit::{App, Window};
use irori_core::{GameId, UserAction};

use crate::i18n::{t, t_args};
use crate::{Route, backend, navigate};

/// The game settings section where the actions are edited.
pub const SETTINGS_SECTION: &str = "actions";

/// The actions of `id` that have something to run, with their place in the
/// setting. Reads the settings in memory.
pub fn list(id: &GameId, cx: &App) -> Vec<(usize, UserAction)> {
    backend(cx)
        .settings
        .game(id)
        .actions
        .into_iter()
        .enumerate()
        .filter(|(_, a)| a.is_set())
        .collect()
}

/// The icon of an action: a link, a program in the prefix, or a program.
pub fn icon(action: &UserAction) -> IconName {
    if action.is_link() {
        IconName::ExternalLink
    } else if action.counts_as_playing {
        IconName::Play
    } else {
        IconName::SquareTerminal
    }
}

/// Start the `index`-th action of `id`, saying that it started or why it
/// didn't.
pub fn run(id: GameId, index: usize, window: &mut Window, cx: &mut App) {
    let Some(action) = backend(cx).settings.game(&id).actions.get(index).cloned() else {
        return;
    };
    let label = action.label();
    let b = backend(cx);
    let task = crate::run(cx, async move { b.run_user_action(&id, index).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => window.push_notification(
                    Notification::success(t_args("ui.actions.started", &[("name", &label)])),
                    cx,
                ),
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}"))
                        .title(t_args("ui.actions.failed", &[("name", &label)])),
                    cx,
                ),
            });
        })
        .detach();
}

/// Open the game settings section where `id`'s actions are edited.
pub fn edit(id: &GameId, cx: &mut App) {
    navigate(Route::GameSettings(id.clone(), SETTINGS_SECTION.into()), cx);
}

/// Append `id`'s actions to `menu`, then "Edit actions…". With no action
/// only "Add an action…" is added when `offer_add`.
pub fn menu_items(mut menu: PopupMenu, id: &GameId, offer_add: bool, cx: &App) -> PopupMenu {
    let actions = list(id, cx);
    if actions.is_empty() && !offer_add {
        return menu;
    }
    for (index, action) in &actions {
        let (id, index) = (id.clone(), *index);
        menu = menu.item(
            PopupMenuItem::new(action.label())
                .icon(Icon::new(icon(action)))
                .on_click(move |_, window, cx| run(id.clone(), index, window, cx)),
        );
    }
    let id = id.clone();
    let label = if actions.is_empty() {
        t("ui.actions.add")
    } else {
        t("ui.actions.edit")
    };
    menu.item(
        PopupMenuItem::new(label)
            .icon(Icon::new(IconName::Pencil))
            .on_click(move |_, _, cx| edit(&id, cx)),
    )
}
