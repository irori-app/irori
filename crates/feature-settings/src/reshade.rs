//! ReShade for one game, in the game's Tools section: install (asking first
//! when the game has anticheat), turn on or off, update and remove, with the
//! install's steps and download progress shown in place.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString,
    Styled as _, Window, div,
};
use irori_core::GameId;
use irori_core::process::CancellationToken;
use irori_reshade::{Manifest, ReshadeError, Step};
use irori_services::{ReshadeStatus, supports_reshade};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::widgets::{RowState, field_row, group_panel, muted};

enum Load {
    Idle,
    Loading,
    Ready(ReshadeStatus),
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Installing,
    Toggling,
    Removing,
}

/// What a running install reports.
enum Report {
    Step(Step),
    Percent(Option<f32>),
}

/// The ReShade rows of one game.
pub(crate) struct ReshadePanel {
    id: GameId,
    load: Load,
    busy: Option<Busy>,
    step: Option<Step>,
    percent: Option<f32>,
    cancel: Option<CancellationToken>,
}

/// The message for a failed operation, in the interface language.
fn error_text(e: &anyhow::Error) -> SharedString {
    match ReshadeError::of(e) {
        Some(ReshadeError::Banned { exe }) => {
            t_args("settings.reshade.error.banned", &[("program", exe)])
        }
        Some(ReshadeError::NoApi) => t("settings.reshade.error.no_api"),
        Some(ReshadeError::Unsupported(api)) => t_args(
            "settings.reshade.error.unsupported",
            &[("api", api.label())],
        ),
        Some(ReshadeError::UnsupportedArch) => t("settings.reshade.error.arch"),
        Some(ReshadeError::DllTaken { name }) => {
            t_args("settings.reshade.error.dll_taken", &[("file", name)])
        }
        Some(ReshadeError::OutsideGame) => t("settings.reshade.error.outside"),
        None => format!("{e:#}").into(),
    }
}

fn step_text(step: Step) -> SharedString {
    t(match step {
        Step::Checking => "settings.reshade.step.checking",
        Step::DownloadingReshade => "settings.reshade.step.reshade",
        Step::DownloadingShaders => "settings.reshade.step.shaders",
        Step::DownloadingCompiler => "settings.reshade.step.compiler",
        Step::Writing => "settings.reshade.step.writing",
    })
}

impl ReshadePanel {
    pub(crate) fn new(id: GameId) -> Self {
        Self {
            id,
            load: Load::Idle,
            busy: None,
            step: None,
            percent: None,
            cancel: None,
        }
    }

    fn applies(&self, cx: &Context<Self>) -> bool {
        backend(cx)
            .game(&self.id)
            .is_some_and(|g| supports_reshade(&g))
    }

    fn title(&self, cx: &Context<Self>) -> String {
        backend(cx)
            .game(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string())
    }

    /// Read the install state the first time the panel is shown.
    pub(crate) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.load, Load::Idle) && self.applies(cx) {
            self.reload(window, cx);
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load = Load::Loading;
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move { b.reshade_status(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.load = match result {
                    Ok(status) => Load::Ready(status),
                    Err(e) => Load::Failed(error_text(&e)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn status(&self) -> Option<&ReshadeStatus> {
        match &self.load {
            Load::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// Install, after asking when the game has anticheat.
    fn ask_install(
        &mut self,
        chosen: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anticheat = self
            .status()
            .map(|s| s.anticheat.join(", "))
            .unwrap_or_default();
        let installed = self.status().is_some_and(|s| s.manifest.is_some());
        if anticheat.is_empty() || installed {
            self.install(chosen, window, cx);
            return;
        }
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let chosen = chosen.clone();
            alert
                .title(t_args(
                    "settings.reshade.anticheat.title",
                    &[("title", &title)],
                ))
                .description(t_args(
                    "settings.reshade.anticheat.description",
                    &[("title", &title), ("anticheat", &anticheat)],
                ))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.reshade.anticheat.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.install(chosen.clone(), window, cx))
                        .ok();
                    true
                })
        });
    }

    fn choose_program(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.reshade.choose_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(exe) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    this.ask_install(Some(exe), window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn install(&mut self, chosen: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Installing);
        self.step = None;
        self.percent = None;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Report>();
        let step_tx = tx.clone();
        let step: Arc<dyn Fn(Step) + Send + Sync> = Arc::new(move |s| {
            let _ = step_tx.send(Report::Step(s));
        });
        let progress: irori_core::install::ProgressSink = Arc::new(move |p| {
            let _ = tx.send(Report::Percent(p.percent));
        });
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move {
            b.install_reshade(&id, chosen, step, progress, cancel).await
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Some(report) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    match report {
                        Report::Step(s) => {
                            this.step = Some(s);
                            this.percent = None;
                        }
                        Report::Percent(p) => this.percent = p,
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let title = self.title(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.cancel = None;
                this.step = None;
                match result {
                    Ok(_) => window.push_notification(
                        Notification::success(t_args(
                            "settings.reshade.installed_notice",
                            &[("title", &title)],
                        )),
                        cx,
                    ),
                    Err(e) if irori_runtime::is_cancelled(&e) => window
                        .push_notification(Notification::info(t("settings.reshade.cancelled")), cx),
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e))
                            .title(t("settings.reshade.install_failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_enabled(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Toggling);
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move { b.set_reshade_enabled(&id, enabled).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.reshade.failed")),
                        cx,
                    );
                }
                this.reload(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t_args(
                    "settings.reshade.remove.title",
                    &[("title", &title)],
                ))
                .description(t("settings.reshade.remove.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.reshade.remove.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.remove(window, cx)).ok();
                    true
                })
        });
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Removing);
        let b = backend(cx);
        let id = self.id.clone();
        let title = self.title(cx);
        let task = irori_ui::run(cx, async move { b.remove_reshade(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(report) => {
                        let mut note = Notification::success(t_args(
                            "settings.reshade.removed_notice",
                            &[("title", &title)],
                        ));
                        if !report.kept.is_empty() {
                            let files = report
                                .kept
                                .iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect::<Vec<_>>()
                                .join(", ");
                            let count = report.kept.len().to_string();
                            let key = if report.kept.len() == 1 {
                                "settings.reshade.kept_one"
                            } else {
                                "settings.reshade.kept_other"
                            };
                            note = Notification::success(t_args(
                                key,
                                &[("count", &count), ("files", &files)],
                            ))
                            .title(t_args(
                                "settings.reshade.removed_notice",
                                &[("title", &title)],
                            ));
                        }
                        window.push_notification(note, cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.reshade.failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_installing(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut text = self
            .step
            .map(step_text)
            .unwrap_or_else(|| t("settings.reshade.step.checking"))
            .to_string();
        if let Some(p) = self.percent {
            text = format!("{text} {}%", p.round() as u32);
        }
        let row = RowState {
            label: t("settings.reshade.label"),
            help: text.into(),
            ..Default::default()
        };
        let cancel = Button::new("reshade-cancel")
            .outline()
            .label(t("settings.common.cancel"))
            .disabled(self.cancel.as_ref().is_none_or(|c| c.is_cancelled()))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(c) = &this.cancel {
                    c.cancel();
                }
                cx.notify();
            }));
        let control = h_flex()
            .gap_2()
            .child(Button::new("reshade-installing").ghost().loading(true))
            .child(cancel)
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }

    fn render_installed(
        &self,
        status: &ReshadeStatus,
        manifest: &Manifest,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.busy.is_some();
        let program = manifest
            .exe
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut help = t_args(
            "settings.reshade.installed_help",
            &[
                ("dll", &manifest.dll),
                ("api", manifest.api.label()),
                ("program", &program),
            ],
        )
        .to_string();
        if !manifest.overrides.is_empty() {
            let dlls = manifest
                .overrides
                .iter()
                .map(|d| format!("{d}.dll"))
                .collect::<Vec<_>>()
                .join(", ");
            help.push('\n');
            help.push_str(&t_args("settings.reshade.wine_note", &[("dlls", &dlls)]));
        }
        let incomplete = !manifest.complete;
        let label = t_args(
            "settings.reshade.installed_label",
            &[("version", &manifest.version)],
        );
        let row = RowState {
            label: label.clone(),
            help: help.into(),
            error: incomplete.then(|| t("settings.reshade.incomplete")),
            ..Default::default()
        };
        let toggle = Switch::new("reshade-enabled")
            .checked(manifest.enabled)
            .disabled(busy || incomplete)
            .accessibility_label(t("settings.reshade.enabled"))
            .on_change(
                cx.listener(|this, on: &bool, window, cx| this.set_enabled(*on, window, cx)),
            );
        let update = status
            .update_available()
            .then(|| status.latest.clone())
            .flatten()
            .filter(|_| !incomplete);
        let control = h_flex()
            .gap_2()
            .items_center()
            .when_some(update, |row, latest| {
                row.child(
                    Button::new("reshade-update")
                        .outline()
                        .small()
                        .label(t_args("settings.reshade.update", &[("version", &latest)]))
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.install(None, window, cx)),
                        ),
                )
            })
            .child(
                Button::new("reshade-remove")
                    .ghost()
                    .small()
                    .label(t("settings.common.remove"))
                    .loading(self.busy == Some(Busy::Removing))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_remove(window, cx))),
            )
            .when(!incomplete, |row| row.child(toggle))
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }

    fn render_not_installed(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let row = RowState {
            label: t("settings.reshade.label"),
            help: t("settings.reshade.help"),
            ..Default::default()
        };
        let control = h_flex()
            .gap_2()
            .child(
                Button::new("reshade-choose")
                    .ghost()
                    .small()
                    .label(t("settings.reshade.choose_program"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.choose_program(window, cx))),
            )
            .child(
                Button::new("reshade-install")
                    .outline()
                    .small()
                    .label(t("settings.reshade.install"))
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.ask_install(None, window, cx)),
                    ),
            )
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }
}

impl Render for ReshadePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.applies(cx) {
            return div()
                .py_3()
                .child(muted(t("settings.reshade.unsupported"), cx))
                .into_any_element();
        }
        let body: AnyElement = if self.busy == Some(Busy::Installing) {
            self.render_installing(cx)
        } else {
            match &self.load {
                Load::Idle | Load::Loading => div()
                    .px_4()
                    .py_3()
                    .child(muted(t("settings.reshade.checking"), cx))
                    .into_any_element(),
                Load::Failed(e) => {
                    let row = RowState {
                        label: t("settings.reshade.label"),
                        error: Some(e.clone()),
                        ..Default::default()
                    };
                    let retry = Button::new("reshade-retry")
                        .outline()
                        .small()
                        .label(t("settings.common.try_again"))
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx)))
                        .into_any_element();
                    field_row(row, retry, cx).into_any_element()
                }
                Load::Ready(status) => match &status.manifest {
                    Some(m) => self.render_installed(status, m, cx),
                    None => self.render_not_installed(cx),
                },
            }
        };
        v_flex()
            .pt_2()
            .child(group_panel(cx).child(body))
            .into_any_element()
    }
}
