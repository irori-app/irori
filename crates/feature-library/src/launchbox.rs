//! Downloading LaunchBox's game list on request from outside the art picker
//! (the command palette), with a notice when it starts, ends or fails. The
//! ROM library uses it for its covers; the picker offers its boxes.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Window};
use irori_emulation::LaunchBoxStatus;
use irori_ui::i18n::t;

/// Whether there is a ROM library with games whose box art LaunchBox gives.
pub fn offers_launchbox(cx: &App) -> bool {
    crate::emulation_store(cx).is_some_and(|s| {
        use irori_core::Store as _;
        s.launchbox_status() != LaunchBoxStatus::Downloading && !s.games().is_empty()
    })
}

/// Download LaunchBox's game list now and look the ROM library's games up
/// in it.
pub fn download_launchbox(window: &mut Window, cx: &mut App) {
    let Some(store) = crate::emulation_store(cx) else {
        return;
    };
    window.push_notification(
        Notification::info(t("library.art.launchbox.downloading")),
        cx,
    );
    let task = irori_ui::run(cx, async move { store.download_launchbox().await });
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
