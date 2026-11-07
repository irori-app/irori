//! The Sunshine panel of General → Integrations: whether Sunshine is on this
//! computer, the login of its web UI, syncing now, taking Irori's games out,
//! and how the last sync went.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_services::{SunshineLogin, SunshineReport, SunshineStatus};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{muted, panel};

/// Where Sunshine is published.
const SUNSHINE_URL: &str = "https://app.lizardbyte.dev/Sunshine/";

pub(crate) struct SunshinePanel {
    username: Entity<InputState>,
    password: Entity<InputState>,
    /// `None` until read off the UI thread.
    status: Option<SunshineStatus>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl SunshinePanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.sunshine.username")));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.sunshine.password"))
                .masked(true)
        });
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            // Syncs and the setting both reach the settings model.
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(
                &password,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_login(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            username,
            password,
            status: None,
            busy: false,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.sunshine_status().await });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn save_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = SunshineLogin {
            username: self.username.read(cx).value().trim().to_string(),
            password: self.password.read(cx).value().to_string(),
        };
        if login.username.is_empty() || login.password.is_empty() {
            return;
        }
        self.set_login(Some(login), window, cx);
    }

    fn set_login(
        &mut self,
        login: Option<SunshineLogin>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saving = login.is_some();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.set_sunshine_login(login).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) if saving => {
                        this.username
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        this.password
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        window.push_notification(
                            Notification::success(t("settings.sunshine.login_saved")),
                            cx,
                        );
                    }
                    Ok(()) => {}
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.sunshine.login_failed")),
                        cx,
                    ),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// Sync now, or with `remove` take Irori's games out of Sunshine.
    fn run(&mut self, remove: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            if remove {
                b.remove_from_sunshine().await
            } else {
                b.sync_sunshine().await
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(report) => window.push_notification(
                        Notification::success(summary(&report)).title(if remove {
                            t("settings.sunshine.removed")
                        } else {
                            t("settings.sunshine.synced")
                        }),
                        cx,
                    ),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.sunshine.sync_failed")),
                        cx,
                    ),
                }
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn render_found(&self, status: &SunshineStatus, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (icon, color, text) = match &status.install {
            Some(install) => (
                IconName::CircleCheck,
                theme.success,
                if install.flatpak {
                    t_args(
                        "settings.sunshine.found_flatpak",
                        &[("port", &status.port.to_string())],
                    )
                } else {
                    t_args(
                        "settings.sunshine.found",
                        &[("port", &status.port.to_string())],
                    )
                },
            ),
            None if status.program_installed => (
                IconName::Info,
                theme.muted_foreground,
                t("settings.sunshine.never_ran"),
            ),
            None => (
                IconName::Info,
                theme.muted_foreground,
                t("settings.sunshine.missing"),
            ),
        };
        let port = status.port;
        h_flex()
            .gap_2()
            .flex_wrap()
            .child(Icon::new(icon).small().text_color(color))
            .child(div().flex_1().min_w_0().text_sm().child(text))
            .when(status.install.is_some(), |row| {
                row.child(
                    Button::new("sunshine-open")
                        .ghost()
                        .small()
                        .icon(IconName::ExternalLink)
                        .label(t("settings.sunshine.open_web_ui"))
                        .on_click(move |_, _, cx| {
                            cx.open_url(&format!("https://localhost:{port}"))
                        }),
                )
            })
            .when(status.install.is_none(), |row| {
                row.child(
                    Button::new("sunshine-get")
                        .ghost()
                        .small()
                        .icon(IconName::ExternalLink)
                        .label(t("settings.sunshine.get"))
                        .on_click(|_, _, cx| cx.open_url(SUNSHINE_URL)),
                )
            })
            .into_any_element()
    }

    fn render_login(&self, status: &SunshineStatus, cx: &mut Context<Self>) -> AnyElement {
        match &status.username {
            Some(user) => h_flex()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().text_sm().child(t_args(
                    "settings.sunshine.login_saved_as",
                    &[("user", user)],
                )))
                .child(
                    Button::new("sunshine-forget-login")
                        .ghost()
                        .small()
                        .label(t("settings.sunshine.forget_login"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.set_login(None, window, cx)),
                        ),
                )
                .into_any_element(),
            None => v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(div().flex_1().min_w_32().child(Input::new(&self.username)))
                        .child(div().flex_1().min_w_32().child(Input::new(&self.password)))
                        .child(
                            Button::new("sunshine-save-login")
                                .outline()
                                .small()
                                .icon(IconName::LogIn)
                                .label(t("settings.sunshine.save_login"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_login(window, cx)),
                                ),
                        ),
                )
                .child(muted(t("settings.sunshine.login_hint"), cx))
                .into_any_element(),
        }
    }

    fn render_last(&self, report: &SunshineReport, cx: &mut Context<Self>) -> AnyElement {
        let when = report.local_time();
        if let Some(error) = &report.error {
            return Alert::error("sunshine-last-error", SharedString::from(error.clone()))
                .title(t_args("settings.sunshine.last_failed", &[("time", &when)]))
                .into_any_element();
        }
        v_flex()
            .gap_1()
            .child(muted(
                t_args(
                    "settings.sunshine.last_synced",
                    &[("time", &when), ("changes", &summary(report))],
                ),
                cx,
            ))
            .children(report.failed.iter().map(|line| {
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(line.clone())
            }))
            .into_any_element()
    }
}

/// "2 added, 1 updated, none removed".
fn summary(report: &SunshineReport) -> SharedString {
    t_args(
        "settings.sunshine.changes",
        &[
            ("added", &report.added.to_string()),
            ("updated", &report.updated.to_string()),
            ("removed", &report.removed.to_string()),
        ],
    )
}

impl Render for SunshinePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(status) = self.status.clone() else {
            return panel(cx)
                .p_4()
                .child(muted(t("settings.sunshine.looking"), cx));
        };
        let enabled = backend(cx).settings.app().sunshine.enabled;
        let busy = self.busy || status.syncing;
        let can_sync = status.username.is_some() && !busy;
        panel(cx)
            .p_4()
            .gap_3()
            .child(muted(t("settings.sunshine.description"), cx))
            .child(self.render_found(&status, cx))
            .child(self.render_login(&status, cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("sunshine-sync")
                            .outline()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(t("settings.sunshine.sync_now"))
                            .loading(busy)
                            .disabled(!can_sync || !enabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run(false, window, cx)),
                            ),
                    )
                    .when(status.applications > 0, |row| {
                        row.child(
                            Button::new("sunshine-remove")
                                .ghost()
                                .small()
                                .icon(IconName::Trash)
                                .label(t_args(
                                    "settings.sunshine.remove",
                                    &[("count", &status.applications.to_string())],
                                ))
                                .disabled(!can_sync)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.run(true, window, cx)),
                                ),
                        )
                    }),
            )
            .when_some(status.last.clone(), |body, report| {
                body.child(self.render_last(&report, cx))
            })
            .child(muted(t("settings.sunshine.quit_hint"), cx))
    }
}
