//! Bulk actions on the games picked in selection mode (install, uninstall,
//! add to Steam, categories, play status, favourite and hide), from a bar
//! floating over the library. Edits of library data say what they did in a
//! notification with an Undo that puts every game back as it was.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, ThemeStyled as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use irori_core::settings::LibraryLayout;
use irori_core::{GameId, GameInfo, RemoveArgs, StatusKind};
use irori_services::{PlayStatus, UserDataService};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::LibraryView;
use crate::commands::GameState;
use crate::layouts::{self, Click};

/// Titles listed in the uninstall confirmation before "and N more".
const LISTED: usize = 8;

/// What the bulk bar offers for the picked games.
#[derive(Clone, Default)]
pub(super) struct BulkSummary {
    count: usize,
    installable: Rc<Vec<Arc<GameInfo>>>,
    removable: Rc<Vec<Arc<GameInfo>>>,
    for_steam: Rc<Vec<Arc<GameInfo>>>,
    ids: Rc<Vec<GameId>>,
    all_favourite: bool,
    all_hidden: bool,
    /// Each category, and whether every picked game is in it.
    categories: Rc<Vec<(String, bool)>>,
}

/// What a [`BulkSummary`] depends on: the picks, the shown list and the
/// library data by identity, and the few games that are busy right now.
#[derive(PartialEq)]
struct BulkKey {
    picks: u64,
    games: usize,
    user: usize,
    statuses: Vec<(GameId, StatusKind)>,
    launching: Vec<GameId>,
}

/// The last [`BulkSummary`], so drawing the bar with thousands of games
/// picked doesn't go through them on every frame.
#[derive(Default)]
pub(super) struct BulkCache {
    key: Option<BulkKey>,
    summary: Rc<BulkSummary>,
    computes: usize,
}

impl LibraryView {
    /// A click on a game: Shift extends the selection, Ctrl (or any click in
    /// selection mode) picks it. Otherwise the list opens its page; the grid
    /// and Focus select it (the grid opens its drawer) and open the page on
    /// a double click.
    pub(super) fn click_game(
        &mut self,
        id: &GameId,
        modifiers: Modifiers,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused = Some(id.clone());
        self.focused_ix = self.games.position(id);
        let click = layouts::click(
            self.layout,
            count,
            modifiers.shift,
            modifiers.secondary(),
            self.selection.is_active(),
        );
        match click {
            Click::Extend => self.selection.extend_to(id, &self.frame.games),
            Click::Pick => self.selection.toggle(id),
            Click::Open => {
                irori_ui::navigate(irori_ui::Route::Game(id.clone()), cx);
                return;
            }
            Click::Select => {
                // Arrow keys go on from the clicked game.
                self.grid_focus.focus(window, cx);
                match self.layout {
                    LibraryLayout::Grid => self.open_drawer(window, cx),
                    _ => self.show_focused(false, window, cx),
                }
            }
        }
        cx.notify();
    }

    /// The toolbar's "Select" / "Done".
    pub(super) fn toggle_selection_mode(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_active() {
            self.selection.end();
        } else {
            self.selection.start();
        }
        cx.notify();
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection.select_all(&self.frame.games);
        cx.notify();
    }

    /// "Combine with other copies…" on `id`: selection mode with it picked,
    /// so the user picks its other copies and combines them from the bar.
    pub(super) fn start_combining(&mut self, id: &GameId, cx: &mut Context<Self>) {
        self.selection.end();
        self.selection.toggle(id);
        cx.notify();
    }

    /// Make the picked games one entry, and leave selection mode.
    fn combine_picked(&mut self, ids: Vec<GameId>, window: &mut Window, cx: &mut Context<Self>) {
        let count = ids.len();
        irori_ui::copies::combine(ids, cx);
        self.selection.end();
        window.push_notification(
            Notification::success(t_args(
                "library.bulk.done.combined",
                &[("count", &count.to_string())],
            )),
            cx,
        );
        cx.notify();
    }

    /// Escape: leave selection mode, else close the drawer.
    pub(super) fn end_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_active() {
            self.selection.end();
            cx.notify();
        } else if !self.close_drawer(cx) {
            cx.propagate();
        }
    }

    fn picked(&self) -> Vec<Arc<GameInfo>> {
        self.selection.games(&self.frame.games)
    }

    /// What the bulk bar offers, worked out again only when the picks, the
    /// shown games, the library data or a game's state changed.
    fn bulk_summary(&mut self, cx: &App) -> Rc<BulkSummary> {
        let user = models(cx).library.read(cx).user.clone();
        let mut statuses: Vec<_> = models(cx)
            .statuses
            .read(cx)
            .all()
            .map(|s| (s.id.clone(), s.kind))
            .collect();
        statuses.sort_by_key(|(id, _)| id.key());
        let mut launching: Vec<_> = self.launching.iter().cloned().collect();
        launching.sort_by_key(GameId::key);
        let key = BulkKey {
            picks: self.selection.revision(),
            games: Arc::as_ptr(&self.frame.games) as *const () as usize,
            user: Arc::as_ptr(&user) as usize,
            statuses,
            launching,
        };
        if self.bulk.key.as_ref() == Some(&key) {
            return self.bulk.summary.clone();
        }
        let picked = self.picked();
        let states: Vec<_> = picked
            .iter()
            .map(|g| self.state_for(g, false, cx))
            .collect();
        let pick = |keep: &dyn Fn(&Arc<GameInfo>, &GameState) -> bool| {
            Rc::new(
                picked
                    .iter()
                    .zip(&states)
                    .filter(|(g, s)| keep(g, s))
                    .map(|(g, _)| g.clone())
                    .collect::<Vec<_>>(),
            )
        };
        let installable = pick(&|g, s| {
            !s.installed
                && s.installable
                && !s.is_queued()
                && !s.is_busy()
                && !s.browser
                && g.third_party_managed_app.is_none()
        });
        let removable = pick(&|_, s| s.installed && !s.external && !s.is_playing() && !s.is_busy());
        let for_steam = pick(&|_, s| !s.browser && !s.external && (s.installed || s.installable));
        let ids: Vec<GameId> = picked.iter().map(|g| g.id()).collect();
        let keys: Vec<String> = ids.iter().map(GameId::key).collect();
        let all_favourite = !keys.is_empty() && keys.iter().all(|k| user.favourites.contains(k));
        let all_hidden = !keys.is_empty() && keys.iter().all(|k| user.hidden.contains(k));
        let categories = user
            .categories
            .iter()
            .map(|(name, games)| {
                (
                    name.clone(),
                    !keys.is_empty() && keys.iter().all(|k| games.contains(k)),
                )
            })
            .collect();
        self.bulk.summary = Rc::new(BulkSummary {
            count: picked.len(),
            installable,
            removable,
            for_steam,
            ids: Rc::new(ids),
            all_favourite,
            all_hidden,
            categories: Rc::new(categories),
        });
        self.bulk.key = Some(key);
        self.bulk.computes += 1;
        self.bulk.summary.clone()
    }

    /// How many times the bulk bar's offers were worked out.
    #[cfg(test)]
    pub(crate) fn bulk_computes(&self) -> usize {
        self.bulk.computes
    }

    /// Queue every picked game that can be installed, with default options.
    fn install_picked(
        &mut self,
        games: Vec<Arc<GameInfo>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let mut failed = Vec::new();
            let mut queued = 0usize;
            for g in games {
                match b.install_with_defaults(&g.id()).await {
                    Ok(()) => queued += 1,
                    Err(e) => failed.push((g.title.clone(), format!("{e:#}"))),
                }
            }
            Ok((queued, failed))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((queued, failed)) = task.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if queued > 0 {
                    window.push_notification(
                        Notification::success(t_args(
                            "library.bulk.install_queued",
                            &[("count", &queued.to_string())],
                        )),
                        cx,
                    );
                }
                for (title, error) in failed {
                    window.push_notification(
                        Notification::error(error)
                            .title(t_args("library.bulk.install_failed", &[("title", &title)])),
                        cx,
                    );
                }
                this.selection.end();
                cx.notify();
            });
        })
        .detach();
    }

    /// Add every picked game to Steam in one pass, installed or not.
    fn add_picked_to_steam(
        &mut self,
        games: Vec<Arc<GameInfo>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = backend(cx).settings.app();
        let task = irori_ui::run(cx, async move {
            irori_extras::shortcuts::add_to_steam(&games, &app).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(added) => window.push_notification(
                        Notification::success(t_args(
                            "library.bulk.steam_added",
                            &[("count", &added.to_string())],
                        )),
                        cx,
                    ),
                    Err(e) => crate::notify_error(t("library.bulk.steam_failed"), &e, window, cx),
                }
                this.selection.end();
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask, then uninstall every picked installed game one after another.
    fn confirm_uninstall(
        &mut self,
        games: Vec<Arc<GameInfo>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = games.len();
        let heading = if count == 1 {
            t_args(
                "library.bulk.uninstall_title_one",
                &[("title", &games[0].title)],
            )
        } else {
            t_args(
                "library.bulk.uninstall_title_other",
                &[("count", &count.to_string())],
            )
        };
        let mut lines: Vec<String> = games.iter().take(LISTED).map(|g| g.title.clone()).collect();
        if count > LISTED {
            lines.push(
                t_args(
                    "library.bulk.and_more",
                    &[("count", &(count - LISTED).to_string())],
                )
                .to_string(),
            );
        }
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let (games, view) = (games.clone(), view.clone());
            dialog.title(heading.clone()).child(
                v_flex()
                    .gap_4()
                    .child(
                        div()
                            .text_sm()
                            .child(t("library.bulk.uninstall_description")),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .text_sm()
                            .text_color(muted)
                            .children(lines.iter().map(|l| div().truncate().child(l.clone()))),
                    )
                    .child(
                        DialogFooter::new()
                            .child(
                                Button::new("bulk-uninstall-cancel")
                                    .ghost()
                                    .label(t("library.bulk.cancel"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("bulk-uninstall-confirm")
                                    .danger()
                                    .label(t("library.bulk.uninstall_confirm"))
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let games = games.clone();
                                        let _ = view.update(cx, |this, cx| {
                                            this.selection.end();
                                            cx.notify();
                                        });
                                        uninstall_all(games, window, cx);
                                    }),
                            ),
                    ),
            )
        });
    }

    /// The bar over the library while selection mode is on.
    pub(super) fn render_bulk_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.selection.is_active() {
            return None;
        }
        let summary = self.bulk_summary(cx);
        let BulkSummary {
            count,
            installable,
            removable,
            for_steam,
            ids,
            all_favourite,
            all_hidden,
            categories,
        } = (*summary).clone();
        let theme = cx.theme();
        let status: SharedString = match count {
            0 => t("library.bulk.none"),
            1 => t("library.bulk.count_one"),
            n => t_args("library.bulk.count_other", &[("count", &n.to_string())]),
        };
        let nothing = count == 0;

        let install = {
            let games = installable.clone();
            Button::new("bulk-install")
                .small()
                .primary()
                .icon(IconName::Download)
                .label(t_args(
                    "library.bulk.install",
                    &[("count", &installable.len().to_string())],
                ))
                .disabled(installable.is_empty())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.install_picked(games.to_vec(), window, cx)
                }))
        };
        let uninstall = {
            let games = removable.clone();
            Button::new("bulk-uninstall")
                .small()
                .outline()
                .icon(IconName::Trash)
                .label(t_args(
                    "library.bulk.uninstall",
                    &[("count", &removable.len().to_string())],
                ))
                .disabled(removable.is_empty())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_uninstall(games.to_vec(), window, cx)
                }))
        };
        let steam = {
            let games = for_steam.clone();
            Button::new("bulk-steam")
                .small()
                .outline()
                .icon(IconName::Gamepad2)
                .label(t_args(
                    "library.bulk.add_to_steam",
                    &[("count", &for_steam.len().to_string())],
                ))
                .disabled(for_steam.is_empty())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_picked_to_steam(games.to_vec(), window, cx)
                }))
        };
        let combine = {
            let ids = ids.clone();
            Button::new("bulk-combine")
                .small()
                .outline()
                .icon(IconName::Combine)
                .label(t("library.bulk.combine"))
                .tooltip(t("library.bulk.combine_tooltip"))
                .disabled(count < 2)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.combine_picked(ids.to_vec(), window, cx)
                }))
        };
        let category_ids = ids.clone();
        let view = cx.entity().downgrade();
        let categories_button = Button::new("bulk-categories")
            .small()
            .outline()
            .icon(IconName::Tags)
            .label(t("library.bulk.categories"))
            .disabled(nothing)
            .dropdown_menu(move |mut menu, _, _| {
                for (name, all_in) in categories.iter() {
                    let (ids, name, add) = (category_ids.clone(), name.clone(), !*all_in);
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(*all_in).on_click(
                        move |_, window, cx| {
                            let message = counted(
                                if add {
                                    (
                                        "library.bulk.done.categorized_one",
                                        "library.bulk.done.categorized_other",
                                    )
                                } else {
                                    (
                                        "library.bulk.done.uncategorized_one",
                                        "library.bulk.done.uncategorized_other",
                                    )
                                },
                                ids.len(),
                                &[("category", &name)],
                            );
                            let name = name.clone();
                            edit_with_undo(
                                ids.to_vec(),
                                message,
                                move |u, ids| u.set_category_many(ids, &name, add),
                                window,
                                cx,
                            )
                        },
                    ));
                }
                if !categories.is_empty() {
                    menu = menu.separator();
                }
                let view = view.clone();
                menu.item(
                    PopupMenuItem::new(t("library.bulk.manage_categories")).on_click(
                        move |_, window, cx| {
                            let _ = view.update(cx, |_, cx| cx.notify());
                            crate::dialogs::categories::open_categories_dialog(None, window, cx)
                        },
                    ),
                )
            });
        let status_ids = ids.clone();
        let status_button = Button::new("bulk-play-status")
            .small()
            .outline()
            .icon(IconName::Flag)
            .label(irori_ui::play_status::title())
            .disabled(nothing)
            .dropdown_menu(move |menu, _, cx| {
                irori_ui::play_status::menu_many_with(menu, status_ids.to_vec(), cx, set_statuses)
            });
        let favourite = {
            let ids = ids.clone();
            Button::new("bulk-favourite")
                .small()
                .ghost()
                .icon(if all_favourite {
                    IconName::StarOff
                } else {
                    IconName::Star
                })
                .label(if all_favourite {
                    t("library.bulk.unfavourite")
                } else {
                    t("library.bulk.favourite")
                })
                .disabled(nothing)
                .on_click(move |_, window, cx| {
                    let on = !all_favourite;
                    let key = if on {
                        (
                            "library.bulk.done.favourited_one",
                            "library.bulk.done.favourited_other",
                        )
                    } else {
                        (
                            "library.bulk.done.unfavourited_one",
                            "library.bulk.done.unfavourited_other",
                        )
                    };
                    let message = counted(key, ids.len(), &[]);
                    edit_with_undo(
                        ids.to_vec(),
                        message,
                        move |u, ids| u.set_favourites(ids, on),
                        window,
                        cx,
                    )
                })
        };
        let hide = Button::new("bulk-hide")
            .small()
            .ghost()
            .icon(if all_hidden {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .label(if all_hidden {
                t("library.bulk.unhide")
            } else {
                t("library.bulk.hide")
            })
            .disabled(nothing)
            .on_click(move |_, window, cx| {
                let on = !all_hidden;
                let key = if on {
                    (
                        "library.bulk.done.hidden_one",
                        "library.bulk.done.hidden_other",
                    )
                } else {
                    (
                        "library.bulk.done.unhidden_one",
                        "library.bulk.done.unhidden_other",
                    )
                };
                let message = counted(key, ids.len(), &[]);
                edit_with_undo(
                    ids.to_vec(),
                    message,
                    move |u, ids| u.set_hidden_many(ids, on),
                    window,
                    cx,
                )
            });

        Some(
            div()
                .absolute()
                .bottom_6()
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .px_4()
                .child(
                    h_flex()
                        .id("library-bulk-bar")
                        .gap_2()
                        .px_3()
                        .py_2()
                        .flex_wrap()
                        .max_w_full()
                        .popover_style(cx)
                        .rounded(theme.radius_lg)
                        .child(div().px_1().text_sm().font_semibold().child(status))
                        .when(!nothing, |bar| {
                            bar.child(install)
                                .child(uninstall)
                                .child(steam)
                                .child(combine)
                                .child(categories_button)
                                .child(status_button)
                                .child(favourite)
                                .child(hide)
                        })
                        .child(
                            Button::new("bulk-select-all")
                                .small()
                                .ghost()
                                .label(t("library.bulk.select_all"))
                                .on_click(cx.listener(|this, _, _, cx| this.select_all(cx))),
                        )
                        .child(
                            Button::new("bulk-done")
                                .small()
                                .ghost()
                                .label(t("library.bulk.done"))
                                .on_click(cx.listener(|this, _, _, cx| this.end_selection(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The `_one` or `_other` key of `keys` for `count`, with `count` and
/// `args` filled in.
fn counted(keys: (&str, &str), count: usize, args: &[(&str, &str)]) -> SharedString {
    let count_text = count.to_string();
    let mut all: Vec<(&str, &str)> = vec![("count", &count_text)];
    all.extend_from_slice(args);
    t_args(if count == 1 { keys.0 } else { keys.1 }, &all)
}

/// Set (or clear) the play status of the picked games, with an undo.
fn set_statuses(
    ids: Vec<GameId>,
    status: Option<PlayStatus>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let message = match status {
        Some(status) => counted(
            (
                "library.bulk.done.status_one",
                "library.bulk.done.status_other",
            ),
            ids.len(),
            &[("status", &irori_ui::play_status::label(status))],
        ),
        None => counted(
            (
                "library.bulk.done.status_cleared_one",
                "library.bulk.done.status_cleared_other",
            ),
            ids.len(),
            &[],
        ),
    };
    edit_with_undo(
        ids,
        message,
        move |u, ids| u.set_play_status_many(ids, status),
        window,
        cx,
    );
}

/// Apply a bulk edit of library data off the UI thread and say so in a
/// notification whose Undo restores every game's previous favourite,
/// hidden, category and play-status values.
fn edit_with_undo(
    ids: Vec<GameId>,
    message: SharedString,
    edit: impl FnOnce(&UserDataService, &[GameId]) + Send + 'static,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    let b = backend(cx);
    let before = b.userdata.capture(&ids);
    irori_ui::state::run_infallible(cx, async move { edit(&b.userdata, &ids) }).detach();
    window.push_notification(
        Notification::success(message)
            .action(move |_, _, cx| {
                let note = cx.entity().downgrade();
                let before = before.clone();
                Button::new("bulk-undo")
                    .label(t("library.bulk.undo"))
                    .small()
                    .on_click(move |_, window, cx| {
                        let (b, before) = (backend(cx), before.clone());
                        irori_ui::state::run_infallible(
                            cx,
                            async move { b.userdata.restore(&before) },
                        )
                        .detach();
                        let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                    })
            })
            .autohide(true),
        cx,
    );
}

/// Uninstall `games` one after another, reporting each failure.
fn uninstall_all(games: Vec<Arc<GameInfo>>, window: &mut Window, cx: &mut gpui_kit::App) {
    let b = backend(cx);
    let args = RemoveArgs {
        remove_prefix: false,
        delete_files: true,
        remove_settings: false,
    };
    let task = irori_ui::run(cx, async move {
        let mut failed = Vec::new();
        for g in games {
            if let Err(e) = b.uninstall(&g.id(), args).await {
                failed.push((g.title.clone(), format!("{e:#}")));
            }
        }
        Ok(failed)
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(failed) = task.await else { return };
            let _ = cx.update(|window, cx| {
                for (title, error) in failed {
                    window.push_notification(
                        Notification::error(error).title(t_args(
                            "library.bulk.uninstall_failed",
                            &[("title", &title)],
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
}
