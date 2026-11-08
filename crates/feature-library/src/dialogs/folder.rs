//! "Add a folder": pick a folder, see the games in it (each with the program
//! that starts it), tick the ones to add. Games already in the library are
//! marked and left alone. Cover art comes from a desktop entry's own
//! picture, or, when the user leaves "Find cover art online" ticked, from
//! Steam through PCGamingWiki.

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Size, Styled as _, Window, div, px, rems, size,
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
    find_art: bool,
    adding: bool,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
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
            find_art: true,
            adding: false,
            sizes: Rc::default(),
            sized_for: (0, px(0.)),
        }
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
                                .child(
                                    div()
                                        .text_sm()
                                        .font_medium()
                                        .truncate()
                                        .child(game.title.clone()),
                                )
                                .child(div().text_xs().text_color(muted).truncate().child(detail)),
                        )
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
