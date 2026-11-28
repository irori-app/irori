//! Card 1 of `Sync`: connection status, method selector (`Off | Folder |
//! WebDAV | rclone`), credentials/paths, device name, and playtime-only toggle.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use irori_core::{SyncMode, SyncPrefs};
use irori_services::backup_schedule::local_time;
use irori_services::sync::rclone::INSTALL_URL as RCLONE_INSTALL_URL;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::widgets::{RowState, field_row, group_panel, muted};

use super::{SyncPanel, counted};

const MODES: [SyncMode; 4] = [
    SyncMode::Off,
    SyncMode::Folder,
    SyncMode::Webdav,
    SyncMode::Rclone,
];

impl SyncPanel {
    pub(super) fn render_connection_card(
        &self,
        prefs: &SyncPrefs,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg, success) = (theme.border, theme.muted_foreground, theme.success);
        let mode = prefs.mode;

        // Row 1: When Off -> calm explanation; when active -> status + primary "Sync now"
        let top_row = if mode == SyncMode::Off {
            v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child(t("settings.sync.off_headline")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.sync.description")),
                )
                .into_any_element()
        } else {
            let status_left = self.render_sync_status_header(busy, success, muted_fg, cx);
            h_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_3()
                .flex_wrap()
                .items_center()
                .justify_between()
                .child(status_left)
                .child(
                    Button::new("sync-now")
                        .primary()
                        .small()
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
                )
                .into_any_element()
        };

        // Row 2: Sync method selector
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
        let method_row = field_row(
            RowState {
                label: t("settings.sync.method_label"),
                help: t("settings.sync.method_help"),
                ..RowState::default()
            },
            modes.into_any_element(),
            cx,
        );

        let mut card = group_panel(cx)
            .child(top_row)
            .child(div().border_t_1().border_color(border).child(method_row));

        // Optional status alerts (newer version, unreadable files)
        if mode != SyncMode::Off
            && let Some(last) = &self.status.last
        {
            if last.newer_version {
                card = card.child(
                    div()
                        .px_4()
                        .py_2p5()
                        .border_t_1()
                        .border_color(border)
                        .child(Alert::warning(
                            "sync-newer",
                            t("settings.sync.newer_version"),
                        )),
                );
            }
            let unreadable: Vec<&str> = last
                .skipped
                .iter()
                .filter(|s| s.ends_with(".json"))
                .map(String::as_str)
                .collect();
            if !unreadable.is_empty() {
                card = card.child(div().px_4().py_2().border_t_1().border_color(border).child(
                    muted(
                        t_args(
                            "settings.sync.unreadable",
                            &[("files", &unreadable.join(", "))],
                        ),
                        cx,
                    ),
                ));
            }
        }

        // Row 3: Method-specific connection controls
        match mode {
            SyncMode::Off => {}
            SyncMode::Folder => {
                let folder = prefs.folder.trim().to_string();
                let mono = cx.theme().mono_font_family.clone();
                card = card.child(
                    div().border_t_1().border_color(border).child(
                        v_flex()
                            .w_full()
                            .px_4()
                            .py_3()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_medium()
                                                    .child(t("settings.sync.folder_label")),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted_fg)
                                                    .when(!folder.is_empty(), |d| {
                                                        d.font_family(mono)
                                                    })
                                                    .truncate()
                                                    .child(if folder.is_empty() {
                                                        t("settings.sync.folder_none")
                                                    } else {
                                                        folder.into()
                                                    }),
                                            ),
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
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(t("settings.sync.folder_hint")),
                            ),
                    ),
                );
            }
            SyncMode::Webdav => {
                let login = match &self.saved_user {
                    Some(user) => h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(t_args("settings.sync.login_saved_as", &[("user", user)])),
                        )
                        .child(
                            Button::new("sync-forget-login")
                                .ghost()
                                .small()
                                .label(t("settings.sync.forget_login"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.forget_login(window, cx)
                                    }),
                                ),
                        )
                        .into_any_element(),
                    None => h_flex()
                        .w_full()
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
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_login(window, cx)),
                                ),
                        )
                        .into_any_element(),
                };
                card = card.child(
                    div().border_t_1().border_color(border).child(
                        v_flex()
                            .w_full()
                            .px_4()
                            .py_3()
                            .gap_2p5()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(t("settings.sync.webdav_url")),
                                    )
                                    .child(Input::new(&self.url)),
                            )
                            .child(login)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(t("settings.sync.login_hint")),
                            ),
                    ),
                );
            }
            SyncMode::Rclone => {
                card = card.child(
                    div()
                        .border_t_1()
                        .border_color(border)
                        .px_4()
                        .py_3()
                        .child(self.render_rclone(cx)),
                );
            }
        }

        // Rows 4 & 5: Device name & playtime-only toggle (when active)
        if mode != SyncMode::Off {
            let device_row = field_row(
                RowState {
                    label: t("settings.sync.device_name"),
                    help: t("settings.sync.device_name_hint"),
                    ..RowState::default()
                },
                Input::new(&self.device)
                    .id("sync-device-name")
                    .aria_label(t("settings.sync.device_name"))
                    .w_64()
                    .into_any_element(),
                cx,
            );
            let playtime_row = field_row(
                RowState {
                    label: t("settings.sync.playtime_only"),
                    help: t("settings.sync.playtime_only_help"),
                    ..RowState::default()
                },
                Switch::new("sync-playtime-only")
                    .checked(prefs.playtime_only)
                    .accessibility_label(t("settings.sync.playtime_only"))
                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                        let on = *on;
                        backend(cx)
                            .settings
                            .update_app(|s| s.sync.playtime_only = on);
                        this.sync_now(false, window, cx);
                    }))
                    .into_any_element(),
                cx,
            );
            card = card
                .child(div().border_t_1().border_color(border).child(device_row))
                .child(div().border_t_1().border_color(border).child(playtime_row));
        }

        card
    }

    fn render_sync_status_header(
        &self,
        busy: bool,
        success: gpui_kit::Hsla,
        muted_fg: gpui_kit::Hsla,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        match &self.status.last {
            Some(last) if last.error.is_some() => {
                let error = last.error.clone().unwrap_or_default();
                let mut col = v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(Alert::error("sync-error", error).title(t("settings.sync.failed")));
                if let Some(ok) = &self.status.last_success {
                    col = col.child(muted(
                        t_args("settings.sync.last", &[("time", &local_time(ok))]),
                        cx,
                    ));
                }
                col.into_any_element()
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
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2p5()
                    .items_start()
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .text_color(success)
                            .mt_0p5(),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_sm().font_semibold().truncate().child(t_args(
                                "settings.sync.last",
                                &[("time", &local_time(&last.at))],
                            )))
                            .child(div().text_xs().text_color(muted_fg).child(summary)),
                    )
                    .into_any_element()
            }
            None if !busy => v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t("settings.sync.never")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.sync.description")),
                )
                .into_any_element(),
            None => v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t("settings.sync.syncing")),
                )
                .into_any_element(),
        }
    }

    /// rclone: found or how to get it, and the remote to sync with.
    fn render_rclone(&self, cx: &Context<Self>) -> impl IntoElement {
        let muted_fg = cx.theme().muted_foreground;
        let mut out = v_flex().w_full().gap_2p5();
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
                out = out
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(t("settings.sync.rclone_remote")),
                                    )
                                    .child(Tag::success().xsmall().child(found)),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .truncate()
                                            .child(info.program.clone()),
                                    )
                                    .child(
                                        Button::new("rclone-refresh")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::RefreshCw)
                                            .accessibility_label(t("settings.sync.rclone_again"))
                                            .tooltip(t("settings.sync.rclone_again"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.look_for_rclone(cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(Input::new(&self.remote));
                match &info.remotes {
                    Ok(remotes) if remotes.is_empty() => {
                        out = out.child(muted(t("settings.sync.rclone_no_remotes"), cx));
                    }
                    Ok(remotes) => {
                        out = out.child(h_flex().gap_1p5().flex_wrap().children(
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
                out = out.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(t("settings.sync.rclone_hint")),
                );
            }
        }
        out
    }
}
