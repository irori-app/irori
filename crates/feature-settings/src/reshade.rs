//! ReShade for one game, in the game's Tools section: install (asking first
//! when the game has anticheat, and before taking over a ReShade installed
//! by hand), turn on or off, update and remove, with the install's steps and
//! download progress shown in place. Removing ReShade removes the RenoDX mod
//! it loads too, so the RenoDX panel is told.
//!
//! Steam starts its games itself, so a Steam game under Proton loads
//! ReShade only when Steam's launch options set `WINEDLLOVERRIDES`. After
//! each change the panel brings that line in step when Steam is closed, and
//! otherwise offers to, asking before it closes Steam.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, EventEmitter, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Window, div,
};
use irori_core::GameId;
use irori_core::process::CancellationToken;
use irori_reshade::{GraphicsMod, Manifest, ReshadeError, Step};
use irori_services::{ReshadeStatus, reshade_needs_steam_overrides, supports_reshade};
use irori_steam::{LaunchOptionsState, SteamRunning, SteamStore};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::graphics_mods::{
    self, GraphicsModsChanged, Report, progress_channel, removed_notification, step_text,
};
use crate::widgets::{RowState, field_row, group_panel, muted};

enum Load {
    Idle,
    Loading,
    Ready(Box<ReshadeStatus>),
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Installing,
    Toggling,
    Removing,
}

/// The ReShade rows of one game.
pub(crate) struct ReshadePanel {
    id: GameId,
    load: Load,
    busy: Option<Busy>,
    step: Option<Step>,
    percent: Option<f32>,
    cancel: Option<CancellationToken>,
    /// A Steam game under Proton: Steam's launch options, once read.
    steam: Option<SteamLine>,
}

/// Steam's launch options of a game whose ReShade needs them.
struct SteamLine {
    store: Arc<SteamStore>,
    state: Option<LaunchOptionsState>,
    /// The DLLs a ReShade that was removed had Steam load, to take out.
    removed: Vec<String>,
    /// Bring the line in step on the next read when Steam is closed.
    follow: bool,
    saving: bool,
}

/// The launch line ReShade's install wants: its DLLs loaded from the game
/// folder while it is on and complete, nothing about them otherwise (or
/// about those of a ReShade just removed). `None` when the line is right.
fn wanted_line(
    current: Option<&str>,
    manifest: Option<&Manifest>,
    removed: &[String],
) -> Option<Option<String>> {
    let line = current.unwrap_or_default();
    let on = manifest.is_some_and(|m| m.enabled && m.complete);
    let ours: Vec<&str> = manifest
        .map(|m| m.overrides.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let unset: Vec<&str> = ours
        .iter()
        .copied()
        .chain(removed.iter().map(String::as_str))
        .collect();
    let set: &[&str] = if on { &ours } else { &[] };
    let next = irori_steam::with_dll_overrides(line, set, &unset);
    (next != line.trim()).then(|| (!next.is_empty()).then_some(next))
}

fn error_text(e: &anyhow::Error) -> SharedString {
    graphics_mods::error_text(e, GraphicsMod::Reshade)
}

impl EventEmitter<GraphicsModsChanged> for ReshadePanel {}

impl ReshadePanel {
    pub(crate) fn new(id: GameId, cx: &mut Context<Self>) -> Self {
        let steam = backend(cx)
            .game(&id)
            .filter(|g| reshade_needs_steam_overrides(g))
            .and_then(|_| crate::stores::steam(cx))
            .map(|store| SteamLine {
                store,
                state: None,
                removed: Vec::new(),
                follow: false,
                saving: false,
            });
        Self {
            id,
            load: Load::Idle,
            busy: None,
            step: None,
            percent: None,
            cancel: None,
            steam,
        }
    }

    /// Read Steam's launch options, then write the line ReShade wants when
    /// a change asked to and Steam is closed.
    fn read_steam_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(steam) = &self.steam else { return };
        let store = steam.store.clone();
        let app = self.id.app_name.to_string();
        let task = irori_ui::run(cx, async move { store.launch_options(&app).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(steam) = &mut this.steam else { return };
                match result {
                    Ok(state) => steam.state = Some(state),
                    Err(e) => tracing::info!("Steam's launch options: {e:#}"),
                }
                let follow = std::mem::take(&mut steam.follow);
                let closed = steam.state.as_ref().is_some_and(|s| !s.steam_running);
                if follow && closed && this.steam_change().is_some() {
                    this.write_steam_line(false, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The line to write into Steam, when it differs from Steam's.
    fn steam_change(&self) -> Option<Option<String>> {
        let steam = self.steam.as_ref()?;
        let state = steam.state.as_ref()?;
        let manifest = self.status().and_then(|s| s.manifest.as_ref());
        wanted_line(state.line.as_deref(), manifest, &steam.removed)
    }

    /// Update Steam's launch options, asking to close Steam when it runs.
    fn update_steam_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.steam.as_ref().and_then(|s| s.state.as_ref()) else {
            return;
        };
        if !state.steam_running {
            self.write_steam_line(false, window, cx);
            return;
        }
        if !state.can_close_steam {
            window.push_notification(
                Notification::error(t("settings.steam_launch.session"))
                    .title(t("settings.steam_launch.save_failed")),
                cx,
            );
            return;
        }
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
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
                    this.update(cx, |this, cx| this.write_steam_line(true, window, cx))
                        .ok();
                    true
                })
        });
    }

    fn write_steam_line(&mut self, close_steam: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line) = self.steam_change() else {
            return;
        };
        let Some(steam) = &mut self.steam else { return };
        if steam.saving {
            return;
        }
        steam.saving = true;
        let store = steam.store.clone();
        let app = self.id.app_name.to_string();
        let task = irori_ui::run(cx, async move {
            if close_steam {
                store.close_steam().await?;
            }
            store.set_launch_options(&app, line).await
        });
        let title = self.title(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(steam) = &mut this.steam {
                    steam.saving = false;
                    if result.is_ok() {
                        steam.removed.clear();
                    }
                }
                match result {
                    Ok(()) => window.push_notification(
                        Notification::success(t_args(
                            "settings.reshade.steam.updated",
                            &[("title", &title)],
                        )),
                        cx,
                    ),
                    // Steam started meanwhile: the row offers it again.
                    Err(e) if e.downcast_ref::<SteamRunning>().is_some() => {}
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.steam_launch.save_failed")),
                        cx,
                    ),
                }
                this.read_steam_line(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// After a change to ReShade, follow it in Steam's launch options.
    fn follow_in_steam(&mut self) {
        if let Some(steam) = &mut self.steam {
            steam.follow = true;
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

    /// Read the install state again when it was read before: another
    /// graphics mod changed it.
    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.load, Load::Idle) && self.busy.is_none() {
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
            this.update_in(cx, |this, window, cx| {
                this.load = match result {
                    Ok(status) => Load::Ready(Box::new(status)),
                    Err(e) => Load::Failed(error_text(&e)),
                };
                this.read_steam_line(window, cx);
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
            self.install(chosen, false, window, cx);
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
                    this.update(cx, |this, cx| {
                        this.install(chosen.clone(), false, window, cx)
                    })
                    .ok();
                    true
                })
        });
    }

    /// The game folder has a ReShade installed without Irori: offer to take
    /// it over rather than refuse.
    fn ask_adopt(&mut self, chosen: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let chosen = chosen.clone();
            alert
                .title(t_args("settings.reshade.adopt.title", &[("title", &title)]))
                .description(t("settings.reshade.adopt.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.reshade.adopt.confirm"))
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| {
                        this.install(chosen.clone(), true, window, cx)
                    })
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

    fn install(
        &mut self,
        chosen: Option<PathBuf>,
        adopt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let asked = chosen.clone();
        self.busy = Some(Busy::Installing);
        self.step = None;
        self.percent = None;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (step, progress, mut rx) = progress_channel();
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move {
            b.install_reshade(&id, chosen, adopt, step, progress, cancel)
                .await
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
                this.follow_in_steam();
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
                    Err(e)
                        if !adopt
                            && matches!(
                                ReshadeError::of(&e),
                                Some(ReshadeError::ManualInstall { .. })
                            ) =>
                    {
                        this.ask_adopt(asked.clone(), window, cx)
                    }
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e))
                            .title(t("settings.reshade.install_failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
                cx.emit(GraphicsModsChanged);
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
                this.follow_in_steam();
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.reshade.failed")),
                        cx,
                    );
                }
                this.reload(window, cx);
                cx.emit(GraphicsModsChanged);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn confirm_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let description = if self.status().is_some_and(|s| s.renodx.is_some()) {
            t("settings.reshade.remove.description_renodx")
        } else {
            t("settings.reshade.remove.description")
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t_args(
                    "settings.reshade.remove.title",
                    &[("title", &title)],
                ))
                .description(description.clone())
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
        // The record goes with ReShade; its DLLs leave Steam's line after.
        let overrides = self
            .status()
            .and_then(|s| s.manifest.as_ref())
            .map(|m| m.overrides.clone());
        if let Some(steam) = &mut self.steam
            && let Some(overrides) = overrides
        {
            steam.removed = overrides;
        }
        let b = backend(cx);
        let id = self.id.clone();
        let title = self.title(cx);
        let task = irori_ui::run(cx, async move { b.remove_reshade(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.follow_in_steam();
                match result {
                    Ok(report) => {
                        let removed =
                            t_args("settings.reshade.removed_notice", &[("title", &title)]);
                        window.push_notification(removed_notification(removed, &report), cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.reshade.failed")),
                        cx,
                    ),
                }
                this.reload(window, cx);
                cx.emit(GraphicsModsChanged);
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
        if manifest.addon {
            help.push('\n');
            help.push_str(&t("settings.reshade.addon_note"));
        }
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
                            cx.listener(|this, _, window, cx| {
                                this.install(None, false, window, cx)
                            }),
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

    /// Whether Steam's launch options load ReShade, with a way to fix them.
    fn render_steam_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let steam = self.steam.as_ref()?;
        let change = self.steam_change()?;
        let state = steam.state.as_ref()?;
        let help = match &change {
            Some(line) => t_args("settings.reshade.steam.needs", &[("line", line)]),
            None => t("settings.reshade.steam.clear"),
        };
        let row = RowState {
            label: t("settings.field.panel_steam_launch.label"),
            help,
            disabled_reason: (state.steam_running && !state.can_close_steam)
                .then(|| t("settings.steam_launch.session")),
            ..Default::default()
        };
        let button = Button::new("reshade-steam-line")
            .outline()
            .small()
            .label(t("settings.reshade.steam.update"))
            .loading(steam.saving)
            .disabled(steam.saving || self.busy.is_some())
            .on_click(cx.listener(|this, _, window, cx| this.update_steam_line(window, cx)))
            .into_any_element();
        Some(field_row(row, button, cx).into_any_element())
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
        let steam_line = if self.busy.is_none() {
            self.render_steam_line(cx)
        } else {
            None
        };
        v_flex()
            .pt_2()
            .child(group_panel(cx).child(body).children(steam_line))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(enabled: bool) -> Manifest {
        serde_json::from_value(serde_json::json!({
            "version": "6.8.0", "exe": "/g/Game.exe", "machine": "x64", "api": "dxgi",
            "dll": "dxgi.dll", "dir": "/g", "enabled": enabled, "complete": true,
            "overrides": ["dxgi", "d3dcompiler_47"], "files": []
        }))
        .unwrap()
    }

    #[test]
    fn steams_line_follows_reshade() {
        let on = manifest(true);
        let line = wanted_line(Some("-novid"), Some(&on), &[])
            .unwrap()
            .unwrap();
        assert_eq!(
            line,
            "WINEDLLOVERRIDES='dxgi=n,b;d3dcompiler_47=n,b' %command% -novid"
        );
        assert_eq!(wanted_line(Some(&line), Some(&on), &[]), None);
        // Turned off: the overrides leave, the rest of the line stays.
        assert_eq!(
            wanted_line(Some(&line), Some(&manifest(false)), &[]),
            Some(Some("-novid".to_string()))
        );
        // Removed: the remembered DLLs leave; an empty line clears it.
        let removed = vec!["dxgi".to_string(), "d3dcompiler_47".to_string()];
        let only = "WINEDLLOVERRIDES='dxgi=n,b;d3dcompiler_47=n,b' %command%";
        assert_eq!(wanted_line(Some(only), None, &removed), Some(None));
        assert_eq!(wanted_line(None, None, &[]), None);
    }
}
