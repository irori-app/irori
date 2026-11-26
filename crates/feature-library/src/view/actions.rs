//! Game commands, launching, keyboard navigation, and layout or filter
//! actions on [`LibraryView`].

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::{Context, Entity, Focusable as _, ScrollStrategy, SharedString, Window};
use irori_core::settings::{CoverShape, FocusCoverSize, LibraryLayout, ListRowSize};
use irori_core::{GameId, GameInfo, Runner, UpdateRequest};
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

use super::{Item, LibraryView, persist_ui};
use crate::LibraryEvent;
use crate::chips::{self, PresetChip};
use crate::commands::{self, GameCommand};
use crate::dialogs;
use crate::filter::{Grouping, ViewFilter};
use crate::filters_panel::FilterModel;
use crate::grid::{self, Move};
use crate::launch::{self, LaunchChoice};
use crate::views::{self, TabKey, TabViewPrefs};

impl LibraryView {
    /// Run a command on a game. Commands owned by other features are emitted
    /// as [`LibraryEvent`]s.
    pub fn run_command(
        &mut self,
        id: &GameId,
        command: GameCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(game) = self.game(id, cx) else {
            return;
        };
        let b = backend(cx);
        let id = id.clone();
        let title = game.title.clone();
        match command {
            GameCommand::Play => self.play(&game, window, cx),
            GameCommand::BackToGame => irori_ui::game_window::back_to_game(id, window, cx),
            GameCommand::Stop => {
                let task = irori_ui::run(cx, async move { b.stop(&id).await });
                let failed = t_args("library.error.stop", &[("title", &title)]);
                report(task, failed, window, cx);
            }
            GameCommand::Install => cx.emit(LibraryEvent::RequestInstall(id)),
            GameCommand::Update => {
                irori_ui::state::run_infallible(cx, async move {
                    b.enqueue_update(id, title, UpdateRequest::default())
                })
                .detach();
            }
            GameCommand::RemoveFromQueue => {
                irori_ui::state::run_infallible(cx, async move { b.cancel_download(&id, true) })
                    .detach();
            }
            GameCommand::OpenPage => navigate(Route::Game(id), cx),
            GameCommand::Settings => navigate(Route::GameSettings(id, "general".into()), cx),
            GameCommand::Logs => cx.emit(LibraryEvent::RequestLogs(id)),
            GameCommand::ToggleFavourite => {
                let on = !models(cx).library.read(cx).is_favourite(&id);
                irori_ui::state::run_infallible(
                    cx,
                    async move { b.userdata.set_favourite(&id, on) },
                )
                .detach();
            }
            GameCommand::ToggleHidden => {
                let on = !models(cx).library.read(cx).is_hidden(&id);
                irori_ui::state::run_infallible(cx, async move { b.userdata.set_hidden(&id, on) })
                    .detach();
            }
            GameCommand::ToggleCategory(name) => {
                let member = models(cx)
                    .library
                    .read(cx)
                    .user
                    .categories
                    .get(&name)
                    .is_some_and(|set| set.contains(&id.key()));
                irori_ui::state::run_infallible(cx, async move {
                    b.userdata.set_category(&id, &name, !member)
                })
                .detach();
            }
            GameCommand::NewCategory => {
                dialogs::categories::open_new_category_dialog(id, window, cx)
            }
            GameCommand::Edit => dialogs::edit_game::open_edit_game_dialog(&id, window, cx),
            GameCommand::EditMetadata => {
                dialogs::metadata::open_metadata_dialog(vec![id], window, cx)
            }
            GameCommand::RemoveFromRecent => {
                irori_ui::state::run_infallible(cx, async move { b.userdata.remove_recent(&id) })
                    .detach();
            }
            GameCommand::AddShortcut | GameCommand::RemoveShortcut => {
                let add = command == GameCommand::AddShortcut;
                let app = b.settings.app();
                let task = irori_ui::run(cx, async move {
                    use irori_extras::shortcuts;
                    if add {
                        let targets = shortcuts::ShortcutTargets {
                            desktop: true,
                            app_menu: true,
                            steam: false,
                        };
                        shortcuts::add(&game, &app, targets).await.map(|_| ())
                    } else {
                        shortcuts::remove(&game, &app).await
                    }
                });
                let failed = if add {
                    t_args("library.error.add_shortcut", &[("title", &title)])
                } else {
                    t_args("library.error.remove_shortcut", &[("title", &title)])
                };
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| match result {
                        Ok(()) => {
                            this.shortcuts.insert(id, (add, Instant::now()));
                        }
                        Err(e) => {
                            this.shortcuts.remove(&id);
                            crate::notify_error(failed, &e, window, cx);
                        }
                    })
                    .ok();
                })
                .detach();
            }
            GameCommand::AddToSteam => {
                let app = b.settings.app();
                let task = irori_ui::run(cx, async move {
                    let targets = irori_extras::shortcuts::ShortcutTargets {
                        steam: true,
                        ..Default::default()
                    };
                    irori_extras::shortcuts::add(&game, &app, targets).await
                });
                let failed = t_args("library.error.add_to_steam", &[("title", &title)]);
                let done = t_args("library.steam_added", &[("title", &title)]);
                let queued = t_args("library.steam_queued", &[("title", &title)]);
                cx.spawn_in(window, async move |_, cx| {
                    let result = task.await;
                    cx.update(|window, cx| match result {
                        Ok(report) => window.push_notification(
                            gpui_kit::component::notification::Notification::success(
                                if report.steam_queued { queued } else { done },
                            ),
                            cx,
                        ),
                        Err(e) => crate::notify_error(failed, &e, window, cx),
                    })
                    .ok();
                })
                .detach();
            }
            GameCommand::RemoveFromSteam => {
                let app = b.settings.app();
                let ids = std::collections::HashSet::from([id.clone()]);
                let task = irori_ui::run(cx, async move {
                    irori_extras::shortcuts::remove_from_steam(Some(&ids), &app).await
                });
                let failed = t_args("library.error.remove_from_steam", &[("title", &title)]);
                let done = t_args("library.steam_removed", &[("title", &title)]);
                let absent = t_args("library.steam_absent", &[("title", &title)]);
                let queued = t_args("library.steam_remove_queued", &[("title", &title)]);
                cx.spawn_in(window, async move |_, cx| {
                    let result = task.await;
                    cx.update(|window, cx| match result {
                        Ok(report) => window.push_notification(
                            gpui_kit::component::notification::Notification::success(
                                if report.queued > 0 {
                                    queued
                                } else if report.removed > 0 {
                                    done
                                } else {
                                    absent
                                },
                            ),
                            cx,
                        ),
                        Err(e) => crate::notify_error(failed, &e, window, cx),
                    })
                    .ok();
                })
                .detach();
            }
            GameCommand::OpenFolder => {
                if let Some(path) = game.install_path() {
                    cx.open_with_system(&path);
                }
            }
            GameCommand::Verify if id.runner == Runner::Steam => {
                let task = irori_ui::run(cx, async move {
                    b.client_action(&id, irori_core::ClientAction::Verify).await
                });
                let failed = t_args("library.error.verify", &[("title", &title)]);
                report(task, failed, window, cx);
            }
            GameCommand::Uninstall if id.runner == Runner::Steam => {
                let task = irori_ui::run(cx, async move {
                    b.client_action(&id, irori_core::ClientAction::Uninstall)
                        .await
                });
                let failed = t_args("library.error.uninstall_in_client", &[("title", &title)]);
                report(task, failed, window, cx);
            }
            GameCommand::Verify => {
                let task = irori_ui::run(cx, async move { b.repair(&id).await });
                let failed = t_args("library.error.verify", &[("title", &title)]);
                report(task, failed, window, cx);
            }
            GameCommand::Uninstall => cx.emit(LibraryEvent::RequestUninstall(id)),
            GameCommand::Locate => cx.emit(LibraryEvent::RequestLocate(id)),
            GameCommand::PlayAsPcGame => {
                dialogs::sideload::open_play_as_pc_game_dialog(&id, window, cx)
            }
        }
    }

    fn play(&mut self, game: &Arc<GameInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let id = game.id();
        let online = models(cx).accounts.read(cx).online;
        let has_update = models(cx).library.read(cx).has_update(&id);
        let view = cx.entity().downgrade();
        let title = game.title.clone();
        if !online && !game.can_run_offline {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let view = view.clone();
                let id = id.clone();
                alert
                    .title(t_args("library.offline.title", &[("title", &title)]))
                    .description(t("library.offline.description"))
                    .confirm()
                    .ok_text(t("library.offline.play_anyway"))
                    .on_ok(move |_, window, cx| {
                        let id = id.clone();
                        view.update(cx, |this, cx| this.launch(&id, window, cx))
                            .ok();
                        true
                    })
            });
        } else if has_update && game.runner() != Runner::Sideload {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (update_view, play_view) = (view.clone(), view.clone());
                let (update_id, play_id) = (id.clone(), id.clone());
                alert
                    .title(t_args("library.update_first.title", &[("title", &title)]))
                    .description(t("library.update_first.description"))
                    .confirm()
                    .ok_text(t("library.update_first.update"))
                    .ok_variant(ButtonVariant::Primary)
                    .cancel_text(t("library.update_first.play_now"))
                    .on_ok(move |_, window, cx| {
                        let id = update_id.clone();
                        update_view
                            .update(cx, |this, cx| {
                                this.run_command(&id, GameCommand::Update, window, cx)
                            })
                            .ok();
                        true
                    })
                    .on_cancel(move |_, window, cx| {
                        let id = play_id.clone();
                        play_view
                            .update(cx, |this, cx| {
                                // The user chose the installed version.
                                this.skip_version_check.insert(id.clone());
                                this.launch(&id, window, cx)
                            })
                            .ok();
                        true
                    })
            });
        } else {
            self.launch(&id, window, cx);
        }
    }

    /// Start a game: directly when it has one launch option or none,
    /// otherwise after asking which option to use.
    fn launch(&mut self, id: &GameId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.launching.insert(id.clone()) {
            return;
        }
        cx.notify();
        let b = backend(cx);
        let lookup = id.clone();
        let task = irori_ui::run(cx, async move {
            let options = match b.store(lookup.runner) {
                Some(store) => store
                    .launch_options(&lookup.app_name)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let last = b.settings.game(&lookup).last_used_launch_option;
            Ok((options, last))
        });
        let id = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (options, last) = task.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                match launch::decide(options, last.as_ref()) {
                    LaunchChoice::Direct(option) => this.start_launch(&id, option, window, cx),
                    LaunchChoice::Ask {
                        options,
                        preselected,
                        remembered,
                    } => {
                        this.launching.remove(&id);
                        cx.notify();
                        let title = this
                            .game(&id, cx)
                            .map(|g| g.title.clone())
                            .unwrap_or_default();
                        let view = cx.entity().downgrade();
                        launch::open_launch_picker(
                            &title,
                            options,
                            preselected,
                            remembered,
                            move |option, window, cx| {
                                view.update(cx, |this, cx| {
                                    if this.launching.insert(id.clone()) {
                                        this.start_launch(&id, Some(option), window, cx);
                                    }
                                })
                                .ok();
                            },
                            window,
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Ask the backend to start a game (already marked as launching). The
    /// backend remembers the chosen option for next time.
    fn start_launch(
        &mut self,
        id: &GameId,
        option: Option<irori_core::LaunchOption>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let b = backend(cx);
        let launch_id = id.clone();
        let params = irori_services::LaunchParams {
            option,
            skip_version_check: self.skip_version_check.remove(id),
            ..Default::default()
        };
        let task = {
            let params = params.clone();
            irori_ui::run(cx, async move { b.launch_with(&launch_id, params).await })
        };
        let id = id.clone();
        let title = self
            .game(&id, cx)
            .map(|g| g.title.clone())
            .unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.launching.remove(&id);
                if let Err(e) = result
                    && !irori_ui::save_conflict::resolve_launch_error(&id, params, &e, window, cx)
                {
                    let failed = t_args("library.error.start", &[("title", &title)]);
                    crate::notify_error(failed, &e, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- keyboard --------------------------------------------------------

    /// `I` in the grid: open the drawer on the focused game, or close it.
    pub(super) fn toggle_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout != LibraryLayout::Grid || self.close_drawer(cx) {
            return;
        }
        if self.focused_in_view().is_some() {
            self.open_drawer(window, cx);
        }
    }

    pub(super) fn move_focus(&mut self, step: Move, window: &mut Window, cx: &mut Context<Self>) {
        self.step_focus(step, false, window, cx);
    }

    /// Move the selection by `step`; with `extend` (Shift), pick the games
    /// from where the Shift steps began to the new one.
    pub(super) fn step_focus(
        &mut self,
        step: Move,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let games = self.frame.games.clone();
        let current = self.focused.as_ref().and_then(|id| self.games.position(id));
        let rem = window.rem_size();
        let row_rem = match self.layout {
            LibraryLayout::Grid | LibraryLayout::Focus => self.frame.metrics.row_rem,
            LibraryLayout::List => self.frame.metrics.list_row_rem,
        };
        let page_rows = (f32::from(self.scroll.bounds().size.height) / f32::from(rem * row_rem))
            .floor()
            .max(1.0) as usize;
        let step = match step {
            Move::PageUp(_) => Move::PageUp(page_rows),
            Move::PageDown(_) => Move::PageDown(page_rows),
            other => other,
        };
        let target = grid::move_in_sections(current, &self.sections(), self.columns(), step);
        if current.is_some() && target == current {
            // At the edge the key is left to others, so a controller (whose
            // arrows are these keys) moves on to the controls around the grid.
            cx.propagate();
            return;
        }
        if let Some(ix) = target {
            let id = games[ix].id();
            if extend {
                let from = current.map_or_else(|| id.clone(), |c| games[c].id());
                self.selection.extend_range(&from, &id, &games);
            } else {
                self.selection.end_range();
            }
            self.check_shortcut(&id, cx);
            self.focused = Some(id);
            self.focused_ix = Some(ix);
            self.show_focused(true, window, cx);
            if let Some(item) = self.item_of_game(ix) {
                // Going up onto a section's first row shows its heading too.
                let upward = matches!(step, Move::Up | Move::PageUp(_) | Move::First);
                let item = match item.checked_sub(1).map(|i| self.frame.items[i]) {
                    Some(Item::GroupHeading(_)) if upward => item - 1,
                    _ => item,
                };
                self.reveal_item(item);
            }
            cx.notify();
        }
    }

    pub(super) fn focused_in_view(&self) -> Option<GameId> {
        let id = self.focused.as_ref()?;
        self.games.position(id).map(|_| id.clone())
    }

    pub(super) fn open_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Enter on a button inside a tile activates that button instead.
        if !self.grid_focus.is_focused(window) {
            cx.propagate();
            return;
        }
        if let Some(id) = self.focused_in_view() {
            if self.selection.is_active() {
                self.selection.toggle(&id);
                cx.notify();
            } else {
                navigate(Route::Game(id), cx);
            }
        }
    }

    pub(super) fn activate_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.grid_focus.is_focused(window) {
            cx.propagate();
            return;
        }
        let Some(id) = self.focused_in_view() else {
            return;
        };
        // In selection mode Space picks the focused game.
        if self.selection.is_active() {
            self.selection.toggle(&id);
            cx.notify();
            return;
        }
        let Some(game) = self.game(&id, cx) else {
            return;
        };
        let state = self.state_for(&game, false, cx);
        if let Some(cmd) = commands::primary(&state) {
            self.run_command(&id, cmd, window, cx);
        }
    }

    /// Enter from the search field: move into the results with the first one
    /// focused.
    pub(super) fn enter_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = self.frame.games.first() {
            self.focused = Some(first.id());
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.grid_focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Escape in the search field clears it; a second Escape leaves it.
    pub(super) fn escape_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.read(cx).value().is_empty() {
            self.grid_focus.focus(window, cx);
        } else {
            self.search
                .update(cx, |s, cx| s.set_value(SharedString::default(), window, cx));
            self.filters
                .update(cx, |m, cx| m.update(|f| f.query.clear(), cx));
        }
    }

    pub(super) fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |s, cx| s.set_value(SharedString::default(), window, cx));
        self.filters
            .update(cx, |m, cx| m.update(ViewFilter::clear_all, cx));
    }

    /// Turn grouping by play status on or off (from game modes too).
    pub(super) fn toggle_status_groups(&mut self, cx: &mut Context<Self>) {
        self.update_filter(
            |f| {
                f.group = match f.group {
                    Grouping::Status => Grouping::None,
                    _ => Grouping::Status,
                }
            },
            cx,
        );
    }

    /// Turn grouping by game mode on or off (from play status too).
    pub(super) fn toggle_mode_groups(&mut self, cx: &mut Context<Self>) {
        self.update_filter(
            |f| {
                f.group = match f.group {
                    Grouping::Mode => Grouping::None,
                    _ => Grouping::Mode,
                }
            },
            cx,
        );
    }

    /// Save a visualization preference on the active tab when one is active;
    /// returns `false` when custom filters are active (no tab matched) so the
    /// caller can fall back to global `UiPrefs`.
    fn save_active_tab_prefs(
        &self,
        f: impl FnOnce(&mut TabViewPrefs),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.active_tab.clone() else {
            return false;
        };
        views::saved_views(cx).update(cx, |m, cx| m.update(|v| v.update_prefs(&tab, f), cx));
        true
    }

    /// Switch to `layout` and remember it on the active tab (or globally when
    /// no tab is active). The selected game stays selected and in view; Focus
    /// selects the first game when none is.
    pub(crate) fn set_layout(&mut self, layout: LibraryLayout, cx: &mut Context<Self>) {
        if self.layout == layout && self.chosen_layout == layout {
            return;
        }
        self.layout = layout;
        self.chosen_layout = layout;
        if !self.save_active_tab_prefs(|p| p.layout = Some(layout), cx) {
            persist_ui(cx, move |ui| ui.library_layout = layout);
        }
        self.scroll_to_focused();
        cx.notify();
    }

    /// Switch cover shape for the active tab (or globally when no tab is active).
    pub(crate) fn set_cover_shape(&mut self, shape: CoverShape, cx: &mut Context<Self>) {
        if self.cover_shape == shape {
            return;
        }
        self.cover_shape = shape;
        if !self.save_active_tab_prefs(|p| p.cover_shape = Some(shape), cx) {
            persist_ui(cx, move |ui| ui.cover_shape = shape);
        }
        cx.notify();
    }

    /// Save the grid tile size for the active tab (or globally when no tab is active).
    pub(crate) fn set_tile_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(grid::MIN_TILE_REM, grid::MAX_TILE_REM);
        self.tile_rem = size;
        if !self.save_active_tab_prefs(|p| p.tile_size = Some(size), cx) {
            persist_ui(cx, move |ui| ui.tile_size = size);
        }
        cx.notify();
    }

    /// Switch list row size for the active tab (or globally when no tab is active).
    pub(crate) fn set_list_row_size(&mut self, size: ListRowSize, cx: &mut Context<Self>) {
        if self.list_row_size == size {
            return;
        }
        self.list_row_size = size;
        if !self.save_active_tab_prefs(|p| p.list_row_size = Some(size), cx) {
            persist_ui(cx, move |ui| ui.library_list_row_size = size);
        }
        cx.notify();
    }

    /// Switch Focus cover size for the active tab (or globally when no tab is active).
    pub(crate) fn set_focus_cover_size(&mut self, size: FocusCoverSize, cx: &mut Context<Self>) {
        if self.focus_cover_size == size {
            return;
        }
        self.focus_cover_size = size;
        if !self.save_active_tab_prefs(|p| p.focus_cover_size = Some(size), cx) {
            persist_ui(cx, move |ui| ui.library_focus_cover_size = size);
        }
        cx.notify();
    }

    /// Restore the visual properties (layout, cover shape, sizes) saved for
    /// `tab`, falling back to global `ui_prefs` for any property the tab hasn't
    /// customized yet.
    fn restore_tab_visuals(
        &mut self,
        tab: &TabKey,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let prefs = views::saved_views(cx)
            .read(cx)
            .views()
            .prefs_for(tab)
            .cloned()
            .unwrap_or_default();
        let next_layout = prefs.layout.unwrap_or(self.ui_prefs.library_layout);
        if self.layout != next_layout || self.chosen_layout != next_layout {
            self.layout = next_layout;
            self.chosen_layout = next_layout;
            self.auto_wide = None;
            self.scroll_to_focused();
        }
        self.cover_shape = prefs.cover_shape.unwrap_or(self.ui_prefs.cover_shape);
        self.list_row_size = prefs
            .list_row_size
            .unwrap_or(self.ui_prefs.library_list_row_size);
        self.focus_cover_size = prefs
            .focus_cover_size
            .unwrap_or(self.ui_prefs.library_focus_cover_size);
        let next_tile = prefs
            .tile_size
            .unwrap_or(self.ui_prefs.tile_size)
            .clamp(grid::MIN_TILE_REM, grid::MAX_TILE_REM);
        if (self.tile_rem - next_tile).abs() > f32::EPSILON {
            self.tile_rem = next_tile;
            if let Some(window) = window {
                self.tile_slider
                    .update(cx, |s, cx| s.set_value(next_tile, window, cx));
                self.tile_slider_dirty = false;
            } else {
                self.tile_slider_dirty = true;
            }
        }
    }

    /// Activate `tab`: apply its filtering criteria (keeping search text) and
    /// restore its saved layout, cover shape, size, and sort/group settings.
    pub(crate) fn apply_tab(
        &mut self,
        tab: TabKey,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let prefs = views::saved_views(cx)
            .read(cx)
            .views()
            .prefs_for(&tab)
            .cloned()
            .unwrap_or_default();
        let mut target = tab_default_filter(&tab);
        if let Some(sort) = prefs.sort {
            target.sort = sort;
        }
        if let Some(desc) = prefs.descending {
            target.descending = desc;
        }
        if let Some(group) = prefs.group {
            target.group = group;
        }

        self.active_tab = Some(tab.clone());
        views::saved_views(cx).update(cx, |m, cx| {
            m.update(|v| v.set_active_tab(Some(tab.clone())), cx)
        });

        self.suppress_filter_sync = true;
        self.filters.update(cx, |m, cx| {
            m.update(
                move |f| {
                    *f = ViewFilter {
                        query: std::mem::take(&mut f.query),
                        ..target
                    };
                },
                cx,
            )
        });
        self.suppress_filter_sync = false;

        self.restore_tab_visuals(&tab, window, cx);
        cx.notify();
    }

    /// Keep `active_tab` and per-tab sort/group prefs synchronized whenever
    /// `self.filters` changes outside `apply_tab` (Sort & Group popover,
    /// Filters popover, keyboard shortcuts, or deleting a collection).
    pub(super) fn sync_tab_from_filter(
        &mut self,
        filters: Entity<FilterModel>,
        cx: &mut Context<Self>,
    ) {
        if self.suppress_filter_sync {
            return;
        }
        let filter = filters.read(cx).filter().clone();
        let smart_ids = {
            let user = &models(cx).library.read(cx).user;
            views::smart_collection_ids(user)
        };
        let next_tab = chips::active_tab_for(&filter, self.active_tab.as_ref(), &smart_ids);
        if next_tab == self.active_tab {
            if let Some(tab) = self.active_tab.clone() {
                let default_f = tab_default_filter(&tab);
                let cur = views::saved_views(cx)
                    .read(cx)
                    .views()
                    .prefs_for(&tab)
                    .cloned()
                    .unwrap_or_default();
                let expected_sort = cur.sort.unwrap_or(default_f.sort);
                let expected_desc = cur.descending.unwrap_or(default_f.descending);
                let expected_group = cur.group.unwrap_or(default_f.group);
                if filter.sort != expected_sort
                    || filter.descending != expected_desc
                    || filter.group != expected_group
                {
                    views::saved_views(cx).update(cx, |m, cx| {
                        m.update(
                            |v| {
                                v.update_prefs(&tab, |p| {
                                    p.sort = Some(filter.sort);
                                    p.descending = Some(filter.descending);
                                    p.group = Some(filter.group);
                                });
                            },
                            cx,
                        )
                    });
                }
            }
            return;
        }
        self.active_tab = next_tab.clone();
        views::saved_views(cx).update(cx, |m, cx| {
            m.update(|v| v.set_active_tab(next_tab.clone()), cx)
        });
        if let Some(tab) = next_tab {
            self.restore_tab_visuals(&tab, None, cx);
        }
    }

    /// Scroll the selected game into view once the next render has laid
    /// out the new view's rows.
    pub(super) fn scroll_to_focused(&mut self) {
        self.scroll_pending = true;
    }

    /// The view's keyboard commands: Alt+1…3 and the palette's.
    pub(super) fn on_show_layout(
        &mut self,
        layout: LibraryLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_layout(layout, cx);
        // Arrow keys move the selection right away.
        if !self.search.focus_handle(cx).is_focused(window) {
            self.grid_focus.focus(window, cx);
        }
    }

    pub(super) fn set_hovered(&mut self, id: &GameId, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hovered.as_ref() != Some(id) {
                self.check_shortcut(id, cx);
                self.hovered = Some(id.clone());
                cx.notify();
            }
        } else if self.hovered.as_ref() == Some(id) {
            self.hovered = None;
            cx.notify();
        }
    }
}

/// Default filter criteria for a tab before per-tab sort/group overrides.
fn tab_default_filter(tab: &TabKey) -> ViewFilter {
    match tab {
        TabKey::All | TabKey::View(_) => PresetChip::All.filter(),
        TabKey::Installed => PresetChip::Installed.filter(),
        TabKey::Favorites => PresetChip::Favourites.filter(),
        TabKey::RecentlyAdded => PresetChip::RecentlyAdded.filter(),
        TabKey::Collection(id) => chips::collection_filter(id),
    }
}

/// Await a backend task and report a failure as a notification.
fn report<T: 'static>(
    task: gpui_kit::Task<anyhow::Result<T>>,
    title: SharedString,
    window: &mut Window,
    cx: &mut Context<LibraryView>,
) {
    cx.spawn_in(window, async move |_, cx| {
        if let Err(e) = task.await {
            cx.update(|window, cx| crate::notify_error(title, &e, window, cx))
                .ok();
        }
    })
    .detach();
}
