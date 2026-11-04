//! Big Picture's derived data: Home's shelves, the Library tab's filters, the
//! rows of the game page, the quick menu, and install defaults. Pure functions
//! over snapshots so they are tested without a window.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use irori_core::{
    BIG_PICTURE_SCALES, GameId, GameInfo, InstallInfo, InstallPlatform, InstallRequest, Runner,
    StatusKind,
};
use irori_services::{LibrarySnapshot, PlayStatus, UserData};
use irori_ui::i18n::{t, t_args};

use crate::layout::HomeLayout;
use crate::text::store_name;
use crate::theme::ShelfKind;

/// A shelf of games on Home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKind {
    ContinuePlaying,
    Favourites,
    Installed,
    RecentlyAdded,
    PlanToPlay,
    /// Emulated games of one system (a systems catalog id).
    System(&'static str),
    /// Every game; only when no other shelf has any.
    All,
}

impl RowKind {
    /// Translated shelf heading.
    pub fn title(self) -> SharedString {
        match self {
            RowKind::ContinuePlaying => t("bigpicture.shelf.continue_playing"),
            RowKind::Installed => t("bigpicture.shelf.installed"),
            RowKind::Favourites => t("bigpicture.shelf.favourites"),
            RowKind::RecentlyAdded => t("bigpicture.shelf.recently_added"),
            RowKind::PlanToPlay => t("bigpicture.shelf.plan_to_play"),
            RowKind::All => t("bigpicture.shelf.all"),
            RowKind::System(id) => irori_engines::systems::get(id)
                .map_or_else(|| SharedString::new_static(id), |s| s.name.clone().into()),
        }
    }

    /// Stable element id prefix of the row.
    pub fn id(self) -> &'static str {
        match self {
            RowKind::ContinuePlaying => "continue",
            RowKind::Installed => "installed",
            RowKind::Favourites => "favourites",
            RowKind::RecentlyAdded => "recently-added",
            RowKind::PlanToPlay => "plan-to-play",
            RowKind::All => "all",
            RowKind::System(id) => id,
        }
    }
}

/// One shelf and its games, in display order.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    pub games: Vec<Arc<GameInfo>>,
}

fn parse_key(key: &str) -> Option<GameId> {
    let (runner, app) = key.split_once(':')?;
    Some(GameId::new(Runner::from_id(runner)?, app))
}

/// Games on the "Recently added" shelf.
const RECENTLY_ADDED: usize = 24;

/// Build Home's shelves of `shelves`, in that order (the systems shelf
/// becomes one per system with emulated games). Hidden games are left out
/// and empty shelves dropped; "All games" stands in when nothing else has a
/// game, so a new library still shows something to play.
pub fn build_rows(
    library: &LibrarySnapshot,
    user: &UserData,
    max_recent: usize,
    shelves: &[ShelfKind],
) -> Vec<Row> {
    let visible: Vec<&Arc<GameInfo>> = library
        .games
        .iter()
        .filter(|g| !user.hidden.contains(&g.id().key()))
        .collect();
    let by_key: HashMap<String, &Arc<GameInfo>> =
        visible.iter().map(|g| (g.id().key(), *g)).collect();
    let pick = |keep: &dyn Fn(&GameInfo) -> bool| -> Vec<Arc<GameInfo>> {
        visible
            .iter()
            .filter(|g| keep(g))
            .map(|g| (*g).clone())
            .collect()
    };

    let mut rows = Vec::new();
    for shelf in shelves {
        match shelf {
            ShelfKind::ContinuePlaying => rows.push(Row {
                kind: RowKind::ContinuePlaying,
                games: user
                    .recent
                    .iter()
                    .filter(|k| parse_key(k).is_some())
                    .filter_map(|k| by_key.get(k.as_str()).map(|g| (*g).clone()))
                    .take(max_recent.max(1))
                    .collect(),
            }),
            ShelfKind::Favourites => rows.push(Row {
                kind: RowKind::Favourites,
                games: pick(&|g| user.favourites.contains(&g.id().key())),
            }),
            ShelfKind::Installed => rows.push(Row {
                kind: RowKind::Installed,
                games: pick(&|g| g.is_installed),
            }),
            ShelfKind::RecentlyAdded => {
                let added = |g: &GameInfo| {
                    g.date_added
                        .clone()
                        .or_else(|| user.first_seen.get(&g.id().key()).cloned())
                };
                let mut games: Vec<Arc<GameInfo>> =
                    pick(&|g| added(g).is_some_and(|d| !d.is_empty()));
                games.sort_by(|a, b| {
                    added(b)
                        .cmp(&added(a))
                        .then_with(|| sort_key(a).cmp(&sort_key(b)))
                });
                games.truncate(RECENTLY_ADDED);
                rows.push(Row {
                    kind: RowKind::RecentlyAdded,
                    games,
                });
            }
            ShelfKind::PlanToPlay => rows.push(Row {
                kind: RowKind::PlanToPlay,
                games: pick(&|g| user.play_status(&g.id().key()) == Some(PlayStatus::PlanToPlay)),
            }),
            ShelfKind::Systems => {
                for system in irori_engines::systems::all() {
                    let id = system.id.as_str();
                    rows.push(Row {
                        kind: RowKind::System(id),
                        games: pick(&|g| g.system.as_deref() == Some(id)),
                    });
                }
            }
        }
    }
    rows.retain(|r| !r.games.is_empty());
    if rows.is_empty() && !visible.is_empty() {
        rows.push(Row {
            kind: RowKind::All,
            games: pick(&|_| true),
        });
    }
    rows
}

// ---- Library tab --------------------------------------------------------

/// Which games the Library tab lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Show {
    #[default]
    All,
    Favourites,
    Store(Runner),
    /// Games the user hid, so they can be found and shown again.
    Hidden,
}

impl Show {
    pub fn label(self) -> SharedString {
        match self {
            Show::All => t("bigpicture.show.all"),
            Show::Favourites => t("bigpicture.show.favourites"),
            Show::Store(runner) => store_name(runner),
            Show::Hidden => t("bigpicture.show.hidden"),
        }
    }
}

/// The Library tab's order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    #[default]
    Title,
    RecentlyPlayed,
    MostPlayed,
    RecentlyAdded,
}

impl Sort {
    pub const ALL: [Sort; 4] = [
        Sort::Title,
        Sort::RecentlyPlayed,
        Sort::MostPlayed,
        Sort::RecentlyAdded,
    ];

    pub fn label(self) -> SharedString {
        t(match self {
            Sort::Title => "bigpicture.sort.title",
            Sort::RecentlyPlayed => "bigpicture.sort.recent",
            Sort::MostPlayed => "bigpicture.sort.most_played",
            Sort::RecentlyAdded => "bigpicture.sort.added",
        })
    }
}

/// The Library tab's install filter. Play statuses join it once the library
/// records them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum StatusFilter {
    #[default]
    Any,
    Installed,
    NotInstalled,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 3] = [
        StatusFilter::Any,
        StatusFilter::Installed,
        StatusFilter::NotInstalled,
    ];

    pub fn label(self) -> SharedString {
        t(match self {
            StatusFilter::Any => "bigpicture.status_filter.any",
            StatusFilter::Installed => "bigpicture.status_filter.installed",
            StatusFilter::NotInstalled => "bigpicture.status_filter.not_installed",
        })
    }

    fn keeps(self, game: &GameInfo) -> bool {
        match self {
            StatusFilter::Any => true,
            StatusFilter::Installed => game.is_installed,
            StatusFilter::NotInstalled => !game.is_installed,
        }
    }
}

/// The Library tab's filter chips, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Show,
    Sort,
    Status,
    Search,
}

impl Chip {
    pub const ALL: [Chip; 4] = [Chip::Show, Chip::Sort, Chip::Status, Chip::Search];

    pub fn label(self) -> SharedString {
        t(match self {
            Chip::Show => "bigpicture.chip.show",
            Chip::Sort => "bigpicture.chip.sort",
            Chip::Status => "bigpicture.chip.status",
            Chip::Search => "bigpicture.chip.search",
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Chip::Show => "bp-chip-show",
            Chip::Sort => "bp-chip-sort",
            Chip::Status => "bp-chip-status",
            Chip::Search => "bp-chip-search",
        }
    }
}

/// Everything the Library tab filters and sorts by.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LibraryFilter {
    pub show: Show,
    pub sort: Sort,
    pub status: StatusFilter,
    pub query: String,
}

/// What the Show chip offers for this library: every game, favourites, each
/// store when there are several, and hidden games when there are any.
pub fn show_options(library: &LibrarySnapshot, user: &UserData) -> Vec<Show> {
    let mut out = vec![Show::All, Show::Favourites];
    let stores: Vec<Runner> = Runner::ALL
        .into_iter()
        .filter(|r| library.games.iter().any(|g| g.runner() == *r))
        .collect();
    if stores.len() > 1 {
        out.extend(stores.into_iter().map(Show::Store));
    }
    if library
        .games
        .iter()
        .any(|g| user.hidden.contains(&g.id().key()))
    {
        out.push(Show::Hidden);
    }
    out
}

/// The option after `current` in `options`, wrapping to the first.
pub fn cycle<T: PartialEq + Copy>(options: &[T], current: T) -> T {
    let next = options
        .iter()
        .position(|o| *o == current)
        .map_or(0, |i| (i + 1) % options.len().max(1));
    options.get(next).copied().unwrap_or(current)
}

/// The Library tab's games for `filter`. Hidden games appear only under
/// [`Show::Hidden`]. The search matches every word of the query anywhere in
/// the title, ignoring case.
pub fn library_games(
    library: &LibrarySnapshot,
    user: &UserData,
    filter: &LibraryFilter,
) -> Vec<Arc<GameInfo>> {
    let words: Vec<String> = filter
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut games: Vec<Arc<GameInfo>> = library
        .games
        .iter()
        .filter(|g| {
            let key = g.id().key();
            let hidden = user.hidden.contains(&key);
            let shown = match filter.show {
                Show::All => !hidden,
                Show::Favourites => !hidden && user.favourites.contains(&key),
                Show::Store(runner) => !hidden && g.runner() == runner,
                Show::Hidden => hidden,
            };
            shown && filter.status.keeps(g) && {
                let title = g.title.to_lowercase();
                words.iter().all(|w| title.contains(w.as_str()))
            }
        })
        .cloned()
        .collect();
    let played = |g: &GameInfo| user.playtime.get(&g.id().key());
    let by_title = |a: &Arc<GameInfo>, b: &Arc<GameInfo>| sort_key(a).cmp(&sort_key(b));
    match filter.sort {
        Sort::Title => games.sort_by(by_title),
        Sort::RecentlyPlayed => games.sort_by(|a, b| {
            let last = |g: &GameInfo| played(g).and_then(|p| p.last_played.clone());
            last(b).cmp(&last(a)).then_with(|| by_title(a, b))
        }),
        Sort::MostPlayed => games.sort_by_key(|g| {
            (
                Reverse(played(g).map_or(0, |p| p.total_minutes)),
                sort_key(g),
            )
        }),
        Sort::RecentlyAdded => games.sort_by(|a, b| {
            let added = |g: &GameInfo| {
                g.date_added
                    .clone()
                    .or_else(|| user.first_seen.get(&g.id().key()).cloned())
            };
            added(b).cmp(&added(a)).then_with(|| by_title(a, b))
        }),
    }
    games
}

fn sort_key(game: &GameInfo) -> String {
    if game.sort_title.is_empty() {
        game.title.to_lowercase()
    } else {
        game.sort_title.to_lowercase()
    }
}

// ---- game page ----------------------------------------------------------

/// What the game page needs to know about a game's current state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameState {
    pub installed: bool,
    pub installable: bool,
    pub playing: bool,
    /// Current operation, if any (installing, launching, verifying…).
    pub busy: Option<StatusKind>,
    /// Queued or downloading in the download queue.
    pub in_queue: bool,
    pub has_update: bool,
    pub favourite: bool,
    pub hidden: bool,
    /// Another launcher (Steam) installs and removes it.
    pub external: bool,
}

/// A command that runs through the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameAction {
    Play,
    Stop,
    Install,
    CancelDownload,
    Update,
    Uninstall,
}

impl GameAction {
    /// Translated label.
    pub fn label(self) -> SharedString {
        t(match self {
            GameAction::Play => "bigpicture.action.play",
            GameAction::Stop => "bigpicture.action.stop",
            GameAction::Install => "bigpicture.action.install",
            GameAction::CancelDownload => "bigpicture.action.cancel_download",
            GameAction::Update => "bigpicture.action.update",
            GameAction::Uninstall => "bigpicture.action.uninstall",
        })
    }

    pub fn icon(self) -> IconName {
        match self {
            GameAction::Play => IconName::Play,
            GameAction::Stop => IconName::CircleStop,
            GameAction::Install => IconName::Download,
            GameAction::CancelDownload => IconName::X,
            GameAction::Update => IconName::CircleArrowUp,
            GameAction::Uninstall => IconName::Trash,
        }
    }
}

/// The game's main command: Stop while it runs, else Play, Cancel download
/// while it downloads, else Install.
pub fn main_action(state: GameState) -> GameAction {
    if state.playing {
        GameAction::Stop
    } else if state.installed {
        GameAction::Play
    } else if state.in_queue {
        GameAction::CancelDownload
    } else {
        GameAction::Install
    }
}

/// A row of the game page's action list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRow {
    Action(GameAction),
    /// ‹ Windows › — the platform Install picks, for games with several.
    Platform,
    Favourite,
    Settings,
    Playtime,
    Hidden,
}

impl PageRow {
    /// Stable element id.
    pub fn id(self) -> &'static str {
        match self {
            PageRow::Action(GameAction::Play) => "bp-row-play",
            PageRow::Action(GameAction::Stop) => "bp-row-stop",
            PageRow::Action(GameAction::Install) => "bp-row-install",
            PageRow::Action(GameAction::CancelDownload) => "bp-row-cancel-download",
            PageRow::Action(GameAction::Update) => "bp-row-update",
            PageRow::Action(GameAction::Uninstall) => "bp-row-uninstall",
            PageRow::Platform => "bp-row-platform",
            PageRow::Favourite => "bp-row-favourite",
            PageRow::Settings => "bp-row-settings",
            PageRow::Playtime => "bp-row-playtime",
            PageRow::Hidden => "bp-row-hidden",
        }
    }

    /// Whether the row starts the "Manage" group.
    pub fn starts_manage(self) -> bool {
        matches!(
            self,
            PageRow::Action(GameAction::Uninstall) | PageRow::Hidden
        )
    }

    /// Whether Left and Right change the row's value.
    pub fn has_value(self) -> bool {
        matches!(
            self,
            PageRow::Platform | PageRow::Favourite | PageRow::Hidden
        )
    }
}

/// The game page's rows: the main command, Update and Cancel download when
/// they apply, the install platform, Favourite, Game settings and Playtime,
/// then the "Manage" group (Uninstall, Hidden). `platforms` is how many
/// platforms the game installs for.
pub fn page_rows(state: GameState, platforms: usize) -> Vec<PageRow> {
    let main = main_action(state);
    let mut out = vec![PageRow::Action(main)];
    if state.installed && !state.playing && state.has_update && !state.in_queue {
        out.push(PageRow::Action(GameAction::Update));
    }
    if state.in_queue && main != GameAction::CancelDownload {
        out.push(PageRow::Action(GameAction::CancelDownload));
    }
    if main == GameAction::Install && platforms > 1 {
        out.push(PageRow::Platform);
    }
    out.extend([PageRow::Favourite, PageRow::Settings, PageRow::Playtime]);
    if state.installed && !state.playing && !state.external {
        out.push(PageRow::Action(GameAction::Uninstall));
    }
    out.push(PageRow::Hidden);
    out
}

/// Whether `action` can run now, and if not, why (translated).
pub fn disabled_reason(action: GameAction, state: GameState) -> Option<SharedString> {
    let busy = state
        .busy
        .filter(|k| *k != StatusKind::Queued && *k != StatusKind::Playing);
    match action {
        GameAction::Play | GameAction::Update | GameAction::Uninstall if busy.is_some() => {
            Some(t("bigpicture.action.busy"))
        }
        GameAction::Install if !state.installable => Some(t("bigpicture.action.not_installable")),
        _ => None,
    }
}

// ---- covers -------------------------------------------------------------

/// What a click on a cover does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverClick {
    /// Move the focus there, as the controller would; the backdrop follows.
    Focus,
    /// Open the game's page.
    Open,
}

/// A click focuses a cover, as in the main library; a click on the cover
/// already focused, or the second click of a double click, opens its page.
/// `focused` is read when the click lands, not when the cover was drawn.
pub fn cover_click(focused: bool, click_count: usize) -> CoverClick {
    if focused || click_count >= 2 {
        CoverClick::Open
    } else {
        CoverClick::Focus
    }
}

/// The command a cover runs without opening the game's page (its button
/// under the mouse, and Alt): Play or Install when it can run now. Anything
/// else (Stop, a download, a busy game) is left to the page.
pub fn cover_action(state: GameState) -> Option<GameAction> {
    let main = main_action(state);
    (matches!(main, GameAction::Play | GameAction::Install)
        && disabled_reason(main, state).is_none())
    .then_some(main)
}

/// Platforms the game installs for, in the order the platform row cycles.
pub fn install_platforms(game: &GameInfo) -> Vec<InstallPlatform> {
    game.platforms
        .iter()
        .copied()
        .filter(|p| *p != InstallPlatform::Browser)
        .collect()
}

// ---- quick menu ---------------------------------------------------------

/// Entries of the quick menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    PauseDownloads,
    ResumeDownloads,
    /// Cycles Big Picture's interface size; carries the current percent.
    InterfaceSize(u32),
    /// Cycles Home's layout; carries the current one.
    Layout(HomeLayout),
    /// Opens the theme editor.
    Themes,
    ToggleFullscreen,
    /// Leave Big Picture and open the "Set up emulation" guide.
    SetUpEmulation,
    ExitBigPicture,
    QuitIrori,
}

impl MenuItem {
    /// Translated entry label.
    pub fn label(self) -> SharedString {
        if let MenuItem::InterfaceSize(percent) = self {
            return t_args(
                "bigpicture.menu.interface_size",
                &[("percent", percent.to_string().as_str())],
            );
        }
        if let MenuItem::Layout(layout) = self {
            return t_args(
                "bigpicture.menu.layout",
                &[("layout", layout.label().as_str())],
            );
        }
        t(match self {
            MenuItem::InterfaceSize(_) | MenuItem::Layout(_) => unreachable!("handled above"),
            MenuItem::Themes => "bigpicture.menu.themes",
            MenuItem::PauseDownloads => "bigpicture.menu.pause_downloads",
            MenuItem::ResumeDownloads => "bigpicture.menu.resume_downloads",
            MenuItem::ToggleFullscreen => "bigpicture.menu.toggle_fullscreen",
            MenuItem::SetUpEmulation => "bigpicture.menu.set_up_emulation",
            MenuItem::ExitBigPicture => "bigpicture.exit",
            MenuItem::QuitIrori => "bigpicture.menu.quit",
        })
    }

    pub fn icon(self) -> IconName {
        match self {
            MenuItem::PauseDownloads => IconName::CircleStop,
            MenuItem::ResumeDownloads => IconName::Play,
            MenuItem::InterfaceSize(_) => IconName::ZoomIn,
            MenuItem::Layout(_) => IconName::LayoutDashboard,
            MenuItem::Themes => IconName::Palette,
            MenuItem::ToggleFullscreen => IconName::Maximize,
            MenuItem::SetUpEmulation => IconName::Joystick,
            MenuItem::ExitBigPicture => IconName::LogOut,
            MenuItem::QuitIrori => IconName::Power,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            MenuItem::PauseDownloads => "bp-menu-pause",
            MenuItem::ResumeDownloads => "bp-menu-resume",
            MenuItem::InterfaceSize(_) => "bp-menu-size",
            MenuItem::Layout(_) => "bp-menu-layout",
            MenuItem::Themes => "bp-menu-themes",
            MenuItem::ToggleFullscreen => "bp-menu-fullscreen",
            MenuItem::SetUpEmulation => "bp-menu-emulation",
            MenuItem::ExitBigPicture => "bp-menu-exit",
            MenuItem::QuitIrori => "bp-menu-quit",
        }
    }
}

/// Quick menu entries given the download queue state, the interface size
/// in percent and Home's layout.
pub fn menu_items(
    has_downloads: bool,
    paused: bool,
    scale: u32,
    layout: HomeLayout,
) -> Vec<MenuItem> {
    let mut out = Vec::with_capacity(9);
    if has_downloads {
        out.push(if paused {
            MenuItem::ResumeDownloads
        } else {
            MenuItem::PauseDownloads
        });
    }
    out.extend([
        MenuItem::InterfaceSize(scale),
        MenuItem::Layout(layout),
        MenuItem::Themes,
        MenuItem::ToggleFullscreen,
        MenuItem::SetUpEmulation,
        MenuItem::ExitBigPicture,
        MenuItem::QuitIrori,
    ]);
    out
}

/// The interface size after `current` in [`BIG_PICTURE_SCALES`], wrapping
/// back to the smallest.
pub fn next_scale(current: u32) -> u32 {
    BIG_PICTURE_SCALES
        .iter()
        .copied()
        .find(|s| *s > current)
        .unwrap_or(BIG_PICTURE_SCALES[0])
}

/// Platform to install when the user picks no options (see
/// [`GameInfo::default_platform`]).
pub fn default_platform(game: &GameInfo) -> InstallPlatform {
    game.default_platform()
}

/// Install request with default options (see
/// [`InstallRequest::with_defaults`]).
pub fn default_install_request(
    info: &InstallInfo,
    install_dir: &Path,
    platform: InstallPlatform,
) -> InstallRequest {
    InstallRequest::with_defaults(info, install_dir, platform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{PlaytimeRecord, SdlOption};

    fn game(runner: Runner, app: &str, installed: bool) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                runner: Some(runner),
                app_name: app.into(),
                title: app.to_uppercase(),
                is_installed: installed,
                installable: true,
                ..Default::default()
            }
            .finalize(),
        )
    }

    fn kinds(rows: &[Row]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    fn titles(games: &[Arc<GameInfo>]) -> Vec<&str> {
        games.iter().map(|g| g.title.as_str()).collect()
    }

    #[test]
    fn rows_skip_hidden_and_empty_shelves() {
        let mut lib = LibrarySnapshot::default();
        lib.games = vec![
            game(Runner::Gog, "a", true),
            game(Runner::Gog, "b", false),
            game(Runner::Gog, "c", true),
        ];
        let mut user = UserData::default();
        user.hidden.insert("gog:c".into());
        user.recent = vec!["gog:c".into(), "gog:b".into(), "bogus".into()];
        let rows = build_rows(&lib, &user, 8, &ShelfKind::DEFAULT);
        assert_eq!(
            kinds(&rows),
            vec![RowKind::ContinuePlaying, RowKind::Installed]
        );
        assert_eq!(titles(&rows[0].games), vec!["B"]);
        assert_eq!(titles(&rows[1].games), vec!["A"]);
    }

    #[test]
    fn themes_choose_and_order_the_shelves() {
        let mut lib = LibrarySnapshot::default();
        let mut newer = (*game(Runner::Gog, "new", false)).clone();
        newer.date_added = Some("2026-05-01T00:00:00Z".into());
        lib.games = vec![
            game(Runner::Gog, "a", true),
            game(Runner::Gog, "old", false),
            Arc::new(newer),
            game(Runner::Gog, "never", false),
        ];
        let mut user = UserData::default();
        user.first_seen
            .insert("gog:old".into(), "2025-01-01T00:00:00Z".into());
        user.play_status.insert(
            "gog:old".into(),
            irori_services::PlayStatusRecord {
                status: PlayStatus::PlanToPlay,
                since: "2026-01-01T00:00:00Z".into(),
            },
        );
        let rows = build_rows(
            &lib,
            &user,
            8,
            &[
                ShelfKind::PlanToPlay,
                ShelfKind::RecentlyAdded,
                ShelfKind::Installed,
            ],
        );
        assert_eq!(
            kinds(&rows),
            vec![
                RowKind::PlanToPlay,
                RowKind::RecentlyAdded,
                RowKind::Installed
            ]
        );
        assert_eq!(titles(&rows[0].games), vec!["OLD"]);
        assert_eq!(titles(&rows[1].games), vec!["NEW", "OLD"]);
        // No shelves at all still shows every game.
        assert_eq!(kinds(&build_rows(&lib, &user, 8, &[])), vec![RowKind::All]);
    }

    #[test]
    fn all_games_stand_in_for_an_empty_home() {
        let mut lib = LibrarySnapshot::default();
        lib.games = vec![
            game(Runner::Gog, "a", false),
            game(Runner::Legendary, "b", false),
        ];
        let rows = build_rows(&lib, &UserData::default(), 8, &ShelfKind::DEFAULT);
        assert_eq!(kinds(&rows), vec![RowKind::All]);
        assert_eq!(rows[0].games.len(), 2);

        let mut user = UserData::default();
        user.favourites.insert("legendary:b".into());
        assert_eq!(
            kinds(&build_rows(&lib, &user, 8, &ShelfKind::DEFAULT)),
            vec![RowKind::Favourites]
        );
        assert!(build_rows(&LibrarySnapshot::default(), &user, 8, &ShelfKind::DEFAULT).is_empty());
    }

    #[test]
    fn library_filters_sort_and_search() {
        let mut lib = LibrarySnapshot::default();
        lib.games = vec![
            game(Runner::Gog, "charlie", true),
            game(Runner::Legendary, "alpha", false),
            game(Runner::Gog, "bravo two", true),
            game(Runner::Gog, "delta", false),
        ];
        let mut user = UserData::default();
        user.hidden.insert("gog:delta".into());
        user.favourites.insert("gog:charlie".into());
        user.playtime.insert(
            "gog:charlie".into(),
            PlaytimeRecord {
                total_minutes: 30,
                last_played: Some("2026-10-01T10:00:00Z".into()),
                ..Default::default()
            },
        );
        user.playtime.insert(
            "legendary:alpha".into(),
            PlaytimeRecord {
                total_minutes: 90,
                last_played: Some("2026-09-01T10:00:00Z".into()),
                ..Default::default()
            },
        );
        let mut filter = LibraryFilter::default();
        assert_eq!(
            titles(&library_games(&lib, &user, &filter)),
            vec!["ALPHA", "BRAVO TWO", "CHARLIE"]
        );
        filter.sort = Sort::RecentlyPlayed;
        assert_eq!(
            titles(&library_games(&lib, &user, &filter)),
            vec!["CHARLIE", "ALPHA", "BRAVO TWO"]
        );
        filter.sort = Sort::MostPlayed;
        assert_eq!(
            titles(&library_games(&lib, &user, &filter)),
            vec!["ALPHA", "CHARLIE", "BRAVO TWO"]
        );
        filter.status = StatusFilter::Installed;
        assert_eq!(
            titles(&library_games(&lib, &user, &filter)),
            vec!["CHARLIE", "BRAVO TWO"]
        );
        filter.query = "two  BRA".into();
        assert_eq!(
            titles(&library_games(&lib, &user, &filter)),
            vec!["BRAVO TWO"]
        );
        let hidden = LibraryFilter {
            show: Show::Hidden,
            ..Default::default()
        };
        assert_eq!(titles(&library_games(&lib, &user, &hidden)), vec!["DELTA"]);
        let favourites = LibraryFilter {
            show: Show::Favourites,
            ..Default::default()
        };
        assert_eq!(
            titles(&library_games(&lib, &user, &favourites)),
            vec!["CHARLIE"]
        );
        let epic = LibraryFilter {
            show: Show::Store(Runner::Legendary),
            ..Default::default()
        };
        assert_eq!(titles(&library_games(&lib, &user, &epic)), vec!["ALPHA"]);

        assert_eq!(
            show_options(&lib, &user),
            vec![
                Show::All,
                Show::Favourites,
                Show::Store(Runner::Legendary),
                Show::Store(Runner::Gog),
                Show::Hidden
            ]
        );
        assert_eq!(
            show_options(&lib, &UserData::default()).last(),
            Some(&Show::Store(Runner::Gog))
        );
        assert_eq!(cycle(&Sort::ALL, Sort::RecentlyAdded), Sort::Title);
        assert_eq!(cycle(&Sort::ALL, Sort::Title), Sort::RecentlyPlayed);
        assert_eq!(cycle(&[Show::All], Show::Hidden), Show::All);
    }

    #[test]
    fn page_rows_follow_state() {
        let installed = GameState {
            installed: true,
            installable: true,
            has_update: true,
            ..Default::default()
        };
        assert_eq!(
            page_rows(installed, 2),
            vec![
                PageRow::Action(GameAction::Play),
                PageRow::Action(GameAction::Update),
                PageRow::Favourite,
                PageRow::Settings,
                PageRow::Playtime,
                PageRow::Action(GameAction::Uninstall),
                PageRow::Hidden,
            ]
        );
        let playing = GameState {
            playing: true,
            ..installed
        };
        assert_eq!(page_rows(playing, 1)[0], PageRow::Action(GameAction::Stop));
        assert!(!page_rows(playing, 1).contains(&PageRow::Action(GameAction::Uninstall)));
        let not_installed = GameState {
            installable: true,
            ..Default::default()
        };
        assert_eq!(
            &page_rows(not_installed, 2)[..2],
            &[PageRow::Action(GameAction::Install), PageRow::Platform]
        );
        assert!(!page_rows(not_installed, 1).contains(&PageRow::Platform));
        let queued = GameState {
            in_queue: true,
            installable: true,
            busy: Some(StatusKind::Queued),
            ..Default::default()
        };
        assert_eq!(
            &page_rows(queued, 2)[..2],
            &[
                PageRow::Action(GameAction::CancelDownload),
                PageRow::Favourite
            ]
        );
        let updating = GameState {
            in_queue: true,
            ..installed
        };
        assert_eq!(
            &page_rows(updating, 1)[..2],
            &[
                PageRow::Action(GameAction::Play),
                PageRow::Action(GameAction::CancelDownload)
            ]
        );
        let steam = GameState {
            external: true,
            ..installed
        };
        assert!(!page_rows(steam, 1).contains(&PageRow::Action(GameAction::Uninstall)));

        let verifying = GameState {
            busy: Some(StatusKind::Repairing),
            ..installed
        };
        assert!(disabled_reason(GameAction::Play, verifying).is_some());
        assert!(disabled_reason(GameAction::Stop, verifying).is_none());
        assert!(disabled_reason(GameAction::Install, GameState::default()).is_some());
    }

    #[test]
    fn a_click_focuses_a_cover_and_a_second_opens_it() {
        assert_eq!(cover_click(false, 1), CoverClick::Focus);
        assert_eq!(cover_click(true, 1), CoverClick::Open);
        // A double click opens even when the first click's redraw hasn't
        // happened yet.
        assert_eq!(cover_click(false, 2), CoverClick::Open);
        assert_eq!(cover_click(true, 3), CoverClick::Open);
    }

    #[test]
    fn covers_offer_play_or_install_only_when_they_can_run() {
        let installed = GameState {
            installed: true,
            installable: true,
            ..Default::default()
        };
        assert_eq!(cover_action(installed), Some(GameAction::Play));
        let not_installed = GameState {
            installable: true,
            ..Default::default()
        };
        assert_eq!(cover_action(not_installed), Some(GameAction::Install));

        let playing = GameState {
            playing: true,
            busy: Some(StatusKind::Playing),
            ..installed
        };
        assert_eq!(cover_action(playing), None, "Stop stays on the page");
        let verifying = GameState {
            busy: Some(StatusKind::Repairing),
            ..installed
        };
        assert_eq!(cover_action(verifying), None);
        let downloading = GameState {
            in_queue: true,
            busy: Some(StatusKind::Installing),
            ..not_installed
        };
        assert_eq!(cover_action(downloading), None);
        assert_eq!(cover_action(GameState::default()), None, "not installable");
    }

    #[test]
    fn install_platforms_leave_out_the_browser() {
        let g = GameInfo {
            platforms: vec![
                InstallPlatform::Windows,
                InstallPlatform::Browser,
                InstallPlatform::Linux,
            ],
            ..Default::default()
        };
        assert_eq!(
            install_platforms(&g),
            vec![InstallPlatform::Windows, InstallPlatform::Linux]
        );
    }

    #[test]
    fn menu_offers_download_control_only_with_downloads() {
        assert_eq!(
            menu_items(false, false, 100, HomeLayout::Shelves)[0],
            MenuItem::InterfaceSize(100)
        );
        assert_eq!(
            menu_items(true, false, 100, HomeLayout::Shelves)[0],
            MenuItem::PauseDownloads
        );
        assert_eq!(
            menu_items(true, true, 150, HomeLayout::Shelves)[0],
            MenuItem::ResumeDownloads
        );
        let items = menu_items(true, true, 150, HomeLayout::Wall);
        assert_eq!(items[1], MenuItem::InterfaceSize(150));
        assert_eq!(items[2], MenuItem::Layout(HomeLayout::Wall));
        assert_eq!(items[3], MenuItem::Themes);
    }

    #[test]
    fn interface_size_cycles_through_the_offered_scales() {
        assert_eq!(next_scale(100), 125);
        assert_eq!(next_scale(175), 200);
        assert_eq!(next_scale(200), 100);
        assert_eq!(next_scale(130), 150);
        assert_eq!(next_scale(0), 100);
    }

    #[test]
    fn install_defaults() {
        let mut g = GameInfo {
            platforms: vec![InstallPlatform::Windows, InstallPlatform::Linux],
            ..Default::default()
        };
        assert_eq!(
            default_platform(&g),
            if cfg!(target_os = "linux") {
                InstallPlatform::Linux
            } else {
                InstallPlatform::Windows
            }
        );
        g.platforms = vec![InstallPlatform::Windows];
        assert_eq!(default_platform(&g), InstallPlatform::Windows);
        g.platforms.clear();
        g.is_linux_native = true;
        assert_eq!(
            default_platform(&g),
            if cfg!(target_os = "linux") {
                InstallPlatform::Linux
            } else {
                InstallPlatform::Windows
            }
        );

        let info = InstallInfo {
            sdl: vec![
                SdlOption {
                    tags: vec!["core".into()],
                    mandatory: true,
                    ..Default::default()
                },
                SdlOption {
                    tags: vec!["hd".into()],
                    default_selected: true,
                    ..Default::default()
                },
                SdlOption {
                    tags: vec!["extra".into()],
                    ..Default::default()
                },
            ],
            dependencies: vec!["vcredist".into()],
            ..Default::default()
        };
        let req = default_install_request(&info, Path::new("/games"), InstallPlatform::Linux);
        assert_eq!(req.sdl, vec!["core", "hd"]);
        assert_eq!(req.dependencies, vec!["vcredist"]);
        assert_eq!(req.path, Path::new("/games"));
        assert!(req.dlcs.is_empty());
    }
}
