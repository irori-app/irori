//! Reading the Steam library from the Steam client, without a Steam sign-in
//! in Irori (`irori_steam::SteamStore::read_client_library`): the question
//! that turns it on, the restart of Steam it may need, and the part of
//! Steam's account card that shows and changes it.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div,
};
use irori_steam::{ClientLibraryError, ClientLibraryMode, ClientLibraryState, SteamStore};
use irori_ui::i18n::{t, t_args};

use crate::common::{notify_error, notify_success};

struct SteamHandle(Arc<SteamStore>);
impl Global for SteamHandle {}

/// Whether a read of the Steam client runs; views observe it.
#[derive(Default)]
pub(crate) struct SteamLibraryOps {
    pub reading: bool,
}

struct OpsHandle(Entity<SteamLibraryOps>);
impl Global for OpsHandle {}

/// Give the management screens the Steam store (the backend exposes
/// stores only as `dyn Store`), for reading the library from the Steam
/// client.
pub fn install_steam_store(store: Arc<SteamStore>, cx: &mut App) {
    cx.set_global(SteamHandle(store));
}

fn steam_store(cx: &App) -> Option<Arc<SteamStore>> {
    cx.try_global::<SteamHandle>().map(|h| h.0.clone())
}

pub(crate) fn steam_library_ops(cx: &mut App) -> Entity<SteamLibraryOps> {
    if let Some(h) = cx.try_global::<OpsHandle>() {
        return h.0.clone();
    }
    let e = cx.new(|_| SteamLibraryOps::default());
    cx.set_global(OpsHandle(e.clone()));
    e
}

/// Where reading the Steam client stands; `None` without the Steam store.
pub fn steam_client_library_state(cx: &App) -> Option<ClientLibraryState> {
    steam_store(cx).map(|s| s.client_library_state())
}

/// Whether the library should suggest reading the Steam client.
pub fn suggest_steam_client_library(cx: &App) -> bool {
    steam_client_library_state(cx).is_some_and(|s| s.suggest())
}

/// Put the library's suggestion away for good.
pub fn dismiss_steam_client_library_hint(cx: &mut App) {
    let Some(steam) = steam_store(cx) else {
        return;
    };
    let task = irori_ui::run(cx, async move { steam.dismiss_client_library_hint().await });
    cx.spawn(async move |_| {
        if let Err(e) = task.await {
            tracing::warn!("putting the Steam library suggestion away: {e:#}");
        }
    })
    .detach();
}

/// Explain what reading the Steam client involves and let the user turn it
/// on: once (the default) or kept on.
pub fn open_steam_library_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    if steam_store(cx).is_none() {
        return;
    }
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let muted = cx.theme().muted_foreground;
        let choice = |label: SharedString, help: SharedString| {
            v_flex()
                .gap_0p5()
                .child(div().font_medium().child(label))
                .child(div().text_sm().text_color(muted).child(help))
                .into_any_element()
        };
        alert
            .title(t("manage.steam_library.dialog.title"))
            .description(
                v_flex()
                    .gap_3()
                    .child(div().child(t("manage.steam_library.dialog.what")))
                    .child(div().child(t("manage.steam_library.dialog.how")))
                    .child(
                        div()
                            .text_color(cx.theme().warning)
                            .child(t("manage.steam_library.dialog.risk")),
                    )
                    .child(choice(
                        t("manage.steam_library.dialog.once"),
                        t("manage.steam_library.dialog.once_help"),
                    ))
                    .child(choice(
                        t("manage.steam_library.dialog.keep"),
                        t("manage.steam_library.dialog.keep_help"),
                    )),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("steam-library-cancel")
                            .label(t("manage.steam_library.dialog.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("steam-library-keep")
                            .outline()
                            .label(t("manage.steam_library.dialog.keep"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                start(ClientLibraryMode::Keep, false, window, cx);
                            }),
                    )
                    .child(
                        Button::new("steam-library-once")
                            .primary()
                            .label(t("manage.steam_library.dialog.once"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                start(ClientLibraryMode::Once, false, window, cx);
                            }),
                    ),
            )
    });
}

/// Turn reading on in `mode` and read now, restarting Steam when `restart`.
fn start(mode: ClientLibraryMode, restart: bool, window: &mut Window, cx: &mut App) {
    let Some(steam) = steam_store(cx) else {
        return;
    };
    let ops = steam_library_ops(cx);
    if ops.read(cx).reading {
        return;
    }
    ops.update(cx, |o, cx| {
        o.reading = true;
        cx.notify();
    });
    let task = irori_ui::run(cx, async move {
        if steam.client_library_state().mode != mode || !steam.client_library_state().marker {
            steam.enable_client_library(mode).await?;
        }
        steam.read_client_library(restart).await
    });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = handle.update(cx, |_, window, cx| {
            ops.update(cx, |o, cx| {
                o.reading = false;
                cx.notify();
            });
            match result {
                Ok(count) => {
                    let message = match count {
                        1 => t("manage.steam_library.read_done_one"),
                        n => t_args(
                            "manage.steam_library.read_done_other",
                            &[("count", &n.to_string())],
                        ),
                    };
                    notify_success(message, window, cx);
                }
                Err(e) => match e.downcast_ref::<ClientLibraryError>() {
                    Some(ClientLibraryError::DebuggingOff { millennium: false }) if !restart => {
                        ask_to_restart(mode, window, cx)
                    }
                    Some(kind) => notify_error(
                        t("manage.steam_library.read_failed"),
                        &anyhow::anyhow!(error_text(kind)),
                        window,
                        cx,
                    ),
                    None => notify_error(t("manage.steam_library.read_failed"), &e, window, cx),
                },
            }
        });
    })
    .detach();
}

/// Steam runs without remote debugging: ask before restarting it.
fn ask_to_restart(mode: ClientLibraryMode, window: &mut Window, cx: &mut App) {
    let can_restart = steam_client_library_state(cx).is_some_and(|s| s.can_restart_steam);
    if !can_restart {
        notify_error(
            t("manage.steam_library.read_failed"),
            &anyhow::anyhow!(error_text(&ClientLibraryError::SteamSession)),
            window,
            cx,
        );
        return;
    }
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title(t("manage.steam_library.restart.title"))
            .description(t("manage.steam_library.restart.description"))
            .show_cancel(true)
            .cancel_text(t("manage.steam_library.dialog.cancel"))
            .ok_text(t("manage.steam_library.restart.ok"))
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                // The dialog is still open here; start once it has closed.
                window.defer(cx, move |window, cx| start(mode, true, window, cx));
                true
            })
    });
}

/// Switch reading off: remove the file Irori created.
fn switch_off(window: &mut Window, cx: &mut App) {
    let Some(steam) = steam_store(cx) else {
        return;
    };
    let removes = steam.client_library_state().own_marker;
    let task = irori_ui::run(cx, async move { steam.disable_client_library().await });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = handle.update(cx, |_, window, cx| match result {
            Ok(()) if removes => notify_success(t("manage.steam_library.switched_off"), window, cx),
            Ok(()) => {}
            Err(e) => notify_error(t("manage.steam_library.switch_off_failed"), &e, window, cx),
        });
    })
    .detach();
}

/// Forget the games read from the Steam client.
fn forget(window: &mut Window, cx: &mut App) {
    let Some(steam) = steam_store(cx) else {
        return;
    };
    let task = irori_ui::run(cx, async move { steam.forget_client_library().await });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        if let Err(e) = task.await {
            let _ = handle.update(cx, |_, window, cx| {
                notify_error(t("manage.steam_library.forget_failed"), &e, window, cx)
            });
        }
    })
    .detach();
}

/// The user's words for why the client couldn't be read.
pub(crate) fn error_text(error: &ClientLibraryError) -> SharedString {
    match error {
        ClientLibraryError::NotRunning => t("manage.steam_library.error.not_running"),
        ClientLibraryError::DebuggingOff { millennium: false } => {
            t("manage.steam_library.error.debugging_off")
        }
        ClientLibraryError::DebuggingOff { millennium: true } => {
            t("manage.steam_library.error.millennium")
        }
        ClientLibraryError::NotSteam { port } => t_args(
            "manage.steam_library.error.not_steam",
            &[("port", &port.to_string())],
        ),
        ClientLibraryError::Starting => t("manage.steam_library.error.starting"),
        ClientLibraryError::Unsupported => t("manage.steam_library.error.unsupported"),
        ClientLibraryError::SteamSession => t("manage.steam_library.error.session"),
    }
}

/// What the Steam card says about reading the client, for `state`.
pub(crate) fn status_text(state: &ClientLibraryState, now: u64) -> SharedString {
    if let Some(at) = state.read_at {
        let when = crate::format::relative_time(at, now);
        return match state.games {
            1 => t_args("manage.steam_library.games_one", &[("when", &when)]),
            n => t_args(
                "manage.steam_library.games_other",
                &[("count", &n.to_string()), ("when", &when)],
            ),
        };
    }
    match state.mode {
        ClientLibraryMode::Off => t("manage.steam_library.hint"),
        _ => t("manage.steam_library.waiting"),
    }
}

/// The part of Steam's account card about reading the client, shown while
/// no Steam account is signed in to Irori.
pub(crate) fn render_section(state: &ClientLibraryState, reading: bool, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let now = irori_core::util::unix_now();
    let on = state.mode != ClientLibraryMode::Off;
    let read_label = if state.read_at.is_some() {
        t("manage.steam_library.read_again")
    } else {
        t("manage.steam_library.read")
    };
    let mode = if on {
        state.mode
    } else {
        ClientLibraryMode::Once
    };
    let read = Button::new("steam-library-read")
        .debug_selector(|| "steam-library-read".into())
        .small()
        .outline()
        .icon(IconName::LibraryBig)
        .label(read_label)
        .loading(reading)
        .disabled(reading)
        .on_click(move |_, window, cx| {
            // The first time, explain before anything changes.
            if on {
                start(mode, false, window, cx)
            } else {
                open_steam_library_dialog(window, cx)
            }
        });
    let switch_off_button = (on || state.own_marker).then(|| {
        Button::new("steam-library-off")
            .debug_selector(|| "steam-library-off".into())
            .small()
            .ghost()
            .icon(IconName::PowerOff)
            .label(t("manage.steam_library.switch_off"))
            .disabled(reading)
            .on_click(|_, window, cx| switch_off(window, cx))
    });
    let forget_button = (state.read_at.is_some() && !on).then(|| {
        Button::new("steam-library-forget")
            .debug_selector(|| "steam-library-forget".into())
            .small()
            .ghost()
            .icon(IconName::Trash)
            .label(t("manage.steam_library.forget"))
            .disabled(reading)
            .on_click(|_, window, cx| forget(window, cx))
    });
    v_flex()
        .gap_2()
        .pt_3()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .child(status_text(state, now)),
        )
        .when(state.mode == ClientLibraryMode::Keep, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.steam_library.kept_on")),
            )
        })
        .when(state.marker && !state.own_marker, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("manage.steam_library.other_program")),
            )
        })
        .child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(read)
                .children(switch_off_button)
                .children(forget_button),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_reading_stands() {
        crate::i18n::register();
        let never = ClientLibraryState::default();
        assert_eq!(status_text(&never, 0), t("manage.steam_library.hint"));
        let waiting = ClientLibraryState {
            mode: ClientLibraryMode::Once,
            ..Default::default()
        };
        assert_eq!(status_text(&waiting, 0), t("manage.steam_library.waiting"));
        let read = ClientLibraryState {
            games: 12,
            read_at: Some(100),
            ..Default::default()
        };
        assert!(status_text(&read, 130).contains("12"));
        for e in [
            ClientLibraryError::NotRunning,
            ClientLibraryError::DebuggingOff { millennium: true },
            ClientLibraryError::NotSteam { port: 8080 },
            ClientLibraryError::Starting,
            ClientLibraryError::Unsupported,
            ClientLibraryError::SteamSession,
        ] {
            assert!(!error_text(&e).starts_with("manage."), "{e:?}");
        }
        assert!(error_text(&ClientLibraryError::NotSteam { port: 8080 }).contains("8080"));
    }
}
