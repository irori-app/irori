//! Application-level operations that need crates the UI layer does not depend
//! on (the concrete store crates, or another feature). The shell installs them
//! at startup; feature crates call them.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui_kit::{AnyView, App, Global, Task, Window};
use irori_core::{GameId, Runner};

/// What a Heroic import brought over, for the confirmation message.
#[derive(Debug, Clone, Default)]
pub struct ImportSummary {
    pub stores: Vec<String>,
    pub games_with_settings: usize,
    pub favourites: usize,
    pub categories: usize,
}

/// A Lutris game that was not imported, and why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LutrisSkip {
    pub title: String,
    /// A stable reason id (`notInstalled`, `unknownRunner`, `fileMissing`,
    /// …) that the summary translates.
    pub reason: String,
    /// The runner, core, file or error the reason is about.
    pub detail: Option<String>,
}

/// What a Lutris, Bottles, Faugus Launcher or Playnite import did, for its
/// summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LutrisSummary {
    /// The launcher imported from, when it isn't Lutris ("Bottles").
    pub launcher: Option<String>,
    /// Games in Lutris's library.
    pub found: usize,
    /// Games added to Irori (apps and ROM library entries).
    pub imported: usize,
    /// Games Irori already had through its own stores or Steam; their
    /// favourites, categories and playtime were brought over.
    pub matched: usize,
    /// Store games Irori doesn't have yet, with the store to log in to.
    pub store_games: Vec<(String, String)>,
    pub skipped: Vec<LutrisSkip>,
    /// Games whose Wine build Irori doesn't have, with the build's name;
    /// they use the default build.
    pub wine_missing: Vec<(String, String)>,
    pub favourites: usize,
    pub categories: usize,
    pub playtime_games: usize,
    /// Lutris files that could not be read.
    pub warnings: Vec<String>,
}

/// How the library shows a game without leaving the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamePanel {
    /// The whole game page beside the Focus view's column of covers. The
    /// library draws the game's backdrop behind both; the panel draws none.
    Focus,
    /// A summary of the game in the grid's side drawer.
    Drawer,
}

/// What a view shown inside another screen calls to close itself.
pub type CloseFn = Rc<dyn Fn(&mut Window, &mut App)>;

type ImportFn = Arc<dyn Fn(&mut App) -> Task<Result<ImportSummary>>>;
type GamePanelFn = Arc<dyn Fn(GameId, GamePanel, &mut Window, &mut App) -> AnyView>;
type GameSettingsFn = Arc<dyn Fn(GameId, CloseFn, &mut Window, &mut App) -> AnyView>;
type LutrisFn = Arc<dyn Fn(&mut App) -> Task<Result<LutrisSummary>>>;
type OpenFn = Arc<dyn Fn(&mut Window, &mut App)>;
type StorePageFn = Arc<dyn Fn(Runner, String, &mut Window, &mut App)>;
type UnlocksFn = Arc<dyn Fn(GameId, Vec<irori_core::Achievement>, &mut App) -> bool>;
type GameFn = Arc<dyn Fn(GameId, &mut Window, &mut App)>;
type GameViewFn = Arc<dyn Fn(GameId, &mut Window, &mut App) -> AnyView>;
type PathFn = Arc<dyn Fn(PathBuf, &mut Window, &mut App)>;
type ViewFn = Arc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// The library's suggestion to read the Steam library from the Steam client
/// (the Steam store and its dialog live outside the UI layer).
#[derive(Clone)]
pub struct SteamLibraryHint {
    /// Whether the suggestion would help now. Reads memory only.
    pub shown: Arc<dyn Fn(&App) -> bool>,
    /// Explain and offer to read the library.
    pub open: OpenFn,
    /// Put the suggestion away for good.
    pub dismiss: Arc<dyn Fn(&mut App)>,
}

#[derive(Clone, Default)]
pub struct Hooks {
    pub import_from_heroic: Option<ImportFn>,
    /// Open the "Set up emulation" guide (it lives in the management
    /// screens, which the library and Big Picture don't depend on).
    pub open_emulation_setup: Option<OpenFn>,
    pub import_from_lutris: Option<LutrisFn>,
    pub import_from_bottles: Option<LutrisFn>,
    pub import_from_faugus: Option<LutrisFn>,
    pub import_from_playnite: Option<LutrisFn>,
    /// Make a view of one game for the library's Focus view and drawer (the
    /// game page lives in the game feature, which the library doesn't
    /// depend on).
    pub game_panel: Option<GamePanelFn>,
    /// Make the settings of one game for a screen that shows them in place
    /// (Big Picture, which the settings feature doesn't depend on): its
    /// Back calls the given function instead of leaving the screen.
    pub game_settings: Option<GameSettingsFn>,
    /// Show a page of a store's website in the store window (it lives in
    /// the management screens).
    pub open_store_page: Option<StorePageFn>,
    pub steam_library_hint: Option<SteamLibraryHint>,
    /// Show what a running game just unlocked over it (the notice window
    /// lives in Big Picture, beside the in-game overlay). Returns false when
    /// it can't, so they become a notice instead.
    pub show_unlocks: Option<UnlocksFn>,
    /// Ask to uninstall a game (the confirmation lives in the game
    /// feature, which the settings don't depend on).
    pub uninstall: Option<GameFn>,
    /// Make the Setup card of one game for its page: what its settings
    /// change, by section (the settings feature draws it).
    pub game_setup: Option<GameViewFn>,
    /// Make what the empty library shows: the game sources found on this
    /// computer (the management screens draw it, from stores the library
    /// doesn't know).
    pub found_sources: Option<ViewFn>,
    /// Make the Accounts view shown inside Settings (lives in `irori-manage`,
    /// which `irori-settings` does not depend on).
    pub accounts_view: Option<ViewFn>,
    /// Make the Wine & Proton manager view shown inside Settings.
    pub wine_manager_view: Option<ViewFn>,
    /// Make the Emulators view shown inside Settings.
    pub emulators_view: Option<ViewFn>,
    /// Play one game (its copy) as the game page does: saves conflicts and
    /// missing files answered there (the game feature).
    pub play: Option<GameFn>,
    /// Install one game (its copy): the install sheet, or the store's own
    /// client for games Irori doesn't install (the game feature).
    pub install: Option<GameFn>,
    /// Open the "Add game" dialog pre-filled for a local file or folder
    /// (lives in the library feature, which the manage feature doesn't depend on).
    pub add_to_library: Option<PathFn>,
}

impl Global for Hooks {}

pub fn install(hooks: Hooks, cx: &mut App) {
    cx.set_global(hooks);
}

/// Import accounts, installs, settings and library data from Heroic.
pub fn import_from_heroic(cx: &mut App) -> Task<Result<ImportSummary>> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.import_from_heroic.clone());
    match hook {
        Some(f) => f(cx),
        None => Task::ready(Err(anyhow::anyhow!("import is not available"))),
    }
}

/// Open the "Set up emulation" guide; without the hook, the Emulators page.
pub fn open_emulation_setup(window: &mut Window, cx: &mut App) {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.open_emulation_setup.clone())
    {
        Some(open) => open(window, cx),
        None => crate::navigate(crate::Route::Emulators, cx),
    }
}

/// The game sources found on this computer, for the empty library; `None`
/// without the hook (tests), where the library says it is empty instead.
pub fn found_sources(window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.found_sources.clone())?;
    Some(hook(window, cx))
}

/// The Accounts view embedded inside Settings; `None` without the hook (tests).
pub fn accounts_view(window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.accounts_view.clone())?;
    Some(hook(window, cx))
}

/// The Wine & Proton manager view embedded inside Settings; `None` without the hook (tests).
pub fn wine_manager_view(window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.wine_manager_view.clone())?;
    Some(hook(window, cx))
}

/// The Emulators view embedded inside Settings; `None` without the hook (tests).
pub fn emulators_view(window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.emulators_view.clone())?;
    Some(hook(window, cx))
}

/// Import games and library data from Lutris.
pub fn import_from_lutris(cx: &mut App) -> Task<Result<LutrisSummary>> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.import_from_lutris.clone());
    match hook {
        Some(f) => f(cx),
        None => Task::ready(Err(anyhow::anyhow!("import is not available"))),
    }
}

/// Import the programs set up in Bottles.
pub fn import_from_bottles(cx: &mut App) -> Task<Result<LutrisSummary>> {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.import_from_bottles.clone())
    {
        Some(f) => f(cx),
        None => Task::ready(Err(anyhow::anyhow!("import is not available"))),
    }
}

/// Import the games set up in Faugus Launcher.
pub fn import_from_faugus(cx: &mut App) -> Task<Result<LutrisSummary>> {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.import_from_faugus.clone())
    {
        Some(f) => f(cx),
        None => Task::ready(Err(anyhow::anyhow!("import is not available"))),
    }
}

/// Import the games of Playnite (Windows).
pub fn import_from_playnite(cx: &mut App) -> Task<Result<LutrisSummary>> {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.import_from_playnite.clone())
    {
        Some(f) => f(cx),
        None => Task::ready(Err(anyhow::anyhow!("import is not available"))),
    }
}

/// Whether Bottles has a bottles folder on this computer (native or
/// Flatpak). A custom bottles folder is found by the import itself.
pub fn bottles_detected() -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    [
        data.join("bottles/bottles"),
        data.join("bottles/data.yml"),
        home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
    ]
    .iter()
    .any(|p| p.exists())
}

/// Whether Faugus Launcher has a game list on this computer.
pub fn faugus_detected() -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    [
        config.join("faugus-launcher/games.json"),
        home.join(".var/app/io.github.Faugus.faugus-launcher/config/faugus-launcher/games.json"),
    ]
    .iter()
    .any(|p| p.is_file())
}

/// A view of `id` shaped as `panel`, or `None` without the hook (tests).
pub fn game_panel(
    id: GameId,
    panel: GamePanel,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.game_panel.clone())?;
    Some(hook(id, panel, window, cx))
}

/// The Setup card of `id` for its game page; `None` without the hook.
pub fn game_setup(id: GameId, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.game_setup.clone())?;
    Some(hook(id, window, cx))
}

/// The settings of `id` for a screen that shows them in place, closed with
/// `on_close`; `None` without the hook (tests).
pub fn game_settings(
    id: GameId,
    on_close: CloseFn,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyView> {
    let hook = cx
        .try_global::<Hooks>()
        .and_then(|h| h.game_settings.clone())?;
    Some(hook(id, on_close, window, cx))
}

/// Show `unlocks` of the running game `id` over it; false without the hook
/// or when nothing can show over the game.
pub fn show_unlocks(id: GameId, unlocks: Vec<irori_core::Achievement>, cx: &mut App) -> bool {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.show_unlocks.clone())
    {
        Some(show) => show(id, unlocks, cx),
        None => false,
    }
}

/// Play `id`, as its page does; without the hook, open the page, where
/// Play is.
pub fn play_game(id: GameId, window: &mut Window, cx: &mut App) {
    match cx.try_global::<Hooks>().and_then(|h| h.play.clone()) {
        Some(play) => play(id, window, cx),
        None => crate::navigate(crate::Route::Game(id), cx),
    }
}

/// Install `id`, as its page does; without the hook, open the page, where
/// Install is.
pub fn install_game(id: GameId, window: &mut Window, cx: &mut App) {
    match cx.try_global::<Hooks>().and_then(|h| h.install.clone()) {
        Some(install) => install(id, window, cx),
        None => crate::navigate(crate::Route::Game(id), cx),
    }
}

/// Open the "Add game" dialog pre-filled with `path` (a downloaded file or folder).
pub fn add_to_library(path: PathBuf, window: &mut Window, cx: &mut App) {
    if let Some(open) = cx
        .try_global::<Hooks>()
        .and_then(|h| h.add_to_library.clone())
    {
        open(path, window, cx);
    }
}

/// Ask to uninstall `id`, with the game feature's confirmation; without the
/// hook, open the game's page, where Uninstall is.
pub fn uninstall(id: GameId, window: &mut Window, cx: &mut App) {
    match cx.try_global::<Hooks>().and_then(|h| h.uninstall.clone()) {
        Some(open) => open(id, window, cx),
        None => crate::navigate(crate::Route::Game(id), cx),
    }
}

/// Show `url`, a page of `runner`'s website, in the store window; without
/// the hook, in the browser.
pub fn open_store_page(runner: Runner, url: String, window: &mut Window, cx: &mut App) {
    match cx
        .try_global::<Hooks>()
        .and_then(|h| h.open_store_page.clone())
    {
        Some(open) => open(runner, url, window, cx),
        None => {
            if let Some(url) = crate::web_url(&url) {
                cx.open_url(&url);
            }
        }
    }
}

/// Whether the library should suggest reading the Steam library from the
/// Steam client; false without the hook.
pub fn steam_library_hint_shown(cx: &App) -> bool {
    cx.try_global::<Hooks>()
        .and_then(|h| h.steam_library_hint.as_ref())
        .is_some_and(|hint| (hint.shown)(cx))
}

/// Explain and offer reading the Steam library from the Steam client.
pub fn open_steam_library_hint(window: &mut Window, cx: &mut App) {
    if let Some(hint) = cx
        .try_global::<Hooks>()
        .and_then(|h| h.steam_library_hint.clone())
    {
        (hint.open)(window, cx);
    }
}

/// Stop suggesting to read the Steam library from the Steam client.
pub fn dismiss_steam_library_hint(cx: &mut App) {
    if let Some(hint) = cx
        .try_global::<Hooks>()
        .and_then(|h| h.steam_library_hint.clone())
    {
        (hint.dismiss)(cx);
    }
}

/// Whether a Lutris library exists on this computer (native or Flatpak).
pub fn lutris_detected() -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    [
        home.join(".local/share/lutris/pga.db"),
        home.join(".var/app/net.lutris.Lutris/data/lutris/pga.db"),
    ]
    .iter()
    .any(|p| p.is_file())
        || std::env::var_os("XDG_DATA_HOME")
            .is_some_and(|d| std::path::Path::new(&d).join("lutris/pga.db").is_file())
}

/// Whether a Heroic installation with importable data exists.
pub fn heroic_detected() -> bool {
    irori_core::paths::heroic_config_dir().is_some()
}
