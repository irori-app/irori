//! A Microsoft Store game that can't start: its Wine build lacks
//! `xgameruntime.dll` (the Wine manager offers the Nexus Runtime), or
//! Microsoft refused this device because the account's Store device limit
//! is reached. The dialog says why and leads to the fix. Every place that
//! starts a game reaches this through
//! [`crate::save_conflict::resolve_launch_error`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{App, ParentElement as _, Styled as _, Window, div};
use irori_core::{GameId, XboxProblem, XboxUnavailable};

use crate::i18n::{t, t_args};
use crate::nav::{Route, navigate};
use crate::state::backend;

/// Microsoft's page for unlinking devices.
const DEVICES_URL: &str = "https://account.microsoft.com/devices/content";

/// Handle a Microsoft Store problem in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(problem) = XboxUnavailable::of(error).map(|e| e.problem) else {
        return false;
    };
    let title = backend(cx)
        .game(id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let width = window.rem_size() * 32.;
    let id = id.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (heading, body, hint) = match problem {
            XboxProblem::RuntimeMissing => (
                t("ui.xbox.runtime_title"),
                t_args("ui.xbox.runtime_body", &[("title", &title)]),
                t("ui.xbox.runtime_hint"),
            ),
            XboxProblem::DeviceLimit => (
                t("ui.xbox.device_limit_title"),
                t_args("ui.xbox.device_limit_body", &[("title", &title)]),
                t("ui.xbox.device_limit_hint"),
            ),
        };
        let primary = match problem {
            XboxProblem::RuntimeMissing => Button::new("xbox-problem-wine")
                .primary()
                .label(t("ui.xbox.open_wine_manager"))
                .on_click(|_, window, cx| {
                    window.close_dialog(cx);
                    navigate(Route::WineManager, cx);
                }),
            XboxProblem::DeviceLimit => Button::new("xbox-problem-devices")
                .primary()
                .label(t("ui.xbox.open_devices"))
                .on_click(|_, window, cx| {
                    window.close_dialog(cx);
                    if let Some(url) = crate::web_url(DEVICES_URL)
                        && let Err(e) = open::that_detached(&url)
                    {
                        window.push_notification(
                            Notification::error(format!("{url}: {e}"))
                                .title(t("ui.backups.open_link_failed")),
                            cx,
                        );
                    }
                }),
        };
        let id = id.clone();
        let mut footer = DialogFooter::new().child(
            Button::new("xbox-problem-close")
                .ghost()
                .label(t("ui.sandbox.close"))
                .on_click(|_, window, cx| window.close_dialog(cx)),
        );
        if problem == XboxProblem::RuntimeMissing {
            footer = footer.child(
                Button::new("xbox-problem-settings")
                    .outline()
                    .label(t("ui.xbox.game_settings"))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        navigate(Route::GameSettings(id.clone(), "wine".into()), cx);
                    }),
            );
        }
        dialog
            .title(heading)
            .w(width)
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().child(body))
                    .child(div().text_xs().text_color(muted).child(hint)),
            )
            .footer(footer.child(primary))
    });
    true
}
