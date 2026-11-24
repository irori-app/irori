//! A game, or the Wine build it runs in, needs Rosetta 2 on an Apple silicon
//! Mac that doesn't have it. The dialog says that installing it accepts
//! Apple's licence and, when the user agrees, installs it through Apple's
//! `softwareupdate` (`Backend::install_rosetta`) and starts the game again.
//! Every place that starts a game reaches this through
//! [`crate::save_conflict::resolve_launch_error`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{App, Global, ParentElement as _, Styled as _, Window, div};
use irori_core::{GameId, RosettaMissing};
use irori_services::LaunchParams;

use crate::i18n::{t, t_args};
use crate::state::{backend, run};

/// Whether Rosetta is being installed, so Play pressed meanwhile doesn't
/// start a second `softwareupdate`.
#[derive(Default)]
struct Installing(bool);

impl Global for Installing {}

/// Handle a missing Rosetta in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    params: &LaunchParams,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(missing) = RosettaMissing::of(error).cloned() else {
        return false;
    };
    if cx.default_global::<Installing>().0 {
        notify_installing(window, cx);
        return true;
    }
    let width = window.rem_size() * 32.;
    let (id, params) = (id.clone(), params.clone());
    window.open_dialog(cx, move |dialog, _, cx| {
        let (id, params) = (id.clone(), params.clone());
        dialog
            .title(t("ui.rosetta.missing_title"))
            .w(width)
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .child(t_args("ui.rosetta.missing", &[("what", &missing.what)])),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("ui.rosetta.licence")),
                    ),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("rosetta-missing-cancel")
                            .ghost()
                            .label(t("ui.rosetta.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rosetta-missing-install")
                            .primary()
                            .label(t("ui.rosetta.install_and_play"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                install_then_play(id.clone(), params.clone(), window, cx);
                            }),
                    ),
            )
    });
    true
}

fn notify_installing(window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(t("ui.rosetta.installing_detail")).title(t("ui.rosetta.installing")),
        cx,
    );
}

/// Install Rosetta, then start the game.
fn install_then_play(id: GameId, params: LaunchParams, window: &mut Window, cx: &mut App) {
    cx.default_global::<Installing>().0 = true;
    notify_installing(window, cx);
    let b = backend(cx);
    let title = b
        .game(&id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let task = run(cx, async move { b.install_rosetta().await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| {
                cx.default_global::<Installing>().0 = false;
                match result {
                    Ok(()) => crate::save_conflict::play_again(id, params, &title, window, cx),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("ui.rosetta.install_failed")),
                        cx,
                    ),
                }
            });
        })
        .detach();
}
