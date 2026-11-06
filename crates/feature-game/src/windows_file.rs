//! A Windows file opened with Irori from the file manager (a double-clicked
//! `.exe`, `.msi`, `.bat`, `.cmd` or `.reg`): ask which game's Wine prefix
//! opens it, then open it there.

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
    let picker = cx.new(|cx| WindowsFilePicker::new(file, window, cx));
    window.open_dialog(cx, move |d, window, _| {
        d.title(t("game.windows_file.title"))
            .w(window.rem_size() * 30.)
            .child(picker.clone())
    });
}

struct WindowsFilePicker {
    file: PathBuf,
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
    fn new(file: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        let recent = backend(cx).userdata.snapshot().recent.clone();
        let mut games: Vec<Arc<GameInfo>> = library
            .games
            .iter()
            .filter(|g| irori_services::opens_windows_files(g))
            .cloned()
            .collect();
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
            file,
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

    fn file_name(&self) -> String {
        self.file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.file.display().to_string())
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
        let file = self.file.clone();
        let task = irori_ui::run(cx, async move { b.open_windows_file(&id, &file).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.starting = false;
                let name = this.file_name();
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        notify_success(
                            window,
                            cx,
                            t_args(
                                "game.windows_file.started",
                                &[("file", &name), ("game", &title)],
                            ),
                        );
                    }
                    Err(e) => notify_error(
                        window,
                        cx,
                        t_args("game.ops.run_failed", &[("path", &name)]),
                        &e,
                    ),
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
        let body = if self.games.is_empty() {
            div()
                .text_sm()
                .child(t("game.windows_file.no_games"))
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
                    .child(t_args(
                        "game.windows_file.body",
                        &[("file", &self.file_name())],
                    )),
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
