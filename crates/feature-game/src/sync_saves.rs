//! "Sync saves now" from the game menu and the command palette: one sync of a
//! game's cloud saves in the store's own direction, told with a notification.
//! Saves changed on both sides open the cloud-saves sheet, where the user
//! chooses which to keep; the game page shows the sync as it runs.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Window};
use irori_core::{GameId, GameInfo, SaveConflict, SyncDirection};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::ops::notify_error;
use crate::saves::{CloudSupport, open_cloud_saves_sheet};

/// Whether `game` can sync its cloud saves now: its store keeps saves, the
/// game uses them and is installed (natively or under Wine alike).
pub fn offers_sync(game: &GameInfo, cx: &App) -> bool {
    let store_supports = backend(cx)
        .store(game.runner())
        .is_some_and(|s| s.supports_cloud_saves());
    CloudSupport::of(game, store_supports) == CloudSupport::Supported
}

/// Sync `id`'s cloud saves now, letting the store pick the direction.
pub fn sync_saves_now(id: &GameId, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let Some(game) = b.game(id) else {
        return;
    };
    let title = game.title.clone();
    let task_id = id.clone();
    let task = irori_ui::run(cx, async move {
        if !b.is_online() {
            anyhow::bail!(t("game.saves.offline"));
        }
        b.sync_saves(&task_id, SyncDirection::Auto).await
    });
    let id = id.clone();
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(_) => window.push_notification(
                    Notification::success(t_args("game.saves.synced", &[("title", &title)])),
                    cx,
                ),
                Err(e) if SaveConflict::of(&e).is_some() => {
                    window.push_notification(
                        Notification::warning(t_args(
                            "game.saves.sync_conflict",
                            &[("title", &title)],
                        )),
                        cx,
                    );
                    open_cloud_saves_sheet(id.clone(), window, cx);
                }
                Err(e) => notify_error(window, cx, t("game.saves.sync_failed"), &e),
            });
        })
        .detach();
}
