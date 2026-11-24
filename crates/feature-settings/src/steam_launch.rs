//! A Steam game's launch options in Steam itself (Launch section): the line
//! Steam starts the game with (`VAR=value wrapper %command% arguments`),
//! read from and written to the account's `localconfig.vdf` through
//! `irori_steam::SteamStore`.
//!
//! Steam rewrites that file when it exits, so a new line is saved only
//! while Steam is closed. When it runs, Irori asks before closing it, and
//! never closes it in Steam's Game Mode.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_steam::{LaunchOptionsState, SteamRunning, SteamStore};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::widgets::muted;

enum Load {
    Loading,
    Ready(LaunchOptionsState),
    Failed(SharedString),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Saving,
    Closing,
}

pub(crate) struct SteamLaunchPanel {
    id: GameId,
    steam: Arc<SteamStore>,
    load: Load,
    busy: Busy,
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// What saving `typed` means: `None` when it is what Steam has already,
/// else the line to write (`Some(None)` clears it).
fn change(current: Option<&str>, typed: &str) -> Option<Option<String>> {
    let typed = typed.trim();
    let wanted = (!typed.is_empty()).then(|| typed.to_string());
    (wanted.as_deref() != current).then_some(wanted)
}

impl SteamLaunchPanel {
    pub(crate) fn new(
        id: GameId,
        steam: Arc<SteamStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.steam_launch.placeholder"))
        });
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.save_typed(window, cx);
            }
        });
        let mut this = Self {
            id,
            steam,
            load: Load::Loading,
            busy: Busy::Idle,
            input,
            _subscriptions: vec![sub],
        };
        this.reload(window, cx);
        this
    }

    fn title(&self, cx: &Context<Self>) -> String {
        backend(cx)
            .game(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string())
    }

    fn current(&self) -> Option<&str> {
        match &self.load {
            Load::Ready(state) => state.line.as_deref(),
            _ => None,
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let steam = self.steam.clone();
        let app = self.id.app_name.to_string();
        let task = irori_ui::run(cx, async move { steam.launch_options(&app).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(state) => {
                        let line = state.line.clone().unwrap_or_default();
                        this.input.update(cx, |s, cx| s.set_value(line, window, cx));
                        this.load = Load::Ready(state);
                    }
                    Err(e) => this.load = Load::Failed(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.input.read(cx).value().to_string();
        self.choose(typed, window, cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.choose(String::new(), window, cx);
    }

    fn choose(&mut self, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(state) = &self.load else {
            return;
        };
        if self.busy != Busy::Idle {
            return;
        }
        let Some(line) = change(state.line.as_deref(), &typed) else {
            return;
        };
        if state.steam_running {
            self.ask_to_close_steam(line, window, cx);
        } else {
            self.save(line, false, window, cx);
        }
    }

    fn ask_to_close_steam(
        &mut self,
        line: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_close = matches!(&self.load, Load::Ready(s) if s.can_close_steam);
        if !can_close {
            window.push_notification(
                Notification::error(t("settings.steam_launch.session"))
                    .title(t("settings.steam_launch.save_failed")),
                cx,
            );
            return;
        }
        let title = self.title(cx);
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let line = line.clone();
            alert
                .title(t_args(
                    "settings.steam_launch.close.title",
                    &[("title", &title)],
                ))
                .description(t("settings.steam_launch.close.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.steam_launch.close.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let line = line.clone();
                    panel
                        .update(cx, |p, cx| p.save(line, true, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// Save `line`, closing Steam first when `close_steam`.
    fn save(
        &mut self,
        line: Option<String>,
        close_steam: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = if close_steam {
            Busy::Closing
        } else {
            Busy::Saving
        };
        cx.notify();
        let steam = self.steam.clone();
        let app = self.id.app_name.to_string();
        let chosen = line.clone();
        let task = irori_ui::run(cx, async move {
            if close_steam {
                steam.close_steam().await?;
            }
            steam.set_launch_options(&app, chosen).await
        });
        let title = self.title(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = Busy::Idle;
                match result {
                    Ok(()) => {
                        if let Load::Ready(state) = &mut this.load {
                            state.line = line.clone();
                            state.steam_running = false;
                        }
                        let shown = line.clone().unwrap_or_default();
                        this.input
                            .update(cx, |s, cx| s.set_value(shown, window, cx));
                        let key = if line.is_some() {
                            "settings.steam_launch.saved"
                        } else {
                            "settings.steam_launch.cleared"
                        };
                        window.push_notification(
                            Notification::success(t_args(key, &[("title", &title)])),
                            cx,
                        );
                    }
                    Err(e) if e.downcast_ref::<SteamRunning>().is_some() && !close_steam => {
                        // Steam started since the panel looked.
                        if let Load::Ready(state) = &mut this.load {
                            state.steam_running = true;
                        }
                        this.ask_to_close_steam(line, window, cx);
                    }
                    Err(e) => {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.steam_launch.save_failed")),
                            cx,
                        );
                        if close_steam {
                            this.reload(window, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The line shown, once read.
    #[cfg(test)]
    pub(crate) fn state_for_test(&self) -> Option<&LaunchOptionsState> {
        match &self.load {
            Load::Ready(state) => Some(state),
            _ => None,
        }
    }
}

impl Render for SteamLaunchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match &self.load {
            Load::Loading => {
                return v_flex().child(muted(t("settings.steam_launch.loading"), cx));
            }
            Load::Failed(error) => {
                return v_flex().child(
                    Alert::error("steam-launch-error", error.clone())
                        .title(t("settings.steam_launch.load_failed")),
                );
            }
            Load::Ready(state) => state,
        };
        let note = match self.busy {
            Busy::Closing => Some(t("settings.steam_launch.closing")),
            Busy::Saving => Some(t("settings.steam_launch.saving")),
            Busy::Idle if state.steam_running && !state.can_close_steam => {
                Some(t("settings.steam_launch.session"))
            }
            Busy::Idle if state.steam_running => Some(t("settings.steam_launch.running")),
            Busy::Idle => None,
        };
        let busy = self.busy != Busy::Idle;
        let has_line = self.current().is_some();
        v_flex()
            .gap_2()
            .child(muted(t("settings.steam_launch.help"), cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(div().flex_1().min_w_64().child(Input::new(&self.input)))
                    .child(
                        Button::new("steam-launch-save")
                            .outline()
                            .small()
                            .icon(IconName::Save)
                            .label(t("settings.steam_launch.save"))
                            .loading(busy)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save_typed(window, cx)),
                            ),
                    )
                    .when(has_line, |row| {
                        row.child(
                            Button::new("steam-launch-clear")
                                .ghost()
                                .small()
                                .icon(IconName::Trash)
                                .label(t("settings.steam_launch.clear"))
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.clear(window, cx)),
                                ),
                        )
                    }),
            )
            .when_some(note, |body, note| body.child(muted(note, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_changes_only_what_differs() {
        assert_eq!(change(Some("-novid"), " -novid "), None);
        assert_eq!(change(None, "   "), None);
        assert_eq!(change(Some("-novid"), ""), Some(None));
        assert_eq!(
            change(None, "%command% -x"),
            Some(Some("%command% -x".to_string()))
        );
    }
}
