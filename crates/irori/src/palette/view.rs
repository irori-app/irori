//! The palette's view: the search field and its grouped rows, and beside them
//! a preview of the highlighted row. Commands, pages, and settings are built
//! and pre-folded once when the palette opens; games are scored per query with
//! the library's search and drawn with their cover, while groups reorder
//! dynamically by relevance and host context.
//!
//! → on a game lists that game's actions (a second level, searched like the
//! first); ←, Escape and the "Back" row return to the first level.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::actions::Cancel;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::input::{Enter as InputEnter, Escape as InputEscape, MoveLeft, MoveRight};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px, rems,
};
use irori_core::{GameId, GameInfo, StatusKind};
use irori_services::search::Query;
use irori_ui::art::LruImageCache;
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::entry::Entry;
use super::preview::PreviewPane;
use super::score::{
    EntryKind, GroupKind, ScopeFilter, ScoredGroup, ShownGroup, parse_input, rank_groups,
};
use super::search::{MAX_GAMES, rank_games_scored};
use super::{PaletteTarget, run_target};
use crate::strings::english;

/// Where the palette shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    /// A dialog of the main window (`Ctrl+K`).
    Dialog,
    /// The quick launcher's own window (`crate::quick_launch`): Enter plays.
    Window,
}

/// One row: what it says, what confirming it does, and the game or setting it
/// represents.
#[derive(Clone)]
pub(super) struct Row {
    pub(super) entry: Entry,
    pub(super) target: PaletteTarget,
    pub(super) game: Option<Arc<GameInfo>>,
    /// The game a command row acts on, previewed beside the list but drawn
    /// as a command.
    pub(super) about: Option<Arc<GameInfo>>,
    /// Subtle trailing detail (store & state for games, breadcrumb for settings
    /// and settings pages).
    pub(super) detail: Option<SharedString>,
    /// Registry path when this row represents an individual editable setting.
    pub(super) setting_path: Option<&'static str>,
    /// Plain-language help shown in the preview pane for settings.
    pub(super) help: Option<SharedString>,
}

impl Row {
    pub(super) fn command(entry: Entry, target: PaletteTarget) -> Self {
        Self {
            entry,
            target,
            game: None,
            about: None,
            detail: None,
            setting_path: None,
            help: None,
        }
    }

    /// The game the preview shows for this row.
    pub(super) fn previewed_game(&self) -> Option<&Arc<GameInfo>> {
        self.game.as_ref().or(self.about.as_ref())
    }

    fn item(&self) -> CommandItem {
        if let Some(game) = self.game.clone() {
            let (label, detail) = (self.entry.label.clone(), self.detail.clone());
            return self
                .entry
                .item_with(move |_, _| game_row(game.clone(), label.clone(), detail.clone()));
        }
        if let Some(detail) = self.detail.clone() {
            let (icon, label) = (self.entry.icon, self.entry.label.clone());
            return self
                .entry
                .item_with(move |_, _| detail_row(icon, label.clone(), detail.clone()));
        }
        self.entry.item()
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

/// A page or setting row with a subtle trailing breadcrumb.
fn detail_row(
    icon: Option<IconName>,
    label: SharedString,
    detail: SharedString,
) -> impl IntoElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .gap_2()
        .items_center()
        .when_some(icon, |row, icon| row.child(Icon::new(icon).small()))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .child(
            div()
                .flex_shrink_0()
                .max_w(rems(14.))
                .truncate()
                .text_xs()
                .opacity(0.65)
                .child(detail),
        )
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

/// Play/resume target for `game` (used by Quick Launcher `Enter`, Dialog
/// `Shift+Enter`, and explicit `play <game>` / `jogar <game>` queries).
fn play_or_resume_target(game: &GameInfo, kind: Option<StatusKind>) -> PaletteTarget {
    let id = game.id();
    match kind {
        Some(StatusKind::Playing) => PaletteTarget::BackToGame(id),
        Some(StatusKind::Launching) => PaletteTarget::CancelLaunch(id),
        Some(k) if k.is_busy() => PaletteTarget::Open(id),
        _ if game.is_installed => PaletteTarget::Play(id),
        _ => PaletteTarget::Open(id),
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
                .keywords([
                    t("shell.palette.back_to_game_keywords"),
                    english("shell.palette.back_to_game_keywords").into(),
                ])
                .icon(IconName::AppWindow)
                .kind(EntryKind::Live)
                .show_when_empty(true)
                .finish(),
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
                    .keywords([
                        t("shell.palette.cancel_launch_keywords"),
                        english("shell.palette.cancel_launch_keywords").into(),
                    ])
                    .icon(IconName::X)
                    .kind(EntryKind::Live)
                    .show_when_empty(true)
                    .finish(),
                PaletteTarget::CancelLaunch(id.clone()),
            ),
        );
    }
    rows
}

/// Which rows the palette lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Level {
    /// Commands, pages, settings, and games.
    Root,
    /// One game's actions.
    Game(GameId),
    /// Which games to pick a random one from.
    Random,
    /// A random pick from a pool, with "Pick another".
    Picked(irori_library::RandomPool, GameId),
}

/// The rows of the random pick levels: the pools, or what to do with the
/// game picked.
fn random_rows(level: &Level, cx: &mut App) -> Vec<Row> {
    match level {
        Level::Random => {
            let mut rows: Vec<Row> = irori_library::random_pools(cx)
                .into_iter()
                .map(|(pool, label)| {
                    Row::command(
                        Entry::new()
                            .label(label)
                            .keywords([
                                t("shell.palette.random_keywords"),
                                english("shell.palette.random_keywords").into(),
                            ])
                            .icon(IconName::Dices)
                            .finish(),
                        PaletteTarget::RandomFrom(pool),
                    )
                })
                .collect();
            rows.push(back_row());
            rows
        }
        Level::Picked(pool, id) => {
            let Some(game) = models(cx).library.read(cx).snapshot.get(id).cloned() else {
                return vec![back_row()];
            };
            let kind = models(cx).statuses.read(cx).kind(id);
            let title = game.title.as_str();
            let mut rows = Vec::new();
            match kind {
                Some(StatusKind::Playing) => rows.push(Row::command(
                    Entry::new()
                        .label(t_args("shell.palette.back_to_game", &[("title", title)]))
                        .icon(IconName::AppWindow)
                        .finish(),
                    PaletteTarget::BackToGame(id.clone()),
                )),
                Some(kind) if kind.is_busy() => {}
                _ if game.is_installed => rows.push(Row::command(
                    Entry::new()
                        .label(t_args("shell.palette.play", &[("title", title)]))
                        .icon(IconName::Play)
                        .finish(),
                    PaletteTarget::Play(id.clone()),
                )),
                _ => {}
            }
            rows.push(Row::command(
                Entry::new()
                    .label(t_args("shell.palette.show_game", &[("title", title)]))
                    .icon(IconName::Gamepad2)
                    .finish(),
                PaletteTarget::Open(id.clone()),
            ));
            rows.push(Row::command(
                Entry::new()
                    .label(t("shell.palette.pick_another"))
                    .english_label(english("shell.palette.pick_another"))
                    .icon(IconName::Dices)
                    .finish(),
                PaletteTarget::RandomFrom(pool.clone()),
            ));
            rows.push(Row::command(
                Entry::new()
                    .label(t("shell.palette.pick_other_games"))
                    .english_label(english("shell.palette.pick_other_games"))
                    .icon(IconName::ListFilter)
                    .finish(),
                PaletteTarget::Random,
            ));
            rows.push(back_row());
            rows.into_iter()
                .map(|row| Row {
                    about: Some(game.clone()),
                    ..row
                })
                .collect()
        }
        Level::Root | Level::Game(_) => Vec::new(),
    }
}

/// The row back to the palette's first level.
fn back_row() -> Row {
    Row::command(
        Entry::new()
            .label(t("shell.palette.back"))
            .english_label(english("shell.palette.back"))
            .icon(IconName::ArrowLeft)
            .finish(),
        PaletteTarget::Back,
    )
}

/// Whether `row` belongs to the `#` categories / saved-views scope.
fn is_category_or_view(row: &Row) -> bool {
    matches!(
        row.target,
        PaletteTarget::Category(_)
            | PaletteTarget::View(_)
            | PaletteTarget::NewSmartCategory
            | PaletteTarget::ManageCategories
            | PaletteTarget::ManageViews
    )
}

/// The facts the level of `game`'s actions is built from.
fn game_facts(game: &GameInfo, cx: &mut App) -> super::game::GameFacts {
    let id = game.id();
    let actions = irori_ui::user_actions::list(&id, cx);
    let syncable = irori_game::offers_sync(game, cx);
    let kind = models(cx).statuses.read(cx).kind(&id);
    let copies = irori_ui::copies::labelled(&id, cx)
        .into_iter()
        .filter(|(copy, _)| copy.id() != id)
        .filter_map(|(copy, label)| {
            let busy = models(cx)
                .statuses
                .read(cx)
                .kind(&copy.id())
                .is_some_and(irori_core::StatusKind::is_busy);
            let action = irori_ui::copies::copy_action(&copy, busy)?;
            Some(super::game::CopyRow {
                id: copy.id(),
                label: irori_ui::copies::action_label(action, &label),
                action,
            })
        })
        .collect();
    let library = models(cx).library.read(cx);
    super::game::GameFacts {
        kind,
        status: library.user.play_status(&id.key()),
        favourite: library.is_favourite(&id),
        hidden: library.is_hidden(&id),
        syncable,
        actions,
        copies,
    }
}

/// The palette: a unified grouped search over pages, settings, commands, and
/// games, with a live preview pane.
pub(crate) struct PaletteView {
    state: Entity<CommandState>,
    preview: Entity<PreviewPane>,
    images: Entity<LruImageCache>,
    host: Host,
    level: Level,
    /// Pre-folded pages, settings, and commands built when the palette opened.
    commands: Vec<Row>,
    /// The running and starting games the first rows are about.
    live: Vec<(GameId, StatusKind)>,
    /// The library revision the game rows were found in.
    revision: u64,
    query: String,
    /// The grouped rows the list holds now, in order.
    shown: Rc<Vec<ShownGroup>>,
    _subscriptions: Vec<Subscription>,
}

impl PaletteView {
    pub(crate) fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let mut subscriptions = vec![
            cx.observe(&m.statuses, |this, _, cx| {
                let live = live_games(cx);
                if live != this.live {
                    this.live = live;
                    this.rebuild(cx);
                }
            }),
            // A game's actions follow its favourite, hidden and status marks.
            cx.observe(&m.library, |this, _, cx| {
                if this.level != Level::Root {
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
        if host == Host::Window {
            // Only once it had the focus: some desktops report the new
            // window inactive before they hand it the keyboard.
            let mut had_focus = false;
            subscriptions.push(cx.observe_window_activation(window, move |_, window, _| {
                if window.is_window_active() {
                    had_focus = true;
                } else if had_focus {
                    window.remove_window();
                }
            }));
        }
        let mut view = Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            preview: {
                let weak = cx.weak_entity();
                let show_actions = move |id: &GameId, window: &mut Window, cx: &mut App| {
                    let id = id.clone();
                    let _ = weak.update(cx, |v, cx| v.enter_game(id, window, cx));
                };
                cx.new(|cx| PreviewPane::new(host, Rc::new(show_actions), cx))
            },
            images: LruImageCache::new(MAX_GAMES * 2, cx),
            host,
            level: Level::Root,
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

    /// List `id`'s actions.
    fn enter_game(&mut self, id: GameId, window: &mut Window, cx: &mut Context<Self>) {
        self.set_level(Level::Game(id), window, cx);
    }

    /// Show `level` with an empty query.
    fn set_level(&mut self, level: Level, window: &mut Window, cx: &mut Context<Self>) {
        self.level = level;
        self.query.clear();
        self.rebuild(cx);
        // The field reports the cleared query back; after this update.
        let state = self.state.clone();
        window.defer(cx, move |window, cx| {
            state.update(cx, |s, cx| s.set_query("", window, cx));
            state.update(cx, |s, cx| s.focus(window, cx));
        });
    }

    /// Escape: back to the first level; there, with nothing typed, the
    /// launcher closes (the dialog closes on its own).
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.level != Level::Root {
            cx.stop_propagation();
            self.set_level(Level::Root, window, cx);
        } else if self.host == Host::Window && self.query.is_empty() {
            cx.stop_propagation();
            window.remove_window();
        }
    }

    /// Look up the row at `ix` inside `self.shown`.
    fn row_at(&self, ix: IndexPath) -> Option<&Row> {
        self.shown.get(ix.section)?.rows.get(ix.row)
    }

    /// The highlighted row.
    fn selected(&self, cx: &App) -> Option<&Row> {
        let ix = self.state.read(cx).selected_index()?;
        self.row_at(ix)
    }

    pub(super) fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.rebuild(cx);
        }
    }

    /// Build a game row for `g`.
    fn make_game_row(&self, g: Arc<GameInfo>, kind: Option<StatusKind>) -> Row {
        let id = g.id();
        let detail = format!("{} · {}", g.runner().short_name(), state_label(&g, kind));
        let target = match self.host {
            Host::Dialog => PaletteTarget::Open(id),
            Host::Window => play_or_resume_target(&g, kind),
        };
        Row {
            entry: Entry::new()
                .label(g.title.clone())
                .keywords([g.runner().short_name(), g.runner().store_name()])
                .icon(IconName::Gamepad2)
                .kind(EntryKind::Game),
            target,
            game: Some(g),
            about: None,
            detail: Some(detail.into()),
            setting_path: None,
            help: None,
        }
    }

    /// The top recently played games as rows.
    fn recent_game_rows(&mut self, cx: &App) -> Vec<Row> {
        let recent = crate::sidebar::recent_games(cx);
        self.revision = models(cx).library.read(cx).snapshot.revision;
        let statuses = models(cx).statuses.read(cx);
        recent
            .into_iter()
            .map(|g| {
                let kind = statuses.kind(&g.id());
                self.make_game_row(g, kind)
            })
            .collect()
    }

    /// Games matched and scored by `SearchIndex`.
    fn scored_games(&mut self, query: &str, cx: &App) -> Vec<(i64, Arc<GameInfo>)> {
        let library = models(cx).library.read(cx);
        self.revision = library.snapshot.revision;
        let (user, locked) = (&library.user, library.snapshot.hidden_locked);
        // Hidden games behind the PIN aren't found here either.
        let keep = |g: &GameInfo| !(locked && user.hidden.contains(&g.id().key()));
        rank_games_scored(
            &library.snapshot.games,
            library.snapshot.search(),
            query,
            keep,
            MAX_GAMES,
        )
    }

    /// Curated empty-query home groups, tailored to `Host::Dialog` vs
    /// `Host::Window` (Quick Launcher).
    fn empty_groups(&mut self, cx: &App) -> Vec<ShownGroup> {
        let mut groups = Vec::new();
        let mut push_group = |kind: GroupKind, rows: Vec<Row>| {
            if !rows.is_empty() {
                groups.push(ShownGroup {
                    #[cfg(test)]
                    kind,
                    heading: t(kind.heading_key()),
                    rows,
                });
            }
        };

        let mut live = lead_rows(&self.live, cx);
        live.extend(
            self.commands
                .iter()
                .filter(|r| r.entry.kind == EntryKind::Live && r.entry.show_when_empty)
                .cloned(),
        );
        push_group(GroupKind::Live, live);

        let recent = self.recent_game_rows(cx);
        match self.host {
            Host::Dialog => {
                let current_game: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| r.entry.kind == EntryKind::CurrentGame && r.entry.show_when_empty)
                    .cloned()
                    .collect();
                push_group(GroupKind::CurrentGame, current_game);
                push_group(GroupKind::Recent, recent);
                let pages: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| r.entry.kind == EntryKind::Page && r.entry.show_when_empty)
                    .cloned()
                    .collect();
                push_group(GroupKind::Pages, pages);
                let quick: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| r.entry.kind == EntryKind::Command && r.entry.show_when_empty)
                    .cloned()
                    .collect();
                push_group(GroupKind::QuickActions, quick);
            }
            Host::Window => {
                push_group(GroupKind::Recent, recent);
                let pages: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| {
                        r.entry.kind == EntryKind::Page
                            && r.entry.show_when_empty
                            && r.entry.quick_launch
                    })
                    .cloned()
                    .collect();
                push_group(GroupKind::Pages, pages);
            }
        }
        groups
    }

    /// Curated view when the user typed only a scope prefix (`,`, `>`, `#`).
    fn scoped_empty_groups(&self, scope: ScopeFilter) -> Vec<ShownGroup> {
        let mut groups = Vec::new();
        let allowed = |r: &Row| self.host == Host::Dialog || r.entry.quick_launch;
        match scope {
            ScopeFilter::Settings => {
                let pages: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| allowed(r) && r.entry.kind == EntryKind::Page)
                    .take(16)
                    .cloned()
                    .collect();
                if !pages.is_empty() {
                    groups.push(ShownGroup {
                        #[cfg(test)]
                        kind: GroupKind::Pages,
                        heading: t(GroupKind::Pages.heading_key()),
                        rows: pages,
                    });
                }
                let settings: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| allowed(r) && r.entry.kind == EntryKind::Setting)
                    .take(15)
                    .cloned()
                    .collect();
                if !settings.is_empty() {
                    groups.push(ShownGroup {
                        #[cfg(test)]
                        kind: GroupKind::Settings,
                        heading: t(GroupKind::Settings.heading_key()),
                        rows: settings,
                    });
                }
            }
            ScopeFilter::Commands => {
                let cmds: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| {
                        allowed(r)
                            && matches!(r.entry.kind, EntryKind::CurrentGame | EntryKind::Command)
                    })
                    .take(25)
                    .cloned()
                    .collect();
                if !cmds.is_empty() {
                    groups.push(ShownGroup {
                        #[cfg(test)]
                        kind: GroupKind::Commands,
                        heading: t(GroupKind::Commands.heading_key()),
                        rows: cmds,
                    });
                }
            }
            ScopeFilter::Categories => {
                let cats: Vec<Row> = self
                    .commands
                    .iter()
                    .filter(|r| allowed(r) && is_category_or_view(r))
                    .take(25)
                    .cloned()
                    .collect();
                if !cats.is_empty() {
                    groups.push(ShownGroup {
                        #[cfg(test)]
                        kind: GroupKind::Commands,
                        heading: t(GroupKind::Commands.heading_key()),
                        rows: cats,
                    });
                }
            }
            ScopeFilter::All | ScopeFilter::PlayIntent => {}
        }
        groups
    }

    /// Find the grouped rows for the query again. The list highlights its first
    /// row after a change, so the preview follows it.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let raw_query = self.query.trim().to_owned();
        if let Level::Game(id) = self.level.clone() {
            let game = models(cx).library.read(cx).snapshot.get(&id).cloned();
            let Some(game) = game else {
                // The game left the library meanwhile.
                self.level = Level::Root;
                return self.rebuild(cx);
            };
            let facts = game_facts(&game, cx);
            let rows: Vec<Row> = super::game::level_rows(&game, &facts)
                .into_iter()
                .filter(|(entry, _)| entry.matches_text(&raw_query))
                .map(|(entry, target)| Row {
                    about: Some(game.clone()),
                    ..Row::command(entry, target)
                })
                .collect();
            return self.show_flat(rows, cx);
        }
        if matches!(self.level, Level::Random | Level::Picked(..)) {
            let rows = random_rows(&self.level, cx)
                .into_iter()
                .filter(|r| r.entry.matches_text(&raw_query))
                .collect();
            return self.show_flat(rows, cx);
        }

        let parsed = parse_input(&raw_query);
        if parsed.text.is_empty() {
            let groups = if parsed.scope == ScopeFilter::All {
                self.empty_groups(cx)
            } else {
                self.scoped_empty_groups(parsed.scope)
            };
            return self.show_groups(groups, cx);
        }

        let Some(query) = Query::new(parsed.text) else {
            return self.show_groups(Vec::new(), cx);
        };
        let full_query = Query::new(parsed.full);

        let include_games = matches!(parsed.scope, ScopeFilter::All | ScopeFilter::PlayIntent);
        let scored_games = if include_games {
            self.scored_games(parsed.text, cx)
        } else {
            Vec::new()
        };

        let mut live_rows: Vec<(i64, Row)> = Vec::new();
        let mut current_game_rows: Vec<(i64, Row)> = Vec::new();
        let mut page_rows: Vec<(i64, Row)> = Vec::new();
        let mut setting_rows: Vec<(i64, Row)> = Vec::new();
        let mut game_rows: Vec<(i64, Row)> = Vec::new();
        let mut command_rows: Vec<(i64, Row)> = Vec::new();

        // Explicit `play <game>` / `jogar <game>` puts direct Play rows first.
        if parsed.scope == ScopeFilter::PlayIntent {
            let statuses = models(cx).statuses.read(cx);
            for (score, g) in scored_games.iter().filter(|(_, g)| g.is_installed).take(4) {
                let kind = statuses.kind(&g.id());
                let target = play_or_resume_target(g, kind);
                let (label, icon) = match &target {
                    PaletteTarget::BackToGame(_) => (
                        t_args("shell.palette.back_to_game", &[("title", &g.title)]),
                        IconName::AppWindow,
                    ),
                    PaletteTarget::CancelLaunch(_) => (
                        t_args("shell.palette.cancel_launch", &[("title", &g.title)]),
                        IconName::X,
                    ),
                    _ => (
                        t_args("shell.palette.play", &[("title", &g.title)]),
                        IconName::Play,
                    ),
                };
                live_rows.push((
                    score + 1000,
                    Row {
                        about: Some(g.clone()),
                        ..Row::command(
                            Entry::new()
                                .label(label)
                                .icon(icon)
                                .kind(EntryKind::Live)
                                .finish(),
                            target,
                        )
                    },
                ));
            }
        }

        // Running/launching lead rows match either directly or via their game.
        if parsed.scope == ScopeFilter::All {
            for row in lead_rows(&self.live, cx) {
                let matched_game = match &row.target {
                    PaletteTarget::BackToGame(id) | PaletteTarget::CancelLaunch(id) => scored_games
                        .iter()
                        .find(|(_, g)| g.id() == *id)
                        .map(|(s, _)| *s),
                    _ => None,
                };
                if let Some(score) = row.entry.score(&query).or(matched_game) {
                    live_rows.push((score, row));
                }
            }
        }

        {
            let statuses = models(cx).statuses.read(cx);
            for (score, g) in &scored_games {
                let kind = statuses.kind(&g.id());
                game_rows.push((*score, self.make_game_row(g.clone(), kind)));
            }
        }

        // For `PlayIntent`, score commands against the full string so queries
        // like `"play status"` still match `"Group the library by play status"`.
        let cmd_query = if parsed.scope == ScopeFilter::PlayIntent {
            full_query.as_ref().unwrap_or(&query)
        } else {
            &query
        };

        for row in &self.commands {
            if self.host == Host::Window && !row.entry.quick_launch {
                continue;
            }
            let in_scope = match parsed.scope {
                ScopeFilter::All | ScopeFilter::PlayIntent => true,
                ScopeFilter::Settings => {
                    matches!(row.entry.kind, EntryKind::Page | EntryKind::Setting)
                }
                ScopeFilter::Commands => {
                    matches!(row.entry.kind, EntryKind::Command | EntryKind::CurrentGame)
                }
                ScopeFilter::Categories => is_category_or_view(row),
            };
            if !in_scope {
                continue;
            }
            let Some(score) = row.entry.score(cmd_query) else {
                continue;
            };
            match GroupKind::from_entry_kind(row.entry.kind) {
                GroupKind::Live => live_rows.push((score, row.clone())),
                GroupKind::CurrentGame => current_game_rows.push((score, row.clone())),
                GroupKind::Pages => page_rows.push((score, row.clone())),
                GroupKind::Settings => setting_rows.push((score, row.clone())),
                GroupKind::Games => game_rows.push((score, row.clone())),
                GroupKind::Commands | GroupKind::Recent | GroupKind::QuickActions => {
                    command_rows.push((score, row.clone()))
                }
            }
        }

        let groups = rank_groups(
            self.host,
            vec![
                ScoredGroup {
                    kind: GroupKind::Live,
                    heading: t(GroupKind::Live.heading_key()),
                    rows: live_rows,
                },
                ScoredGroup {
                    kind: GroupKind::CurrentGame,
                    heading: t(GroupKind::CurrentGame.heading_key()),
                    rows: current_game_rows,
                },
                ScoredGroup {
                    kind: GroupKind::Pages,
                    heading: t(GroupKind::Pages.heading_key()),
                    rows: page_rows,
                },
                ScoredGroup {
                    kind: GroupKind::Settings,
                    heading: t(GroupKind::Settings.heading_key()),
                    rows: setting_rows,
                },
                ScoredGroup {
                    kind: GroupKind::Games,
                    heading: t(GroupKind::Games.heading_key()),
                    rows: game_rows,
                },
                ScoredGroup {
                    kind: GroupKind::Commands,
                    heading: t(GroupKind::Commands.heading_key()),
                    rows: command_rows,
                },
            ],
        );
        self.show_groups(groups, cx);
    }

    fn show_flat(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        let groups = if rows.is_empty() {
            Vec::new()
        } else {
            vec![ShownGroup {
                #[cfg(test)]
                kind: GroupKind::Commands,
                heading: SharedString::default(),
                rows,
            }]
        };
        self.show_groups(groups, cx);
    }

    fn show_groups(&mut self, groups: Vec<ShownGroup>, cx: &mut Context<Self>) {
        let first = groups.first().and_then(|g| g.rows.first()).cloned();
        self.shown = Rc::new(groups);
        self.preview.update(cx, |p, cx| p.show(first, cx));
        cx.notify();
    }

    /// All rows currently displayed across every group, flattened in order.
    #[cfg(test)]
    pub(super) fn shown_rows(&self) -> Rc<Vec<Row>> {
        Rc::new(
            self.shown
                .iter()
                .flat_map(|g| g.rows.iter().cloned())
                .collect(),
        )
    }

    /// The groups currently displayed, in order.
    #[cfg(test)]
    pub(super) fn shown_groups(&self) -> Rc<Vec<ShownGroup>> {
        self.shown.clone()
    }

    /// The level the palette lists.
    #[cfg(test)]
    pub(super) fn level(&self) -> &Level {
        &self.level
    }

    /// Run the confirmed row; the palette's own rows change its level.
    pub(super) fn confirm(
        view: &Entity<Self>,
        host: Host,
        row: Row,
        window: &mut Window,
        cx: &mut App,
    ) {
        match row.target {
            PaletteTarget::GameActions(id) => {
                return view.update(cx, |v, cx| v.enter_game(id, window, cx));
            }
            PaletteTarget::Back => {
                return view.update(cx, |v, cx| v.set_level(Level::Root, window, cx));
            }
            PaletteTarget::Random => {
                return view.update(cx, |v, cx| v.set_level(Level::Random, window, cx));
            }
            PaletteTarget::RandomFrom(pool) => {
                match irori_library::pick_random_from(&pool, cx) {
                    Some(id) => {
                        view.update(cx, |v, cx| v.set_level(Level::Picked(pool, id), window, cx))
                    }
                    None => window
                        .push_notification(Notification::info(t("shell.palette.random_empty")), cx),
                }
                return;
            }
            _ => {}
        }
        match host {
            Host::Dialog => {
                window.close_dialog(cx);
                run_target(row.target, window, cx);
            }
            Host::Window => crate::quick_launch::run(row.target, cx),
        }
    }
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let host = self.host;
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
                    (v.row_at(ix).cloned(), v.preview.clone())
                };
                preview.update(cx, |p, cx| p.show(row, cx));
            }
        };
        let on_confirm = move |ix: IndexPath, window: &mut Window, cx: &mut App| {
            let Some(view) = weak.upgrade() else { return };
            let row = view.read(cx).row_at(ix).cloned();
            if let Some(row) = row {
                Self::confirm(&view, host, row, window, cx);
            }
        };
        let placeholder = match &self.level {
            Level::Root => t("shell.search.placeholder"),
            Level::Game(id) => {
                let title = irori_ui::backend(cx)
                    .game(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_default();
                t_args("shell.palette.actions_placeholder", &[("title", &title)])
            }
            Level::Random => t("shell.palette.random_placeholder"),
            Level::Picked(_, id) => {
                let title = irori_ui::backend(cx)
                    .game(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_default();
                t_args("shell.palette.picked_placeholder", &[("title", &title)])
            }
        };
        let mut list = Command::new(&self.state)
            .bordered(false)
            .rounded(cx.theme().radius_lg)
            .placeholder(placeholder)
            .max_h(px(440.))
            .filterable(false)
            .on_query(on_query)
            .on_select(on_select)
            .on_confirm(on_confirm);
        for (ix, group) in self.shown.iter().enumerate() {
            if ix > 0 {
                list = list.separator();
            }
            let mut cmd_group = CommandGroup::new();
            if !group.heading.is_empty() {
                cmd_group = cmd_group.label(group.heading.clone());
            }
            cmd_group = cmd_group.items(group.rows.iter().map(Row::item));
            list = list.group(cmd_group);
        }
        let launcher = self.host == Host::Window;
        let theme = cx.theme();
        let (surface, border) = (theme.popover, theme.border);
        h_flex()
            .image_cache(self.images.clone())
            .w_full()
            .items_stretch()
            .when(launcher, |root| {
                root.size_full()
                    .items_start()
                    .bg(surface)
                    .border_1()
                    .border_color(border)
            })
            // In the capture phase, before the search field takes the keys
            // for its cursor and the dialog takes Escape.
            .capture_action(cx.listener(|this, _: &MoveRight, window, cx| {
                if this.level == Level::Root
                    && let Some(id) = this
                        .selected(cx)
                        .and_then(|r| r.game.as_ref())
                        .map(|g| g.id())
                {
                    cx.stop_propagation();
                    this.enter_game(id, window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveLeft, window, cx| {
                if this.level != Level::Root && this.query.is_empty() {
                    cx.stop_propagation();
                    this.set_level(Level::Root, window, cx);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &InputEscape, window, cx| this.escape(window, cx)),
            )
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| this.escape(window, cx)))
            .capture_action(cx.listener(|this, enter: &InputEnter, window, cx| {
                if !enter.shift {
                    return;
                }
                let Some(game) = this.selected(cx).and_then(|r| r.game.clone()) else {
                    return;
                };
                match this.host {
                    // Shift+Enter in Quick Launcher opens the game in Irori.
                    Host::Window => {
                        cx.stop_propagation();
                        crate::quick_launch::close(cx);
                        crate::quick_launch::show_in_main(PaletteTarget::Open(game.id()), cx);
                    }
                    // Shift+Enter in the in-app palette plays/resumes directly.
                    Host::Dialog => {
                        let kind = models(cx).statuses.read(cx).kind(&game.id());
                        let target = play_or_resume_target(&game, kind);
                        cx.stop_propagation();
                        window.close_dialog(cx);
                        run_target(target, window, cx);
                    }
                }
            }))
            .child(div().flex_1().min_w_0().child(list))
            .when(is_wide(window), |row| row.child(self.preview.clone()))
    }
}
