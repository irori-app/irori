//! The Wine prefixes on this computer and the installed games using each,
//! for the Wine manager's Prefixes tab.
//!
//! A prefix is listed when it sits in the prefix folder, is the game
//! defaults' prefix, a third-party launcher's, or the prefix an installed
//! game runs in. Only installed games count as using one: a game that was
//! uninstalled says only that the prefix was its own, so the user can tell a
//! prefix worth keeping from one that is left over. Games that run natively,
//! in a browser, or through Steam (whose Proton prefixes are Steam's) use
//! none. Listing reads folder names only; sizes stay with Settings → Storage.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use irori_core::settings::AppSettings;
use irori_core::{GameId, HostPlatform, InstallPlatform, Paths, Runner};

use crate::Backend;
use crate::storage::{paths_in_json, resolve};

/// One prefix and who uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixEntry {
    pub path: PathBuf,
    /// Installed games that run in it, by title.
    pub games: Vec<(GameId, String)>,
    /// Games no longer installed whose prefix it was, by title.
    pub former: Vec<String>,
    /// The game defaults' prefix (`winePrefix`).
    pub is_default: bool,
    /// A third-party launcher was set up in it.
    pub is_launcher: bool,
}

impl PrefixEntry {
    /// The folder name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Nothing installed uses it.
    pub fn unused(&self) -> bool {
        self.games.is_empty() && !self.is_default && !self.is_launcher
    }
}

/// What the list is built from; paths as settings and stores name them.
#[derive(Debug, Default)]
struct Sources {
    /// Prefixes found in the prefix folder.
    found: Vec<PathBuf>,
    /// Installed games: their prefix and their folder (a launcher's game
    /// lives inside its launcher's prefix).
    installed: Vec<(GameId, String, PathBuf, Option<PathBuf>)>,
    /// Games not installed, by their default prefix.
    former: Vec<(String, PathBuf)>,
    default: Option<PathBuf>,
    launchers: Vec<PathBuf>,
}

/// The folder Irori makes prefixes in: `defaultWinePrefixDir`, else
/// `Prefixes` in the install folder.
fn prefix_root(app: &AppSettings) -> PathBuf {
    if app.default_wine_prefix_dir.is_empty() {
        irori_core::paths::expand_home(&app.default_install_path).join("Prefixes")
    } else {
        irori_core::paths::expand_home(&app.default_wine_prefix_dir)
    }
}

/// Group the sources into entries, each path once, by name. `exists` says
/// whether a path is a prefix (tests answer without a disk).
fn entries(sources: Sources, exists: impl Fn(&Path) -> bool) -> Vec<PrefixEntry> {
    let mut by_path: BTreeMap<PathBuf, PrefixEntry> = BTreeMap::new();
    let entry = |map: &mut BTreeMap<PathBuf, PrefixEntry>, path: &Path| {
        map.entry(path.to_path_buf())
            .or_insert_with(|| PrefixEntry {
                path: path.to_path_buf(),
                games: Vec::new(),
                former: Vec::new(),
                is_default: false,
                is_launcher: false,
            })
            .path
            .clone()
    };
    for path in sources.found.iter().filter(|p| exists(p)) {
        entry(&mut by_path, path);
    }
    if let Some(default) = sources.default.as_deref().filter(|p| exists(p)) {
        let key = entry(&mut by_path, default);
        by_path.get_mut(&key).expect("just added").is_default = true;
    }
    for launcher in sources.launchers.iter().filter(|p| exists(p)) {
        let key = entry(&mut by_path, launcher);
        by_path.get_mut(&key).expect("just added").is_launcher = true;
    }
    for (id, title, prefix, folder) in &sources.installed {
        if exists(prefix) {
            entry(&mut by_path, prefix);
        }
        // Its own prefix, and any prefix its folder lies in.
        for e in by_path.values_mut() {
            let inside = folder.as_deref().is_some_and(|f| f.starts_with(&e.path));
            if (&e.path == prefix || inside) && !e.games.iter().any(|(g, _)| g == id) {
                e.games.push((id.clone(), title.clone()));
            }
        }
    }
    for (title, prefix) in &sources.former {
        if let Some(e) = by_path.get_mut(prefix)
            && !e.games.iter().any(|(_, t)| t == title)
        {
            e.former.push(title.clone());
        }
    }
    let mut list: Vec<PrefixEntry> = by_path.into_values().collect();
    for e in &mut list {
        e.games.sort_by_key(|(_, t)| t.to_lowercase());
        e.former.sort_by_key(|t| t.to_lowercase());
    }
    list.sort_by_key(|e| e.name().to_lowercase());
    list
}

/// Whether a game of this platform and runner runs in a prefix Irori makes.
fn uses_prefix(runner: Runner, platform: InstallPlatform, browser: bool) -> bool {
    runner != Runner::Steam && !browser && HostPlatform::current().needs_wine(platform)
}

impl Backend {
    /// Every Wine prefix on this computer with the installed games that use
    /// it, by name. Reads folders, so it runs on a blocking thread.
    pub async fn wine_prefixes(&self) -> Vec<PrefixEntry> {
        let app = self.settings.app();
        let mut sources = Sources::default();
        for game in self.library().all_games().filter(|g| !g.is_dlc) {
            let platform = game
                .installed_platform()
                .unwrap_or(InstallPlatform::Windows);
            if !uses_prefix(game.runner(), platform, game.browser_url.is_some()) {
                continue;
            }
            let id = game.id();
            if game.is_installed {
                let settings = self.settings.game(&id);
                let prefix = irori_runtime::prefix_path(&app, &settings, game);
                sources
                    .installed
                    .push((id, game.title.clone(), prefix, game.install_path()));
            } else {
                sources.former.push((
                    game.title.clone(),
                    irori_runtime::default_prefix(&app, game),
                ));
            }
        }
        let root = prefix_root(&app);
        let default = app.game.wine_prefix.trim();
        sources.default = (!default.is_empty()).then(|| irori_core::paths::expand_home(default));
        let launchers_state = Paths::get()
            .store_config_dir(Runner::Launchers)
            .join("launchers.json");
        tokio::task::spawn_blocking(move || {
            if let Ok(Some(state)) =
                irori_core::json_store::read::<serde_json::Value>(&launchers_state)
            {
                paths_in_json(&state, &mut sources.launchers);
            }
            sources.found = std::fs::read_dir(&root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            // One spelling of each path, links followed, so a game naming its
            // prefix through a link still finds it.
            let canon = |p: &mut PathBuf| *p = resolve(p);
            sources.found.iter_mut().for_each(canon);
            sources.launchers.iter_mut().for_each(canon);
            sources.default.iter_mut().for_each(canon);
            for (_, _, prefix, folder) in &mut sources.installed {
                canon(prefix);
                folder.iter_mut().for_each(canon);
            }
            for (_, prefix) in &mut sources.former {
                canon(prefix);
            }
            entries(sources, irori_runtime::is_prefix)
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(app: &str) -> GameId {
        GameId::new(Runner::Gog, app)
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn sources() -> Sources {
        Sources {
            found: vec![
                p("/p/Hades"),
                p("/p/Old"),
                p("/p/Battle.net"),
                p("/p/notes"),
            ],
            installed: vec![
                (
                    id("hades"),
                    "Hades".into(),
                    p("/p/Hades"),
                    Some(p("/g/Hades")),
                ),
                (
                    id("hades2"),
                    "Hades II".into(),
                    p("/p/Hades"),
                    Some(p("/g/Hades II")),
                ),
                (
                    GameId::new(Runner::Launchers, "battlenet:wow"),
                    "World of Warcraft".into(),
                    p("/p/Battle.net"),
                    Some(p("/p/Battle.net/drive_c/Program Files/WoW")),
                ),
                (id("own"), "Own".into(), p("/elsewhere/own"), None),
            ],
            former: vec![
                ("Old Game".into(), p("/p/Old")),
                ("Hades".into(), p("/p/Hades")),
            ],
            default: Some(p("/p/Default")),
            launchers: vec![p("/p/Battle.net"), p("/t/wine/bin/wine")],
        }
    }

    fn exists(path: &Path) -> bool {
        // Folders that aren't prefixes (notes) and Wine programs aren't listed.
        !path.ends_with("notes") && !path.starts_with("/t")
    }

    #[test]
    fn lists_each_prefix_once_with_the_installed_games_using_it() {
        let list = entries(sources(), exists);
        let names: Vec<String> = list.iter().map(PrefixEntry::name).collect();
        assert_eq!(names, ["Battle.net", "Default", "Hades", "Old", "own"]);
        let hades = &list[2];
        let titles: Vec<&str> = hades.games.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(titles, ["Hades", "Hades II"]);
        // Installed now, so not a former user.
        assert!(hades.former.is_empty());
        assert!(!hades.unused());
    }

    #[test]
    fn a_prefix_only_uninstalled_games_used_is_unused() {
        let list = entries(sources(), exists);
        let old = list.iter().find(|e| e.name() == "Old").unwrap();
        assert!(old.games.is_empty());
        assert_eq!(old.former, ["Old Game"]);
        assert!(old.unused());
    }

    #[test]
    fn launchers_and_the_default_are_marked_and_count_as_used() {
        let list = entries(sources(), exists);
        let battle = list.iter().find(|e| e.name() == "Battle.net").unwrap();
        assert!(battle.is_launcher && !battle.unused());
        assert_eq!(battle.games.len(), 1, "its game lives inside it");
        let default = list.iter().find(|e| e.name() == "Default").unwrap();
        assert!(default.is_default && default.games.is_empty() && !default.unused());
    }

    #[test]
    fn games_outside_wine_use_no_prefix() {
        assert!(!uses_prefix(Runner::Steam, InstallPlatform::Windows, false));
        assert!(!uses_prefix(Runner::Gog, InstallPlatform::Windows, true));
        assert!(!uses_prefix(
            Runner::Gog,
            HostPlatform::current().native(),
            false
        ));
        assert_eq!(
            uses_prefix(Runner::Gog, InstallPlatform::Windows, false),
            !cfg!(windows)
        );
    }
}
