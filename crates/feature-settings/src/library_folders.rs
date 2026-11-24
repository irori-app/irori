//! "Library folders" panel of General → Folders (`iroriLibraryFolders`):
//! named folders games install into beside the default install folder (a
//! second drive, an SD card). Each row says how many installed games the
//! folder holds and moves them all into another folder; a named folder
//! leaves the list (its games stay where they are). The install sheet and
//! the library's selection bar offer the same folders.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _,
    PathPromptOptions, Render, Styled as _, Subscription, Window, div,
};
use irori_core::GameInfo;
use irori_core::settings::{LibraryFolder, folder_name, library_folder_of, same_folder};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, library_folders, models};

use crate::widgets::{muted, panel};

/// The installed games Irori moves, by the library folder holding them.
pub(crate) fn games_by_folder(
    folders: &[LibraryFolder],
    games: impl Iterator<Item = Arc<GameInfo>>,
) -> Vec<Vec<Arc<GameInfo>>> {
    let mut out = vec![Vec::new(); folders.len()];
    for game in games {
        if !game.is_installed || !game.id().runner.manages_installs() {
            continue;
        }
        if let Some(ix) = game
            .install_path()
            .and_then(|p| library_folder_of(folders, &p))
        {
            out[ix].push(game);
        }
    }
    out
}

/// The list with `folder` added, unless that folder is listed already (or
/// is the default one). An empty name becomes the folder's own name.
pub(crate) fn with_folder(
    default_path: &str,
    list: &[LibraryFolder],
    name: &str,
    path: &str,
) -> Option<Vec<LibraryFolder>> {
    if same_folder(default_path, path) || list.iter().any(|f| same_folder(&f.path, path)) {
        return None;
    }
    let name = match name.trim() {
        "" => folder_name(path),
        name => name.to_string(),
    };
    let mut list = list.to_vec();
    list.push(LibraryFolder {
        name,
        path: path.to_string(),
    });
    Some(list)
}

pub(crate) struct LibraryFoldersPanel {
    name: Entity<InputState>,
    folders: Vec<LibraryFolder>,
    /// Installed games of each folder, worked out when the library or the
    /// settings change, never per frame.
    games: Vec<Vec<Arc<GameInfo>>>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryFoldersPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.library_folders.name_placeholder"))
        });
        let (settings, library) = {
            let m = models(cx);
            (m.settings.clone(), m.library.clone())
        };
        let subscriptions = vec![
            cx.observe(&settings, |this, _, cx| this.refresh(cx)),
            cx.observe(&library, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.pick_folder(window, cx);
                }
            }),
        ];
        let mut this = Self {
            name,
            folders: Vec::new(),
            games: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let folders = models(cx).settings.read(cx).app.library_folders();
        let snapshot = models(cx).library.read(cx).snapshot.clone();
        self.games = games_by_folder(&folders, snapshot.all_games().cloned());
        self.folders = folders;
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("settings.library_folders.add")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.add(&path.to_string_lossy(), window, cx)
            });
        })
        .detach();
    }

    fn add(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().to_string();
        let b = backend(cx);
        let app = b.settings.app();
        let Some(list) = with_folder(&app.default_install_path, &app.library_folders, &name, path)
        else {
            window.push_notification(
                Notification::warning(t("settings.library_folders.already")),
                cx,
            );
            return;
        };
        b.settings.update_app(|a| a.library_folders = list);
        self.name.update(cx, |s, cx| s.set_value("", window, cx));
    }

    fn remove(&mut self, folder: &LibraryFolder, cx: &mut Context<Self>) {
        let path = folder.path.clone();
        backend(cx)
            .settings
            .update_app(|a| a.library_folders.retain(|f| !same_folder(&f.path, &path)));
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let folder = self.folders[ix].clone();
        let games = self.games.get(ix).cloned().unwrap_or_default();
        let count = games.len();
        let counted = t_args(
            if count == 1 {
                "settings.library_folders.games_one"
            } else {
                "settings.library_folders.games_other"
            },
            &[("count", &count.to_string())],
        );
        let targets: Vec<LibraryFolder> = self
            .folders
            .iter()
            .enumerate()
            .filter(|(i, f)| *i != ix && !f.path.trim().is_empty())
            .map(|(_, f)| f.clone())
            .collect();
        let move_to = Button::new(ElementId::Name(format!("library-folder-move-{ix}").into()))
            .small()
            .outline()
            .icon(IconName::FolderInput)
            .label(t("settings.library_folders.move_to"))
            .disabled(count == 0 || targets.is_empty())
            .dropdown_menu(move |mut menu, _, _| {
                for target in &targets {
                    let (games, target) = (games.clone(), target.clone());
                    menu = menu.item(
                        PopupMenuItem::new(library_folders::label(&target)).on_click(
                            move |_, window, cx| {
                                let ids = library_folders::movable(&games, &target, cx);
                                library_folders::confirm_move(ids, target.clone(), window, cx);
                            },
                        ),
                    );
                }
                menu
            });
        let theme = cx.theme();
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .when(ix > 0, |row| row.border_t_1().border_color(theme.border))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .truncate()
                            .child(library_folders::label(&folder)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(folder.path.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(counted),
                    ),
            )
            .child(move_to)
            .when(!folder.is_default(), |row| {
                row.child(
                    Button::new(ElementId::Name(
                        format!("library-folder-remove-{ix}").into(),
                    ))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(t("settings.library_folders.remove"))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&folder, cx))),
                )
            })
            .into_any_element()
    }
}

impl Render for LibraryFoldersPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = (0..self.folders.len())
            .map(|ix| self.render_row(ix, cx))
            .collect();
        panel(cx)
            .children(rows)
            .when(self.folders.len() < 2, |body| {
                body.child(
                    div()
                        .px_4()
                        .pb_2()
                        .child(muted(t("settings.library_folders.empty"), cx)),
                )
            })
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().child(Input::new(&self.name)))
                    .child(
                        Button::new("library-folder-add")
                            .outline()
                            .small()
                            .icon(IconName::FolderPlus)
                            .label(t("settings.library_folders.add"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use irori_core::{InstalledInfo, Runner};

    use super::*;

    fn installed(runner: Runner, app: &str, path: &str) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            is_installed: true,
            install: Some(InstalledInfo {
                install_path: path.into(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    #[test]
    fn adds_each_folder_once_and_names_unnamed_ones() {
        let list = with_folder("/games", &[], "", "/run/media/me/SD card/").unwrap();
        assert_eq!(list[0].name, "SD card");
        assert!(with_folder("/games", &list, "Again", "/run/media/me/SD card").is_none());
        assert!(with_folder("/games", &list, "Default", "/games/").is_none());
        let list = with_folder("/games", &list, " Big ", "/mnt/big").unwrap();
        assert_eq!(list[1].name, "Big");
    }

    #[test]
    fn counts_only_installed_games_irori_moves() {
        let folders = vec![
            LibraryFolder {
                name: String::new(),
                path: "/games".into(),
            },
            LibraryFolder {
                name: "SD".into(),
                path: "/sd".into(),
            },
        ];
        let mut not_installed = (*installed(Runner::Gog, "c", "/sd/C")).clone();
        not_installed.is_installed = false;
        let games = vec![
            installed(Runner::Gog, "a", "/games/GOG/A"),
            installed(Runner::Legendary, "b", "/sd/B"),
            Arc::new(not_installed),
            // Steam keeps its own folders.
            installed(Runner::Steam, "d", "/sd/D"),
            installed(Runner::Gog, "e", "/elsewhere/E"),
        ];
        let by = games_by_folder(&folders, games.into_iter());
        assert_eq!(by[0].len(), 1);
        assert_eq!(by[1].len(), 1);
        assert_eq!(&*by[1][0].app_name, "b");
    }
}
