//! Restoring a backup on another computer, user or system: its paths
//! checked on this computer, moved through the folders that moved
//! (`backup_paths`), missing Wine builds replaced by installed ones of the
//! same name, and the store games installed then attached again.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use irori_core::{GameId, Runner, WineInstallation};
use serde_json::Value;

use crate::Backend;
use crate::backup::{
    Backup, BackupParts, InstallRecord, PathCheck, PathKind, RestoreSummary, game_id,
};
use crate::backup_paths::Remap;

/// The user's remaps first (they name the folders they found), then the
/// automatic ones; within each, the most specific first.
pub(crate) fn all_remaps(backup: &Backup, user: &[Remap]) -> Vec<Remap> {
    let mut user = user.to_vec();
    user.sort_by_key(|r| std::cmp::Reverse(r.from.len()));
    user.extend(backup.automatic_remaps());
    user
}

/// A remap from where the user found a missing path: the part both paths
/// end with is the same folder, so what comes before it moved. Found
/// `/mnt/old/Games/Hades` at `/run/media/ana/Games/Hades`, every path under
/// `/mnt/old` is now under `/run/media/ana`.
pub fn remap_from_located(original: &str, found: &str) -> Remap {
    let parts = |p: &str| -> Vec<String> {
        p.split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };
    let (a, b) = (parts(original), parts(found));
    let common = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x.eq_ignore_ascii_case(y))
        .count();
    // Keep one folder of each side, so a remap never names a drive's root.
    let common = common
        .min(a.len().saturating_sub(1))
        .min(b.len().saturating_sub(1));
    let cut = |path: &str, n: usize| -> String {
        let mut out = path.trim_end_matches(['/', '\\']).to_string();
        for _ in 0..n {
            if let Some(at) = out.rfind(['/', '\\']) {
                out.truncate(at);
            }
        }
        out
    };
    Remap::new(cut(original, common), cut(found, common))
}

/// Whether a store's installs are attached again from a backup: the stores
/// whose `import` registers a folder. Steam's and the launchers' games are
/// their clients', ROMs come from the folder scan and added games keep
/// their own records.
pub(crate) fn reattachable(runner: Runner) -> bool {
    runner.manages_installs() && !matches!(runner, Runner::Sideload | Runner::Emulation)
}

/// Where a backup names a Wine build that isn't at its path here, use an
/// installed build of the same name (a GE-Proton the Wine manager put in
/// another folder on this computer).
fn replace_missing_builds(backup: &mut Backup, builds: &[irori_runtime::DiscoveredWine]) {
    let replace = |map: &mut serde_json::Map<String, Value>| {
        let Some(current) = map
            .get("wineVersion")
            .and_then(|v| serde_json::from_value::<WineInstallation>(v.clone()).ok())
            .filter(|w| w.is_set())
        else {
            return;
        };
        if Path::new(&current.bin).exists() {
            return;
        }
        if let Some(found) = builds
            .iter()
            .find(|b| b.installation.name == current.name && b.installation.kind == current.kind)
            && let Ok(value) = serde_json::to_value(&found.installation)
        {
            map.insert("wineVersion".into(), value);
        }
    };
    if let Some(Value::Object(map)) = &mut backup.settings {
        replace(map);
    }
    for value in backup.game_settings.values_mut() {
        if let Value::Object(map) = value {
            replace(map);
        }
    }
}

impl Backend {
    /// The Wine and Proton builds on this computer; none in tests, which
    /// never look at the user's folders.
    async fn wine_builds(&self) -> Vec<irori_runtime::DiscoveredWine> {
        if self.isolated {
            return Vec::new();
        }
        irori_runtime::discovery::discover(&self.settings.app()).await
    }

    /// Check on this computer every path the chosen parts of `backup` need,
    /// after the automatic remaps and the user's. Reads the disk and lists
    /// the Wine builds: call it from the backend runtime.
    pub async fn plan_restore(
        &self,
        backup: &Backup,
        parts: BackupParts,
        remaps: &[Remap],
    ) -> Vec<PathCheck> {
        let remaps = all_remaps(backup, remaps);
        let (rewritten, _) = backup.rewritten(&remaps, cfg!(windows));
        let builds = self.wine_builds().await;
        let names: BTreeSet<String> = builds.into_iter().map(|b| b.installation.name).collect();
        let checks = self.path_checks(backup, &rewritten, parts);
        self.spawn_blocking(move || {
            checks
                .into_iter()
                .map(|mut c| {
                    c.found = match c.kind {
                        PathKind::WineBuild => names.contains(&c.path),
                        PathKind::Install | PathKind::Prefix => Path::new(&c.path).exists(),
                    };
                    c
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// The paths to check, not yet checked: install folders, prefixes and
    /// Wine builds (by name, since a build found under another folder
    /// does).
    fn path_checks(
        &self,
        original: &Backup,
        rewritten: &Backup,
        parts: BackupParts,
    ) -> Vec<PathCheck> {
        let title = |key: &str| {
            game_id(key)
                .and_then(|id| self.game(&id))
                .map(|g| g.title.clone())
                .or_else(|| {
                    original
                        .installs
                        .iter()
                        .find(|r| r.game == key)
                        .map(|r| r.title.clone())
                })
                .unwrap_or_else(|| key.to_string())
        };
        let mut out = Vec::new();
        if parts.installs {
            for (before, after) in original.installs.iter().zip(&rewritten.installs) {
                out.push(PathCheck {
                    kind: PathKind::Install,
                    game: game_id(&after.game),
                    title: after.title.clone(),
                    original: before.path.clone(),
                    path: after.path.clone(),
                    found: false,
                });
            }
        }
        let mut settings_checks =
            |game: Option<GameId>, title: String, before: &Value, after: &Value| {
                // Windows runs its games without Wine.
                if cfg!(windows) {
                    return;
                }
                if let Some(prefix) = after.get("winePrefix").and_then(Value::as_str)
                    && !prefix.trim().is_empty()
                {
                    out.push(PathCheck {
                        kind: PathKind::Prefix,
                        game: game.clone(),
                        title: title.clone(),
                        original: before
                            .get("winePrefix")
                            .and_then(Value::as_str)
                            .unwrap_or(prefix)
                            .to_string(),
                        path: prefix.to_string(),
                        found: false,
                    });
                }
                if let Some(name) = after
                    .get("wineVersion")
                    .and_then(|w| w.get("name"))
                    .and_then(Value::as_str)
                    .filter(|n| !n.trim().is_empty())
                {
                    out.push(PathCheck {
                        kind: PathKind::WineBuild,
                        game,
                        title,
                        original: name.to_string(),
                        path: name.to_string(),
                        found: false,
                    });
                }
            };
        if parts.settings
            && let (Some(before), Some(after)) = (&original.settings, &rewritten.settings)
        {
            settings_checks(None, String::new(), before, after);
        }
        if parts.game_settings {
            for (key, after) in &rewritten.game_settings {
                let before = original.game_settings.get(key).unwrap_or(after);
                settings_checks(game_id(key), title(key), before, after);
            }
        }
        out
    }

    /// Restore the chosen parts of `backup` with its paths moved to this
    /// computer (the automatic remaps, then the user's `remaps`), a game's
    /// missing Wine build replaced by an installed one of the same name,
    /// and the installed games found again attached to their stores.
    pub async fn restore_backup_with(
        &self,
        backup: &Backup,
        parts: BackupParts,
        user_remaps: &[Remap],
    ) -> Result<RestoreSummary> {
        let remaps = all_remaps(backup, user_remaps);
        let (mut rewritten, dropped) = backup.rewritten(&remaps, cfg!(windows));
        let builds = self.wine_builds().await;
        replace_missing_builds(&mut rewritten, &builds);
        let b = self.clone();
        let restored = rewritten.clone();
        let mut summary = self
            .spawn_blocking(move || b.restore_rewritten(&restored, parts))
            .await??;
        summary.paths_dropped = dropped;
        if parts.installs {
            summary.installs_attached = self.attach_installs(&rewritten.installs).await;
        }
        summary.missing = self
            .plan_restore(backup, parts, user_remaps)
            .await
            .into_iter()
            .filter(|c| !c.found)
            .collect();
        Ok(summary)
    }

    /// Attach each recorded install whose folder is here to its store, when
    /// the store lists the game and it isn't installed now.
    async fn attach_installs(&self, records: &[InstallRecord]) -> usize {
        let mut attached = 0;
        for record in records {
            let Some(id) = game_id(&record.game) else {
                continue;
            };
            let path = std::path::PathBuf::from(&record.path);
            let here = {
                let path = path.clone();
                self.spawn_blocking(move || path.is_dir())
                    .await
                    .unwrap_or(false)
            };
            let listed = self.game(&id).is_some_and(|g| !g.is_installed);
            if !here || !listed {
                continue;
            }
            match self.import(&id, &path, record.platform).await {
                Ok(()) => attached += 1,
                Err(e) => tracing::warn!("attaching {} again: {e:#}", record.title),
            }
        }
        attached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_located_folder_moves_what_lies_beside_it() {
        assert_eq!(
            remap_from_located("/mnt/old/Games/Hades", "/run/media/ana/Games/Hades"),
            Remap::new("/mnt/old", "/run/media/ana")
        );
        assert_eq!(
            remap_from_located(r"D:\Games\Hades", "/home/ana/Games/Hades"),
            Remap::new("D:", "/home/ana"),
            "across systems, without letter case"
        );
        // Renamed: only that folder moved.
        assert_eq!(
            remap_from_located("/mnt/old/Games/Hades", "/home/ana/Hades II"),
            Remap::new("/mnt/old/Games/Hades", "/home/ana/Hades II")
        );
        // Never a drive's root on either side.
        assert_eq!(
            remap_from_located("/Hades", "/Hades"),
            Remap::new("/Hades", "/Hades")
        );
    }

    #[test]
    fn only_folder_installs_are_attached_again() {
        assert!(reattachable(Runner::Gog));
        assert!(reattachable(Runner::Legendary));
        assert!(!reattachable(Runner::Steam));
        assert!(!reattachable(Runner::Sideload));
        assert!(!reattachable(Runner::Emulation));
        assert!(!reattachable(Runner::Launchers));
    }

    #[test]
    fn a_missing_build_becomes_the_installed_one_of_its_name() {
        let installed = WineInstallation {
            bin: "/opt/proton/GE-Proton9-20/proton".into(),
            name: "GE-Proton9-20".into(),
            kind: irori_core::settings::WineKind::Proton,
            ..Default::default()
        };
        let mut backup = Backup {
            game_settings: [(
                "gog:1".to_string(),
                serde_json::json!({"wineVersion": {
                    "bin": "/nowhere/GE-Proton9-20/proton",
                    "name": "GE-Proton9-20",
                    "type": "proton",
                }}),
            )]
            .into(),
            ..Default::default()
        };
        replace_missing_builds(
            &mut backup,
            &[irori_runtime::DiscoveredWine {
                installation: installed.clone(),
                source: irori_runtime::WineSource::Irori,
            }],
        );
        let now: WineInstallation =
            serde_json::from_value(backup.game_settings["gog:1"]["wineVersion"].clone()).unwrap();
        assert_eq!(now, installed);
    }
}
