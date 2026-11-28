use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, ClipboardItem, Context, Window};
use irori_core::crash::redact_home_paths;
use irori_extras::logs as log_uploads;

use super::LogViewer;
use crate::backend;
use crate::game_log::LevelFilter;
use crate::i18n::{t, t_args};

impl LogViewer {
    pub(super) fn toggle_follow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow = !self.follow;
        if self.follow {
            self.start(window, cx);
        } else {
            self.watch_task = None;
        }
        cx.notify();
    }

    pub fn toggle_write_logs(&mut self, enable: bool, cx: &mut Context<Self>) {
        let disable = !enable;
        backend(cx)
            .settings
            .update_app(|s| s.disable_logs = disable);
        cx.notify();
    }

    pub fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text: String = self
            .rows
            .iter()
            .filter_map(|r| self.lines.get(r.entry_idx))
            .map(|l| self.maybe_redact(&l.raw))
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        window.push_notification(
            Notification::success(t_args(
                "ui.logs.copied_lines",
                &[("count", &self.rows.len().to_string())],
            )),
            cx,
        );
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_line(&self, row_idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(row_idx) else {
            return;
        };
        let Some(line) = self.lines.get(row.entry_idx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(self.maybe_redact(&line.raw)));
        window.push_notification(Notification::success(t("ui.logs.line_copied")), cx);
    }

    pub(super) fn copy_context_snippet(
        &self,
        row_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.get(row_idx) else {
            return;
        };
        let start = row.entry_idx.saturating_sub(2);
        let end = (row.entry_idx + 2).min(self.lines.len().saturating_sub(1));
        let body = self.lines[start..=end]
            .iter()
            .map(|l| self.maybe_redact(&l.raw))
            .collect::<Vec<_>>()
            .join("\n");
        let fenced = format!("```text\n{body}\n```");
        cx.write_to_clipboard(ClipboardItem::new_string(fenced));
        window.push_notification(Notification::success(t("ui.logs.snippet_copied")), cx);
    }

    pub fn open_in_editor(&self, window: &mut Window, cx: &mut App) {
        if self.path.exists() {
            cx.open_with_system(&self.path);
        } else {
            window.push_notification(Notification::info(t("ui.logs.empty_title")), cx);
        }
    }

    pub fn reveal_file(&self, cx: &mut App) {
        if self.path.exists() {
            cx.reveal_path(&self.path);
        } else if let Some(dir) = self.path.parent() {
            cx.open_with_system(dir);
        }
    }

    pub fn open_logs_folder(window: &mut Window, cx: &mut App) {
        let dir = irori_core::Paths::get().logs_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            window.push_notification(
                Notification::error(format!("{e}")).title(t("ui.logs.open_folder_failed")),
                cx,
            );
            return;
        }
        cx.open_with_system(&dir);
    }

    pub(super) fn load_uploads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.upload_name.clone() else {
            self.uploads.clear();
            return;
        };
        let task = crate::run(cx, async move { log_uploads::list_uploaded().await });
        cx.spawn_in(window, async move |this, cx| {
            let uploads = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.uploads = uploads
                    .into_iter()
                    .filter(|u| u.name == name.as_ref())
                    .collect();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn confirm_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_upload() {
            return;
        }
        let weak = cx.entity().downgrade();
        let desc = if self.redact_home {
            t_args(
                "ui.logs.upload_description_redacted",
                &[("days", &log_uploads::EXPIRY_DAYS.to_string())],
            )
        } else {
            t_args(
                "ui.logs.upload_description_raw",
                &[("days", &log_uploads::EXPIRY_DAYS.to_string())],
            )
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .title(t("ui.logs.upload_title"))
                .description(desc.clone())
                .show_cancel(true)
                .cancel_text(t("ui.backups.cancel"))
                .ok_text(t("ui.logs.upload_confirm"))
                .on_ok(move |_, window, cx| {
                    let weak = weak.clone();
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.upload(window, cx));
                        });
                    });
                    true
                })
        });
    }

    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.upload_name.clone() else {
            return;
        };
        if self.uploading {
            return;
        }
        self.uploading = true;
        let path = self.path.clone();
        let redact = self.redact_home;
        let home = self.home_dir.clone();
        let task = crate::run(cx, async move {
            let raw = log_uploads::read_tail(&path, log_uploads::MAX_UPLOAD_BYTES).await?;
            let content = if redact {
                redact_home_paths(&raw, home.as_deref())
            } else {
                raw
            };
            log_uploads::upload_text(&content, name.as_ref()).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.uploading = false;
                match result {
                    Ok(log) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(log.url.clone()));
                        window.push_notification(
                            Notification::success(t_args(
                                "ui.logs.upload_link_copied",
                                &[("url", &log.url)],
                            ))
                            .title(t("ui.logs.upload_done")),
                            cx,
                        );
                        this.uploads.insert(0, log);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("ui.logs.upload_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn forget_upload(&mut self, token: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.deleting_uploads.insert(token.clone()) {
            return;
        }
        cx.notify();
        let task = crate::run(cx, {
            let token = token.clone();
            async move { log_uploads::delete_uploaded(&token).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.deleting_uploads.remove(&token);
                match result {
                    Ok(_) => this.uploads.retain(|u| u.delete_token != token),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("ui.logs.upload_delete_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn jump_error(&mut self, next: bool, cx: &mut Context<Self>) {
        if self.error_rows.is_empty() {
            return;
        }
        let len = self.error_rows.len();
        let pos = match self.error_cursor {
            Some(c) if next => (c + 1) % len,
            Some(c) => (c + len - 1) % len,
            None if next => 0,
            None => len - 1,
        };
        self.error_cursor = Some(pos);
        if let Some(&row_idx) = self.error_rows.get(pos) {
            self.scroll_to_row(row_idx, cx);
        }
    }

    pub(super) fn jump_match(&mut self, next: bool, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len();
        self.match_cursor = if next {
            (self.match_cursor + 1) % len
        } else {
            (self.match_cursor + len - 1) % len
        };
        self.scroll_to_row(self.match_cursor, cx);
    }

    pub(super) fn reset_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exclude = false;
        self.level = LevelFilter::All;
        self.selected_modules.clear();
        self.selected_row = None;
        self.error_cursor = None;
        self.match_cursor = 0;
        self.query.update(cx, |s, cx| s.set_value("", window, cx));
        self.refilter(cx);
    }
}
