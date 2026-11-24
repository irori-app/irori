//! Games GOG Galaxy installed on Windows, so they show as installed in
//! Irori without being downloaded again. Galaxy lists them in
//! `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>` (`gameID`, `path`), and
//! each folder holds the `goggame-<id>.info` the usual import reads. After
//! each refresh, an owned game Irori has no install of is imported from
//! its Galaxy folder once per folder: a folder that didn't import (moved,
//! half uninstalled) isn't read again. Nothing of Galaxy's is written.

use std::collections::BTreeSet;
use std::path::PathBuf;

use irori_core::{InstallPlatform, json_store};
use serde::{Deserialize, Serialize};

use crate::GogStore;

/// One game Galaxy lists as installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GalaxyInstall {
    pub app_name: String,
    pub path: PathBuf,
}

impl GalaxyInstall {
    fn key(&self) -> String {
        format!("{}|{}", self.app_name, self.path.display())
    }
}

/// The Galaxy installs to import: owned, not installed in Irori, and not
/// tried before at that folder.
pub(crate) fn to_import(
    found: Vec<GalaxyInstall>,
    owned: impl Fn(&str) -> bool,
    installed: impl Fn(&str) -> bool,
    tried: &BTreeSet<String>,
) -> Vec<GalaxyInstall> {
    found
        .into_iter()
        .filter(|g| owned(&g.app_name) && !installed(&g.app_name) && !tried.contains(&g.key()))
        .collect()
}

/// The folders already tried.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Tried {
    installs: BTreeSet<String>,
}

#[cfg(windows)]
fn read_registry() -> Vec<GalaxyInstall> {
    use windows_registry::LOCAL_MACHINE;
    let Ok(games) = LOCAL_MACHINE.open(r"SOFTWARE\WOW6432Node\GOG.com\Games") else {
        return Vec::new();
    };
    let Ok(ids) = games.keys() else {
        return Vec::new();
    };
    ids.filter_map(|id| {
        let key = games.open(&id).ok()?;
        let app_name = key.get_string("gameID").unwrap_or(id);
        let path = PathBuf::from(key.get_string("path").ok()?);
        (app_name.chars().all(|c| c.is_ascii_digit()) && path.is_dir())
            .then_some(GalaxyInstall { app_name, path })
    })
    .collect()
}

/// Galaxy exists only on Windows (and macOS, whose installs Irori doesn't
/// read).
#[cfg(not(windows))]
fn read_registry() -> Vec<GalaxyInstall> {
    Vec::new()
}

impl GogStore {
    /// Import the games Galaxy installed that Irori has no install of.
    pub(crate) async fn import_galaxy_installs(&self) {
        let found = match tokio::task::spawn_blocking(read_registry).await {
            Ok(found) if !found.is_empty() => found,
            _ => return,
        };
        let file = self.paths.config.join("galaxy-imports.json");
        let mut tried: Tried = json_store::read_or_default(&file);
        let todo = {
            let state = self.state.read();
            to_import(
                found,
                |app| state.view.get(app).is_some_and(|g| !g.is_dlc),
                |app| self.record(app).is_some(),
                &tried.installs,
            )
        };
        if todo.is_empty() {
            return;
        }
        for install in todo {
            match self
                .import_game(&install.app_name, &install.path, InstallPlatform::Windows)
                .await
            {
                Ok(()) => tracing::info!(
                    "imported {} from GOG Galaxy's folder {}",
                    install.app_name,
                    install.path.display()
                ),
                Err(e) => tracing::info!("GOG Galaxy's install of {}: {e:#}", install.app_name),
            }
            tried.installs.insert(install.key());
        }
        if let Err(e) = json_store::write(&file, &tried) {
            tracing::warn!("recording GOG Galaxy's installs: {e:#}");
        }
        self.bus
            .emit(irori_core::Event::LibraryChanged(irori_core::Runner::Gog));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_owned_games_irori_lacks_once_per_folder() {
        let at = |app: &str, path: &str| GalaxyInstall {
            app_name: app.into(),
            path: path.into(),
        };
        let found = vec![
            at("1", "C:/GOG Games/Owned"),
            at("2", "C:/GOG Games/Installed here"),
            at("3", "C:/GOG Games/Not owned"),
            at("4", "C:/GOG Games/Tried"),
        ];
        let tried = BTreeSet::from([at("4", "C:/GOG Games/Tried").key()]);
        let todo = to_import(found, |a| a != "3", |a| a == "2", &tried);
        assert_eq!(todo, [at("1", "C:/GOG Games/Owned")]);
        // A folder moved since counts as new.
        let moved = to_import(vec![at("4", "D:/Tried")], |_| true, |_| false, &tried);
        assert_eq!(moved.len(), 1);
    }
}
