//! The computer itself from the quick menu: powering off, restarting,
//! sleeping and logging out, each after a question naming it. The actions
//! offered are the ones the system allows ([`irori_services::power`]),
//! asked again whenever the menu opens. Running games are stopped and their
//! sessions ended (saves synced) by the backend before it acts.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Context, SharedString, Window};
use irori_services::PowerAction;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::{BigPictureView, Overlay};

/// The question before `action`.
pub(super) fn power_title(action: PowerAction) -> SharedString {
    t(match action {
        PowerAction::PowerOff => "bigpicture.power.power_off_title",
        PowerAction::Restart => "bigpicture.power.restart_title",
        PowerAction::Sleep => "bigpicture.power.sleep_title",
        PowerAction::LogOut => "bigpicture.power.log_out_title",
    })
}

/// The title of the notification when `action` failed.
fn power_error(action: PowerAction) -> SharedString {
    t(match action {
        PowerAction::PowerOff => "bigpicture.error.power_off",
        PowerAction::Restart => "bigpicture.error.restart",
        PowerAction::Sleep => "bigpicture.error.sleep",
        PowerAction::LogOut => "bigpicture.error.log_out",
    })
}

impl BigPictureView {
    /// Ask the system again which power actions it allows.
    pub(super) fn refresh_power(&mut self, cx: &mut Context<Self>) {
        let b = irori_ui::backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.power_actions().await });
        cx.spawn(async move |this, cx| {
            let Some(actions) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.power != actions {
                    this.power = actions;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// What the question before `action` explains: what happens to open
    /// programs, and the games Irori stops first.
    pub(super) fn power_description(&self, action: PowerAction, cx: &App) -> SharedString {
        let mut text = String::from(t(match action {
            PowerAction::Sleep => "bigpicture.power.sleep_description",
            _ => "bigpicture.power.closes_programs",
        }));
        let m = models(cx);
        let playing: Vec<_> = m.statuses.read(cx).playing().cloned().collect();
        let running = match playing.as_slice() {
            [] => None,
            [id] => {
                let title = m
                    .library
                    .read(cx)
                    .snapshot
                    .get(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| id.app_name.to_string());
                Some(t_args(
                    "bigpicture.power.game_running",
                    &[("title", title.as_str())],
                ))
            }
            _ => Some(t("bigpicture.power.games_running")),
        };
        if let Some(running) = running {
            text.push('\n');
            text.push_str(&running);
        }
        text.into()
    }

    /// Take `action` once running games have ended, reporting a failure.
    pub(super) fn power(
        &mut self,
        action: PowerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.power_pending.is_some() {
            return;
        }
        self.power_pending = Some(action);
        cx.notify();
        let b = irori_ui::backend(cx);
        let task = irori_ui::run(cx, async move { b.power(action).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.power_pending = None;
                if matches!(this.overlay, Some(Overlay::Power { .. })) {
                    this.overlay = None;
                }
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(power_error(action)),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }
}
