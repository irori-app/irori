//! Settings -> Logs diagnostic hub (Proposal B): unified sources rail
//! (Irori current & previous sessions, crash reports, and recent game logs),
//! positive-logic `Write diagnostic logs` toggle (`disableLogs`), paired
//! `Show file | Logs folder` actions, and the shared [`LogViewer`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, rems,
};
use irori_core::{GameId, Paths};
use irori_ui::game_log::{LogSeveritySummary, rotated_log_path, summarize_log_file};
use irori_ui::i18n::{t, t_args};
pub use irori_ui::log_viewer::{LogViewer, LogViewerOptions};
use irori_ui::{backend, models};

/// Maximum recent game log files listed in the sidebar rail.
const MAX_RECENT_GAME_LOGS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceKind {
    AppCurrent,
    AppPrevious,
    Crash,
    Game(Option<GameId>),
}

#[derive(Debug, Clone)]
struct LogSource {
    id: SharedString,
    kind: SourceKind,
    title: SharedString,
    subtitle: SharedString,
    path: PathBuf,
    upload_name: Option<SharedString>,
    summary: LogSeveritySummary,
}

impl LogSource {
    fn app_current() -> Self {
        let path = Paths::get().launcher_log_file();
        Self {
            id: "app-current".into(),
            kind: SourceKind::AppCurrent,
            title: t("settings.logs.source.app_current"),
            subtitle: "irori.log".into(),
            path,
            upload_name: Some("Irori log".into()),
            summary: LogSeveritySummary::default(),
        }
    }

    fn game_id(&self) -> Option<GameId> {
        match &self.kind {
            SourceKind::Game(id) => id.clone(),
            _ => None,
        }
    }
}

/// Settings -> Logs page: diagnostic hub with sources rail and unified viewer.
pub struct LogsHubView {
    viewer: Entity<LogViewer>,
    sources: Vec<LogSource>,
    active: usize,
    _subscriptions: Vec<Subscription>,
}

impl LogsHubView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = LogSource::app_current();
        let viewer = cx.new(|cx| {
            LogViewer::new(
                initial.path.clone(),
                initial.upload_name.clone(),
                LogViewerOptions {
                    compact: false,
                    show_action_bar: false,
                    game_id: None,
                },
                window,
                cx,
            )
        });
        let settings = models(cx).settings.clone();
        let subs = vec![
            cx.observe(&viewer, |_, _, cx| cx.notify()),
            cx.observe(&settings, |_, _, cx| cx.notify()),
        ];
        Self {
            viewer,
            sources: vec![initial],
            active: 0,
            _subscriptions: subs,
        }
    }

    /// Start watching the active log and refresh the sidebar sources off the UI thread.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer.update(cx, |v, cx| v.start(window, cx));
        self.refresh_sources(window, cx);
    }

    /// Stop watching when leaving Settings -> Logs.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.viewer.update(cx, |v, _| v.stop());
    }

    fn refresh_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = Paths::get();
        let logs_dir = paths.logs_dir();
        let app_log = paths.launcher_log_file();
        let mut game_map: HashMap<PathBuf, (GameId, SharedString, bool)> = HashMap::new();
        for game in &models(cx).library.read(cx).snapshot.games {
            let id = game.id();
            let title: SharedString = game.title.clone().into();
            game_map.insert(paths.game_log_file(&id), (id.clone(), title.clone(), false));
            game_map.insert(paths.operation_log_file(&id), (id, title, true));
        }

        let app_current_label = t("settings.logs.source.app_current");
        let app_prev_label = t("settings.logs.source.app_previous");
        let launch_sub = t("settings.logs.source.game_launch");
        let install_sub = t("settings.logs.source.game_install");

        let task = irori_ui::run(cx, async move {
            let list = tokio::task::spawn_blocking(move || {
                scan_sources(
                    &logs_dir,
                    &app_log,
                    &game_map,
                    app_current_label,
                    app_prev_label,
                    launch_sub,
                    install_sub,
                )
            })
            .await
            .unwrap_or_default();
            Ok(list)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(sources) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let prev_path = this.sources.get(this.active).map(|s| s.path.clone());
                if !sources.is_empty() {
                    this.sources = sources;
                    this.active = prev_path
                        .and_then(|p| this.sources.iter().position(|s| s.path == p))
                        .unwrap_or(0);
                    this.sync_viewer_to_active(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_source(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.sources.len() || ix == self.active {
            return;
        }
        self.active = ix;
        self.sync_viewer_to_active(window, cx);
        cx.notify();
    }

    fn sync_viewer_to_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(src) = self.sources.get(self.active) else {
            return;
        };
        let path = src.path.clone();
        let upload_name = src.upload_name.clone();
        let game_id = src.game_id();
        self.viewer.update(cx, |v, cx| {
            v.set_source(path, upload_name, game_id, window, cx);
        });
    }

    fn delete_crash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let logs_dir = Paths::get().logs_dir();
        let task = irori_ui::run(cx, async move {
            tokio::task::spawn_blocking(move || irori_core::crash::delete_report(&logs_dir, &path))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| match res {
                Ok(()) => this.refresh_sources(window, cx),
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}")).title(t("settings.crash.delete_failed")),
                    cx,
                ),
            });
        })
        .detach();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let disable_logs = models(cx).settings.read(cx).app.disable_logs;
        let write_enabled = !disable_logs;
        let (can_copy, can_upload, is_copied, is_uploading, uploads_count) = {
            let v = self.viewer.read(cx);
            (
                v.can_copy(),
                v.can_upload(),
                v.is_copied(),
                v.is_uploading(),
                v.uploads().len(),
            )
        };
        let subtitle = self
            .sources
            .get(self.active)
            .map(|s| format!("{} · {}", s.title, s.subtitle))
            .unwrap_or_else(|| t("settings.page.logs_description").to_string());
        let viewer_copy = self.viewer.clone();
        let viewer_upload = self.viewer.clone();
        let viewer_editor = self.viewer.clone();
        let viewer_reveal = self.viewer.clone();

        h_flex()
            .w_full()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(t("settings.page.logs")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(subtitle),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .child(
                        h_flex()
                            .px_2p5()
                            .h(rems(1.85))
                            .gap_2()
                            .items_center()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(if write_enabled {
                                theme.border
                            } else {
                                theme.warning.opacity(0.45)
                            })
                            .bg(if write_enabled {
                                theme.muted
                            } else {
                                theme.warning.opacity(0.1)
                            })
                            .child(
                                Switch::new("logs-write-diagnostic")
                                    .small()
                                    .checked(write_enabled)
                                    .label(t("settings.logs.write_diagnostic"))
                                    .tooltip(t("settings.logs.write_diagnostic_tip"))
                                    .on_change(|enabled, _, cx| {
                                        let disable = !*enabled;
                                        backend(cx)
                                            .settings
                                            .update_app(|s| s.disable_logs = disable);
                                    }),
                            ),
                    )
                    .child(
                        Button::new("hub-copy")
                            .small()
                            .outline()
                            .icon(if is_copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .label(if is_copied {
                                t("settings.common.copied")
                            } else {
                                t("settings.common.copy")
                            })
                            .disabled(!can_copy)
                            .on_click(move |_, window, cx| {
                                viewer_copy.update(cx, |v, cx| v.copy(window, cx));
                            }),
                    )
                    .child(
                        Button::new("hub-upload")
                            .small()
                            .outline()
                            .icon(IconName::Upload)
                            .label(t("settings.logs.upload.action"))
                            .loading(is_uploading)
                            .disabled(!can_upload)
                            .on_click(move |_, window, cx| {
                                viewer_upload.update(cx, |v, cx| v.confirm_upload(window, cx));
                            }),
                    )
                    .when(uploads_count > 0, |row| {
                        let pop = self
                            .viewer
                            .update(cx, |v, cx| v.render_uploads_popover("hub-uploads-pop", cx));
                        row.child(pop)
                    })
                    .child(
                        Button::new("hub-editor")
                            .small()
                            .ghost()
                            .icon(IconName::ExternalLink)
                            .tooltip(t("settings.logs.open_in_editor"))
                            .on_click(move |_, window, cx| {
                                viewer_editor.update(cx, |v, cx| v.open_in_editor(window, cx));
                            }),
                    )
                    .child(
                        ButtonGroup::new("hub-folder-group")
                            .outline()
                            .small()
                            .child(
                                Button::new("log-reveal")
                                    .icon(IconName::FileSearch)
                                    .label(t("settings.logs.show_file"))
                                    .tooltip(t("settings.logs.show_file"))
                                    .on_click(move |_, _, cx| {
                                        viewer_reveal.update(cx, |v, cx| v.reveal_file(cx));
                                    }),
                            )
                            .child(
                                Button::new("log-open-folder")
                                    .icon(IconName::FolderOpen)
                                    .label(t("settings.logs.logs_folder"))
                                    .tooltip(t("settings.logs.logs_folder_tip"))
                                    .on_click(|_, window, cx| {
                                        LogViewer::open_logs_folder(window, cx);
                                    }),
                            ),
                    ),
            )
    }

    fn render_sources_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let (
            border,
            muted_bg,
            muted_fg,
            active_bg,
            hover_bg,
            danger,
            warning,
            success,
            radius,
            radius_lg,
        ) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted,
                theme.muted_foreground,
                theme.list_active,
                theme.list_hover,
                theme.danger,
                theme.warning,
                theme.success,
                theme.radius,
                theme.radius_lg,
            )
        };

        let mut app_items = Vec::new();
        let mut crash_items = Vec::new();
        let mut game_items = Vec::new();
        for (ix, src) in self.sources.iter().enumerate() {
            match src.kind {
                SourceKind::AppCurrent | SourceKind::AppPrevious => app_items.push((ix, src)),
                SourceKind::Crash => crash_items.push((ix, src)),
                SourceKind::Game(_) => game_items.push((ix, src)),
            }
        }

        let mut rail = v_flex()
            .id("logs-sources-rail")
            .w(rems(13.5))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .p_2()
            .gap_1()
            .rounded(radius_lg)
            .border_1()
            .border_color(border)
            .bg(muted_bg)
            .overflow_y_scrollbar();

        let render_group = |title: SharedString,
                            items: Vec<(usize, &LogSource)>,
                            first: bool,
                            cx: &mut Context<Self>|
         -> AnyElement {
            v_flex()
                .gap_0p5()
                .when(!first, |v| v.pt_2())
                .child(
                    div()
                        .px_2()
                        .pb_0p5()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted_fg)
                        .child(title),
                )
                .children(items.into_iter().map(|(ix, src)| {
                    let is_active = ix == self.active;
                    let (badge_text, badge_color) = if src.summary.errors > 0 {
                        (
                            t_args(
                                "settings.logs.badge_err",
                                &[("count", &src.summary.errors.to_string())],
                            ),
                            danger,
                        )
                    } else if src.summary.warnings > 0 {
                        (
                            t_args(
                                "settings.logs.badge_warn",
                                &[("count", &src.summary.warnings.to_string())],
                            ),
                            warning,
                        )
                    } else {
                        (SharedString::from(src.summary.lines.to_string()), success)
                    };
                    h_flex()
                        .id(ElementId::Name(format!("log-src-{}", src.id).into()))
                        .w_full()
                        .px_2()
                        .py_1p5()
                        .gap_2()
                        .items_center()
                        .rounded(radius)
                        .cursor_pointer()
                        .when(is_active, |d| d.bg(active_bg))
                        .hover(move |s| s.bg(hover_bg))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_source(ix, window, cx);
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_medium()
                                        .truncate()
                                        .child(src.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .truncate()
                                        .child(src.subtitle.clone()),
                                ),
                        )
                        .when(src.summary.lines > 0, |r| {
                            r.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(99.))
                                    .bg(badge_color.opacity(0.15))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(badge_color)
                                    .child(badge_text),
                            )
                        })
                }))
                .into_any_element()
        };

        if !app_items.is_empty() {
            rail = rail.child(render_group(
                t("settings.logs.group.app"),
                app_items,
                true,
                cx,
            ));
        }
        if !crash_items.is_empty() {
            rail = rail.child(render_group(
                t("settings.logs.group.crashes"),
                crash_items,
                false,
                cx,
            ));
        }
        if !game_items.is_empty() {
            rail = rail.child(render_group(
                t("settings.logs.group.games"),
                game_items,
                false,
                cx,
            ));
        }
        rail.into_any_element()
    }

    fn render_crash_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let src = self.sources.get(self.active)?;
        let SourceKind::Crash = src.kind else {
            return None;
        };
        let theme = cx.theme();
        let review_path = src.path.clone();
        let delete_path = src.path.clone();
        Some(
            h_flex()
                .w_full()
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.warning.opacity(0.4))
                .bg(theme.warning.opacity(0.1))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(theme.warning),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .truncate()
                        .child(t("settings.crash.recent_help")),
                )
                .child(
                    Button::new("hub-crash-review")
                        .xsmall()
                        .outline()
                        .label(t("settings.crash.review_action"))
                        .on_click(move |_, window, cx| {
                            crate::open_crash_report_sheet(review_path.clone(), window, cx);
                        }),
                )
                .child(
                    Button::new("hub-crash-delete")
                        .xsmall()
                        .ghost()
                        .icon(IconName::Trash)
                        .tooltip(t("settings.crash.delete"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.delete_crash(delete_path.clone(), window, cx);
                        })),
                )
                .into_any_element(),
        )
    }
}

impl Render for LogsHubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_rail = window.viewport_size().width >= window.rem_size() * 58.;
        v_flex()
            .id("logs-hub")
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap_3()
            .child(self.render_header(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .gap_3()
                    .when(show_rail, |row| row.child(self.render_sources_rail(cx)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .gap_2()
                            .children(self.render_crash_bar(cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .child(self.viewer.clone()),
                            ),
                    ),
            )
    }
}

fn scan_sources(
    logs_dir: &Path,
    app_log: &Path,
    game_map: &HashMap<PathBuf, (GameId, SharedString, bool)>,
    app_current_label: SharedString,
    app_prev_label: SharedString,
    launch_sub: SharedString,
    install_sub: SharedString,
) -> Vec<LogSource> {
    let mut out = vec![LogSource {
        id: "app-current".into(),
        kind: SourceKind::AppCurrent,
        title: app_current_label,
        subtitle: "irori.log".into(),
        path: app_log.to_path_buf(),
        upload_name: Some("Irori log".into()),
        summary: summarize_log_file(app_log),
    }];

    let old_log = rotated_log_path(app_log);
    if old_log.is_file() {
        out.push(LogSource {
            id: "app-previous".into(),
            kind: SourceKind::AppPrevious,
            title: app_prev_label,
            subtitle: "irori.log.old".into(),
            summary: summarize_log_file(&old_log),
            path: old_log,
            upload_name: Some("Irori previous log".into()),
        });
    }

    for (ix, report) in irori_core::crash::list_reports(logs_dir)
        .into_iter()
        .enumerate()
    {
        let when = crate::crash_report::format_crash_timestamp(report.timestamp)
            .unwrap_or_else(|| "-".into());
        let title = report
            .summary
            .message
            .as_deref()
            .filter(|m| !m.trim().is_empty() && *m != "(no message)")
            .or(report.summary.location.as_deref())
            .unwrap_or("Crash report");
        out.push(LogSource {
            id: format!("crash-{ix}").into(),
            kind: SourceKind::Crash,
            title: SharedString::from(title.to_string()),
            subtitle: SharedString::from(when),
            summary: summarize_log_file(&report.path),
            path: report.path,
            upload_name: Some("Irori crash report".into()),
        });
    }

    let games_dir = logs_dir.join("games");
    if let Ok(entries) = std::fs::read_dir(&games_dir) {
        let mut files: Vec<(SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("log") {
                    return None;
                }
                let mtime = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                Some((mtime, p))
            })
            .collect();
        files.sort_by_key(|b| std::cmp::Reverse(b.0));
        for (ix, (_, path)) in files.into_iter().take(MAX_RECENT_GAME_LOGS).enumerate() {
            let (id, title, is_install) = if let Some((id, title, is_install)) = game_map.get(&path)
            {
                (Some(id.clone()), title.clone(), *is_install)
            } else {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("game")
                    .to_string();
                let is_install = stem.ends_with(".install");
                let clean = stem.strip_suffix(".install").unwrap_or(&stem).to_string();
                (None, SharedString::from(clean), is_install)
            };
            let upload_name = format!("Log of {title}").into();
            out.push(LogSource {
                id: format!("game-{ix}").into(),
                kind: SourceKind::Game(id),
                title,
                subtitle: if is_install {
                    install_sub.clone()
                } else {
                    launch_sub.clone()
                },
                summary: summarize_log_file(&path),
                path,
                upload_name: Some(upload_name),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_app_rotated_crash_and_game_sources() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path();
        let app_log = logs.join("irori.log");
        let old_log = logs.join("irori.log.old");
        let games = logs.join("games");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(&app_log, "2026-11-28T11:32:01.000Z  INFO irori: starting\n").unwrap();
        std::fs::write(
            &old_log,
            "2026-11-28T02:10:01.000Z  WARN irori: old warning\n",
        )
        .unwrap();
        let game_log = games.join("epic-hades2.log");
        std::fs::write(&game_log, "0124:err:module:import_dll missing\n").unwrap();

        let mut map = HashMap::new();
        let gid = GameId {
            runner: irori_core::Runner::Legendary,
            app_name: "hades2".into(),
        };
        map.insert(
            game_log.clone(),
            (gid.clone(), SharedString::from("Hades II"), false),
        );

        let sources = scan_sources(
            logs,
            &app_log,
            &map,
            "Current".into(),
            "Previous".into(),
            "Launch".into(),
            "Install".into(),
        );
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].kind, SourceKind::AppCurrent);
        assert_eq!(sources[0].summary.lines, 1);
        assert_eq!(sources[1].kind, SourceKind::AppPrevious);
        assert_eq!(sources[1].summary.warnings, 1);
        assert_eq!(sources[2].kind, SourceKind::Game(Some(gid)));
        assert_eq!(sources[2].title.as_ref(), "Hades II");
        assert_eq!(sources[2].summary.errors, 1);
    }
}
