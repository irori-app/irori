//! Settings → Saves & backup → Sync: cross-computer library sync, cloud copies
//! (Irori backups, Ludusavi saves, screenshots), unified device list, and
//! conflict log.

mod connection;
mod copies;
mod devices;

use std::collections::HashMap;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
};
use irori_core::{CopyMode, GameId, Runner, SyncMode};
use irori_services::backup_schedule::local_time;
use irori_services::sync::ConflictField;
use irori_services::sync::copies::{CopyKind, CopySize};
use irori_services::{RcloneInfo, SyncConflict, SyncDevice, SyncStatus, WebDavLogin};
use irori_ui::i18n::{t, t_args};
use irori_ui::save_backups::LudusaviPanel;
use irori_ui::{backend, models};

use crate::widgets::group_header;

/// Events emitted from [`SyncPanel`] to its parent [`crate::backup::BackupPanel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncPanelEvent {
    /// Jump to the `Game saves` tab (e.g. to set up Ludusavi).
    OpenSaves,
}

/// Snapshot of sync health read by the header status card and Game saves bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyncSummary {
    pub mode: SyncMode,
    pub devices: usize,
    pub conflicts: usize,
    pub has_error: bool,
    pub saves_copy_mode: CopyMode,
}

/// The Sync tab's state.
pub struct SyncPanel {
    pub(super) ludusavi: Entity<LudusaviPanel>,
    pub(super) url: Entity<InputState>,
    pub(super) username: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    pub(super) device: Entity<InputState>,
    pub(super) remote: Entity<InputState>,
    /// rclone as found: `None` while looking, `Some(None)` when missing.
    pub(super) rclone: Option<Option<RcloneInfo>>,
    pub(super) status: SyncStatus,
    /// The saved WebDAV user name, read off the UI thread.
    pub(super) saved_user: Option<String>,
    /// Titles of the games the conflicts name.
    pub(super) titles: HashMap<String, String>,
    pub(super) busy: bool,
    /// Cloud copies being sent.
    pub(super) sending: bool,
    /// Other computers' copies, once looked for.
    pub(super) devices: Option<Vec<SyncDevice>>,
    pub(super) finding: bool,
    /// The computer and kind being brought back.
    pub(super) bringing: Option<(String, CopyKind)>,
    /// How much each kind of cloud copy holds, once measured.
    pub(super) sizes: Option<Vec<CopySize>>,
    pub(super) measuring: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SyncPanelEvent> for SyncPanel {}

impl SyncPanel {
    pub fn new(
        ludusavi: Entity<LudusaviPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prefs = backend(cx).settings.app().sync.clone();
        let url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.sync.webdav_url_placeholder"))
                .default_value(prefs.webdav_url.clone())
        });
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.sync.username")));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.sync.password"))
                .masked(true)
        });
        let device = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(irori_services::sync::default_device_name())
                .default_value(prefs.device_name.clone())
        });
        let remote = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.sync.rclone_remote_placeholder"))
                .default_value(prefs.rclone_remote.clone())
        });
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.observe(&ludusavi, |_, _, cx| cx.notify()),
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(
                &remote,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_remote(window, cx);
                    }
                },
            ),
            cx.subscribe_in(&url, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_texts(cx);
                }
            }),
            cx.subscribe_in(&device, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_texts(cx);
                }
            }),
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
            ludusavi,
            url,
            username,
            password,
            device,
            remote,
            rclone: None,
            status: SyncStatus::default(),
            saved_user: None,
            titles: HashMap::new(),
            busy: false,
            sending: false,
            devices: None,
            finding: false,
            bringing: None,
            sizes: None,
            measuring: false,
            _subscriptions: subscriptions,
        };
        if prefs.mode != SyncMode::Off {
            this.measure_copies(cx);
        }
        this.refresh(cx);
        this.load_user(cx);
        if prefs.mode == SyncMode::Rclone {
            this.look_for_rclone(cx);
        }
        this
    }

    /// Lightweight summary for the Saves & backup status card and Saves bridge.
    pub(crate) fn summary(&self, cx: &App) -> SyncSummary {
        let prefs = backend(cx).settings.app().sync.clone();
        let devices = self
            .status
            .last
            .as_ref()
            .map(|l| l.devices.len())
            .unwrap_or(0)
            .max(self.devices.as_ref().map_or(0, Vec::len));
        let has_error = self.status.last.as_ref().is_some_and(|l| l.error.is_some());
        SyncSummary {
            mode: prefs.mode,
            devices,
            conflicts: self.status.conflicts.len(),
            has_error,
            saves_copy_mode: prefs.copies.saves,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_mode_for_test(
        &mut self,
        mode: SyncMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_mode(mode, window, cx);
    }

    /// Measure how much each kind of cloud copy holds, off the UI thread.
    pub(super) fn measure_copies(&mut self, cx: &mut Context<Self>) {
        if self.measuring {
            return;
        }
        self.measuring = true;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.cloud_copy_sizes().await });
        cx.spawn(async move |this, cx| {
            let sizes = task.await;
            let _ = this.update(cx, |this, cx| {
                this.measuring = false;
                match sizes {
                    Ok(sizes) => this.sizes = Some(sizes),
                    Err(e) => tracing::debug!("measuring cloud copies: {e:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Find rclone and its remotes, off the UI thread.
    pub(super) fn look_for_rclone(&mut self, cx: &mut Context<Self>) {
        self.rclone = None;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.rclone_info().await });
        cx.spawn(async move |this, cx| {
            let info = task.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.rclone = Some(info);
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the typed remote and sync with it.
    pub(super) fn commit_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let remote = self.remote.read(cx).value().trim().to_string();
        self.set_remote(remote, window, cx);
    }

    pub(super) fn set_remote(
        &mut self,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        if b.settings.app().sync.rclone_remote == remote {
            return;
        }
        if self.remote.read(cx).value().trim() != remote {
            let value = remote.clone();
            self.remote
                .update(cx, |s, cx| s.set_value(value, window, cx));
        }
        b.settings.update_app(|s| s.sync.rclone_remote = remote);
        self.sync_now(false, window, cx);
    }

    pub(super) fn choose_rclone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.sync.rclone_choose")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    let path = p.to_string_lossy().into_owned();
                    backend(cx)
                        .settings
                        .update_app(|s| s.sync.rclone_path = path);
                    this.look_for_rclone(cx);
                });
            }
        })
        .detach();
    }

    /// Re-read the status and the titles of the games it names.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        self.status = b.sync_status();
        self.titles = self
            .status
            .conflicts
            .iter()
            .filter_map(|c| {
                let (runner, app) = c.subject.split_once(':')?;
                let id = GameId::new(Runner::from_id(runner)?, app);
                let title = b.game(&id).map(|g| g.title.clone())?;
                Some((c.subject.clone(), title))
            })
            .collect();
        cx.notify();
    }

    fn load_user(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        // A keyring may ask to be unlocked: off the UI thread and the
        // backend's async workers.
        let task = irori_ui::state::run_infallible(cx, async move {
            let b2 = b.clone();
            b.spawn_blocking(move || b2.webdav_username()).await.ok()
        });
        cx.spawn(async move |this, cx| {
            let user = task.await.flatten().flatten();
            let _ = this.update(cx, |this, cx| {
                this.saved_user = user;
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the typed address and device name.
    pub(super) fn commit_texts(&mut self, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        let device = self.device.read(cx).value().trim().to_string();
        let b = backend(cx);
        let app = b.settings.app();
        if app.sync.webdav_url != url || app.sync.device_name != device {
            b.settings.update_app(|s| {
                s.sync.webdav_url = url;
                s.sync.device_name = device;
            });
        }
    }

    pub(super) fn set_mode(&mut self, mode: SyncMode, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_texts(cx);
        backend(cx).settings.update_app(|s| s.sync.mode = mode);
        if mode == SyncMode::Rclone {
            self.look_for_rclone(cx);
        }
        if mode != SyncMode::Off {
            self.measure_copies(cx);
            self.sync_now(false, window, cx);
        }
        cx.notify();
    }

    pub(super) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("settings.sync.choose_folder")),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let folder = p.to_string_lossy().into_owned();
                    backend(cx).settings.update_app(|s| s.sync.folder = folder);
                    this.sync_now(false, window, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn save_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = WebDavLogin {
            username: self.username.read(cx).value().trim().to_string(),
            password: self.password.read(cx).value().to_string(),
        };
        if login.username.is_empty() {
            return;
        }
        self.commit_texts(cx);
        let user = login.username.clone();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let b2 = b.clone();
            b.spawn_blocking(move || b2.set_webdav_login(Some(login)))
                .await?
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.saved_user = Some(user);
                    this.username
                        .update(cx, |s, cx| s.set_value("", window, cx));
                    this.password
                        .update(cx, |s, cx| s.set_value("", window, cx));
                    window.push_notification(
                        Notification::success(t("settings.sync.login_saved")),
                        cx,
                    );
                    this.sync_now(false, window, cx);
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}")).title(t("settings.sync.login_failed")),
                    cx,
                ),
            });
        })
        .detach();
    }

    pub(super) fn forget_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let b2 = b.clone();
            b.spawn_blocking(move || b2.set_webdav_login(None)).await?
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.saved_user = None;
                    cx.notify();
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}")).title(t("settings.sync.login_failed")),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Sync now; `tell` reports success too, not just failure.
    pub(super) fn sync_now(&mut self, tell: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.sync_now().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.refresh(cx);
                match result {
                    Ok(_) if tell => window
                        .push_notification(Notification::success(t("settings.sync.synced")), cx),
                    Ok(_) => {}
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("settings.sync.failed")),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    pub(super) fn clear_conflicts(&mut self, cx: &mut Context<Self>) {
        backend(cx).clear_sync_conflicts();
        self.refresh(cx);
    }

    pub(super) fn set_copy_mode(&mut self, kind: CopyKind, mode: CopyMode, cx: &mut Context<Self>) {
        backend(cx).settings.update_app(|s| match kind {
            CopyKind::Backups => s.sync.copies.backups = mode,
            CopyKind::Saves => s.sync.copies.saves = mode,
            CopyKind::Screenshots => s.sync.copies.screenshots = mode,
        });
        cx.notify();
    }

    /// Send every kind that isn't off now.
    pub(super) fn send_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        self.sending = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.send_cloud_copies(None).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.sending = false;
                match result {
                    Ok(reports) => {
                        let sent: usize = reports.iter().map(|r| r.sent).sum();
                        window.push_notification(
                            Notification::success(counted(
                                "settings.sync.copies.sent_one",
                                "settings.sync.copies.sent_other",
                                sent,
                            )),
                            cx,
                        );
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.sync.copies.failed")),
                        cx,
                    ),
                }
                this.measure_copies(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn find_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finding = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.cloud_devices().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finding = false;
                match result {
                    Ok(devices) => this.devices = Some(devices),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.sync.copies.find_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn bring_back(
        &mut self,
        device: SyncDevice,
        kind: CopyKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bringing.is_some() {
            return;
        }
        self.bringing = Some((device.id.clone(), kind));
        cx.notify();
        let b = backend(cx);
        let name = device.name.clone();
        let task = irori_ui::run(
            cx,
            async move { b.bring_back_cloud_copies(&device, kind).await },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.bringing = None;
                let note = match result {
                    Ok(count) => Notification::success(t_args(
                        if count == 1 {
                            "settings.sync.copies.brought_one"
                        } else {
                            "settings.sync.copies.brought_other"
                        },
                        &[("count", &count.to_string()), ("device", &name)],
                    )),
                    Err(e) => Notification::error(format!("{e:#}"))
                        .title(t("settings.sync.copies.bring_failed")),
                };
                window.push_notification(note, cx);
                this.measure_copies(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn conflict_text(&self, c: &SyncConflict) -> SharedString {
        let game = self.titles.get(&c.subject).cloned().unwrap_or_else(|| {
            c.subject
                .split_once(':')
                .map_or(c.subject.clone(), |(_, app)| app.to_string())
        });
        let device = if c.device.is_empty() {
            t("settings.sync.another_computer").to_string()
        } else {
            c.device.clone()
        };
        let time = local_time(&c.at);
        let category = c.category.clone().unwrap_or_else(|| c.subject.clone());
        let key = match c.field {
            ConflictField::Favourite => "settings.sync.conflict.favourite",
            ConflictField::Hidden => "settings.sync.conflict.hidden",
            ConflictField::Rating => "settings.sync.conflict.rating",
            ConflictField::PlayStatus => "settings.sync.conflict.play_status",
            ConflictField::Category => "settings.sync.conflict.category",
            ConflictField::CategoryGame => "settings.sync.conflict.category_game",
            ConflictField::Custom => "settings.sync.conflict.custom",
            ConflictField::Goal => "settings.sync.conflict.goal",
            ConflictField::Playtime => "settings.sync.conflict.playtime",
            ConflictField::Journal => "settings.sync.conflict.journal",
        };
        t_args(
            key,
            &[
                ("game", &game),
                ("category", &category),
                ("year", &c.subject),
                ("device", &device),
                ("time", &time),
            ],
        )
    }
}

/// An operating system's name, as its maker writes it.
pub(super) fn os_name(os: &str) -> &str {
    match os {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    }
}

/// "3 changes from other computers" and the like.
pub(super) fn counted(one: &str, other: &str, count: usize) -> SharedString {
    let n = count.to_string();
    t_args(if count == 1 { one } else { other }, &[("count", &n)])
}

pub(super) fn kind_label(kind: CopyKind) -> SharedString {
    t(match kind {
        CopyKind::Backups => "settings.sync.copies.backups",
        CopyKind::Saves => "settings.sync.copies.saves",
        CopyKind::Screenshots => "settings.sync.copies.screenshots",
    })
}

impl Render for SyncPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = backend(cx).settings.app().sync.clone();
        let mode = prefs.mode;
        let busy = self.busy || self.status.running;
        let has_conflicts = !self.status.conflicts.is_empty();

        v_flex()
            .w_full()
            .gap_1()
            .child(group_header(t("settings.sync.title"), None, true, cx))
            .child(self.render_connection_card(&prefs, busy, cx))
            .when(mode != SyncMode::Off, |col| {
                col.child(self.render_copies_section(cx))
                    .child(self.render_devices_section(cx))
                    .when(has_conflicts, |col| {
                        col.child(self.render_conflicts_section(cx))
                    })
            })
    }
}
