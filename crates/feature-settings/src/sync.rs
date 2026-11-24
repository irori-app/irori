//! Settings → Backup → Sync between computers: where the library syncs
//! (a folder something else syncs, WebDAV, or an rclone remote), this
//! computer's name, how
//! the last sync went, the other computers and the changes that lost to
//! theirs. Cloud copies show a mode per kind, how much each kind holds and
//! how much waits to be sent, with a bar of their shares.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Window, div, px, relative,
};
use irori_core::CopyMode;
use irori_core::util::format_bytes;
use irori_core::{GameId, Runner, SyncMode};
use irori_services::backup_schedule::local_time;
use irori_services::sync::ConflictField;
use irori_services::sync::copies::{CopyKind, CopySize, CopyStatus};
use irori_services::sync::rclone::INSTALL_URL as RCLONE_INSTALL_URL;
use irori_services::{RcloneInfo, SyncConflict, SyncDevice, SyncStatus, WebDavLogin};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{muted, panel};

const MODES: [SyncMode; 4] = [
    SyncMode::Off,
    SyncMode::Folder,
    SyncMode::Webdav,
    SyncMode::Rclone,
];

/// The sync section's state.
pub struct SyncPanel {
    url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    device: Entity<InputState>,
    remote: Entity<InputState>,
    /// rclone as found: `None` while looking, `Some(None)` when missing.
    rclone: Option<Option<RcloneInfo>>,
    status: SyncStatus,
    /// The saved WebDAV user name, read off the UI thread.
    saved_user: Option<String>,
    /// Titles of the games the conflicts name.
    titles: HashMap<String, String>,
    busy: bool,
    /// Cloud copies being sent.
    sending: bool,
    /// Other computers' copies, once looked for.
    devices: Option<Vec<SyncDevice>>,
    finding: bool,
    /// The computer and kind being brought back.
    bringing: Option<(String, CopyKind)>,
    /// How much each kind of cloud copy holds, once measured.
    sizes: Option<Vec<CopySize>>,
    measuring: bool,
    _subscriptions: Vec<Subscription>,
}

/// "1.2 GB in 40 files · 300 MB to send" for one kind, or what it says when
/// it holds nothing or everything is sent.
fn size_text(size: &CopySize) -> SharedString {
    if size.files == 0 {
        return t("settings.sync.copies.size_empty");
    }
    let held = t_args(
        if size.files == 1 {
            "settings.sync.copies.size_one"
        } else {
            "settings.sync.copies.size_other"
        },
        &[
            ("size", &format_bytes(size.bytes)),
            ("count", &size.files.to_string()),
        ],
    );
    let waiting = if size.waiting_files == 0 {
        t("settings.sync.copies.all_sent")
    } else {
        t_args(
            "settings.sync.copies.waiting",
            &[("size", &format_bytes(size.waiting_bytes))],
        )
    };
    t_args(
        "settings.sync.copies.size_line",
        &[("held", &held), ("waiting", &waiting)],
    )
}

impl SyncPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            cx.subscribe_in(
                &remote,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.commit_remote(window, cx);
                    }
                },
            ),
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
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

    /// Measure how much each kind of cloud copy holds, off the UI thread.
    fn measure_copies(&mut self, cx: &mut Context<Self>) {
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
    fn look_for_rclone(&mut self, cx: &mut Context<Self>) {
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
    fn commit_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let remote = self.remote.read(cx).value().trim().to_string();
        self.set_remote(remote, window, cx);
    }

    fn set_remote(&mut self, remote: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn choose_rclone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn refresh(&mut self, cx: &mut Context<Self>) {
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
    fn commit_texts(&mut self, cx: &mut Context<Self>) {
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

    fn set_mode(&mut self, mode: SyncMode, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_texts(cx);
        backend(cx).settings.update_app(|s| s.sync.mode = mode);
        if mode == SyncMode::Rclone {
            self.look_for_rclone(cx);
        }
        if mode != SyncMode::Off {
            self.sync_now(false, window, cx);
        }
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn save_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn forget_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn sync_now(&mut self, tell: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    fn clear_conflicts(&mut self, cx: &mut Context<Self>) {
        backend(cx).clear_sync_conflicts();
        self.refresh(cx);
    }

    fn conflict_text(&self, c: &SyncConflict) -> SharedString {
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
fn os_name(os: &str) -> &str {
    match os {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    }
}

/// "3 changes from other computers" and the like.
fn counted(one: &str, other: &str, count: usize) -> SharedString {
    let n = count.to_string();
    t_args(if count == 1 { one } else { other }, &[("count", &n)])
}

impl Render for SyncPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = backend(cx).settings.app().sync.clone();
        let mode = prefs.mode;
        let busy = self.busy || self.status.running;

        let modes = ButtonGroup::new("sync-mode")
            .outline()
            .small()
            .children(MODES.iter().enumerate().map(|(ix, m)| {
                let label = match m {
                    SyncMode::Off => t("settings.sync.mode_off"),
                    SyncMode::Folder => t("settings.sync.mode_folder"),
                    SyncMode::Webdav => SharedString::new_static("WebDAV"),
                    SyncMode::Rclone => SharedString::new_static("rclone"),
                };
                Button::new(SharedString::from(format!("sync-mode-{ix}")))
                    .label(label)
                    .selected(mode == *m)
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                if let Some(&ix) = clicked.first() {
                    this.set_mode(MODES[ix], window, cx);
                }
            }));

        let mut body = panel(cx)
            .p_4()
            .gap_3()
            .child(muted(t("settings.sync.description"), cx))
            .child(h_flex().child(modes));

        match mode {
            SyncMode::Off | SyncMode::Rclone => {}
            SyncMode::Folder => {
                let folder = prefs.folder.trim().to_string();
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(folder.is_empty(), |d| {
                                            d.text_color(cx.theme().muted_foreground)
                                        })
                                        .child(if folder.is_empty() {
                                            t("settings.sync.folder_none")
                                        } else {
                                            folder.into()
                                        }),
                                )
                                .child(
                                    Button::new("sync-folder")
                                        .outline()
                                        .small()
                                        .icon(IconName::FolderOpen)
                                        .label(t("settings.sync.choose_folder"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick_folder(window, cx)
                                        })),
                                ),
                        )
                        .child(muted(t("settings.sync.folder_hint"), cx)),
                );
            }
            SyncMode::Webdav => {
                let login =
                    match &self.saved_user {
                        Some(user) => {
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().min_w_0().truncate().child(t_args(
                                    "settings.sync.login_saved_as",
                                    &[("user", user)],
                                )))
                                .child(
                                    Button::new("sync-forget-login")
                                        .ghost()
                                        .small()
                                        .label(t("settings.sync.forget_login"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.forget_login(window, cx)
                                        })),
                                )
                                .into_any_element()
                        }
                        None => h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(div().flex_1().min_w_32().child(Input::new(&self.username)))
                            .child(div().flex_1().min_w_32().child(Input::new(&self.password)))
                            .child(
                                Button::new("sync-save-login")
                                    .outline()
                                    .small()
                                    .icon(IconName::LogIn)
                                    .label(t("settings.sync.save_login"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_login(window, cx)
                                    })),
                            )
                            .into_any_element(),
                    };
                body = body
                    .child(
                        v_flex()
                            .gap_1()
                            .child(irori_ui::style::caption(t("settings.sync.webdav_url"), cx))
                            .child(Input::new(&self.url)),
                    )
                    .child(login)
                    .child(muted(t("settings.sync.login_hint"), cx));
            }
        }

        if mode == SyncMode::Rclone {
            body = body.child(self.render_rclone(cx));
        }

        if mode != SyncMode::Off {
            body = body
                .child(
                    v_flex()
                        .gap_1()
                        .child(irori_ui::style::caption(t("settings.sync.device_name"), cx))
                        .child(Input::new(&self.device))
                        .child(muted(t("settings.sync.device_name_hint"), cx)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(
                            Switch::new("sync-playtime-only")
                                .label(t("settings.sync.playtime_only"))
                                .checked(prefs.playtime_only)
                                .on_change(cx.listener(|this, on: &bool, window, cx| {
                                    let on = *on;
                                    backend(cx)
                                        .settings
                                        .update_app(|s| s.sync.playtime_only = on);
                                    this.sync_now(false, window, cx);
                                })),
                        )
                        .child(muted(t("settings.sync.playtime_only_help"), cx)),
                )
                .child(self.render_status(busy, cx))
                .child(self.render_copies(cx))
                .child(
                    h_flex().child(
                        Button::new("sync-now")
                            .primary()
                            .icon(IconName::RefreshCw)
                            .label(if busy {
                                t("settings.sync.syncing")
                            } else {
                                t("settings.sync.now")
                            })
                            .loading(busy)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.sync_now(true, window, cx)),
                            ),
                    ),
                );
        }
        body
    }
}

const COPY_MODES: [CopyMode; 3] = [CopyMode::Off, CopyMode::Manual, CopyMode::Automatic];

fn kind_label(kind: CopyKind) -> SharedString {
    t(match kind {
        CopyKind::Backups => "settings.sync.copies.backups",
        CopyKind::Saves => "settings.sync.copies.saves",
        CopyKind::Screenshots => "settings.sync.copies.screenshots",
    })
}

impl SyncPanel {
    fn set_copy_mode(&mut self, kind: CopyKind, mode: CopyMode, cx: &mut Context<Self>) {
        backend(cx).settings.update_app(|s| match kind {
            CopyKind::Backups => s.sync.copies.backups = mode,
            CopyKind::Saves => s.sync.copies.saves = mode,
            CopyKind::Screenshots => s.sync.copies.screenshots = mode,
        });
        cx.notify();
    }

    /// Send every kind that isn't off now.
    fn send_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn find_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn bring_back(
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

    /// Cloud copies: a mode per kind, how the last copy went, sending now,
    /// and bringing back another computer's.
    fn render_copies(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let statuses = backend(cx).cloud_copies_status();
        let any_on = statuses.iter().any(|s| s.mode != CopyMode::Off);
        let mut out = v_flex()
            .gap_2()
            .child(irori_ui::style::caption(
                t("settings.sync.copies.title"),
                cx,
            ))
            .child(muted(t("settings.sync.copies.description"), cx));
        if let Some(bar) = self.render_copies_bar(cx) {
            out = out.child(bar);
        }
        for (row, status) in statuses.iter().enumerate() {
            out = out.child(self.render_copy_row(row, status, cx));
        }
        out = out.child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new("copies-send")
                        .outline()
                        .small()
                        .icon(IconName::CloudUpload)
                        .label(if self.sending {
                            t("settings.sync.copies.sending")
                        } else {
                            t("settings.sync.copies.send_now")
                        })
                        .loading(self.sending)
                        .disabled(self.sending || !any_on)
                        .on_click(cx.listener(|this, _, window, cx| this.send_copies(window, cx))),
                )
                .child(
                    Button::new("copies-find")
                        .ghost()
                        .small()
                        .icon(IconName::CloudDownload)
                        .label(t("settings.sync.copies.find"))
                        .loading(self.finding)
                        .disabled(self.finding)
                        .on_click(cx.listener(|this, _, window, cx| this.find_devices(window, cx))),
                ),
        );
        if let Some(devices) = &self.devices {
            if devices.is_empty() {
                out = out.child(muted(t("settings.sync.copies.no_devices"), cx));
            }
            for (row, device) in devices.iter().enumerate() {
                out = out.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .child(div().min_w_0().truncate().child(device.name.clone()))
                        .child(muted(
                            t_args(
                                "settings.sync.device_seen",
                                &[
                                    ("os", os_name(&device.os)),
                                    ("time", &local_time(&device.updated)),
                                ],
                            ),
                            cx,
                        ))
                        .children(CopyKind::ALL.iter().enumerate().map(|(i, kind)| {
                            let (kind, device) = (*kind, device.clone());
                            let loading = self
                                .bringing
                                .as_ref()
                                .is_some_and(|(id, k)| *id == device.id && *k == kind);
                            Button::new(("copies-bring", row * 3 + i))
                                .outline()
                                .xsmall()
                                .label(kind_label(kind))
                                .loading(loading)
                                .disabled(self.bringing.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.bring_back(device.clone(), kind, window, cx)
                                }))
                        })),
                );
            }
            out = out.child(muted(t("settings.sync.copies.bring_hint"), cx));
        }
        out
    }

    /// The kinds' shares of what the cloud copies hold here, as one bar with
    /// a legend; `None` before they are measured or when they hold nothing.
    fn render_copies_bar(&self, cx: &mut Context<Self>) -> Option<gpui_kit::Div> {
        let sizes = self.sizes.as_ref()?;
        let total: u64 = sizes.iter().map(|s| s.bytes).sum();
        if total == 0 {
            return None;
        }
        let theme = cx.theme();
        let colors = [theme.chart_1, theme.chart_2, theme.chart_3];
        let (track, muted_fg) = (theme.muted, theme.muted_foreground);
        let bar = h_flex()
            .w_full()
            .h(px(8.))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(track)
            .children(sizes.iter().zip(colors).map(|(size, color)| {
                div()
                    .h_full()
                    .w(relative(size.bytes as f32 / total as f32))
                    .bg(color)
            }));
        let legend = h_flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .text_xs()
            .text_color(muted_fg)
            .children(sizes.iter().zip(colors).map(|(size, color)| {
                h_flex()
                    .gap_1p5()
                    .child(div().size_2p5().rounded_full().bg(color))
                    .child(t_args(
                        "settings.sync.copies.legend",
                        &[
                            ("what", &kind_label(size.kind)),
                            ("size", &format_bytes(size.bytes)),
                        ],
                    ))
            }));
        Some(v_flex().gap_1().child(bar).child(legend))
    }

    fn render_copy_row(
        &self,
        row: usize,
        status: &CopyStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind = status.kind;
        let size = self
            .sizes
            .as_ref()
            .and_then(|s| s.iter().find(|s| s.kind == kind))
            .map(size_text);
        let modes = ButtonGroup::new(SharedString::from(format!("copies-mode-{row}")))
            .outline()
            .xsmall()
            .children(COPY_MODES.iter().enumerate().map(|(ix, m)| {
                let label = match m {
                    CopyMode::Off => t("settings.sync.copies.off"),
                    CopyMode::Manual => t("settings.sync.copies.manual"),
                    CopyMode::Automatic => t("settings.sync.copies.automatic"),
                };
                Button::new(SharedString::from(format!("copies-mode-{row}-{ix}")))
                    .label(label)
                    .selected(status.mode == *m)
            }))
            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                if let Some(&ix) = clicked.first() {
                    this.set_copy_mode(kind, COPY_MODES[ix], cx);
                }
            }));
        let last = status.last.as_ref().map(|last| match &last.error {
            Some(e) => SharedString::from(e.clone()),
            None => t_args(
                "settings.sync.copies.last",
                &[
                    ("time", &local_time(&last.at)),
                    ("sent", &last.sent.to_string()),
                    ("kept", &(last.sent + last.unchanged).to_string()),
                ],
            ),
        });
        h_flex()
            .gap_3()
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(kind_label(kind)))
                    .when_some(size, |col, size| col.child(muted(size, cx)))
                    .when_some(last, |col, last| col.child(muted(last, cx))),
            )
            .child(modes)
    }

    /// rclone: found or how to get it, and the remote to sync with.
    fn render_rclone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut out = v_flex().gap_2();
        match &self.rclone {
            None => out = out.child(muted(t("settings.sync.rclone_looking"), cx)),
            Some(None) => {
                out = out
                    .child(
                        Alert::info("rclone-missing", t("settings.sync.rclone_missing"))
                            .title(t("settings.sync.rclone_missing_title")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("rclone-get")
                                    .primary()
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label(t("settings.sync.rclone_get"))
                                    .on_click(|_, _, cx| cx.open_url(RCLONE_INSTALL_URL)),
                            )
                            .child(
                                Button::new("rclone-choose")
                                    .outline()
                                    .small()
                                    .icon(IconName::FolderOpen)
                                    .label(t("settings.sync.rclone_choose"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_rclone(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("rclone-again")
                                    .ghost()
                                    .small()
                                    .icon(IconName::RefreshCw)
                                    .label(t("settings.sync.rclone_again"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.look_for_rclone(cx)),
                                    ),
                            ),
                    );
            }
            Some(Some(info)) => {
                let found = match &info.version {
                    Some(v) => t_args("settings.sync.rclone_found", &[("version", v)]),
                    None => t("settings.sync.rclone_found_no_version"),
                };
                out = out.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::CircleCheck).text_color(cx.theme().success))
                        .child(div().min_w_0().truncate().child(found))
                        .child(muted(SharedString::from(info.program.clone()), cx))
                        .child(
                            Button::new("rclone-refresh")
                                .ghost()
                                .xsmall()
                                .icon(IconName::RefreshCw)
                                .accessibility_label(t("settings.sync.rclone_again"))
                                .tooltip(t("settings.sync.rclone_again"))
                                .on_click(cx.listener(|this, _, _, cx| this.look_for_rclone(cx))),
                        ),
                );
                out = out.child(
                    v_flex()
                        .gap_1()
                        .child(irori_ui::style::caption(
                            t("settings.sync.rclone_remote"),
                            cx,
                        ))
                        .child(Input::new(&self.remote)),
                );
                match &info.remotes {
                    Ok(remotes) if remotes.is_empty() => {
                        out = out.child(muted(t("settings.sync.rclone_no_remotes"), cx));
                    }
                    Ok(remotes) => {
                        out = out.child(h_flex().gap_1().flex_wrap().children(
                            remotes.iter().enumerate().map(|(i, r)| {
                                let remote = r.clone();
                                Button::new(("rclone-remote", i))
                                    .outline()
                                    .xsmall()
                                    .label(r.clone())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_remote(remote.clone(), window, cx)
                                    }))
                            }),
                        ));
                    }
                    Err(e) => out = out.child(muted(SharedString::from(e.clone()), cx)),
                }
                out = out.child(muted(t("settings.sync.rclone_hint"), cx));
            }
        }
        out
    }

    /// The last sync, the other computers and the changes that lost.
    fn render_status(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let status = &self.status;
        let mut out = v_flex().gap_3();
        match &status.last {
            Some(last) if last.error.is_some() => {
                let error = last.error.clone().unwrap_or_default();
                out = out.child(Alert::error("sync-error", error).title(t("settings.sync.failed")));
                if let Some(ok) = &status.last_success {
                    out = out.child(muted(
                        t_args("settings.sync.last", &[("time", &local_time(ok))]),
                        cx,
                    ));
                }
            }
            Some(last) => {
                let summary = if last.changed == 0 && last.sessions_added == 0 {
                    t("settings.sync.nothing_new")
                } else if last.sessions_added == 0 {
                    counted(
                        "settings.sync.changes_one",
                        "settings.sync.changes_other",
                        last.changed,
                    )
                } else {
                    counted(
                        "settings.sync.sessions_one",
                        "settings.sync.sessions_other",
                        last.sessions_added,
                    )
                };
                out = out.child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::CircleCheck).text_color(cx.theme().success))
                        .child(div().min_w_0().child(t_args(
                            "settings.sync.last",
                            &[("time", &local_time(&last.at))],
                        )))
                        .child(muted(summary, cx)),
                );
            }
            None if !busy => out = out.child(muted(t("settings.sync.never"), cx)),
            None => {}
        }
        if let Some(last) = &status.last {
            if last.newer_version {
                out = out.child(Alert::warning(
                    "sync-newer",
                    t("settings.sync.newer_version"),
                ));
            }
            let unreadable: Vec<&str> = last
                .skipped
                .iter()
                .filter(|s| s.ends_with(".json"))
                .map(String::as_str)
                .collect();
            if !unreadable.is_empty() {
                out = out.child(muted(
                    t_args(
                        "settings.sync.unreadable",
                        &[("files", &unreadable.join(", "))],
                    ),
                    cx,
                ));
            }
            let devices = v_flex().gap_1().child(irori_ui::style::caption(
                t("settings.sync.devices_title"),
                cx,
            ));
            out = out.child(if last.devices.is_empty() {
                devices.child(muted(t("settings.sync.no_devices"), cx))
            } else {
                devices.children(last.devices.iter().map(|d| {
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Gamepad).text_color(cx.theme().muted_foreground))
                        .child(div().min_w_0().truncate().child(d.name.clone()))
                        .child(muted(
                            t_args(
                                "settings.sync.device_seen",
                                &[("os", os_name(&d.os)), ("time", &local_time(&d.updated))],
                            ),
                            cx,
                        ))
                }))
            });
        }
        if !status.conflicts.is_empty() {
            out = out.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(irori_ui::style::caption(
                                t("settings.sync.conflicts_title"),
                                cx,
                            )))
                            .child(
                                Button::new("sync-clear-conflicts")
                                    .ghost()
                                    .xsmall()
                                    .label(t("settings.sync.clear"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.clear_conflicts(cx)),
                                    ),
                            ),
                    )
                    .child(muted(t("settings.sync.conflicts_description"), cx))
                    .children(status.conflicts.iter().map(|c| {
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .text_color(cx.theme().warning)
                                    .mt_0p5(),
                            )
                            .child(div().min_w_0().text_sm().child(self.conflict_text(c)))
                    })),
            );
        }
        out
    }
}
