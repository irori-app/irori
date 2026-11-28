//! The ROM library's covers from LaunchBox's box art
//! (`irori_extras::launchbox`).
//!
//! Each game is looked up once per index in its system's LaunchBox games by
//! title, and its cover is the box of its own region (from the file name's
//! badges: `(Europe)` asks for the European box). A game LaunchBox doesn't
//! have keeps libretro's thumbnail; a cover the user chose wins over both,
//! since the library lays the user's art over the store's.
//!
//! The database is downloaded on request ([`Covers::refresh`] with `force`,
//! which the UI does after turning `iroriLaunchBox` on), or on its own while
//! that setting is on, the library has games and the index is due (once a
//! week). Only what the store was built with downloads: the test
//! constructors have no address, so tests never reach LaunchBox.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use irori_extras::launchbox::{self, Index, Lookup};
pub use irori_extras::launchbox::{BoxArt, LaunchBoxGame, Refreshed};
use parking_lot::RwLock;

use crate::RomEntry;

/// LaunchBox's platform names for Irori's systems; every system has at
/// least one (a test checks).
pub const PLATFORMS: &[(&str, &str)] = &[
    ("Nintendo Entertainment System", "nes"),
    ("Nintendo Famicom Disk System", "nes"),
    ("Super Nintendo Entertainment System", "snes"),
    ("Nintendo Satellaview", "snes"),
    ("Nintendo 64", "n64"),
    ("Nintendo 64DD", "n64"),
    ("Nintendo Game Boy", "gb"),
    ("Nintendo Game Boy Color", "gbc"),
    ("Nintendo Game Boy Advance", "gba"),
    ("Nintendo DS", "nds"),
    ("Nintendo 3DS", "3ds"),
    ("Nintendo GameCube", "gamecube"),
    ("Nintendo Wii", "wii"),
    ("Nintendo Wii U", "wiiu"),
    ("Nintendo Switch", "switch"),
    ("Sega Master System", "sms"),
    ("Sega Genesis", "genesis"),
    ("Sega Game Gear", "gamegear"),
    ("Sega CD", "segacd"),
    ("Sega Dreamcast", "dreamcast"),
    ("NEC TurboGrafx-16", "pce"),
    ("NEC TurboGrafx-CD", "pce"),
    ("Sony Playstation", "psx"),
    ("Sony Playstation 2", "ps2"),
    ("Sony Playstation 3", "ps3"),
    ("Sony Playstation 4", "ps4"),
    ("Sony Playstation 5", "ps5"),
    ("Sony PSP", "psp"),
    ("Sony PSP Minis", "psp"),
    ("Sony Playstation Vita", "psvita"),
    ("Microsoft Xbox", "xbox"),
    ("MS-DOS", "dos"),
    ("Windows 3.X", "win3x"),
    ("ScummVM", "scummvm"),
    ("Arcade", "arcade"),
    ("Windows", "doom"),
];

/// [`PLATFORMS`] as the index takes it.
pub fn platforms() -> HashMap<String, String> {
    PLATFORMS
        .iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect()
}

/// Whether LaunchBox's index can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchBoxStatus {
    /// Downloaded: games can be looked up.
    Ready,
    /// Being downloaded now.
    Downloading,
    /// Not downloaded yet (or the last download failed).
    Missing,
}

/// What a game was matched to, and under which title and index.
#[derive(Clone)]
struct Checked {
    title: String,
    revision: u64,
    game: Option<Arc<LaunchBoxGame>>,
}

/// The ROM library's LaunchBox lookups.
pub(crate) struct Covers {
    index: Index,
    /// Downloads are allowed (the app's store, or a test's own server).
    online: bool,
    checked: RwLock<HashMap<String, Checked>>,
    /// The index's `updated_at`: lookups made against another are redone.
    revision: RwLock<u64>,
    downloading: AtomicBool,
}

impl Covers {
    /// Lookups in `dir`, downloading from `url` when there is one.
    pub(crate) fn new(dir: PathBuf, url: Option<String>) -> Self {
        let online = url.is_some();
        Self {
            index: Index::with_url(dir, url.unwrap_or_default()),
            online,
            checked: RwLock::new(HashMap::new()),
            revision: RwLock::new(0),
            downloading: AtomicBool::new(false),
        }
    }

    pub(crate) fn status(&self) -> LaunchBoxStatus {
        if self.downloading.load(Ordering::Relaxed) {
            LaunchBoxStatus::Downloading
        } else if *self.revision.read() > 0 {
            LaunchBoxStatus::Ready
        } else {
            LaunchBoxStatus::Missing
        }
    }

    /// The game `entry` was matched to, if it was and its title still holds.
    pub(crate) fn game(&self, entry: &RomEntry) -> Option<Arc<LaunchBoxGame>> {
        let revision = *self.revision.read();
        self.checked
            .read()
            .get(&entry.app_name)
            .filter(|c| c.title == entry.title && c.revision == revision)
            .and_then(|c| c.game.clone())
    }

    /// The cover of `entry`: its region's box.
    pub(crate) fn cover(&self, entry: &RomEntry) -> Option<String> {
        let game = self.game(entry)?;
        launchbox::best(&game.boxes, &entry.tags).map(BoxArt::url)
    }

    /// Look up the entries not looked up against the current index yet.
    /// Returns whether any cover may have changed. Blocking.
    pub(crate) fn match_entries(&self, entries: &[RomEntry]) -> bool {
        let state = self.index.state();
        let revision = state.updated_at;
        let mut changed = false;
        if *self.revision.read() != revision {
            *self.revision.write() = revision;
            changed = true;
        }
        if revision == 0 {
            return changed;
        }
        let todo: Vec<&RomEntry> = {
            let checked = self.checked.read();
            entries
                .iter()
                .filter(|e| {
                    checked
                        .get(&e.app_name)
                        .is_none_or(|c| c.title != e.title || c.revision != revision)
                })
                .collect()
        };
        if todo.is_empty() {
            return changed;
        }
        let mut by_system: HashMap<&str, Vec<&RomEntry>> = HashMap::new();
        for entry in todo {
            by_system.entry(&entry.system).or_default().push(entry);
        }
        let mut found = Vec::new();
        for (system, entries) in by_system {
            let index = state
                .systems
                .iter()
                .any(|s| s == system)
                .then(|| self.index.load(system))
                .flatten();
            let lookup = index.as_ref().map(Lookup::new);
            for entry in entries {
                let game = lookup
                    .as_ref()
                    .and_then(|l| l.find(&entry.title))
                    .map(|g| Arc::new(g.clone()));
                found.push((
                    entry.app_name.clone(),
                    Checked {
                        title: entry.title.clone(),
                        revision,
                        game,
                    },
                ));
            }
        }
        let mut checked = self.checked.write();
        for (app_name, c) in found {
            changed |=
                c.game.is_some() || checked.get(&app_name).is_some_and(|old| old.game.is_some());
            checked.insert(app_name, c);
        }
        changed
    }

    /// Download the database when due, or always with `force`.
    pub(crate) async fn refresh(&self, force: bool) -> Result<Refreshed> {
        if !self.online {
            if force {
                bail!("LaunchBox's games database can't be downloaded here");
            }
            return Ok(Refreshed::NotDue);
        }
        self.downloading.store(true, Ordering::Relaxed);
        let done = self.index.refresh(&platforms(), force).await;
        self.downloading.store(false, Ordering::Relaxed);
        done
    }

    /// Whether a download would start now, without asking: the index is
    /// due and none runs. Blocking.
    pub(crate) fn is_due(&self) -> bool {
        self.online
            && !self.downloading.load(Ordering::Relaxed)
            && self
                .index
                .state()
                .is_due(&launchbox::table_key(&platforms()), launchbox::now())
    }

    /// The bytes the index takes, if there is one. Blocking.
    pub(crate) fn stored(&self) -> Option<u64> {
        Some(self.index.size()).filter(|n| *n > 0)
    }

    /// Delete the index, after `fs_guard` vetted its folder.
    pub(crate) async fn remove(&self, app: &irori_core::AppSettings) -> Result<()> {
        let dir = self.index.dir().to_path_buf();
        if dir.exists() {
            irori_core::fs_guard::ensure_deletable(&dir, app)?;
        }
        self.index.remove().await
    }

    /// The games of `system` whose names hold `query`. Blocking.
    pub(crate) fn search(&self, system: &str, query: &str, limit: usize) -> Vec<LaunchBoxGame> {
        self.index
            .load(system)
            .map(|index| index.search(query, limit).into_iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_system_named_has_a_platform() {
        for (_, system) in PLATFORMS {
            assert!(
                irori_engines::systems::get(system).is_some(),
                "{system} is not one of Irori's systems"
            );
        }
        for system in irori_engines::systems::all() {
            assert!(
                PLATFORMS.iter().any(|(_, s)| *s == system.id),
                "{} has no LaunchBox platform",
                system.id
            );
        }
    }
}
