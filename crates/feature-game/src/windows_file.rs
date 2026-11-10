//! A Windows file opened with Irori from the file manager (a double-clicked
//! `.exe`, `.msi`, `.bat`, `.cmd` or `.reg`), or a link of another program's
//! scheme pointed at Irori (`ddnet:…`): ask which game's Wine prefix opens
//! it, then open it there.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Window, div, rems,
};
use irori_core::{GameId, GameInfo};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::ops::{notify_error, notify_success};

/// Ask which game's Wine prefix opens `file`, then open it there.
pub fn open_windows_file_dialog(file: PathBuf, window: &mut Window, cx: &mut gpui_kit::App) {
    open_picker(Opened::File(file), window, cx);
}

/// Ask which game's Wine prefix opens the link `uri` (a scheme another
/// program registered there), then open it there.
pub fn open_windows_uri_dialog(uri: String, window: &mut Window, cx: &mut gpui_kit::App) {
    open_picker(Opened::Uri(uri), window, cx);
}

fn open_picker(opened: Opened, window: &mut Window, cx: &mut gpui_kit::App) {
    let title = match &opened {
        Opened::File(_) => t("game.windows_file.title"),
        Opened::Uri(_) => t("game.windows_file.uri_title"),
    };
    let picker = cx.new(|cx| WindowsFilePicker::new(opened, window, cx));
    window.open_dialog(cx, move |d, window, _| {
        d.title(title.clone())
            .w(window.rem_size() * 30.)
            .child(picker.clone())
    });
}

/// What the picker opens in the chosen game's prefix.
#[derive(Clone)]
enum Opened {
    File(PathBuf),
    Uri(String),
}

impl Opened {
    /// How the picker and its notices name it: a file by its name, a link
    /// as it is (the user should see what a page or program handed over),
    /// shortened past [`LINK_SHOWN_CHARS`].
    fn name(&self) -> String {
        match self {
            Opened::File(file) => file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.display().to_string()),
            Opened::Uri(uri) => shown_link(uri),
        }
    }
}

/// Characters of a link the picker shows; links may run to thousands.
const LINK_SHOWN_CHARS: usize = 160;

fn shown_link(uri: &str) -> String {
    match uri.char_indices().nth(LINK_SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &uri[..end]),
        None => uri.to_string(),
    }
}

struct WindowsFilePicker {
    opened: Opened,
    /// Games whose prefix can open the file, the most recently played first.
    games: Vec<Arc<GameInfo>>,
    /// Indices into `games` that match the search.
    shown: Vec<usize>,
    chosen: Option<GameId>,
    query: Entity<InputState>,
    starting: bool,
    _subscriptions: Vec<Subscription>,
}

impl WindowsFilePicker {
    fn new(opened: Opened, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        let recent = backend(cx).userdata.snapshot().recent.clone();
        let mut games = pickable_games(&library);
        games.sort_by_cached_key(|g| {
            let key = g.id().key();
            (
                recent.iter().position(|k| *k == key).unwrap_or(usize::MAX),
                g.sort_title.clone(),
            )
        });
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("game.windows_file.search")));
        let subscriptions = vec![cx.subscribe_in(
            &query,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.filter(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.open(window, cx),
                _ => {}
            },
        )];
        let chosen = games.first().map(|g| g.id());
        Self {
            opened,
            shown: (0..games.len()).collect(),
            games,
            chosen,
            query,
            starting: false,
            _subscriptions: subscriptions,
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let needle = self.query.read(cx).value().trim().to_lowercase();
        self.shown = self
            .games
            .iter()
            .enumerate()
            .filter(|(_, g)| needle.is_empty() || g.title.to_lowercase().contains(&needle))
            .map(|(ix, _)| ix)
            .collect();
        // Keep the choice visible: a hidden game would open unseen.
        let visible = |id: &GameId| self.shown.iter().any(|&ix| self.games[ix].id() == *id);
        if !self.chosen.as_ref().is_some_and(visible) {
            self.chosen = self.shown.first().map(|&ix| self.games[ix].id());
        }
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.chosen.clone().filter(|_| !self.starting) else {
            return;
        };
        let title = self
            .games
            .iter()
            .find(|g| g.id() == id)
            .map(|g| g.title.clone())
            .unwrap_or_default();
        self.starting = true;
        cx.notify();
        let b = backend(cx);
        let opened = self.opened.clone();
        let task = irori_ui::run(cx, async move {
            match &opened {
                Opened::File(file) => b.open_windows_file(&id, file).await,
                Opened::Uri(uri) => b.open_windows_uri(&id, uri).await,
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.starting = false;
                let name = this.opened.name();
                let is_uri = matches!(this.opened, Opened::Uri(_));
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        let message = if is_uri {
                            t_args(
                                "game.windows_file.uri_started",
                                &[("link", &name), ("game", &title)],
                            )
                        } else {
                            t_args(
                                "game.windows_file.started",
                                &[("file", &name), ("game", &title)],
                            )
                        };
                        notify_success(window, cx, message);
                    }
                    Err(e) => {
                        let heading = if is_uri {
                            t_args("game.windows_file.uri_failed", &[("link", &name)])
                        } else {
                            t_args("game.ops.run_failed", &[("path", &name)])
                        };
                        notify_error(window, cx, heading, &e)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Height of one game in the picker, in rems.
const GAME_ROW_REMS: f32 = 2.;

impl Render for WindowsFilePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let starting = self.starting;
        let row_h = window.rem_size() * GAME_ROW_REMS;
        let count = self.shown.len();
        let sizes = std::rc::Rc::new(vec![gpui_kit::size(gpui_kit::px(0.), row_h); count]);
        let list = gpui_kit::component::v_virtual_list(
            cx.entity(),
            "windows-file-games",
            sizes,
            move |this: &mut Self, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|row| {
                        let ix = *this.shown.get(row)?;
                        let g = this.games.get(ix)?;
                        let id = g.id();
                        let checked = this.chosen.as_ref() == Some(&id);
                        let label: SharedString = g.title.clone().into();
                        Some(
                            div()
                                .h(rems(GAME_ROW_REMS))
                                .flex()
                                .items_center()
                                .min_w_0()
                                .child(
                                    Radio::new(("windows-file-game", ix))
                                        .label(label)
                                        .checked(checked)
                                        .disabled(starting)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.chosen = Some(id.clone());
                                            cx.notify();
                                        })),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect()
            },
        )
        .w_full()
        .h(rems(GAME_ROW_REMS * count.clamp(1, 8) as f32));
        let is_uri = matches!(self.opened, Opened::Uri(_));
        let body = if self.games.is_empty() {
            div()
                .text_sm()
                .child(if is_uri {
                    t("game.windows_file.uri_no_games")
                } else {
                    t("game.windows_file.no_games")
                })
                .into_any_element()
        } else if self.shown.is_empty() {
            div()
                .h(rems(GAME_ROW_REMS))
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t("game.windows_file.no_matches"))
                .into_any_element()
        } else {
            list.into_any_element()
        };
        v_flex()
            .gap_3()
            .child(
                div()
                    .min_w_0()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(match &self.opened {
                        Opened::File(_) => {
                            t_args("game.windows_file.body", &[("file", &self.opened.name())])
                        }
                        Opened::Uri(_) => t_args(
                            "game.windows_file.uri_body",
                            &[("link", &self.opened.name())],
                        ),
                    }),
            )
            .when(!self.games.is_empty(), |col| {
                col.child(Input::new(&self.query).prefix(Icon::new(IconName::Search).small()))
            })
            .child(body)
            .child(
                h_flex().justify_end().child(
                    Button::new("windows-file-open")
                        .primary()
                        .icon(IconName::Play)
                        .label(t("game.windows_file.open"))
                        .loading(starting)
                        .disabled(starting || self.chosen.is_none())
                        .on_click(cx.listener(|this, _, window, cx| this.open(window, cx))),
                ),
            )
    }
}

/// The games a Windows file can open in, each copy of an entry on its
/// own: the entry shows one copy, and the file may be meant for another.
fn pickable_games(library: &irori_services::LibrarySnapshot) -> Vec<Arc<GameInfo>> {
    library
        .all_games()
        .filter(|g| irori_services::opens_windows_files(g))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_links_are_shortened_for_the_picker() {
        assert_eq!(shown_link("ddnet:[::1]:8303"), "ddnet:[::1]:8303");
        let long = format!("someapp:{}", "é".repeat(400));
        let shown = shown_link(&long);
        assert_eq!(shown.chars().count(), LINK_SHOWN_CHARS + 1);
        assert!(shown.ends_with('…'));
        assert!(long.starts_with(shown.trim_end_matches('…')));
    }
}
