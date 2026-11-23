//! "Add a folder": pick a folder, see the games in it (each with the program
//! that starts it), tick the ones to add, rename one, or start one with
//! another program of its folder. Games already in the library are marked
//! and left alone. Cover art comes from a desktop entry's own picture, or,
//! when the user leaves "Find cover art online" ticked, from Steam through
//! PCGamingWiki.

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Size, Styled as _, Subscription, Window, div, px, rems, size,
};
use irori_sideload::scan::{FoundGame, FoundKind};
use irori_sideload::{SideloadApp, SideloadPlatform};
use irori_ui::i18n::{t, t_args};

use super::{dialog_width, pick_path};

const ROW_REMS: f32 = 3.25;

/// One found game and whether it will be added.
struct Row {
    game: FoundGame,
    /// Already in the library (same program).
    known: bool,
    picked: bool,
}

/// The list of games found in a folder.
pub struct AddFolderDialog {
    folder: PathBuf,
    rows: Vec<Row>,
    /// Programs the library already starts.
    known: HashSet<PathBuf>,
    /// The row whose title is being edited, with its field.
    renaming: Option<(usize, Entity<InputState>)>,
    find_art: bool,
    adding: bool,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
    _renaming: Option<Subscription>,
}

impl AddFolderDialog {
    fn new(folder: PathBuf, found: Vec<FoundGame>, known: HashSet<PathBuf>) -> Self {
        let rows = found
            .into_iter()
            .map(|game| {
                let known = known.contains(&game.executable);
                Row {
                    game,
                    known,
                    picked: !known,
                }
            })
            .collect();
        Self {
            folder,
            rows,
            known,
            renaming: None,
            find_art: true,
            adding: false,
            sizes: Rc::default(),
            sized_for: (0, px(0.)),
            _renaming: None,
        }
    }

    /// Edit the title of row `ix` in place.
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let title = row.game.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |s, cx| s.focus(window, cx));
        self._renaming =
            Some(
                cx.subscribe_in(&input, window, |this, _, event, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(cx),
                    _ => {}
                }),
            );
        self.renaming = Some((ix, input));
        cx.notify();
    }

    /// Keep the edited title and stop editing.
    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some((ix, input)) = self.renaming.take() else {
            return;
        };
        self._renaming = None;
        let title = super::text(&input, cx);
        if let Some(row) = self.rows.get_mut(ix) {
            rename(&mut row.game, &title);
        }
        cx.notify();
    }

    /// Start row `ix`'s game with `program` (one of its folder's others).
    fn choose_program(&mut self, ix: usize, program: PathBuf, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(ix) else {
            return;
        };
        if choose_program(&mut row.game, &program) {
            let was_known = row.known;
            row.known = self.known.contains(&row.game.executable);
            if row.known {
                row.picked = false;
            } else if was_known {
                row.picked = true;
            }
        }
        cx.notify();
    }

    fn picked(&self) -> Vec<FoundGame> {
        self.rows
            .iter()
            .filter(|r| r.picked && !r.known)
            .map(|r| r.game.clone())
            .collect()
    }

    fn set_all(&mut self, on: bool, cx: &mut Context<Self>) {
        for row in self.rows.iter_mut().filter(|r| !r.known) {
            row.picked = on;
        }
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_rename(cx);
        let games = self.picked();
        let Some(store) = crate::sideload_store(cx) else {
            return;
        };
        if games.is_empty() || self.adding {
            return;
        }
        self.adding = true;
        cx.notify();
        let find_art = self.find_art;
        let task = irori_ui::run(cx, async move {
            let mut added = 0usize;
            let mut failed = Vec::new();
            for game in games {
                // Pictures only: themed icon names and SVGs are not cover art.
                let picture = game.icon.as_ref().filter(|p| super::is_picture(p));
                let (cover, background) = match picture.map(|p| p.display().to_string()) {
                    Some(c) => (Some(c), None),
                    None if find_art => {
                        match irori_extras::wiki::steam_art_for_title(&game.title).await {
                            Some((c, h)) => (Some(c), Some(h)),
                            None => (None, None),
                        }
                    }
                    None => (None, None),
                };
                let app = SideloadApp {
                    title: game.title.clone(),
                    executable: Some(game.executable.clone()),
                    platform: game.platform,
                    install_folder: game.install_folder.clone(),
                    launcher_args: game.args.clone(),
                    cover_art: cover,
                    background_art: background,
                    ..Default::default()
                };
                match store.add_app(app).await {
                    Ok(_) => added += 1,
                    Err(e) => failed.push((game.title, format!("{e:#}"))),
                }
            }
            Ok((added, failed))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.adding = false;
                match result {
                    Ok((added, failed)) => {
                        window.close_dialog(cx);
                        if added > 0 {
                            let message = if added == 1 {
                                t("library.folder.added_one")
                            } else {
                                t_args(
                                    "library.folder.added_other",
                                    &[("count", &added.to_string())],
                                )
                            };
                            window.push_notification(Notification::success(message), cx);
                        }
                        for (title, error) in failed {
                            window.push_notification(
                                Notification::error(error).title(t_args(
                                    "library.folder.add_failed",
                                    &[("title", &title)],
                                )),
                                cx,
                            );
                        }
                    }
                    Err(e) => crate::notify_error(t("library.folder.title"), &e, window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?;
                let game = &row.game;
                let editing = self
                    .renaming
                    .as_ref()
                    .filter(|(at, _)| *at == ix)
                    .map(|(_, input)| input.clone());
                let programs: Rc<Vec<PathBuf>> = Rc::new(programs_of(game));
                let detail: SharedString = if row.known {
                    t("library.folder.already_added")
                } else {
                    let program = relative(&game.executable, &self.folder);
                    match game.kind {
                        FoundKind::DesktopEntry => t_args(
                            "library.folder.from_desktop_entry",
                            &[("program", &program)],
                        ),
                        FoundKind::Shortcut => {
                            t_args("library.folder.from_shortcut", &[("program", &program)])
                        }
                        FoundKind::Program | FoundKind::AppBundle => program.into(),
                    }
                };
                let platform = match game.platform {
                    SideloadPlatform::Windows => "Windows",
                    SideloadPlatform::Linux => "Linux",
                    SideloadPlatform::Mac => "macOS",
                    SideloadPlatform::Browser => "",
                };
                Some(
                    h_flex()
                        .h(rems(ROW_REMS))
                        .w_full()
                        .gap_3()
                        .px_1()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            Checkbox::new(SharedString::from(format!("folder-game-{ix}")))
                                .checked(row.picked && !row.known)
                                .disabled(row.known || self.adding)
                                .on_change(cx.listener(move |this, on: &bool, _, cx| {
                                    if let Some(row) = this.rows.get_mut(ix) {
                                        row.picked = *on;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(match &editing {
                                    Some(input) => Input::new(input).xsmall().into_any_element(),
                                    None => div()
                                        .text_sm()
                                        .font_medium()
                                        .truncate()
                                        .child(game.title.clone())
                                        .into_any_element(),
                                })
                                .child(div().text_xs().text_color(muted).truncate().child(detail)),
                        )
                        .when(editing.is_none() && !row.known, |d| {
                            d.child(
                                Button::new(SharedString::from(format!("folder-rename-{ix}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Pencil)
                                    .tooltip(t("library.folder.rename"))
                                    .disabled(self.adding)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_rename(ix, window, cx)
                                    })),
                            )
                        })
                        .when(!programs.is_empty() && !self.adding, |d| {
                            let view = cx.entity().downgrade();
                            let current = game.executable.clone();
                            let folder = game
                                .install_folder
                                .clone()
                                .unwrap_or_else(|| self.folder.clone());
                            d.child(
                                Button::new(SharedString::from(format!("folder-program-{ix}")))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::ChevronDown)
                                    .tooltip(t("library.folder.choose_program"))
                                    .dropdown_menu(move |mut menu, _, _| {
                                        for program in programs.iter() {
                                            let (view, program) = (view.clone(), program.clone());
                                            let label = relative(&program, &folder);
                                            let checked = program == current;
                                            menu = menu.item(
                                                PopupMenuItem::new(label)
                                                    .checked(checked)
                                                    .on_click(move |_, _, cx| {
                                                        let program = program.clone();
                                                        let _ = view.update(cx, |this, cx| {
                                                            this.choose_program(ix, program, cx)
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .child(platform),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

/// Give `game` the title `title`; an empty one keeps the title it had.
fn rename(game: &mut FoundGame, title: &str) {
    let title = title.trim();
    if !title.is_empty() {
        game.title = title.to_string();
    }
}

/// Every program `game` can start with: the chosen one first, then its
/// folder's others. Empty when there is nothing to choose between.
fn programs_of(game: &FoundGame) -> Vec<PathBuf> {
    if game.others.is_empty() {
        return Vec::new();
    }
    std::iter::once(game.executable.clone())
        .chain(game.others.iter().map(|(p, _)| p.clone()))
        .collect()
}

/// Start `game` with `program`, one of [`programs_of`]; the program it had
/// joins the others. False when `program` isn't one of them.
fn choose_program(game: &mut FoundGame, program: &Path) -> bool {
    if game.executable == program {
        return true;
    }
    let Some(at) = game.others.iter().position(|(p, _)| p == program) else {
        return false;
    };
    let (path, platform) = game.others.remove(at);
    let previous = (
        std::mem::replace(&mut game.executable, path),
        std::mem::replace(&mut game.platform, platform),
    );
    game.others.insert(0, previous);
    true
}

/// `path` shown from inside `folder`.
fn relative(path: &Path, folder: &Path) -> String {
    path.strip_prefix(folder)
        .unwrap_or(path)
        .display()
        .to_string()
}

impl Render for AddFolderDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let count = self.rows.len();
        let row_h = window.rem_size() * ROW_REMS;
        if self.sized_for != (count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (count, row_h);
        }
        let picked = self.picked().len();
        let addable = self.rows.iter().filter(|r| !r.known).count();
        let folder = self.folder.display().to_string();
        let summary = match count {
            0 => t_args("library.folder.none_found", &[("folder", &folder)]),
            1 => t_args("library.folder.found_one", &[("folder", &folder)]),
            n => t_args(
                "library.folder.found_other",
                &[("count", &n.to_string()), ("folder", &folder)],
            ),
        };
        let list = v_virtual_list(
            cx.entity(),
            "folder-games",
            self.sizes.clone(),
            |this, range, _, cx| this.render_rows(range, cx),
        )
        .h(rems((count.min(7) as f32) * ROW_REMS))
        .w_full();
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child(summary))
            .when(count > 0, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("folder-all")
                                .small()
                                .ghost()
                                .label(t("library.folder.select_all"))
                                .disabled(addable == 0 || self.adding)
                                .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
                        )
                        .child(
                            Button::new("folder-none")
                                .small()
                                .ghost()
                                .label(t("library.folder.select_none"))
                                .disabled(picked == 0 || self.adding)
                                .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
                        ),
                )
                .child(list)
                .child(
                    Checkbox::new("folder-find-art")
                        .label(t("library.folder.find_art"))
                        .checked(self.find_art)
                        .disabled(self.adding)
                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                            this.find_art = *on;
                            cx.notify();
                        })),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(Icon::new(IconName::Info).xsmall())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(t("library.folder.skipped_hint")),
                        ),
                )
            })
            .child(
                DialogFooter::new()
                    .child(
                        Button::new("folder-cancel")
                            .ghost()
                            .label(t("library.common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("folder-add")
                            .primary()
                            .icon(IconName::Plus)
                            .label(match picked {
                                1 => t("library.folder.add_one"),
                                n => {
                                    t_args("library.folder.add_other", &[("count", &n.to_string())])
                                }
                            })
                            .loading(self.adding)
                            .disabled(picked == 0 || self.adding)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
    }
}

/// Pick a folder, look for games in it and offer them.
pub fn open_add_folder_dialog(window: &mut Window, cx: &mut App) {
    if crate::sideload_store(cx).is_none() {
        crate::notify_error(
            t("library.folder.title"),
            &anyhow::anyhow!(t("library.sideload.error.unavailable").to_string()),
            window,
            cx,
        );
        return;
    }
    let pick = pick_path(true, t("library.folder.pick"), cx);
    window
        .spawn(cx, async move |cx| {
            let Some(folder) = pick.await else { return };
            let _ = cx.update(|window, cx| offer_folder(folder, window, cx));
        })
        .detach();
}

/// Look for games in a folder dropped on the library and offer them.
pub(crate) fn open_add_dropped_folder_dialog(folder: PathBuf, window: &mut Window, cx: &mut App) {
    if crate::sideload_store(cx).is_none() {
        crate::notify_error(
            t("library.folder.title"),
            &anyhow::anyhow!(t("library.sideload.error.unavailable").to_string()),
            window,
            cx,
        );
        return;
    }
    offer_folder(folder, window, cx);
}

/// Scan `folder` off the UI thread, then offer the games found in it.
fn offer_folder(folder: PathBuf, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let scan = cx.update(|_, cx| {
                let folder = folder.clone();
                let scan = irori_ui::backend(cx)
                    .spawn_blocking(move || irori_sideload::scan::scan_folder(&folder));
                irori_ui::run(cx, async move { Ok(scan.await?) })
            });
            let Ok(scan) = scan else { return };
            let found = scan.await;
            let _ = cx.update(|window, cx| {
                let found = match found {
                    Ok(found) => found,
                    Err(e) => {
                        crate::notify_error(t("library.folder.title"), &e, window, cx);
                        return;
                    }
                };
                let known: HashSet<PathBuf> = crate::sideload_store(cx)
                    .map(|s| {
                        use irori_core::Store as _;
                        s.games()
                            .iter()
                            .filter_map(|g| g.install.as_ref())
                            .map(|i| PathBuf::from(&i.executable))
                            .collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let view = cx.new(|_| AddFolderDialog::new(folder.clone(), found, known));
                let width = dialog_width(window, 34.);
                window.open_dialog(cx, move |dialog, _, _| {
                    dialog
                        .title(t("library.folder.title"))
                        .w(width)
                        .child(view.clone())
                });
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> FoundGame {
        FoundGame {
            title: "Celeste".into(),
            executable: "/games/Celeste/Celeste.exe".into(),
            platform: SideloadPlatform::Windows,
            args: String::new(),
            install_folder: Some("/games/Celeste".into()),
            icon: None,
            kind: FoundKind::Program,
            others: vec![
                (
                    "/games/Celeste/Launcher.exe".into(),
                    SideloadPlatform::Windows,
                ),
                ("/games/Celeste/celeste.sh".into(), SideloadPlatform::Linux),
            ],
        }
    }

    #[test]
    fn a_rename_keeps_the_old_title_when_left_empty() {
        let mut g = game();
        rename(&mut g, "  Celeste Classic ");
        assert_eq!(g.title, "Celeste Classic");
        rename(&mut g, "   ");
        assert_eq!(g.title, "Celeste Classic");
    }

    #[test]
    fn choosing_another_program_swaps_it_with_the_chosen_one() {
        let mut g = game();
        assert_eq!(programs_of(&g).len(), 3);
        assert!(choose_program(
            &mut g,
            Path::new("/games/Celeste/celeste.sh")
        ));
        assert_eq!(g.executable, PathBuf::from("/games/Celeste/celeste.sh"));
        assert_eq!(g.platform, SideloadPlatform::Linux);
        assert_eq!(
            programs_of(&g),
            [
                PathBuf::from("/games/Celeste/celeste.sh"),
                PathBuf::from("/games/Celeste/Celeste.exe"),
                PathBuf::from("/games/Celeste/Launcher.exe"),
            ]
        );
        // The program it has, or one from elsewhere, changes nothing.
        assert!(choose_program(
            &mut g,
            Path::new("/games/Celeste/celeste.sh")
        ));
        assert!(!choose_program(&mut g, Path::new("/bin/sh")));
        assert_eq!(g.others.len(), 2);
        // Nothing to choose between: no menu.
        g.others.clear();
        assert!(programs_of(&g).is_empty());
    }
}
