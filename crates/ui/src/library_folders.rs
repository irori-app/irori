//! Library folders (`iroriLibraryFolders`) as every screen shows them: the
//! default install folder first, then the user's named folders. Settings,
//! the install sheet and the library's selection bar name them with
//! [`label`] and move games into one with [`confirm_move`], so each asks and
//! reports the same way.

use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, SharedString, Window};
use irori_core::settings::LibraryFolder;
use irori_core::{GameId, GameInfo};

use crate::i18n::{t, t_args};
use crate::{backend, models};

/// Every library folder, the default install folder first.
pub fn all(cx: &App) -> Vec<LibraryFolder> {
    models(cx).settings.read(cx).app.library_folders()
}

/// What a folder is called on screen.
pub fn label(folder: &LibraryFolder) -> SharedString {
    if folder.is_default() {
        t("ui.library_folders.default")
    } else {
        folder.name.clone().into()
    }
}

/// The games among `games` that can move into `folder`: installed by Irori,
/// not running or busy, and not there already.
pub fn movable(games: &[Arc<GameInfo>], folder: &LibraryFolder, cx: &App) -> Vec<GameId> {
    let app = models(cx).settings.read(cx).app.clone();
    let statuses = models(cx).statuses.read(cx);
    games
        .iter()
        .filter(|g| g.id().runner.manages_installs())
        .filter_map(|g| {
            let id = g.id();
            let path = g.install_path()?;
            let base =
                irori_core::paths::expand_home(&app.install_folder_in(&folder.path, id.runner));
            let busy = statuses.get(&id).is_some();
            (!busy && path.parent() != Some(base.as_path())).then_some(id)
        })
        .collect()
}

/// Ask, then move `ids` into `folder` one game at a time, and say how it
/// went: one notification for the games moved, one naming those that
/// weren't.
pub fn confirm_move(ids: Vec<GameId>, folder: LibraryFolder, window: &mut Window, cx: &mut App) {
    if ids.is_empty() {
        window.push_notification(
            Notification::info(t("ui.library_folders.nothing_to_move")),
            cx,
        );
        return;
    }
    let count = ids.len().to_string();
    let name = label(&folder);
    let title = t_args(
        if ids.len() == 1 {
            "ui.library_folders.move_title_one"
        } else {
            "ui.library_folders.move_title_other"
        },
        &[("count", &count), ("folder", &name)],
    );
    let description = t_args(
        "ui.library_folders.move_description",
        &[("path", &folder.path)],
    );
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (ids, folder) = (ids.clone(), folder.clone());
        alert
            .title(title.clone())
            .description(description.clone())
            .show_cancel(true)
            .cancel_text(t("ui.library_folders.cancel"))
            .ok_text(t("ui.library_folders.move_confirm"))
            .ok_variant(ButtonVariant::Primary)
            .on_ok(move |_, window, cx| {
                let (ids, folder) = (ids.clone(), folder.clone());
                // The dialog is still open here; act once it has closed.
                window.defer(cx, move |window, cx| run_move(ids, folder, window, cx));
                true
            })
    });
}

fn run_move(ids: Vec<GameId>, folder: LibraryFolder, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let name = label(&folder);
    let task = crate::state::run_infallible(cx, {
        let folder = folder.clone();
        async move { b.move_installs(&ids, &folder).await }
    });
    window
        .spawn(cx, async move |cx| {
            let Some(report) = task.await else { return };
            let _ = cx.update(|window, cx| {
                if report.moved > 0 {
                    let key = if report.moved == 1 {
                        "ui.library_folders.moved_one"
                    } else {
                        "ui.library_folders.moved_other"
                    };
                    window.push_notification(
                        Notification::success(t_args(
                            key,
                            &[("count", &report.moved.to_string()), ("folder", &name)],
                        )),
                        cx,
                    );
                }
                if !report.failed.is_empty() {
                    let lines: Vec<String> = report
                        .failed
                        .iter()
                        .map(|(title, e)| format!("{title}: {e}"))
                        .collect();
                    window.push_notification(
                        Notification::error(lines.join("\n"))
                            .title(t("ui.library_folders.move_failed")),
                        cx,
                    );
                }
            });
        })
        .detach();
}
