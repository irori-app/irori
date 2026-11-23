//! Copies of one game from several stores, which the library shows as one
//! entry (`irori_services::copies`): which copies a game has, and the menu
//! items that open, prefer and separate them. Each copy keeps its own
//! [`GameId`], so whatever a screen does with one is done with that copy.
//! The changes write the library file, so they run off the UI thread.
//!
//! Copies tell themselves apart ([`copy_label`]): by store, and when that
//! isn't enough, by their own title, a ROM's file name or the emulator that
//! plays it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{App, Global};
use irori_core::{GameId, GameInfo, Runner};

use crate::components::BadgeMark;
use crate::i18n::{t, t_args};
use crate::{Route, backend, models, navigate};

/// Every copy of `id`'s entry, the default one first; empty when it is the
/// only one.
pub fn of(id: &GameId, cx: &App) -> Vec<Arc<GameInfo>> {
    let library = models(cx).library.read(cx);
    let snapshot = &library.snapshot;
    snapshot
        .copies_of(id)
        .iter()
        .filter_map(|copy| snapshot.get(copy).cloned())
        .collect()
}

/// What the badges on `game`'s cover name: each store its entry has a copy
/// on, the default copy's first, with a ROM named by its emulator (else its
/// system) instead of "Emulated".
pub fn badge_marks(game: &Arc<GameInfo>, cx: &App) -> Vec<BadgeMark> {
    let id = game.id();
    let stores = stores(&id, cx);
    if !stores.contains(&Runner::Emulation) {
        return stores.into_iter().map(BadgeMark::Store).collect();
    }
    let copies = of(&id, cx);
    let rom = copies
        .iter()
        .find(|c| c.runner() == Runner::Emulation)
        .unwrap_or(game);
    let emulator = engine_name(rom, cx).or_else(|| {
        let system = irori_engines::systems::get(rom.system.as_deref()?)?;
        Some(system.short.clone())
    });
    stores
        .into_iter()
        .map(|runner| match (&emulator, runner) {
            (Some(name), Runner::Emulation) => BadgeMark::Emulator(name.clone().into()),
            _ => BadgeMark::Store(runner),
        })
        .collect()
}

/// The stores `id`'s entry has a copy on, the default copy's first; just
/// `id`'s own store for a game with one copy.
pub fn stores(id: &GameId, cx: &App) -> Vec<Runner> {
    stores_of(id, models(cx).library.read(cx).snapshot.copies_of(id))
}

/// [`stores`] from the entry's copies, each store once.
fn stores_of(id: &GameId, copies: &[GameId]) -> Vec<Runner> {
    let mut stores = Vec::with_capacity(copies.len().max(1));
    for copy in copies {
        if !stores.contains(&copy.runner) {
            stores.push(copy.runner);
        }
    }
    if stores.is_empty() {
        stores.push(id.runner);
    }
    stores
}

/// What tells one copy of an entry from the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyLabel {
    /// The store, or for a ROM the emulator that plays it.
    pub source: String,
    /// The copy's own title when the copies' titles differ, else the
    /// file it is when the source alone doesn't tell it apart.
    pub detail: Option<String>,
}

impl CopyLabel {
    /// "Steam" or "Steam · Hades II", for a button or a menu.
    pub fn text(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{} · {detail}", self.source),
            None => self.source.clone(),
        }
    }
}

/// How `copy` is told apart from the other copies in `group` (`group`
/// holds `copy` too). `engine` names the emulator playing a ROM. The store
/// comes first; the copy's own title follows when any copy's folded title
/// differs from it; a ROM, or a copy whose store another copy shares, adds
/// its file name instead.
pub fn copy_label(copy: &GameInfo, group: &[Arc<GameInfo>], engine: Option<&str>) -> CopyLabel {
    let rom = copy.runner() == Runner::Emulation;
    let source = match engine.filter(|_| rom) {
        Some(engine) => engine.to_string(),
        None => copy.runner().store_name().to_string(),
    };
    let own = irori_services::search::fold(&copy.title);
    let titles_differ = group
        .iter()
        .any(|other| irori_services::search::fold(&other.title) != own);
    if titles_differ {
        return CopyLabel {
            source,
            detail: Some(copy.title.clone()),
        };
    }
    let shared_store = group
        .iter()
        .filter(|other| other.runner() == copy.runner())
        .count()
        > 1;
    let detail = (rom || shared_store).then(|| file_name(copy)).flatten();
    CopyLabel { source, detail }
}

/// The file a copy is: a ROM's file, else its install folder's name.
fn file_name(copy: &GameInfo) -> Option<String> {
    let install = copy.install.as_ref()?;
    let path = if copy.runner() == Runner::Emulation && !install.executable.is_empty() {
        &install.executable
    } else {
        &install.install_path
    };
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// The emulator that plays a ROM, by name ("RetroArch · Snes9x"): the
/// game's own choice when its settings were read, else the system's default
/// the user set. `None` for other games and when neither says, so a label
/// never waits on the disk.
pub fn engine_name(copy: &GameInfo, cx: &App) -> Option<String> {
    if copy.runner() != Runner::Emulation {
        return None;
    }
    let b = backend(cx);
    let app = models(cx).settings.read(cx).app.clone();
    let chosen = b
        .settings
        .cached_overrides(&copy.id())
        .map(|o| o.apply(&app.game).engine)
        .filter(|e| !e.is_empty())
        .or_else(|| {
            let system = copy.system.as_deref()?;
            app.system_engines.get(system).cloned()
        })?;
    let choice = irori_engines::EngineRef::parse(&chosen)?;
    let manifest = irori_engines::engine(&choice.engine)?;
    Some(
        match choice.core.as_deref().and_then(|core| manifest.core(core)) {
            Some(core) => format!("{} · {}", manifest.name, core.name),
            None => manifest.name.clone(),
        },
    )
}

/// Every copy of `id`'s entry with its label, the default one first; empty
/// when it is the only one. The ROM copies' settings are read off the UI
/// thread once, so their emulators can be named.
pub fn labelled(id: &GameId, cx: &mut App) -> Vec<(Arc<GameInfo>, CopyLabel)> {
    let copies = of(id, cx);
    if copies.len() < 2 {
        return Vec::new();
    }
    warm(&copies, cx);
    copies
        .iter()
        .map(|copy| {
            let engine = engine_name(copy, cx);
            let label = copy_label(copy, &copies, engine.as_deref());
            (copy.clone(), label)
        })
        .collect()
}

/// The ROM copies whose settings were asked for this run.
#[derive(Default)]
struct Warmed(HashSet<GameId>);

impl Global for Warmed {}

/// Read the settings of `copies`' ROMs on the backend, then redraw, so
/// [`engine_name`] knows each one's own emulator.
fn warm(copies: &[Arc<GameInfo>], cx: &mut App) {
    let b = backend(cx);
    let asked = cx.default_global::<Warmed>();
    let ids: Vec<GameId> = copies
        .iter()
        .filter(|c| c.runner() == Runner::Emulation)
        .map(|c| c.id())
        .filter(|id| b.settings.cached_overrides(id).is_none() && asked.0.insert(id.clone()))
        .collect();
    if ids.is_empty() {
        return;
    }
    let task = crate::state::run_infallible(cx, async move {
        for id in &ids {
            b.settings.overrides(id);
        }
    });
    cx.spawn(async move |cx| {
        task.await;
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

/// "Open the Steam copy", for menus and tooltips.
pub fn open_label(store: &str) -> gpui_kit::SharedString {
    t_args("ui.copies.open", &[("store", store)])
}

/// The copies' ids, for the changes below.
fn ids(copies: &[Arc<GameInfo>]) -> Vec<GameId> {
    copies.iter().map(|g| g.id()).collect()
}

/// Make `ids` one library entry.
pub fn combine(ids: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.combine_copies(&ids) }).detach();
}

/// Take `id` out of its entry.
pub fn separate(id: GameId, group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.separate_copy(&id, &group) }).detach();
}

/// Show every copy of `group` on its own.
pub fn separate_all(group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.separate_copies(&group) }).detach();
}

/// Make `id` the copy its entry plays and installs.
pub fn make_default(id: GameId, group: Vec<GameId>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move { b.userdata.set_default_copy(&id, &group) })
        .detach();
}

/// What can be done with a copy right from a menu: play it, install it,
/// or nothing while it is busy (running, installing…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyAction {
    Play,
    Install,
}

/// The [`CopyAction`] `copy` offers while its status is `busy`.
pub fn copy_action(copy: &GameInfo, busy: bool) -> Option<CopyAction> {
    if busy || copy.is_dlc {
        return None;
    }
    if copy.is_installed {
        Some(CopyAction::Play)
    } else if copy.runner().manages_installs() || copy.runner() == Runner::Steam {
        // Steam installs its own games when asked.
        Some(CopyAction::Install)
    } else {
        None
    }
}

/// "Play with Steam" or "Install with GOG · Hades II", for `label`.
pub fn action_label(action: CopyAction, label: &CopyLabel) -> gpui_kit::SharedString {
    let key = match action {
        CopyAction::Play => "ui.copies.play",
        CopyAction::Install => "ui.copies.install",
    };
    t_args(key, &[("copy", &label.text())])
}

/// Play or install `id` as its page would.
pub fn run_action(action: CopyAction, id: GameId, window: &mut gpui_kit::Window, cx: &mut App) {
    match action {
        CopyAction::Play => crate::hooks::play_game(id, window, cx),
        CopyAction::Install => crate::hooks::install_game(id, window, cx),
    }
}

/// Add items for `id`'s copies to `menu`: play or install each other copy
/// and open it, make this one the default, separate it or all of them.
/// Nothing for a game with one copy.
pub fn menu_items(mut menu: PopupMenu, id: &GameId, cx: &App) -> PopupMenu {
    let copies = of(id, cx);
    if copies.len() < 2 {
        return menu;
    }
    let group = ids(&copies);
    let statuses = models(cx).statuses.read(cx);
    for copy in copies.iter().filter(|c| c.id() != *id) {
        let copy_id = copy.id();
        let engine = engine_name(copy, cx);
        let label = copy_label(copy, &copies, engine.as_deref());
        let busy = statuses
            .kind(&copy_id)
            .is_some_and(irori_core::StatusKind::is_busy);
        if let Some(action) = copy_action(copy, busy) {
            let run_id = copy_id.clone();
            menu = menu.item(
                PopupMenuItem::new(action_label(action, &label))
                    .icon(match action {
                        CopyAction::Play => IconName::Play,
                        CopyAction::Install => IconName::Download,
                    })
                    .on_click(move |_, window, cx| run_action(action, run_id.clone(), window, cx)),
            );
        }
        menu = menu.item(
            PopupMenuItem::new(open_label(&label.text()))
                .icon(IconName::Layers)
                .on_click(move |_, _, cx| navigate(Route::Game(copy_id.clone()), cx)),
        );
    }
    if group.first() != Some(id) {
        let (id, group) = (id.clone(), group.clone());
        menu = menu.item(
            PopupMenuItem::new(t("ui.copies.make_default"))
                .icon(IconName::Star)
                .on_click(move |_, _, cx| make_default(id.clone(), group.clone(), cx)),
        );
    }
    let (one, one_group) = (id.clone(), group.clone());
    menu.item(
        PopupMenuItem::new(t("ui.copies.separate"))
            .icon(IconName::Unlink)
            .on_click(move |_, _, cx| separate(one.clone(), one_group.clone(), cx)),
    )
    .item(
        PopupMenuItem::new(t("ui.copies.separate_all"))
            .icon(IconName::Ungroup)
            .on_click(move |_, _, cx| separate_all(group.clone(), cx)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::InstalledInfo;

    fn game(runner: Runner, app: &str, title: &str, file: Option<&str>) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            install: file.map(|f| InstalledInfo {
                install_path: "/roms/snes".into(),
                executable: format!("/roms/snes/{f}"),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    #[test]
    fn copies_with_one_title_are_told_apart_by_store() {
        let steam = game(Runner::Steam, "1", "Hades", None);
        let gog = game(Runner::Gog, "2", "HADES", None);
        let group = [steam.clone(), gog.clone()];
        assert_eq!(copy_label(&steam, &group, None).text(), "Steam");
        assert_eq!(copy_label(&gog, &group, None).text(), "GOG");
    }

    #[test]
    fn copies_with_different_titles_show_their_own() {
        let a = game(Runner::Steam, "1", "Hades", None);
        let b = game(Runner::Gog, "2", "Hades: Deluxe Edition", None);
        let group = [a.clone(), b.clone()];
        assert_eq!(copy_label(&a, &group, None).text(), "Steam · Hades");
        assert_eq!(
            copy_label(&b, &group, None).text(),
            "GOG · Hades: Deluxe Edition"
        );
    }

    #[test]
    fn roms_name_their_emulator_and_file() {
        let usa = game(
            Runner::Emulation,
            "a",
            "Chrono Trigger",
            Some("Chrono Trigger (USA).sfc"),
        );
        let eu = game(
            Runner::Emulation,
            "b",
            "Chrono Trigger",
            Some("Chrono Trigger (Europe).sfc"),
        );
        let group = [usa.clone(), eu.clone()];
        assert_eq!(
            copy_label(&usa, &group, Some("RetroArch · Snes9x")).text(),
            "RetroArch · Snes9x · Chrono Trigger (USA).sfc"
        );
        // Without a known emulator the store's name stands in.
        assert_eq!(
            copy_label(&eu, &group, None).text(),
            "Emulated · Chrono Trigger (Europe).sfc"
        );
        // A ROM beside a store copy still names its file; the store copy
        // needs nothing more.
        let steam = game(Runner::Steam, "1", "Chrono Trigger", None);
        let group = [steam.clone(), usa.clone()];
        assert_eq!(copy_label(&steam, &group, None).text(), "Steam");
        assert_eq!(
            copy_label(&usa, &group, None).detail.as_deref(),
            Some("Chrono Trigger (USA).sfc")
        );
        // The emulator is only ever a ROM's source.
        assert_eq!(
            copy_label(&steam, &group, Some("RetroArch")).source,
            "Steam"
        );
    }

    #[test]
    fn two_copies_on_one_store_name_their_folders() {
        let folder = |app: &str, path: &str| {
            Arc::new(GameInfo {
                runner: Some(Runner::Sideload),
                app_name: app.into(),
                title: "Doom".into(),
                install: Some(InstalledInfo {
                    install_path: path.into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
        };
        let (a, b) = (folder("a", "/games/Doom (GOG)"), folder("b", "/games/Doom"));
        let group = [a.clone(), b.clone()];
        assert_eq!(
            copy_label(&a, &group, None).text(),
            "Added manually · Doom (GOG)"
        );
        assert_eq!(copy_label(&b, &group, None).text(), "Added manually · Doom");
    }

    #[test]
    fn copies_offer_play_or_install_unless_busy() {
        let installed = Arc::new(GameInfo {
            runner: Some(Runner::Gog),
            is_installed: true,
            ..Default::default()
        });
        let missing = game(Runner::Legendary, "2", "Hades", None);
        let steam = game(Runner::Steam, "3", "Hades", None);
        let launcher = game(Runner::Launchers, "ea:4", "Hades", None);
        assert_eq!(copy_action(&installed, false), Some(CopyAction::Play));
        assert_eq!(copy_action(&installed, true), None);
        assert_eq!(copy_action(&missing, false), Some(CopyAction::Install));
        assert_eq!(copy_action(&steam, false), Some(CopyAction::Install));
        assert_eq!(copy_action(&launcher, false), None);
    }

    #[test]
    fn stores_name_each_copy_once_default_first() {
        let id = GameId::new(Runner::Gog, "a");
        assert_eq!(stores_of(&id, &[]), vec![Runner::Gog]);
        let copies = [
            GameId::new(Runner::Steam, "1"),
            GameId::new(Runner::Gog, "a"),
            GameId::new(Runner::Steam, "2"),
        ];
        assert_eq!(stores_of(&id, &copies), vec![Runner::Steam, Runner::Gog]);
    }
}
