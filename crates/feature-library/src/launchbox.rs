//! Downloading LaunchBox's game list on request, from the art picker or the
//! command palette. Asking for it turns `iroriLaunchBox` on (the picker's
//! text and the palette's question say so), so the ROM library keeps it up
//! to date weekly from then on; Settings → Advanced → Artwork turns it off.

use anyhow::Result;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Task, Window};
use irori_emulation::LaunchBoxStatus;
use irori_ui::i18n::t;

/// Whether there is a ROM library with games whose box art LaunchBox gives.
pub fn offers_launchbox(cx: &App) -> bool {
    crate::emulation_store(cx).is_some_and(|s| {
        use irori_core::Store as _;
        s.launchbox_status() != LaunchBoxStatus::Downloading && !s.games().is_empty()
    })
}

/// Turn `iroriLaunchBox` on and download the game list now.
pub(crate) fn turn_on_and_download(cx: &mut App) -> Option<Task<Result<()>>> {
    let store = crate::emulation_store(cx)?;
    let b = irori_ui::backend(cx);
    Some(irori_ui::run(cx, async move {
        b.settings.update_app(|s| s.launchbox = true);
        store.download_launchbox().await
    }))
}

/// The palette's command: download LaunchBox's game list, asking first
/// while `iroriLaunchBox` is off, since that turns it on.
pub fn download_launchbox(window: &mut Window, cx: &mut App) {
    if irori_ui::models(cx).settings.read(cx).app.launchbox {
        return start(window, cx);
    }
    window.open_alert_dialog(cx, |alert, _, _| {
        alert
            .title(t("library.launchbox.confirm_title"))
            .description(t("library.art.launchbox.intro"))
            .show_cancel(true)
            .cancel_text(t("library.common.cancel"))
            .ok_text(t("library.art.launchbox.download"))
            .ok_variant(ButtonVariant::Primary)
            .on_ok(|_, window, cx| {
                // The dialog is still open here; start once it has closed.
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, cx| start(window, cx));
                });
                true
            })
    });
}

fn start(window: &mut Window, cx: &mut App) {
    let Some(task) = turn_on_and_download(cx) else {
        return;
    };
    window.push_notification(
        Notification::info(t("library.art.launchbox.downloading")),
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            cx.update(|window, cx| {
                let notice = match result {
                    Ok(()) => Notification::success(t("library.launchbox.done")),
                    Err(e) => {
                        Notification::error(format!("{e:#}")).title(t("library.launchbox.failed"))
                    }
                };
                window.push_notification(notice, cx);
            })
            .ok();
        })
        .detach();
}
