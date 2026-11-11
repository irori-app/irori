//! Removing several games from the library at once, from the bulk bar, the
//! context menu of a picked game, the command palette and Delete.
//!
//! Only games the library holds itself leave it: added games are forgotten
//! (their files deleted only when the user ticks the box, and then only a
//! folder `fs_guard` allows), and ROMs are hidden from the ROM library, whose
//! files are never deleted. Store games stay, and so do games that are
//! running or busy; the result says how many were left and why.

use std::sync::Arc;

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Global, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, WeakEntity, Window, div,
};
use irori_core::{GameId, GameInfo, RemoveArgs, Runner};
use irori_services::Backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::LibraryView;
use super::bulk::{LISTED, counted};

/// What removing the picked games would do.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemovalPlan {
    /// Added games and ROMs that leave the library.
    pub remove: Vec<Arc<GameInfo>>,
    /// Store games, which can only be uninstalled or hidden.
    pub stores: usize,
    /// Added games and ROMs left because they run or are busy.
    pub busy: usize,
}

impl RemovalPlan {
    /// Whether any picked game is an added one, whose files could go too.
    pub fn has_added(&self) -> bool {
        self.remove.iter().any(|g| g.runner() == Runner::Sideload)
    }
}

/// Whether removing `game` from the library is Irori's to do.
fn removable(game: &GameInfo) -> bool {
    matches!(game.runner(), Runner::Sideload | Runner::Emulation) && !game.is_dlc
}

/// Sort `picked` into what leaves the library and what stays.
pub(crate) fn plan_removal(
    picked: &[Arc<GameInfo>],
    busy: impl Fn(&GameId) -> bool,
) -> RemovalPlan {
    let mut plan = RemovalPlan::default();
    for game in picked {
        if !removable(game) {
            plan.stores += 1;
        } else if busy(&game.id()) {
            plan.busy += 1;
        } else {
            plan.remove.push(game.clone());
        }
    }
    plan
}

/// How removing went: how many games left, and the ones that didn't with
/// the reason.
#[derive(Debug, Default)]
pub(crate) struct RemovalOutcome {
    pub removed: usize,
    pub failed: Vec<(String, String)>,
}

/// Remove `games` one after another. An added game's files go only with
/// `delete_files`; a ROM's never do.
pub(crate) async fn remove_games(
    b: &Backend,
    games: Vec<Arc<GameInfo>>,
    delete_files: bool,
) -> RemovalOutcome {
    let mut outcome = RemovalOutcome::default();
    for game in games {
        let args = RemoveArgs {
            remove_prefix: false,
            delete_files: delete_files && game.runner() == Runner::Sideload,
            remove_settings: false,
        };
        match b.uninstall(&game.id(), args).await {
            Ok(()) => outcome.removed += 1,
            Err(e) => outcome.failed.push((game.title.clone(), format!("{e:#}"))),
        }
    }
    outcome
}

/// The library view palette commands reach.
struct ActiveLibrary(WeakEntity<LibraryView>);
impl Global for ActiveLibrary {}

/// Make `view` the one the palette's library commands act on.
pub(crate) fn set_active_library(view: WeakEntity<LibraryView>, cx: &mut App) {
    cx.set_global(ActiveLibrary(view));
}

/// How many picked games "Remove from library" would take out, while the
/// library is in selection mode with any.
pub fn removable_picks(cx: &App) -> Option<usize> {
    let view = cx.try_global::<ActiveLibrary>()?.0.upgrade()?;
    let view = view.read(cx);
    if !view.selection.is_active() {
        return None;
    }
    let count = view.removal_plan(cx).remove.len();
    (count > 0).then_some(count)
}

/// Ask to remove the picked games from the library (the palette's command).
pub fn remove_picked_from_library(window: &mut Window, cx: &mut App) {
    let Some(view) = cx.try_global::<ActiveLibrary>().and_then(|a| a.0.upgrade()) else {
        return;
    };
    view.update(cx, |this, cx| this.remove_picked(window, cx));
}

impl LibraryView {
    /// What removing the picked games would do.
    pub(super) fn removal_plan(&self, cx: &App) -> RemovalPlan {
        let statuses = models(cx).statuses.read(cx);
        plan_removal(&self.selection.games(&self.frame.games), |id| {
            self.launching.contains(id) || statuses.kind(id).is_some_and(|k| k.is_busy())
        })
    }

    /// Delete, the context menu and the palette: the picked games, or the
    /// focused one outside selection mode. Nothing to remove lets the key
    /// go on.
    pub(super) fn remove_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = if self.selection.is_active() {
            self.removal_plan(cx)
        } else {
            let focused = self.focused.as_ref().and_then(|id| self.game(id, cx));
            let statuses = models(cx).statuses.read(cx);
            plan_removal(&focused.into_iter().collect::<Vec<_>>(), |id| {
                self.launching.contains(id) || statuses.kind(id).is_some_and(|k| k.is_busy())
            })
        };
        if plan.remove.is_empty() {
            if plan.stores + plan.busy > 0 {
                report_left(&plan, window, cx);
            } else {
                cx.propagate();
            }
            return;
        }
        self.confirm_remove(plan, window, cx);
    }

    /// Ask once, naming the count, then remove.
    pub(super) fn confirm_remove(
        &mut self,
        plan: RemovalPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = plan.remove.len();
        let heading = if count == 1 {
            t_args(
                "library.bulk.remove_title_one",
                &[("title", &plan.remove[0].title)],
            )
        } else {
            t_args(
                "library.bulk.remove_title_other",
                &[("count", &count.to_string())],
            )
        };
        let options = cx.new(|_| RemoveOptions {
            plan: plan.clone(),
            delete_files: false,
        });
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (options, view) = (options.clone(), view.clone());
            alert
                .title(heading.clone())
                .child(options.clone())
                .confirm()
                .ok_text(t("library.bulk.remove_confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (plan, delete_files) = {
                        let o = options.read(cx);
                        (o.plan.clone(), o.delete_files)
                    };
                    let _ = view.update(cx, |this, cx| {
                        this.selection.end();
                        cx.notify();
                    });
                    run_removal(plan, delete_files, window, cx);
                    true
                })
        });
    }
}

/// The confirmation's body: what happens, the games, what stays, and the
/// choice to delete added games' files (off unless ticked).
struct RemoveOptions {
    plan: RemovalPlan,
    delete_files: bool,
}

impl Render for RemoveOptions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let count = self.plan.remove.len();
        let mut lines: Vec<String> = self
            .plan
            .remove
            .iter()
            .take(LISTED)
            .map(|g| g.title.clone())
            .collect();
        if count > LISTED {
            lines.push(
                t_args(
                    "library.bulk.and_more",
                    &[("count", &(count - LISTED).to_string())],
                )
                .to_string(),
            );
        }
        let left = left_lines(&self.plan);
        v_flex()
            .gap_3()
            .child(div().text_sm().child(t("library.bulk.remove_description")))
            .child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(muted)
                    .children(lines.into_iter().map(|l| div().truncate().child(l))),
            )
            .children(
                left.into_iter()
                    .map(|l| div().text_xs().text_color(muted).child(l)),
            )
            .when(self.plan.has_added(), |col| {
                col.child(
                    v_flex()
                        .gap_1()
                        .child(
                            Checkbox::new("bulk-remove-delete-files")
                                .label(t("library.bulk.remove_delete_files"))
                                .checked(self.delete_files)
                                .on_change(cx.listener(|this, v: &bool, _, cx| {
                                    this.delete_files = *v;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .pl_6()
                                .text_xs()
                                .text_color(muted)
                                .child(t("library.bulk.remove_delete_files_help")),
                        ),
                )
            })
    }
}

/// Why picked games stay in the library, one line per reason.
fn left_lines(plan: &RemovalPlan) -> Vec<SharedString> {
    let mut lines = Vec::new();
    if plan.stores > 0 {
        lines.push(counted(
            (
                "library.bulk.remove_skipped_one",
                "library.bulk.remove_skipped_other",
            ),
            plan.stores,
            &[],
        ));
    }
    if plan.busy > 0 {
        lines.push(counted(
            (
                "library.bulk.remove_busy_one",
                "library.bulk.remove_busy_other",
            ),
            plan.busy,
            &[],
        ));
    }
    lines
}

/// Say why the picked games stay, when none can leave.
fn report_left(plan: &RemovalPlan, window: &mut Window, cx: &mut App) {
    for line in left_lines(plan) {
        window.push_notification(Notification::info(line), cx);
    }
}

/// Remove the planned games off the UI thread and report the result.
fn run_removal(plan: RemovalPlan, delete_files: bool, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let games = plan.remove.clone();
    let task = irori_ui::run(cx, async move {
        Ok(remove_games(&b, games, delete_files).await)
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(outcome) = task.await else { return };
            let _ = cx.update(|window, cx| {
                if outcome.removed > 0 {
                    window.push_notification(
                        Notification::success(counted(
                            ("library.bulk.removed_one", "library.bulk.removed_other"),
                            outcome.removed,
                            &[],
                        )),
                        cx,
                    );
                }
                report_left(&plan, window, cx);
                for (title, error) in outcome.failed {
                    window.push_notification(
                        Notification::error(error)
                            .title(t_args("library.bulk.remove_failed", &[("title", &title)])),
                        cx,
                    );
                }
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(runner: Runner, app: &str) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                runner: Some(runner),
                app_name: app.into(),
                title: app.into(),
                is_installed: true,
                ..Default::default()
            }
            .finalize(),
        )
    }

    #[test]
    fn a_mixed_selection_removes_only_added_games_and_roms() {
        let picked = vec![
            game(Runner::Gog, "store"),
            game(Runner::Sideload, "added"),
            game(Runner::Emulation, "rom"),
            game(Runner::Steam, "steam"),
            game(Runner::Sideload, "running"),
        ];
        let plan = plan_removal(&picked, |id| &*id.app_name == "running");
        let removed: Vec<&str> = plan.remove.iter().map(|g| g.app_name.as_str()).collect();
        assert_eq!(removed, ["added", "rom"]);
        assert_eq!((plan.stores, plan.busy), (2, 1));
        assert!(plan.has_added());

        let roms = plan_removal(&[game(Runner::Emulation, "rom")], |_| false);
        assert!(!roms.has_added());
        let none = plan_removal(&[], |_| false);
        assert!(none.remove.is_empty() && none.stores == 0 && none.busy == 0);
    }

    #[test]
    fn select_all_leaves_out_hidden_games_behind_the_pin() {
        use std::collections::HashSet;

        use crate::filter::{FilteredGames, HiddenMode, ViewFilter};

        let added = game(Runner::Sideload, "added");
        let secret = game(Runner::Sideload, "secret");
        let rom = game(Runner::Emulation, "rom");
        let mut library = crate::filter::tests::snapshot(vec![added, secret.clone(), rom]);
        library.hidden_locked = true;
        let mut user = irori_services::UserData::default();
        user.hidden.insert(secret.id().key());
        let user = Arc::new(user);
        // Even a view showing hidden games keeps them out while locked.
        let filter = ViewFilter {
            hidden: HiddenMode::Show,
            ..Default::default()
        };
        let none = Arc::new(HashSet::new());
        let shown = FilteredGames::default().get(
            &filter,
            &[Runner::Sideload, Runner::Emulation],
            &library,
            &user,
            &none,
            &none,
        );
        let mut selection = crate::selection::Selection::default();
        selection.start();
        selection.select_all(&shown);
        let picked = selection.games(&shown);
        let plan = plan_removal(&picked, |_| false);
        let removed: Vec<&str> = plan.remove.iter().map(|g| g.app_name.as_str()).collect();
        assert_eq!(removed, ["added", "rom"]);
        assert!(!selection.contains(&secret.id()));
    }
}
