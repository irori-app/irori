//! Ludusavi save backups of one game, and Ludusavi's status for the
//! settings page. The game page shows the panel as a sheet and the game's
//! settings embed it, so both live here.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, SharedString, Styled as _, Subscription, UniformListScrollHandle,
    Window, div, rems, uniform_list,
};
use irori_core::{GameId, GameInfo, Runner, util};
use irori_services::ludusavi::{Change, INSTALL_URL, Source, TitleMatch};
use irori_services::{BackupItem, GameBackups, LudusaviInfo, RestorePlan, SavesKeeper};
use serde_json::Value;

use crate::i18n::{t, t_args};
use crate::state::{backend, run, run_infallible};
use crate::{models, web_url};

/// Rows of the backups list shown before it scrolls.
const VISIBLE_ROWS: usize = 6;

/// Whether Ludusavi can back up `game`: browser games keep no saves, and
/// Ludusavi's data set knows programs, not the ROMs an emulator plays.
pub fn offers_backups(game: &GameInfo) -> bool {
    game.browser_url.is_none() && game.runner() != Runner::Emulation
}

/// Open the save backups of a game as a sheet.
pub fn open_save_backups_sheet(id: GameId, window: &mut Window, cx: &mut App) {
    let Some(game) = backend(cx).game(&id) else {
        return;
    };
    let title = t_args("ui.backups.sheet_title", &[("title", &game.title)]);
    let panel = cx.new(|cx| SaveBackupsPanel::new(id, window, cx));
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title(title.clone())
            .size(rems(32.))
            .child(panel.clone())
    });
}

/// Back up `id`'s saves now and say how it went.
pub fn back_up_now(id: &GameId, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let game = b
        .game(id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let id = id.clone();
    let task = run(cx, async move { b.back_up_saves(&id).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| report_backup(&game, result, window, cx));
        })
        .detach();
}

fn report_backup(
    game: &str,
    result: anyhow::Result<irori_services::SavesDone>,
    window: &mut Window,
    cx: &mut App,
) {
    let note = match result {
        Ok(done) if done.failed > 0 => Notification::warning(t_args(
            plural(
                done.failed,
                "ui.backups.some_failed_one",
                "ui.backups.some_failed_other",
            ),
            &[("count", &done.failed.to_string())],
        ))
        .title(t_args("ui.backups.backed_up_title", &[("title", game)])),
        Ok(done) if done.change == Change::Same => {
            Notification::success(t_args("ui.backups.unchanged", &[("title", game)]))
        }
        Ok(done) => Notification::success(t_args(
            plural(
                done.files,
                "ui.backups.backed_up_one",
                "ui.backups.backed_up_other",
            ),
            &[
                ("count", &done.files.to_string()),
                ("size", &util::format_bytes(done.bytes)),
                ("title", game),
            ],
        )),
        Err(e) => Notification::error(format!("{e:#}"))
            .title(t_args("ui.backups.backup_failed", &[("title", game)])),
    };
    window.push_notification(note, cx);
}

/// The key for `count`: `one` for a single item, else `other`.
fn plural(count: usize, one: &'static str, other: &'static str) -> &'static str {
    if count == 1 { one } else { other }
}

/// Days between two local dates, and the date and time of `rfc3339` in
/// local time.
fn local_when(
    rfc3339: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Option<(i64, String, String)> {
    let when = chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()?
        .with_timezone(&chrono::Local);
    let days = (now.date_naive() - when.date_naive()).num_days();
    Some((
        days,
        when.format("%Y-%m-%d").to_string(),
        when.format("%H:%M").to_string(),
    ))
}

/// "Today at 18:30", "Yesterday at 09:12" or "2024-11-02 at 10:15".
fn when_label(rfc3339: &str) -> SharedString {
    match local_when(rfc3339, chrono::Local::now()) {
        Some((0, _, time)) => t_args("ui.backups.today_at", &[("time", &time)]),
        Some((1, _, time)) => t_args("ui.backups.yesterday_at", &[("time", &time)]),
        Some((_, date, time)) => t_args("ui.backups.date_at", &[("date", &date), ("time", &time)]),
        None => rfc3339.to_string().into(),
    }
}

/// Who keeps a game's saves, in a sentence.
pub fn keeper_text(keeper: SavesKeeper) -> SharedString {
    let after = keeper.ludusavi_after_session;
    match keeper.cloud {
        Some((runner, auto)) => {
            let key = match (auto, after) {
                (true, true) => "ui.backups.keeper_cloud_and_ludusavi",
                (true, false) => "ui.backups.keeper_cloud",
                (false, true) => "ui.backups.keeper_ludusavi_cloud_manual",
                (false, false) => "ui.backups.keeper_manual_both",
            };
            t_args(key, &[("store", runner.store_name())])
        }
        None if after => t("ui.backups.keeper_ludusavi"),
        None => t("ui.backups.keeper_nobody"),
    }
}

fn open_folder(path: PathBuf, window: &mut Window, cx: &mut App) {
    if let Err(e) = open::that_detached(&path) {
        window.push_notification(
            Notification::error(format!("{}: {e}", path.display()))
                .title(t("ui.backups.open_failed")),
            cx,
        );
    }
}

/// Open Ludusavi's installation instructions in the browser.
pub fn open_install_page(window: &mut Window, cx: &mut App) {
    if let Some(url) = web_url(INSTALL_URL)
        && let Err(e) = open::that_detached(&url)
    {
        window.push_notification(
            Notification::error(format!("{url}: {e}")).title(t("ui.backups.open_link_failed")),
            cx,
        );
    }
}

/// Let the user choose the Ludusavi program, and remember it.
fn choose_program(
    window: &mut Window,
    cx: &mut App,
    then: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t("ui.backups.choose_program")),
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let _ = cx.update(|window, cx| {
                let value = path.to_string_lossy().into_owned();
                backend(cx).settings.update_app(|a| a.ludusavi_path = value);
                then(window, cx);
            });
        })
        .detach();
}

/// What the panel shows.
enum Load {
    Loading,
    /// Ludusavi isn't installed.
    Missing,
    Ready(GameBackups),
    Failed(SharedString),
}

/// Ludusavi backups of one game: its title in Ludusavi, the prefix it
/// searches, who keeps its saves, back up now, the backups with their
/// dates and sizes, and restoring one.
pub struct SaveBackupsPanel {
    id: GameId,
    game: String,
    load: Load,
    backing_up: bool,
    /// The backup a restore is being prepared or run for.
    restoring: Option<String>,
    /// Choosing the title by hand.
    picking: bool,
    search: Entity<InputState>,
    results: Option<Vec<TitleMatch>>,
    searching: bool,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SaveBackupsPanel {
    pub fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let game = backend(cx)
            .game(&id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| id.app_name.to_string());
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("ui.backups.search_placeholder"))
                .default_value(game.clone())
        });
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, _, e: &InputEvent, window, cx| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.run_search(window, cx);
                }
            }),
            cx.observe(&settings, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            id,
            game,
            load: Load::Loading,
            backing_up: false,
            restoring: None,
            picking: false,
            search,
            results: None,
            searching: false,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let id = self.id.clone();
        let task = run(cx, async move {
            if b.ludusavi().is_none() {
                return Ok(None);
            }
            b.ludusavi_backups(&id).await.map(Some)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.load = match result {
                    Ok(Some(backups)) => Load::Ready(backups),
                    Ok(None) => Load::Missing,
                    Err(e) => Load::Failed(format!("{e:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn back_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backing_up = true;
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let task = run(cx, async move { b.back_up_saves(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.backing_up = false;
                report_backup(&this.game, result, window, cx);
                this.reload(window, cx);
            });
        })
        .detach();
    }

    /// Ask what a restore would write, then ask the user.
    fn request_restore(&mut self, backup: BackupItem, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring = Some(backup.id.clone());
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let backup_id = backup.id.clone();
        let task = run(cx, async move {
            let plan = b.preview_restore(&id, &backup_id).await?;
            Ok((plan, b.saves_keeper(&id)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((plan, keeper)) => this.confirm_restore(backup, plan, keeper, window, cx),
                Err(e) => {
                    this.restoring = None;
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t_args(
                            "ui.backups.restore_failed",
                            &[("title", &this.game)],
                        )),
                        cx,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn confirm_restore(
        &mut self,
        backup: BackupItem,
        plan: RestorePlan,
        keeper: SavesKeeper,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restoring = None;
        cx.notify();
        let mut text = vec![t_args(
            plural(
                plan.files,
                "ui.backups.restore_files_one",
                "ui.backups.restore_files_other",
            ),
            &[
                ("when", &when_label(&backup.when)),
                ("count", &plan.files.to_string()),
                ("size", &util::format_bytes(plan.bytes)),
            ],
        )];
        if let Some(prefix) = &plan.elsewhere {
            text.push(t_args(
                "ui.backups.restore_elsewhere",
                &[("path", &prefix.display().to_string())],
            ));
        }
        if let Some((runner, true)) = keeper.cloud {
            text.push(t_args(
                "ui.backups.restore_cloud",
                &[("store", runner.store_name())],
            ));
        }
        let description: SharedString = text.join("\n\n").into();
        let title = t_args("ui.backups.restore_title", &[("title", &self.game)]);
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, backup) = (this.clone(), backup.id.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text(t("ui.backups.restore"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (this, backup) = (this.clone(), backup.clone());
                    // The dialog's own update is still running.
                    window.defer(cx, move |window, cx| {
                        this.update(cx, |p, cx| p.restore(backup, window, cx));
                    });
                    true
                })
        });
    }

    fn restore(&mut self, backup: String, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring = Some(backup.clone());
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let task = run(cx, async move { b.restore_saves(&id, &backup).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.restoring = None;
                let note = match result {
                    Ok(done) if done.failed > 0 => Notification::warning(t_args(
                        plural(
                            done.failed,
                            "ui.backups.some_failed_one",
                            "ui.backups.some_failed_other",
                        ),
                        &[("count", &done.failed.to_string())],
                    ))
                    .title(t_args(
                        "ui.backups.restored_title",
                        &[("title", &this.game)],
                    )),
                    Ok(done) => Notification::success(t_args(
                        plural(
                            done.files,
                            "ui.backups.restored_one",
                            "ui.backups.restored_other",
                        ),
                        &[("count", &done.files.to_string()), ("title", &this.game)],
                    )),
                    Err(e) => Notification::error(format!("{e:#}")).title(t_args(
                        "ui.backups.restore_failed",
                        &[("title", &this.game)],
                    )),
                };
                window.push_notification(note, cx);
                this.reload(window, cx);
            });
        })
        .detach();
    }

    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.search.read(cx).value().to_string();
        self.searching = true;
        cx.notify();
        let b = backend(cx);
        let task = run(cx, async move { b.ludusavi_search(&text).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.searching = false;
                match result {
                    Ok(found) => this.results = Some(found),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("ui.backups.search_failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_title(&mut self, title: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        backend(cx).set_ludusavi_title(&self.id, title.as_deref());
        self.picking = false;
        self.results = None;
        self.load = Load::Loading;
        self.reload(window, cx);
        cx.notify();
    }

    fn set_after_session(&mut self, on: bool, cx: &mut Context<Self>) {
        backend(cx).settings.set_game_value(
            &self.id,
            "iroriLudusaviAutoBackup",
            Some(Value::Bool(on)),
        );
        cx.notify();
    }
}

fn section_title(title: SharedString, cx: &App) -> AnyElement {
    div()
        .text_sm()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title)
        .into_any_element()
}

/// Ludusavi isn't installed: what it is and how to get it.
fn render_missing(
    on_found: impl Fn(&mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let retry = on_found.clone();
    v_flex()
        .gap_3()
        .child(
            Alert::info("ludusavi-missing", t("ui.backups.missing_description"))
                .title(t("ui.backups.missing_title")),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new("ludusavi-get")
                        .primary()
                        .icon(IconName::ExternalLink)
                        .label(t("ui.backups.get_ludusavi"))
                        .on_click(|_, window, cx| open_install_page(window, cx)),
                )
                .child(
                    Button::new("ludusavi-choose")
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label(t("ui.backups.choose_program"))
                        .on_click(move |_, window, cx| {
                            choose_program(window, cx, on_found.clone())
                        }),
                )
                .child(
                    Button::new("ludusavi-retry")
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label(t("ui.backups.check_again"))
                        .on_click(move |_, window, cx| retry(window, cx)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t("ui.backups.missing_help")),
        )
        .into_any_element()
}

impl SaveBackupsPanel {
    fn render_match(&self, backups: &GameBackups, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (name, how) = match &backups.title {
            Some(title) if backups.chosen => (title.clone(), t("ui.backups.match_chosen")),
            Some(title) => (title.clone(), t("ui.backups.match_automatic")),
            None => (
                t("ui.backups.no_match").to_string(),
                t("ui.backups.no_match_help"),
            ),
        };
        let mut col = v_flex()
            .gap_2()
            .child(section_title(t("ui.backups.match"), cx))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_semibold().truncate().child(name))
                            .child(div().text_xs().text_color(muted).child(how)),
                    )
                    .child(
                        Button::new("ludusavi-change-title")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t("ui.backups.change_match"))
                            .selected(self.picking)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.picking = !this.picking;
                                if this.picking && this.results.is_none() {
                                    this.run_search(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            );
        if backups.title.is_none() {
            col = col.child(Alert::warning(
                "ludusavi-no-match",
                t("ui.backups.no_match_alert"),
            ));
        }
        if self.picking {
            col = col.child(self.render_picker(backups.chosen, cx));
        }
        col.into_any_element()
    }

    fn render_picker(&self, chosen: bool, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let results = self.results.clone().unwrap_or_default();
        let list = if self.searching {
            div()
                .text_sm()
                .text_color(muted)
                .child(t("ui.backups.searching"))
                .into_any_element()
        } else if self.results.is_some() && results.is_empty() {
            div()
                .text_sm()
                .text_color(muted)
                .child(t("ui.backups.no_results"))
                .into_any_element()
        } else {
            v_flex()
                .gap_0p5()
                .children(results.into_iter().enumerate().map(|(i, found)| {
                    let label = match found.score {
                        Some(score) => t_args(
                            "ui.backups.result_score",
                            &[
                                ("title", &found.title),
                                ("score", &format!("{:.0}", score * 100.0)),
                            ],
                        ),
                        None => found.title.clone().into(),
                    };
                    let title = found.title.clone();
                    Button::new(("ludusavi-result", i))
                        .ghost()
                        .small()
                        .w_full()
                        .justify_start()
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_title(Some(title.clone()), window, cx)
                        }))
                }))
                .into_any_element()
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).small()),
                    )
                    .child(
                        Button::new("ludusavi-search")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label(t("ui.backups.search"))
                            .loading(self.searching)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run_search(window, cx)),
                            ),
                    ),
            )
            .child(list)
            .when(chosen, |col| {
                col.child(
                    h_flex().child(
                        Button::new("ludusavi-automatic")
                            .ghost()
                            .small()
                            .icon(IconName::Undo2)
                            .label(t("ui.backups.use_automatic"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_title(None, window, cx)
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_list(&self, backups: &GameBackups, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let header = section_title(
            t_args(
                "ui.backups.list_title",
                &[("count", &backups.backups.len().to_string())],
            ),
            cx,
        );
        if backups.backups.is_empty() {
            return v_flex()
                .gap_2()
                .child(header)
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("ui.backups.none_yet")),
                )
                .into_any_element();
        }
        let items = backups.backups.clone();
        let count = items.len();
        let this = cx.entity();
        let restoring = self.restoring.clone();
        let busy = self.backing_up || self.restoring.is_some();
        let (border, mono) = (cx.theme().border, cx.theme().mono_font_family.clone());
        let list = uniform_list("ludusavi-backups", count, move |range, _, _| {
            range
                .filter_map(|i| items.get(i).map(|b| (i, b.clone())))
                .map(|(i, backup)| {
                    let mut detail = vec![util::format_bytes(backup.bytes)];
                    detail.push(if backup.differential {
                        t("ui.backups.differential").to_string()
                    } else {
                        t("ui.backups.full").to_string()
                    });
                    if let Some(comment) = &backup.comment {
                        detail.push(comment.clone());
                    }
                    let loading = restoring.as_deref() == Some(backup.id.as_str());
                    let this = this.clone();
                    h_flex()
                        .w_full()
                        .gap_3()
                        .py_2()
                        .items_center()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .truncate()
                                                .child(when_label(&backup.when)),
                                        )
                                        .when(backup.locked, |row| {
                                            row.child(
                                                Icon::new(IconName::Lock)
                                                    .xsmall()
                                                    .text_color(muted),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(detail.join(" · ")),
                                ),
                        )
                        .child(
                            Button::new(("ludusavi-restore", i))
                                .outline()
                                .small()
                                .icon(IconName::ArchiveRestore)
                                .label(t("ui.backups.restore"))
                                .loading(loading)
                                .disabled(busy)
                                .on_click(move |_, window, cx| {
                                    let backup = backup.clone();
                                    this.update(cx, |p, cx| p.request_restore(backup, window, cx));
                                }),
                        )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .h(rems(3.75 * count.min(VISIBLE_ROWS) as f32));
        let folder = backups.folder.clone();
        v_flex()
            .gap_2()
            .child(header)
            .child(list)
            .when_some(folder, |col, folder| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .font_family(mono)
                        .truncate()
                        .child(folder.display().to_string()),
                )
            })
            .into_any_element()
    }
}

impl Render for SaveBackupsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.load {
            Load::Loading => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t("ui.backups.loading"))
                .into_any_element(),
            Load::Missing => {
                let this = cx.entity().downgrade();
                render_missing(
                    move |window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |p, cx| {
                                p.load = Load::Loading;
                                p.reload(window, cx);
                                cx.notify();
                            });
                        }
                    },
                    cx,
                )
            }
            Load::Failed(error) => v_flex()
                .gap_2()
                .child(
                    Alert::error("ludusavi-failed", error.clone())
                        .title(t("ui.backups.load_failed")),
                )
                .child(
                    h_flex().child(
                        Button::new("ludusavi-reload")
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("ui.backups.check_again"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load = Load::Loading;
                                this.reload(window, cx);
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element(),
            Load::Ready(backups) => {
                let backups = backups.clone();
                self.render_ready(&backups, cx)
            }
        };
        v_flex().w_full().gap_6().py_2().child(content)
    }
}

impl SaveBackupsPanel {
    fn render_ready(&self, backups: &GameBackups, cx: &mut Context<Self>) -> AnyElement {
        let b = backend(cx);
        let keeper = b.saves_keeper(&self.id);
        let after_session = b.settings.game(&self.id).ludusavi_auto_backup;
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let matched = backups.title.is_some();
        let busy = self.backing_up || self.restoring.is_some();
        let folder = backups.folder.clone();
        v_flex()
            .gap_6()
            .child(self.render_match(backups, cx))
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title(t("ui.backups.keeper"), cx))
                    .child(div().text_sm().child(keeper_text(keeper)))
                    .when_some(backups.wine_prefix.clone(), |col, prefix| {
                        col.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("ui.backups.prefix"))
                                .child(
                                    div()
                                        .font_family(mono.clone())
                                        .truncate()
                                        .child(prefix.display().to_string()),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("ludusavi-back-up")
                            .primary()
                            .icon(IconName::DatabaseBackup)
                            .label(if self.backing_up {
                                t("ui.backups.backing_up")
                            } else {
                                t("ui.backups.back_up_now")
                            })
                            .loading(self.backing_up)
                            .disabled(busy || !matched)
                            .on_click(cx.listener(|this, _, window, cx| this.back_up(window, cx))),
                    )
                    .child(
                        Button::new("ludusavi-open-folder")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("ui.backups.open_folder"))
                            .disabled(folder.is_none())
                            .on_click(move |_, window, cx| {
                                if let Some(folder) = folder.clone() {
                                    open_folder(folder, window, cx);
                                }
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Switch::new("ludusavi-after-session")
                            .label(t("ui.backups.after_session"))
                            .checked(after_session)
                            .disabled(!matched)
                            .on_change(
                                cx.listener(|this, v: &bool, _, cx| this.set_after_session(*v, cx)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("ui.backups.after_session_help")),
                    ),
            )
            .child(self.render_list(backups, cx))
            .into_any_element()
    }
}

/// Ludusavi's status on the settings page: where it was found and its
/// version, or how to get it; and the default for backing up after each
/// session.
pub struct LudusaviPanel {
    info: Option<Option<LudusaviInfo>>,
    _subscriptions: Vec<Subscription>,
}

impl LudusaviPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut previous = backend(cx).settings.app().ludusavi_path.clone();
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.observe_in(&settings, window, move |this, _, window, cx| {
                // Choosing another program looks again.
                let path = backend(cx).settings.app().ludusavi_path.clone();
                if path != previous {
                    previous = path;
                    this.refresh(window, cx);
                }
                cx.notify();
            }),
        ];
        let mut this = Self {
            info: None,
            _subscriptions: subscriptions,
        };
        this.refresh(window, cx);
        this
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.info = None;
        cx.notify();
        let b = backend(cx);
        let task = run_infallible(cx, async move { b.ludusavi_info().await });
        cx.spawn_in(window, async move |this, cx| {
            let info = task.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.info = Some(info);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for LudusaviPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let status = match &self.info {
            None => div()
                .text_sm()
                .text_color(muted)
                .child(t("ui.backups.looking"))
                .into_any_element(),
            Some(None) => {
                let this = cx.entity().downgrade();
                render_missing(
                    move |window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |p, cx| p.refresh(window, cx));
                        }
                    },
                    cx,
                )
            }
            Some(Some(info)) => {
                let found = match (&info.version, info.source) {
                    (Some(v), Source::Flatpak) => {
                        t_args("ui.backups.found_flatpak", &[("version", v)])
                    }
                    (Some(v), _) => t_args("ui.backups.found", &[("version", v)]),
                    (None, _) => t("ui.backups.found_no_version"),
                };
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Icon::new(IconName::CircleCheck).text_color(cx.theme().success))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_semibold().child(found))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .truncate()
                                    .child(info.command.clone()),
                            ),
                    )
                    .child(
                        Button::new("ludusavi-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .accessibility_label(t("ui.backups.check_again"))
                            .tooltip(t("ui.backups.check_again"))
                            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                    )
                    .into_any_element()
            }
        };
        let default_on = backend(cx).settings.app().game.ludusavi_auto_backup;
        v_flex().w_full().gap_4().child(status).child(
            v_flex()
                .gap_1()
                .child(
                    Switch::new("ludusavi-default-after-session")
                        .label(t("ui.backups.default_after_session"))
                        .checked(default_on)
                        .on_change(|on: &bool, _, cx| {
                            let on = *on;
                            backend(cx)
                                .settings
                                .update_app(|a| a.game.ludusavi_auto_backup = on);
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("ui.backups.default_after_session_help")),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_backups_in_local_time() {
        let now = chrono::Local::now();
        let iso = |d: chrono::DateTime<chrono::Local>| d.to_rfc3339();
        assert_eq!(local_when(&iso(now), now).unwrap().0, 0);
        let yesterday = now - chrono::Duration::days(1);
        assert_eq!(local_when(&iso(yesterday), now).unwrap().0, 1);
        let (days, date, time) = local_when(&iso(now - chrono::Duration::days(30)), now).unwrap();
        assert_eq!(days, 30);
        assert_eq!(date.len(), 10);
        assert_eq!(time.len(), 5);
        assert_eq!(local_when("yesterday", now), None);
    }

    #[test]
    fn says_who_keeps_the_saves() {
        crate::i18n::register(crate::i18n::CATALOGS);
        let keeper = |cloud, after| {
            keeper_text(SavesKeeper {
                cloud,
                ludusavi_after_session: after,
            })
        };
        let epic = |key: &str| t_args(key, &[("store", Runner::Legendary.store_name())]);
        assert_eq!(keeper(None, false), t("ui.backups.keeper_nobody"));
        assert_eq!(keeper(None, true), t("ui.backups.keeper_ludusavi"));
        assert_eq!(
            keeper(Some((Runner::Legendary, true)), true),
            epic("ui.backups.keeper_cloud_and_ludusavi")
        );
        assert_eq!(
            keeper(Some((Runner::Legendary, true)), false),
            epic("ui.backups.keeper_cloud")
        );
        assert_eq!(
            keeper(Some((Runner::Legendary, false)), true),
            epic("ui.backups.keeper_ludusavi_cloud_manual")
        );
        assert_eq!(
            keeper(Some((Runner::Legendary, false)), false),
            epic("ui.backups.keeper_manual_both")
        );
    }

    #[test]
    fn offers_backups_for_games_with_saves() {
        let game = |runner, browser: Option<&str>| GameInfo {
            runner: Some(runner),
            browser_url: browser.map(str::to_string),
            ..Default::default()
        };
        assert!(offers_backups(&game(Runner::Legendary, None)));
        assert!(offers_backups(&game(Runner::Sideload, None)));
        assert!(!offers_backups(&game(
            Runner::Sideload,
            Some("https://example.com")
        )));
        assert!(!offers_backups(&game(Runner::Emulation, None)));
    }
}
