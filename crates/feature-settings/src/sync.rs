//! Settings → Backup → Sync between computers: where the library syncs
//! (a folder something else syncs, or WebDAV), this computer's name, how
//! the last sync went, the other computers and the changes that lost to
//! theirs.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::{GameId, Runner, SyncMode};
use irori_services::backup_schedule::local_time;
use irori_services::sync::ConflictField;
use irori_services::{SyncConflict, SyncStatus, WebDavLogin};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::{muted, panel};

const MODES: [SyncMode; 3] = [SyncMode::Off, SyncMode::Folder, SyncMode::Webdav];

/// The sync section's state.
pub struct SyncPanel {
    url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    device: Entity<InputState>,
    status: SyncStatus,
    /// The saved WebDAV user name, read off the UI thread.
    saved_user: Option<String>,
    /// Titles of the games the conflicts name.
    titles: HashMap<String, String>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
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
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
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
            status: SyncStatus::default(),
            saved_user: None,
            titles: HashMap::new(),
            busy: false,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this.load_user(cx);
        this
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
            SyncMode::Off => {}
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

        if mode != SyncMode::Off {
            body = body
                .child(
                    v_flex()
                        .gap_1()
                        .child(irori_ui::style::caption(t("settings.sync.device_name"), cx))
                        .child(Input::new(&self.device))
                        .child(muted(t("settings.sync.device_name_hint"), cx)),
                )
                .child(self.render_status(busy, cx))
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

impl SyncPanel {
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
