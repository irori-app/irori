//! The palette's view: the search field and its rows, and beside them a
//! preview of the highlighted row. Commands are built once when the palette
//! opens; games are found per query with the library's search and drawn with
//! their cover, so only a bounded number of game rows exists at a time. The
//! rows for running games follow the games as they start and end.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, px, rems,
};
use irori_core::{GameId, GameInfo, StatusKind};
use irori_ui::art::LruImageCache;
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::entry::Entry;
use super::preview::PreviewPane;
use super::search::{MAX_GAMES, MAX_PLAY_ROWS, rank_games};
use super::{PaletteTarget, run_target};

/// Where the palette shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    /// A dialog of the main window (Ctrl+K).
    Dialog,
}

/// One row: what it says, what confirming it does, and the game it is
/// about, if any (drawn with its cover, previewed beside the list).
#[derive(Clone)]
pub(super) struct Row {
    pub(super) entry: Entry,
    pub(super) target: PaletteTarget,
    pub(super) game: Option<Arc<GameInfo>>,
    /// The store and state shown at the end of a game row.
    pub(super) detail: Option<SharedString>,
}

impl Row {
    pub(super) fn command(entry: Entry, target: PaletteTarget) -> Self {
        Self {
            entry,
            target,
            game: None,
            detail: None,
        }
    }

    fn item(&self) -> CommandItem {
        let Some(game) = self.game.clone() else {
            return self.entry.item();
        };
        let (label, detail) = (self.entry.label.clone(), self.detail.clone());
        self.entry
            .item_with(move |_, _| game_row(game.clone(), label.clone(), detail.clone()))
    }
}

/// A game row: its cover, its title and, quieter, its store and state. The
/// row's text colour follows the highlight, so the detail is dimmed by
/// opacity rather than a colour of its own.
fn game_row(
    game: Arc<GameInfo>,
    label: SharedString,
    detail: Option<SharedString>,
) -> impl IntoElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .gap_2()
        .items_center()
        .child(GameCover::new(game).width(rems(1.25)).radius(px(3.)))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(detail, |row, detail| {
            row.child(div().flex_shrink_0().text_xs().opacity(0.7).child(detail))
        })
}

/// The width below which the preview is left out, in rems of the window.
const PREVIEW_ROOM_REMS: f32 = 46.;

/// Whether `window` leaves room for the preview beside the list.
pub(super) fn is_wide(window: &Window) -> bool {
    irori_ui::style::visible_size(window).width >= window.rem_size() * PREVIEW_ROOM_REMS
}

/// The palette's width in `window`: room for the preview when there is.
pub(super) fn width(window: &Window) -> gpui_kit::Pixels {
    let rems = if is_wide(window) { 52. } else { 32. };
    window.rem_size() * rems
}

/// What a game is doing, for its row and its preview.
pub(super) fn state_label(game: &GameInfo, kind: Option<StatusKind>) -> SharedString {
    match kind {
        Some(StatusKind::Playing) => t("shell.palette.state_playing"),
        Some(StatusKind::Launching) => t("shell.palette.state_launching"),
        _ if game.is_installed => t("shell.palette.state_installed"),
        _ => t("shell.palette.state_not_installed"),
    }
}

/// The running and starting games, which the palette's first rows are about.
fn live_games(cx: &App) -> Vec<(GameId, StatusKind)> {
    let b = irori_ui::backend(cx);
    let mut live: Vec<(GameId, StatusKind)> = models(cx)
        .statuses
        .read(cx)
        .all()
        .filter(|s| matches!(s.kind, StatusKind::Playing | StatusKind::Launching))
        .filter(|s| !b.concealed(&s.id))
        .map(|s| (s.id.clone(), s.kind))
        .collect();
    live.sort_by_key(|(id, _)| id.key());
    live
}

/// Rows for the games running (back to them) and starting (cancel).
fn lead_rows(live: &[(GameId, StatusKind)], cx: &App) -> Vec<Row> {
    let b = irori_ui::backend(cx);
    let title = |id: &GameId| {
        b.game(id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| id.app_name.to_string())
    };
    let mut rows = Vec::new();
    for (id, _) in live.iter().filter(|(_, k)| *k == StatusKind::Playing) {
        rows.push(Row::command(
            Entry::new()
                .label(t_args(
                    "shell.palette.back_to_game",
                    &[("title", &title(id))],
                ))
                .keywords([t("shell.palette.back_to_game_keywords")])
                .icon(IconName::AppWindow),
            PaletteTarget::BackToGame(id.clone()),
        ));
    }
    // A launch being prepared comes first: it is what the user waits on.
    for (id, _) in live.iter().filter(|(_, k)| *k == StatusKind::Launching) {
        rows.insert(
            0,
            Row::command(
                Entry::new()
                    .label(t_args(
                        "shell.palette.cancel_launch",
                        &[("title", &title(id))],
                    ))
                    .keywords([t("shell.palette.cancel_launch_keywords")])
                    .icon(IconName::X),
                PaletteTarget::CancelLaunch(id.clone()),
            ),
        );
    }
    rows
}

/// The palette: a search over commands and games, with a preview.
pub(crate) struct PaletteView {
    state: Entity<CommandState>,
    preview: Entity<PreviewPane>,
    images: Entity<LruImageCache>,
    host: Host,
    /// The command rows, built when the palette opened.
    commands: Vec<Row>,
    /// The running and starting games the first rows are about.
    live: Vec<(GameId, StatusKind)>,
    /// The library revision the game rows were found in.
    revision: u64,
    query: String,
    /// The rows the list holds now, in order.
    shown: Rc<Vec<Row>>,
    _subscriptions: Vec<Subscription>,
}

impl PaletteView {
    pub(crate) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let subscriptions = vec![
            cx.observe(&m.statuses, |this, _, cx| {
                let live = live_games(cx);
                if live != this.live {
                    this.live = live;
                    this.rebuild(cx);
                }
            }),
            cx.observe(&m.library, |this, library, cx| {
                let revision = library.read(cx).snapshot.revision;
                if revision != this.revision {
                    this.rebuild(cx);
                }
            }),
        ];
        let mut view = Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            preview: cx.new(|cx| PreviewPane::new(host, cx)),
            images: LruImageCache::new(MAX_GAMES * 2, cx),
            host,
            commands: super::command_rows(window, cx),
            live: live_games(cx),
            revision: 0,
            query: String::new(),
            shown: Rc::default(),
            _subscriptions: subscriptions,
        };
        view.rebuild(cx);
        view
    }

    /// Put the keyboard in `view`'s search field.
    pub(crate) fn focus(view: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let state = view.read(cx).state.clone();
        state.update(cx, |s, cx| s.focus(window, cx));
    }

    fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.rebuild(cx);
        }
    }

    /// Find the rows for the query again. The list highlights its first row
    /// after a change, so the preview follows it.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let query = self.query.trim().to_lowercase();
        let games = self.game_rows(&query, cx);
        // A running game's rows come along when its game is found, typo and
        // all, not only when their own words hold the query.
        let found = |target: &PaletteTarget| match target {
            PaletteTarget::BackToGame(id) | PaletteTarget::CancelLaunch(id) => games
                .iter()
                .any(|g| g.game.as_ref().is_some_and(|g| g.id() == *id)),
            _ => false,
        };
        let mut rows: Vec<Row> = lead_rows(&self.live, cx)
            .into_iter()
            .chain(self.commands.iter().cloned())
            .filter(|r| r.entry.matches(&query) || found(&r.target))
            .collect();
        rows.extend(games);
        self.shown = Rc::new(rows);
        let first = self.shown.first().cloned();
        self.preview.update(cx, |p, cx| p.show(first, cx));
        cx.notify();
    }

    /// The games `query` finds (the recently played ones without a query),
    /// and before them the installed ones to play at once.
    fn game_rows(&mut self, query: &str, cx: &App) -> Vec<Row> {
        let recent = query.is_empty().then(|| crate::sidebar::recent_games(cx));
        let library = models(cx).library.read(cx);
        self.revision = library.snapshot.revision;
        let games = recent.unwrap_or_else(|| {
            let (user, locked) = (&library.user, library.snapshot.hidden_locked);
            // Hidden games behind the PIN aren't found here either.
            let keep = |g: &GameInfo| !(locked && user.hidden.contains(&g.id().key()));
            rank_games(
                &library.snapshot.games,
                library.snapshot.search(),
                query,
                keep,
                MAX_GAMES,
            )
        });
        let statuses = models(cx).statuses.read(cx);
        let mut rows = Vec::with_capacity(games.len() + MAX_PLAY_ROWS);
        if !query.is_empty() {
            for g in games.iter().filter(|g| g.is_installed).take(MAX_PLAY_ROWS) {
                rows.push(Row::command(
                    Entry::new()
                        .label(t_args("shell.palette.play", &[("title", &g.title)]))
                        .keywords([g.runner().short_name()])
                        .icon(IconName::Play),
                    PaletteTarget::Play(g.id()),
                ));
            }
        }
        for g in games {
            let id = g.id();
            let detail = format!(
                "{} · {}",
                g.runner().short_name(),
                state_label(&g, statuses.kind(&id))
            );
            rows.push(Row {
                entry: Entry::new()
                    .label(g.title.clone())
                    .keywords([g.runner().short_name(), g.runner().store_name()])
                    .icon(IconName::Gamepad2),
                target: PaletteTarget::Open(id),
                game: Some(g),
                detail: Some(detail.into()),
            });
        }
        rows
    }

    /// The rows the list holds now.
    #[cfg(test)]
    pub(super) fn shown_rows(&self) -> Rc<Vec<Row>> {
        self.shown.clone()
    }

    /// Run the confirmed row.
    fn confirm(host: Host, row: Row, window: &mut Window, cx: &mut App) {
        match host {
            Host::Dialog => {
                window.close_dialog(cx);
                run_target(row.target, window, cx);
            }
        }
    }
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let host = self.host;
        let items: Vec<CommandItem> = self.shown.iter().map(Row::item).collect();
        let on_query = {
            let weak = weak.clone();
            move |query: &str, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |v, cx| v.set_query(query, cx));
            }
        };
        let on_select = {
            let weak = weak.clone();
            move |ix: IndexPath, _: &mut Window, cx: &mut App| {
                let Some(view) = weak.upgrade() else { return };
                let (row, preview) = {
                    let v = view.read(cx);
                    (v.shown.get(ix.row).cloned(), v.preview.clone())
                };
                preview.update(cx, |p, cx| p.show(row, cx));
            }
        };
        let on_confirm = move |ix: IndexPath, window: &mut Window, cx: &mut App| {
            let row = weak
                .upgrade()
                .and_then(|v| v.read(cx).shown.get(ix.row).cloned());
            if let Some(row) = row {
                Self::confirm(host, row, window, cx);
            }
        };
        let list = Command::new(&self.state)
            .bordered(false)
            .rounded(cx.theme().radius_lg)
            .placeholder(t("shell.search.placeholder"))
            .max_h(px(440.))
            .filterable(false)
            .items(items)
            .on_query(on_query)
            .on_select(on_select)
            .on_confirm(on_confirm);
        h_flex()
            .image_cache(self.images.clone())
            .w_full()
            .items_stretch()
            .child(div().flex_1().min_w_0().child(list))
            .when(is_wide(window), |row| row.child(self.preview.clone()))
    }
}
