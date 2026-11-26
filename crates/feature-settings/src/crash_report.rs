//! Crash report sheet: readable summary, optional home-path redaction, and
//! one-click Copy, Show file, Upload and Report on GitHub actions.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, rems,
};
use irori_core::crash::{CrashSummary, current_home_dir, parse_summary, redact_home_paths};
use irori_extras::logs::UploadedLog;
use irori_runtime::sysinfo;
use irori_services::app_update::InstallKind;
use irori_ui::i18n::{t, t_args};

use crate::feedback::{self, ReportFacts};
use crate::widgets::panel;

/// Format a Unix-seconds timestamp using Irori's localized date format plus
/// UTC time (`HH:MM UTC`).
pub(crate) fn format_crash_timestamp(ts: u64) -> Option<String> {
    if ts == 0 {
        return None;
    }
    let rfc = irori_core::util::rfc3339_from_unix(i64::try_from(ts).ok()?)?;
    let day = irori_ui::dates::localized_day(&rfc);
    Some(match rfc.get(11..16) {
        Some(hhmm) => format!("{day} ({hhmm} UTC)"),
        None => day,
    })
}

fn timestamp_from_path(path: &std::path::Path) -> u64 {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return 0;
    };
    let rest = name.strip_prefix(irori_core::crash::PREFIX).unwrap_or(name);
    let num = rest
        .strip_suffix(irori_core::crash::REPORTED)
        .or_else(|| rest.strip_suffix(".txt"))
        .unwrap_or(rest);
    num.parse().unwrap_or(0)
}

/// Open the crash report sheet for `path`.
pub fn open_crash_report_sheet(path: PathBuf, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| CrashReportSheet::new(path, window, cx));
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title(t("settings.crash.title"))
            .size(rems(46.))
            .child(view.clone())
    });
}

enum LoadState {
    Loading,
    Missing,
    Failed(SharedString),
    Loaded,
}

pub struct CrashReportSheet {
    path: PathBuf,
    timestamp: u64,
    state: LoadState,
    raw_text: String,
    redacted_text: String,
    raw_summary: CrashSummary,
    redacted_summary: CrashSummary,
    redact_home: bool,
    copied: bool,
    reporting: bool,
    uploading: bool,
    uploaded: Option<UploadedLog>,
}

impl CrashReportSheet {
    fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let timestamp = timestamp_from_path(&path);
        let mut this = Self {
            path,
            timestamp,
            state: LoadState::Loading,
            raw_text: String::new(),
            redacted_text: String::new(),
            raw_summary: CrashSummary::default(),
            redacted_summary: CrashSummary::default(),
            redact_home: true,
            copied: false,
            reporting: false,
            uploading: false,
            uploaded: None,
        };
        this.load(window, cx);
        this
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let home = current_home_dir();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(&path) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(raw) => {
                        let redacted = redact_home_paths(&raw, home.as_deref());
                        this.raw_summary = parse_summary(&raw);
                        this.redacted_summary = parse_summary(&redacted);
                        this.raw_text = raw;
                        this.redacted_text = redacted;
                        this.state = LoadState::Loaded;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        this.state = LoadState::Missing;
                    }
                    Err(e) => {
                        this.state = LoadState::Failed(format!("{e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn active_text(&self) -> &str {
        if self.redact_home {
            &self.redacted_text
        } else {
            &self.raw_text
        }
    }

    fn active_summary(&self) -> &CrashSummary {
        if self.redact_home {
            &self.redacted_summary
        } else {
            &self.raw_summary
        }
    }

    fn copy_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.active_text().to_string();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn report_on_github(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reporting {
            return;
        }
        let text = self.active_text().to_string();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            window.push_notification(
                Notification::info(t("settings.crash.copied_for_github")),
                cx,
            );
        }
        self.reporting = true;
        cx.notify();
        let summary = self.active_summary().clone();
        let task = irori_ui::run(cx, async move {
            let info = sysinfo::get(false).await;
            let install = tokio::task::spawn_blocking(InstallKind::detect)
                .await
                .unwrap_or(InstallKind::Manual);
            anyhow::Ok(feedback::crash_report_url(
                &ReportFacts::new(&info, &install),
                &summary,
            ))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.reporting = false;
                match result {
                    Ok(url) => cx.open_url(&url),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.about.feedback.report_failed")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn confirm_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .title(t("settings.logs.upload.title"))
                .description(t("settings.logs.upload.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.logs.upload.confirm"))
                .on_ok(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.upload(window, cx)).ok();
                    true
                })
        });
    }

    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        let content = self.active_text().to_string();
        if content.is_empty() {
            return;
        }
        self.uploading = true;
        cx.notify();
        let task = irori_ui::run(cx, async move {
            irori_extras::logs::upload_text(&content, "Irori crash report").await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.uploading = false;
                match result {
                    Ok(log) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(log.url.clone()));
                        window.push_notification(
                            Notification::success(t_args(
                                "settings.logs.upload.link_copied",
                                &[("url", &log.url)],
                            ))
                            .title(t("settings.logs.upload.done")),
                            cx,
                        );
                        this.uploaded = Some(log);
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.logs.upload.failed")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted_fg, bg, border, radius, mono) = (
            theme.muted_foreground,
            theme.secondary,
            theme.border,
            theme.radius_lg,
            theme.mono_font_family.clone(),
        );
        let summary = self.active_summary();
        let when = format_crash_timestamp(self.timestamp).unwrap_or_else(|| {
            self.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("-")
                .to_string()
        });
        let version = summary
            .version
            .as_deref()
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        let message = summary.message.as_deref().unwrap_or("(no message)");
        let location = summary.location.as_deref().unwrap_or("unknown");

        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded(radius)
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(t_args("settings.crash.saved_at", &[("when", &when)])),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(format!("v{version}")),
                    ),
            )
            .child(div().text_sm().font_medium().child(message.to_string()))
            .child(
                div()
                    .text_xs()
                    .font_family(mono)
                    .text_color(muted_fg)
                    .child(t_args("settings.crash.location", &[("location", location)])),
            )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let loaded = matches!(self.state, LoadState::Loaded);
        h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("crash-report-github")
                    .primary()
                    .small()
                    .icon(IconName::Bug)
                    .label(t("settings.crash.report_github"))
                    .loading(self.reporting)
                    .disabled(self.reporting || !loaded)
                    .on_click(cx.listener(|this, _, window, cx| this.report_on_github(window, cx))),
            )
            .child(
                Button::new("crash-copy")
                    .outline()
                    .small()
                    .icon(if self.copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .label(if self.copied {
                        t("settings.common.copied")
                    } else {
                        t("settings.crash.copy")
                    })
                    .disabled(!loaded)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_report(window, cx))),
            )
            .child(
                Button::new("crash-reveal")
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(t("settings.logs.show_file"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.path.exists() {
                            cx.reveal_path(&this.path);
                        } else if let Some(dir) = this.path.parent() {
                            cx.open_with_system(dir);
                        }
                    })),
            )
            .child(
                Button::new("crash-upload")
                    .outline()
                    .small()
                    .icon(IconName::Upload)
                    .label(t("settings.logs.upload.action"))
                    .loading(self.uploading)
                    .disabled(self.uploading || !loaded)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_upload(window, cx))),
            )
    }
}

impl Render for CrashReportSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted_fg, border, radius, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.radius_lg,
            theme.mono_font_family.clone(),
        );
        let body = match &self.state {
            LoadState::Loading => div()
                .p_4()
                .text_sm()
                .text_color(muted_fg)
                .child(t("settings.common.loading"))
                .into_any_element(),
            LoadState::Missing => div()
                .p_4()
                .text_sm()
                .text_color(muted_fg)
                .child(t_args(
                    "settings.logs.missing",
                    &[("path", &self.path.display().to_string())],
                ))
                .into_any_element(),
            LoadState::Failed(err) => div()
                .p_4()
                .text_sm()
                .text_color(muted_fg)
                .child(t_args("settings.logs.read_failed", &[("error", err)]))
                .into_any_element(),
            LoadState::Loaded => {
                let text = self.active_text().to_string();
                v_flex()
                    .w_full()
                    .gap_0p5()
                    .p_3()
                    .children(text.lines().map(|line| {
                        div()
                            .w_full()
                            .text_xs()
                            .font_family(mono.clone())
                            .whitespace_normal()
                            .child(line.to_string())
                    }))
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(self.render_summary(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(t("settings.crash.privacy_note")),
            )
            .child(self.render_actions(cx))
            .when_some(self.uploaded.as_ref(), |v, u| {
                let url = u.url.clone();
                v.child(
                    panel(cx).child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(url.clone()),
                            )
                            .child(
                                Button::new("crash-upload-copy")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .tooltip(t("settings.logs.upload.copy_link"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            url.clone(),
                                        ))
                                    }),
                            ),
                    ),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Checkbox::new("crash-redact-home")
                            .label(t("settings.crash.redact_home"))
                            .checked(self.redact_home)
                            .on_change(cx.listener(|this, v: &bool, _, cx| {
                                this.redact_home = *v;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("crash-open-logs")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ScrollText)
                            .label(t("settings.crash.open_logs"))
                            .on_click(|_, window, cx| {
                                window.close_sheet(cx);
                                irori_ui::navigate(irori_ui::Route::Settings("logs".into()), cx);
                            }),
                    ),
            )
            .child(
                div()
                    .id("crash-report-body")
                    .flex_1()
                    .min_h_0()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .overflow_y_scrollbar()
                    .child(body),
            )
    }
}
