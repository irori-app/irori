//! OptiScaler for one game, in the game's Tools section below RenoDX:
//! pick release channel (Stable or Nightly), GPU driver mode (Mesa Auto with
//! FakeNVAPI or NVIDIA), proxy file name, and OptiPatcher toggle, inspect or
//! choose the game executable and folder, install (asking first when the game
//! has anticheat), toggle on or off, update, and remove.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::process::CancellationToken;
use irori_core::{GameId, Runner};
use irori_reshade::optiscaler::{GpuDriverMode, OptiscalerChannel, OptiscalerConfig, ProxyDll};
use irori_reshade::optiscaler_keys::vk_display_name;
use irori_reshade::{GraphicsMod, Manifest, Step};
use irori_services::{OptiscalerInstallParams, OptiscalerStatus, supports_reshade};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::form::ChoiceItem;
use crate::graphics_mods::{
    self, GraphicsModsChanged, Report, progress_channel, removed_notification, step_text,
};
use crate::i18n::LanguageStamp;
use crate::optiscaler_key::{KeyCaptured, OptiscalerKeyCapture};
use crate::widgets::{RowState, field_row, group_panel, muted};

const CHANNEL_STABLE: &str = "stable";
const CHANNEL_NIGHTLY: &str = "nightly";
const DRIVER_MESA_AUTO: &str = "mesa_auto";
const DRIVER_NVIDIA: &str = "nvidia";

enum Load {
    Idle,
    Loading,
    Ready(Box<OptiscalerStatus>),
    Failed(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Installing,
    Toggling,
    Removing,
}

/// Cached UI labels in the current interface language, refreshed only when
/// [`LanguageStamp`] reports a language change.
struct Labels {
    label: SharedString,
    help: SharedString,
    unsupported: SharedString,
    checking: SharedString,
    install: SharedString,
    apply: SharedString,
    enabled: SharedString,
    incomplete: SharedString,
    remove: SharedString,
    cancel: SharedString,
    try_again: SharedString,
    step_checking: SharedString,
    channel_label: SharedString,
    channel_help: SharedString,
    channel_stable: SharedString,
    channel_nightly: SharedString,
    driver_label: SharedString,
    driver_help: SharedString,
    driver_mesa_auto: SharedString,
    driver_nvidia: SharedString,
    proxy_label: SharedString,
    proxy_help: SharedString,
    optipatcher_label: SharedString,
    optipatcher_help: SharedString,
    target_label: SharedString,
    target_auto: SharedString,
    choose_program: SharedString,
    choose_prompt: SharedString,
    open_folder: SharedString,
    shortcut_label: SharedString,
    shortcut_help: SharedString,
}

impl Labels {
    fn load() -> Self {
        Self {
            label: t("settings.optiscaler.label"),
            help: t("settings.optiscaler.help"),
            unsupported: t("settings.optiscaler.unsupported"),
            checking: t("settings.optiscaler.checking"),
            install: t("settings.optiscaler.install"),
            apply: t("settings.optiscaler.apply"),
            enabled: t("settings.optiscaler.enabled"),
            incomplete: t("settings.optiscaler.incomplete"),
            remove: t("settings.common.remove"),
            cancel: t("settings.common.cancel"),
            try_again: t("settings.common.try_again"),
            step_checking: t("settings.reshade.step.checking"),
            channel_label: t("settings.optiscaler.channel.label"),
            channel_help: t("settings.optiscaler.channel.help"),
            channel_stable: t("settings.optiscaler.channel.stable"),
            channel_nightly: t("settings.optiscaler.channel.nightly"),
            driver_label: t("settings.optiscaler.gpu_driver.label"),
            driver_help: t("settings.optiscaler.gpu_driver.help"),
            driver_mesa_auto: t("settings.optiscaler.gpu_driver.mesa_auto"),
            driver_nvidia: t("settings.optiscaler.gpu_driver.nvidia"),
            proxy_label: t("settings.optiscaler.proxy_dll.label"),
            proxy_help: t("settings.optiscaler.proxy_dll.help"),
            optipatcher_label: t("settings.optiscaler.optipatcher.label"),
            optipatcher_help: t("settings.optiscaler.optipatcher.help"),
            target_label: t("settings.optiscaler.target.label"),
            target_auto: t("settings.optiscaler.target.auto"),
            choose_program: t("settings.optiscaler.choose_program"),
            choose_prompt: t("settings.optiscaler.choose_prompt"),
            open_folder: t("settings.optiscaler.open_folder"),
            shortcut_label: t("settings.optiscaler.shortcut.label"),
            shortcut_help: t("settings.optiscaler.shortcut.help"),
        }
    }
}

fn error_text(e: &anyhow::Error) -> SharedString {
    graphics_mods::error_text(e, GraphicsMod::Optiscaler)
}

fn channel_key(channel: OptiscalerChannel) -> &'static str {
    match channel {
        OptiscalerChannel::Stable => CHANNEL_STABLE,
        OptiscalerChannel::Nightly => CHANNEL_NIGHTLY,
    }
}

fn channel_from_key(key: &str) -> OptiscalerChannel {
    match key {
        CHANNEL_NIGHTLY => OptiscalerChannel::Nightly,
        _ => OptiscalerChannel::Stable,
    }
}

fn driver_key(driver: GpuDriverMode) -> &'static str {
    match driver {
        GpuDriverMode::MesaAuto => DRIVER_MESA_AUTO,
        GpuDriverMode::Nvidia => DRIVER_NVIDIA,
    }
}

fn driver_from_key(key: &str) -> GpuDriverMode {
    match key {
        DRIVER_NVIDIA => GpuDriverMode::Nvidia,
        _ => GpuDriverMode::MesaAuto,
    }
}

fn channel_items(labels: &Labels) -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::new(
            SharedString::new_static(CHANNEL_STABLE),
            labels.channel_stable.clone(),
        ),
        ChoiceItem::new(
            SharedString::new_static(CHANNEL_NIGHTLY),
            labels.channel_nightly.clone(),
        ),
    ]
}

fn driver_items(labels: &Labels) -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::new(
            SharedString::new_static(DRIVER_MESA_AUTO),
            labels.driver_mesa_auto.clone(),
        ),
        ChoiceItem::new(
            SharedString::new_static(DRIVER_NVIDIA),
            labels.driver_nvidia.clone(),
        ),
    ]
}

fn proxy_items() -> Vec<ChoiceItem> {
    ProxyDll::ALL
        .iter()
        .map(|dll| {
            let name = SharedString::new_static(dll.file_name());
            ChoiceItem::new(name.clone(), name)
        })
        .collect()
}

/// Summary text under the installed OptiScaler row.
pub(crate) fn installed_help(
    status: &OptiscalerStatus,
    manifest: &Manifest,
    runner: Runner,
) -> String {
    let program = manifest
        .exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shortcut = manifest
        .optiscaler
        .as_ref()
        .and_then(|c| c.shortcut_key.as_deref())
        .unwrap_or("");
    let key_name = vk_display_name(shortcut);
    let mut lines = vec![
        t_args(
            "settings.optiscaler.installed_help",
            &[
                ("dll", &manifest.dll),
                ("program", &program),
                ("key", key_name.as_ref()),
            ],
        )
        .to_string(),
    ];
    if !cfg!(target_os = "windows") && !manifest.overrides.is_empty() {
        if runner == Runner::Steam {
            lines.push(t("settings.optiscaler.steam_note").to_string());
        } else {
            let dlls = manifest
                .overrides
                .iter()
                .map(|d| format!("{d}.dll"))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(t_args("settings.optiscaler.wine_note", &[("dlls", &dlls)]).to_string());
        }
    }
    if cfg!(target_os = "linux")
        && runner != Runner::Steam
        && manifest
            .optiscaler
            .as_ref()
            .is_some_and(|c| c.gpu_driver == GpuDriverMode::Nvidia)
    {
        lines.push(t("settings.optiscaler.nvidia_note").to_string());
    }
    if let Some(error) = &status.catalog_error {
        lines.push(t_args("settings.optiscaler.catalog_failed", &[("error", error)]).to_string());
    }
    lines.join("\n")
}

/// Summary of the executable and folder OptiScaler targets.
pub(crate) fn target_summary(
    status: &OptiscalerStatus,
    chosen_exe: Option<&Path>,
    auto_label: &SharedString,
) -> SharedString {
    let exe = chosen_exe.or_else(|| status.target_exe());
    let Some(exe) = exe else {
        return auto_label.clone();
    };
    let program = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| exe.display().to_string());
    let folder = chosen_exe
        .and_then(Path::parent)
        .or_else(|| status.target_dir())
        .or_else(|| exe.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    t_args(
        "settings.optiscaler.target.detected",
        &[("program", &program), ("folder", &folder)],
    )
}

/// Whether the user changed options or chose another executable compared to
/// the currently installed manifest.
pub(crate) fn has_pending_changes(
    status: &OptiscalerStatus,
    config: &OptiscalerConfig,
    chosen_exe: Option<&Path>,
) -> bool {
    let Some(manifest) = &status.manifest else {
        return false;
    };
    let config_changed = status
        .installed_config()
        .is_some_and(|installed| installed != config);
    let exe_changed = chosen_exe.is_some_and(|exe| exe != manifest.exe.as_path());
    config_changed || exe_changed
}

/// The OptiScaler rows of one game.
pub(crate) struct OptiscalerPanel {
    id: GameId,
    load: Load,
    load_gen: u64,
    busy: Option<Busy>,
    step: Option<Step>,
    percent: Option<f32>,
    cancel: Option<CancellationToken>,
    config: OptiscalerConfig,
    config_initialized: bool,
    chosen_exe: Option<PathBuf>,
    channel_select: Entity<SelectState<Vec<ChoiceItem>>>,
    driver_select: Entity<SelectState<Vec<ChoiceItem>>>,
    proxy_select: Entity<SelectState<Vec<ChoiceItem>>>,
    key_capture: Entity<OptiscalerKeyCapture>,
    labels: Labels,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GraphicsModsChanged> for OptiscalerPanel {}

impl OptiscalerPanel {
    pub(crate) fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let labels = Labels::load();
        let config = OptiscalerConfig::default();

        let channel_select = cx.new(|cx| {
            let mut s = SelectState::new(channel_items(&labels), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(channel_key(config.channel)),
                window,
                cx,
            );
            s
        });
        let driver_select = cx.new(|cx| {
            let mut s = SelectState::new(driver_items(&labels), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(driver_key(config.gpu_driver)),
                window,
                cx,
            );
            s
        });
        let proxy_select = cx.new(|cx| {
            let mut s = SelectState::new(proxy_items(), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(config.proxy_dll.file_name()),
                window,
                cx,
            );
            s
        });
        let key_capture = cx.new(|cx| {
            OptiscalerKeyCapture::new("optiscaler-game-shortcut", config.shortcut_key.clone(), cx)
        });

        let sub_channel = cx.subscribe_in(
            &channel_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                let SelectEvent::Confirm(key) = event;
                if let Some(key) = key {
                    let next = channel_from_key(key.as_ref());
                    if this.config.channel != next {
                        this.config.channel = next;
                        this.config_initialized = true;
                        if !matches!(this.load, Load::Idle) && this.busy.is_none() {
                            this.reload(window, cx);
                        } else {
                            cx.notify();
                        }
                    }
                }
            },
        );
        let sub_driver = cx.subscribe_in(
            &driver_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(key) = event;
                if let Some(key) = key {
                    this.config.gpu_driver = driver_from_key(key.as_ref());
                    this.config_initialized = true;
                    cx.notify();
                }
            },
        );
        let sub_proxy = cx.subscribe_in(
            &proxy_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(key) = event;
                if let Some(key) = key
                    && let Some(dll) = ProxyDll::from_file_name(key.as_ref())
                {
                    this.config.proxy_dll = dll;
                    this.config_initialized = true;
                    cx.notify();
                }
            },
        );
        let sub_key = cx.subscribe_in(
            &key_capture,
            window,
            |this, _, event: &KeyCaptured, _, cx| {
                if this.config.shortcut_key != event.0 {
                    this.config.shortcut_key = event.0.clone();
                    this.config_initialized = true;
                    cx.notify();
                }
            },
        );

        Self {
            id,
            load: Load::Idle,
            load_gen: 0,
            busy: None,
            step: None,
            percent: None,
            cancel: None,
            config,
            config_initialized: false,
            chosen_exe: None,
            channel_select,
            driver_select,
            proxy_select,
            key_capture,
            labels,
            language: LanguageStamp::new(),
            _subscriptions: vec![sub_channel, sub_driver, sub_proxy, sub_key],
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

    fn sync_selects(&self, window: &mut Window, cx: &mut Context<Self>) {
        let channel = SharedString::new_static(channel_key(self.config.channel));
        let driver = SharedString::new_static(driver_key(self.config.gpu_driver));
        let proxy = SharedString::new_static(self.config.proxy_dll.file_name());
        self.channel_select
            .update(cx, |s, cx| s.set_selected_value(&channel, window, cx));
        self.driver_select
            .update(cx, |s, cx| s.set_selected_value(&driver, window, cx));
        self.proxy_select
            .update(cx, |s, cx| s.set_selected_value(&proxy, window, cx));
        let shortcut = self.config.shortcut_key.clone();
        self.key_capture
            .update(cx, |k, cx| k.set_value(shortcut, cx));
    }

    fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.labels = Labels::load();
        let ch_items = channel_items(&self.labels);
        let dr_items = driver_items(&self.labels);
        let channel = SharedString::new_static(channel_key(self.config.channel));
        let driver = SharedString::new_static(driver_key(self.config.gpu_driver));
        self.channel_select.update(cx, |s, cx| {
            s.set_items(ch_items, window, cx);
            s.set_selected_value(&channel, window, cx);
        });
        self.driver_select.update(cx, |s, cx| {
            s.set_items(dr_items, window, cx);
            s.set_selected_value(&driver, window, cx);
        });
        cx.notify();
    }

    /// Refresh only the local manifest without hitting GitHub or rescanning
    /// the game directory. Used after Apply, Enable/Disable, and Remove.
    fn reload_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prev = match &self.load {
            Load::Ready(s) => Some((**s).clone()),
            _ => None,
        };
        if prev.is_none() {
            self.reload(window, cx);
            return;
        }
        self.load_gen = self.load_gen.wrapping_add(1);
        let generation = self.load_gen;
        let b = backend(cx);
        let id = self.id.clone();
        let channel = self.config.channel;
        let task = irori_ui::run(cx, async move {
            b.optiscaler_local_status(&id, channel, prev).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _, cx| {
                if this.load_gen != generation {
                    return;
                }
                match result {
                    Ok(status) => this.load = Load::Ready(Box::new(status)),
                    Err(e) => this.load = Load::Failed(error_text(&e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load = Load::Loading;
        self.load_gen = self.load_gen.wrapping_add(1);
        let generation = self.load_gen;
        let b = backend(cx);
        let id = self.id.clone();
        let channel = self.config.channel;
        let task = irori_ui::run(cx, async move { b.optiscaler_status(&id, channel).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.load_gen != generation {
                    return;
                }
                match result {
                    Ok(status) => {
                        if !this.config_initialized {
                            this.config_initialized = true;
                            if let Some(installed) = status.installed_config().cloned() {
                                let channel_differs = installed.channel != status.channel;
                                this.config = installed;
                                this.sync_selects(window, cx);
                                if channel_differs {
                                    this.reload(window, cx);
                                    return;
                                }
                            }
                        }
                        this.load = Load::Ready(Box::new(status));
                    }
                    Err(e) => this.load = Load::Failed(error_text(&e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn status(&self) -> Option<&OptiscalerStatus> {
        match &self.load {
            Load::Ready(s) => Some(s),
            _ => None,
        }
    }

    fn target_dir(&self, status: &OptiscalerStatus, cx: &Context<Self>) -> Option<PathBuf> {
        self.chosen_exe
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| status.target_dir().map(Path::to_path_buf))
            .or_else(|| backend(cx).game(&self.id).and_then(|g| g.install_path()))
    }

    /// Install, after asking when the game has anticheat.
    fn ask_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anticheat = self
            .status()
            .map(|s| s.anticheat.join(", "))
            .unwrap_or_default();
        let installed = self.status().is_some_and(|s| s.manifest.is_some());
        if anticheat.is_empty() || installed {
            self.install(window, cx);
            return;
        }
        let title = self.title(cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t_args(
                    "settings.optiscaler.anticheat.title",
                    &[("title", &title)],
                ))
                .description(t_args(
                    "settings.optiscaler.anticheat.description",
                    &[("title", &title), ("anticheat", &anticheat)],
                ))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.optiscaler.anticheat.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.install(window, cx)).ok();
                    true
                })
        });
    }

    fn choose_program(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.labels.choose_prompt.clone()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(exe) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.chosen_exe = Some(exe);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_change(true, window, cx);
    }

    fn apply_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_change(false, window, cx);
    }

    fn set_busy(&mut self, busy: Option<Busy>, cx: &mut Context<Self>) {
        self.busy = busy;
        let disabled = busy.is_some();
        self.key_capture
            .update(cx, |k, cx| k.set_disabled(disabled, cx));
    }

    fn run_change(&mut self, update_version: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.set_busy(Some(Busy::Installing), cx);
        self.step = None;
        self.percent = None;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (step, progress, mut rx) = progress_channel();
        let b = backend(cx);
        let id = self.id.clone();
        let params = OptiscalerInstallParams {
            config: self.config.clone(),
            chosen_exe: self.chosen_exe.clone(),
        };
        let task = irori_ui::run(cx, async move {
            if update_version {
                b.install_optiscaler(&id, params, step, progress, cancel)
                    .await
            } else {
                b.configure_optiscaler(&id, params, step, progress, cancel)
                    .await
            }
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
                this.set_busy(None, cx);
                this.cancel = None;
                this.step = None;
                match result {
                    Ok(_) => {
                        this.chosen_exe = None;
                        window.push_notification(
                            Notification::success(t_args(
                                "settings.optiscaler.installed_notice",
                                &[("title", &title)],
                            )),
                            cx,
                        );
                    }
                    Err(e) if irori_runtime::is_cancelled(&e) => window.push_notification(
                        Notification::info(t("settings.optiscaler.cancelled")),
                        cx,
                    ),
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e))
                            .title(t("settings.optiscaler.install_failed")),
                        cx,
                    ),
                }
                if update_version {
                    this.reload(window, cx);
                } else {
                    this.reload_local(window, cx);
                }
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
        self.set_busy(Some(Busy::Toggling), cx);
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(
            cx,
            async move { b.set_optiscaler_enabled(&id, enabled).await },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.set_busy(None, cx);
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.optiscaler.failed")),
                        cx,
                    );
                }
                this.reload_local(window, cx);
                cx.emit(GraphicsModsChanged);
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
                    "settings.optiscaler.remove.title",
                    &[("title", &title)],
                ))
                .description(t("settings.optiscaler.remove.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.optiscaler.remove.confirm"))
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
        self.set_busy(Some(Busy::Removing), cx);
        let b = backend(cx);
        let id = self.id.clone();
        let title = self.title(cx);
        let task = irori_ui::run(cx, async move { b.remove_optiscaler(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.set_busy(None, cx);
                match result {
                    Ok(report) => {
                        this.chosen_exe = None;
                        let removed =
                            t_args("settings.optiscaler.removed_notice", &[("title", &title)]);
                        window.push_notification(removed_notification(removed, &report), cx);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(error_text(&e)).title(t("settings.optiscaler.failed")),
                        cx,
                    ),
                }
                this.reload_local(window, cx);
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
            .unwrap_or_else(|| self.labels.step_checking.clone())
            .to_string();
        if let Some(p) = self.percent {
            text = format!("{text} {}%", p.round() as u32);
        }
        let row = RowState {
            label: self.labels.label.clone(),
            help: text.into(),
            ..Default::default()
        };
        let cancel = Button::new("optiscaler-cancel")
            .outline()
            .label(self.labels.cancel.clone())
            .disabled(self.cancel.as_ref().is_none_or(|c| c.is_cancelled()))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(c) = &this.cancel {
                    c.cancel();
                }
                cx.notify();
            }));
        let control = h_flex()
            .gap_2()
            .child(Button::new("optiscaler-installing").ghost().loading(true))
            .child(cancel)
            .into_any_element();
        field_row(row, control, cx).into_any_element()
    }

    fn render_status_row(&self, status: &OptiscalerStatus, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        match &status.manifest {
            Some(manifest) => {
                let incomplete = !manifest.complete;
                let row = RowState {
                    label: t_args(
                        "settings.optiscaler.installed_label",
                        &[("version", &manifest.version)],
                    ),
                    help: installed_help(status, manifest, self.id.runner).into(),
                    error: incomplete.then(|| self.labels.incomplete.clone()),
                    ..Default::default()
                };
                let toggle =
                    Switch::new("optiscaler-enabled")
                        .checked(manifest.enabled)
                        .disabled(busy || incomplete)
                        .accessibility_label(self.labels.enabled.clone())
                        .on_change(cx.listener(|this, on: &bool, window, cx| {
                            this.set_enabled(*on, window, cx)
                        }));
                let channel_changed = manifest
                    .optiscaler
                    .as_ref()
                    .is_some_and(|installed| installed.channel != self.config.channel);
                let update = status
                    .update_available()
                    .then(|| status.latest.as_ref().map(|r| r.version.clone()))
                    .flatten()
                    .filter(|_| !incomplete);
                // Show Apply even when an update is available so changing
                // options never silently upgrades the installed OptiScaler version.
                let pending = !incomplete
                    && !channel_changed
                    && has_pending_changes(status, &self.config, self.chosen_exe.as_deref());
                let control =
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(pending, |row| {
                            row.child(
                                Button::new("optiscaler-apply")
                                    .outline()
                                    .small()
                                    .label(self.labels.apply.clone())
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_config(window, cx)
                                    })),
                            )
                        })
                        .when_some(update, |row, latest| {
                            row.child(
                                Button::new("optiscaler-update")
                                    .outline()
                                    .small()
                                    .label(t_args(
                                        "settings.optiscaler.update",
                                        &[("version", &latest)],
                                    ))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.install(window, cx)),
                                    ),
                            )
                        })
                        .child(
                            Button::new("optiscaler-remove")
                                .ghost()
                                .small()
                                .label(self.labels.remove.clone())
                                .loading(self.busy == Some(Busy::Removing))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_remove(window, cx)
                                })),
                        )
                        .when(!incomplete, |row| row.child(toggle))
                        .into_any_element();
                field_row(row, control, cx).into_any_element()
            }
            None => {
                let mut help = self.labels.help.to_string();
                if let Some(error) = &status.catalog_error {
                    help.push('\n');
                    help.push_str(&t_args(
                        "settings.optiscaler.catalog_failed",
                        &[("error", error)],
                    ));
                }
                let row = RowState {
                    label: self.labels.label.clone(),
                    help: help.into(),
                    ..Default::default()
                };
                let control = Button::new("optiscaler-install")
                    .outline()
                    .small()
                    .label(self.labels.install.clone())
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.ask_install(window, cx)))
                    .into_any_element();
                field_row(row, control, cx).into_any_element()
            }
        }
    }

    fn render_ready(&self, status: &OptiscalerStatus, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let border = cx.theme().border;
        let divider = |el: AnyElement| div().border_t_1().border_color(border).child(el);

        let channel_row = field_row(
            RowState {
                label: self.labels.channel_label.clone(),
                help: self.labels.channel_help.clone(),
                ..Default::default()
            },
            Select::new(&self.channel_select)
                .id("optiscaler-channel")
                .accessibility_label(self.labels.channel_label.clone())
                .disabled(busy)
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let driver_row = field_row(
            RowState {
                label: self.labels.driver_label.clone(),
                help: self.labels.driver_help.clone(),
                ..Default::default()
            },
            Select::new(&self.driver_select)
                .id("optiscaler-gpu-driver")
                .accessibility_label(self.labels.driver_label.clone())
                .disabled(busy)
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let proxy_row = field_row(
            RowState {
                label: self.labels.proxy_label.clone(),
                help: self.labels.proxy_help.clone(),
                ..Default::default()
            },
            Select::new(&self.proxy_select)
                .id("optiscaler-proxy-dll")
                .accessibility_label(self.labels.proxy_label.clone())
                .disabled(busy)
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let optipatcher_row = field_row(
            RowState {
                label: self.labels.optipatcher_label.clone(),
                help: self.labels.optipatcher_help.clone(),
                ..Default::default()
            },
            Switch::new("optiscaler-optipatcher")
                .checked(self.config.enable_optipatcher)
                .disabled(busy)
                .accessibility_label(self.labels.optipatcher_label.clone())
                .on_change(cx.listener(|this, on: &bool, _, cx| {
                    this.config.enable_optipatcher = *on;
                    this.config_initialized = true;
                    cx.notify();
                }))
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let shortcut_row = field_row(
            RowState {
                label: self.labels.shortcut_label.clone(),
                help: self.labels.shortcut_help.clone(),
                ..Default::default()
            },
            self.key_capture.clone().into_any_element(),
            cx,
        )
        .into_any_element();

        let target_folder = self.target_dir(status, cx);
        let open_folder_btn = {
            let folder = target_folder.clone();
            Button::new("optiscaler-open-folder")
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .label(self.labels.open_folder.clone())
                .disabled(folder.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(dir) = &folder {
                        cx.open_with_system(dir);
                    }
                })
        };
        let target_row = field_row(
            RowState {
                label: self.labels.target_label.clone(),
                help: target_summary(status, self.chosen_exe.as_deref(), &self.labels.target_auto),
                ..Default::default()
            },
            h_flex()
                .gap_2()
                .child(
                    Button::new("optiscaler-choose")
                        .ghost()
                        .small()
                        .label(self.labels.choose_program.clone())
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_program(window, cx)),
                        ),
                )
                .child(open_folder_btn)
                .into_any_element(),
            cx,
        )
        .into_any_element();

        v_flex()
            .w_full()
            .child(self.render_status_row(status, cx))
            .child(divider(channel_row))
            .child(divider(driver_row))
            .child(divider(proxy_row))
            .child(divider(optipatcher_row))
            .child(divider(shortcut_row))
            .child(divider(target_row))
            .into_any_element()
    }
}

impl Render for OptiscalerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| {
                this.refresh_language(window, cx);
            });
        }
        if !self.applies(cx) {
            return div()
                .py_3()
                .child(muted(self.labels.unsupported.clone(), cx))
                .into_any_element();
        }
        let body: AnyElement = if self.busy == Some(Busy::Installing) {
            self.render_installing(cx)
        } else {
            match &self.load {
                Load::Idle | Load::Loading => div()
                    .px_4()
                    .py_3()
                    .child(muted(self.labels.checking.clone(), cx))
                    .into_any_element(),
                Load::Failed(e) => {
                    let row = RowState {
                        label: self.labels.label.clone(),
                        error: Some(e.clone()),
                        ..Default::default()
                    };
                    let retry = Button::new("optiscaler-retry")
                        .outline()
                        .small()
                        .label(self.labels.try_again.clone())
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx)))
                        .into_any_element();
                    field_row(row, retry, cx).into_any_element()
                }
                Load::Ready(status) => self.render_ready(status, cx),
            }
        };
        v_flex()
            .pt_2()
            .child(group_panel(cx).child(body))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use irori_reshade::{Api, Detection, Machine, ReshadeError};

    use super::*;

    fn sample_manifest(config: OptiscalerConfig) -> Manifest {
        Manifest {
            kind: GraphicsMod::Optiscaler,
            version: "v0.7.9".into(),
            exe: PathBuf::from("/games/Foo/Binaries/Win64/Foo-Win64-Shipping.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: config.proxy_dll.file_name().into(),
            dir: PathBuf::from("/games/Foo/Binaries/Win64"),
            enabled: true,
            complete: true,
            overrides: vec!["dxgi".into(), "nvapi64".into()],
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon: false,
            source: None,
            optiscaler: Some(config),
        }
    }

    #[test]
    fn key_conversions_round_trip() {
        assert_eq!(
            channel_from_key(channel_key(OptiscalerChannel::Stable)),
            OptiscalerChannel::Stable
        );
        assert_eq!(
            channel_from_key(channel_key(OptiscalerChannel::Nightly)),
            OptiscalerChannel::Nightly
        );
        assert_eq!(
            driver_from_key(driver_key(GpuDriverMode::MesaAuto)),
            GpuDriverMode::MesaAuto
        );
        assert_eq!(
            driver_from_key(driver_key(GpuDriverMode::Nvidia)),
            GpuDriverMode::Nvidia
        );
        assert_eq!(proxy_items().len(), ProxyDll::ALL.len());
    }

    #[test]
    fn pending_changes_tracks_config_and_executable() {
        let base = OptiscalerConfig::default();
        let status = OptiscalerStatus {
            manifest: Some(sample_manifest(base.clone())),
            ..Default::default()
        };
        assert!(!has_pending_changes(&status, &base, None));

        let mut modified = base.clone();
        modified.enable_optipatcher = true;
        assert!(has_pending_changes(&status, &modified, None));

        let same_exe = PathBuf::from("/games/Foo/Binaries/Win64/Foo-Win64-Shipping.exe");
        assert!(!has_pending_changes(&status, &base, Some(&same_exe)));

        let other_exe = PathBuf::from("/games/Foo/Other.exe");
        assert!(has_pending_changes(&status, &base, Some(&other_exe)));
    }

    #[test]
    fn target_summary_prefers_chosen_then_detected_then_auto() {
        crate::i18n::register();
        let auto = t("settings.optiscaler.target.auto");
        let mut status = OptiscalerStatus::default();
        assert_eq!(target_summary(&status, None, &auto), auto);

        status.detected = Some(Detection {
            exe: PathBuf::from("/games/Foo/Binaries/Win64/Foo.exe"),
            dir: PathBuf::from("/games/Foo/Binaries/Win64"),
            machine: Machine::X64,
            api: Some(Api::Dxgi),
            dll: Some("dxgi.dll".into()),
            searched: false,
            banned: None,
        });
        let detected = target_summary(&status, None, &auto);
        assert!(detected.contains("Foo.exe"), "{detected}");
        assert!(detected.contains("/games/Foo/Binaries/Win64"), "{detected}");

        let chosen = PathBuf::from("/games/Foo/Custom/Alt.exe");
        let summary = target_summary(&status, Some(&chosen), &auto);
        assert!(summary.contains("Alt.exe"), "{summary}");
        assert!(summary.contains("/games/Foo/Custom"), "{summary}");
    }

    #[test]
    fn installed_help_mentions_dll_overrides_and_catalog_errors() {
        crate::i18n::register();
        let cfg = OptiscalerConfig {
            gpu_driver: GpuDriverMode::Nvidia,
            shortcut_key: Some("0x2E".into()),
            ..Default::default()
        };
        let manifest = sample_manifest(cfg);
        let status = OptiscalerStatus {
            manifest: Some(manifest.clone()),
            catalog_error: Some("offline".into()),
            ..Default::default()
        };
        let help = installed_help(&status, &manifest, Runner::Sideload);
        assert!(help.contains("dxgi.dll"), "{help}");
        assert!(help.contains("Foo-Win64-Shipping.exe"), "{help}");
        assert!(help.contains("Delete"), "{help}");
        assert!(help.contains("offline"), "{help}");
        if !cfg!(target_os = "windows") {
            assert!(help.contains("dxgi.dll, nvapi64.dll"), "{help}");
        }
        if cfg!(target_os = "linux") {
            assert!(help.contains("PROTON_ENABLE_NVAPI=1"), "{help}");
        }

        let steam_help = installed_help(&status, &manifest, Runner::Steam);
        if !cfg!(target_os = "windows") {
            assert!(steam_help.contains("Proton"), "{steam_help}");
            assert!(
                !steam_help.contains("PROTON_ENABLE_NVAPI=1"),
                "{steam_help}"
            );
        }
    }

    #[test]
    fn dll_taken_error_suggests_alternative_proxies() {
        crate::i18n::register();
        let err = anyhow::Error::new(ReshadeError::DllTaken {
            name: "dxgi.dll".into(),
        });
        let text = error_text(&err);
        assert!(text.contains("dxgi.dll"), "{text}");
        assert!(text.contains("winmm.dll"), "{text}");
        assert!(text.contains("version.dll"), "{text}");
        assert!(text.contains("OptiScaler.asi"), "{text}");
    }

    #[test]
    fn optiscaler_steps_have_localized_text() {
        crate::i18n::register();
        for step in [
            Step::DownloadingOptiscaler,
            Step::DownloadingFakenvapi,
            Step::DownloadingOptipatcher,
        ] {
            let text = step_text(step);
            assert!(!text.is_empty(), "{step:?} returned empty text");
        }
    }

    #[test]
    fn optiscaler_route_id_opens_tools_section() {
        assert_eq!(
            crate::game_view::GameSection::from_id("optiscaler"),
            Some(crate::game_view::GameSection::Tools)
        );
    }
}
