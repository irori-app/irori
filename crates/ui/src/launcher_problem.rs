//! A game that starts through another company's launcher which isn't in its
//! Wine prefix yet (a Rockstar title bought on Epic needs the Rockstar Games
//! Launcher). The dialog offers to run the vendor's installer in that prefix,
//! then starts the game. Every place that starts a game reaches this through
//! [`crate::save_conflict::resolve_launch_error`].

use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{App, Global, ParentElement as _, Styled as _, Window, div};
use irori_core::process::CancellationToken;
use irori_core::{GameId, LauncherMissing};
use irori_services::LaunchParams;

use crate::i18n::{t, t_args};
use crate::state::{backend, run};

/// Games whose launcher is being installed, by [`GameId::key`], so Play
/// pressed meanwhile doesn't start a second installer.
#[derive(Default)]
struct Installing(HashSet<String>);

impl Global for Installing {}

/// Handle a missing launcher in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    params: &LaunchParams,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(missing) = LauncherMissing::of(error).cloned() else {
        return false;
    };
    if cx.default_global::<Installing>().0.contains(&id.key()) {
        notify_installing(&missing.launcher, window, cx);
        return true;
    }
    let title = backend(cx)
        .game(id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let width = window.rem_size() * 32.;
    let (id, params) = (id.clone(), params.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let (id, params, missing) = (id.clone(), params.clone(), missing.clone());
        let launcher = missing.launcher.clone();
        dialog
            .title(t_args(
                "ui.launcher.missing_title",
                &[("launcher", &launcher)],
            ))
            .w(width)
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().child(t_args(
                        "ui.launcher.missing",
                        &[("title", &title), ("launcher", &launcher)],
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t_args("ui.launcher.account", &[("launcher", &launcher)])),
                    ),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("launcher-missing-cancel")
                            .ghost()
                            .label(t("ui.launcher.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("launcher-missing-install")
                            .primary()
                            .label(t("ui.launcher.install_and_play"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                install_then_play(
                                    id.clone(),
                                    params.clone(),
                                    missing.clone(),
                                    window,
                                    cx,
                                );
                            }),
                    ),
            )
    });
    true
}

fn notify_installing(launcher: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(t_args(
            "ui.launcher.installing_detail",
            &[("launcher", launcher)],
        ))
        .title(t_args("ui.launcher.installing", &[("launcher", launcher)])),
        cx,
    );
}

/// Run the launcher's installer in the game's prefix, then start the game.
fn install_then_play(
    id: GameId,
    params: LaunchParams,
    missing: LauncherMissing,
    window: &mut Window,
    cx: &mut App,
) {
    cx.default_global::<Installing>().0.insert(id.key());
    notify_installing(&missing.launcher, window, cx);
    let b = backend(cx);
    let (game, fix) = (id.clone(), missing.clone());
    let task = run(cx, async move {
        b.install_launcher_for(&game, &fix, CancellationToken::new())
            .await?;
        b.launch_with(&game, params).await
    });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                cx.default_global::<Installing>().0.remove(&id.key());
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t_args(
                            "ui.launcher.install_failed",
                            &[("launcher", &missing.launcher)],
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
}
