//! Added games that are probably the same game twice: one added by hand
//! and one imported from Lutris, Bottles or Faugus Launcher, or the same
//! program added twice. [`find`] pairs them (same program, a program inside
//! the other's folder, the same Wine prefix, the same title) and
//! [`Backend::merge_duplicate`] keeps one with what the user kept about
//! both: playtime, sessions, play status, journal, rating, categories and
//! favourite ([`crate::userdata::UserData::carry_over`]). The other leaves
//! the library; its files stay. "Keep both" is remembered on this computer
//! (`state/duplicates.json`).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{Event, GameId, GameInfo, Paths, RemoveArgs, Runner, json_store};
use serde::{Deserialize, Serialize};

use crate::Backend;

/// Why two added games look like one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DuplicateReason {
    /// Both start the same program.
    SameProgram,
    /// One's program is inside the other's folder.
    SameFolder,
    /// Both run in the same Wine prefix.
    SamePrefix,
    /// Both have the same title.
    SameTitle,
}

/// Two added games that are probably one, the one to keep first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub keep: GameId,
    pub other: GameId,
    pub reason: DuplicateReason,
}

/// What [`find`] knows of one added game.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: GameId,
    pub title: String,
    pub program: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    /// The prefix the game's own settings name (none: it follows the
    /// defaults).
    pub prefix: Option<PathBuf>,
    /// Minutes played, so the game with the longer history is kept.
    pub minutes: u64,
}

/// A pair's key, the same whichever way round: what "Keep both" remembers.
fn pair_key(a: &GameId, b: &GameId) -> (String, String) {
    let (a, b) = (a.key(), b.key());
    if a <= b { (a, b) } else { (b, a) }
}

/// The likely duplicates among `games`, strongest reason first, each game
/// in one pair at most; pairs in `dismissed` are left out.
pub fn find(games: &[Candidate], dismissed: &BTreeSet<(String, String)>) -> Vec<Duplicate> {
    let mut found: Vec<(DuplicateReason, usize, usize)> = Vec::new();
    let mut by = |reason, key: Option<String>, map: &mut HashMap<String, usize>, i: usize| {
        if let Some(key) = key.filter(|k| !k.is_empty()) {
            match map.get(&key) {
                Some(&j) => found.push((reason, j, i)),
                None => {
                    map.insert(key, i);
                }
            }
        }
    };
    let (mut programs, mut prefixes, mut titles) = (HashMap::new(), HashMap::new(), HashMap::new());
    let text = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().to_lowercase());
    for (i, g) in games.iter().enumerate() {
        by(
            DuplicateReason::SameProgram,
            text(&g.program),
            &mut programs,
            i,
        );
        by(
            DuplicateReason::SamePrefix,
            text(&g.prefix),
            &mut prefixes,
            i,
        );
        by(
            DuplicateReason::SameTitle,
            Some(crate::search::fold(&g.title)),
            &mut titles,
            i,
        );
    }
    let folders: HashMap<String, usize> = games
        .iter()
        .enumerate()
        .filter_map(|(i, g)| Some((g.folder.as_ref()?.to_string_lossy().to_lowercase(), i)))
        .collect();
    for (i, g) in games.iter().enumerate() {
        let Some(program) = &g.program else { continue };
        // The program's folder and the one above: a whole library folder
        // further up would pair every game in it.
        for dir in program.ancestors().skip(1).take(2) {
            if let Some(&j) = folders.get(&dir.to_string_lossy().to_lowercase())
                && j != i
            {
                found.push((DuplicateReason::SameFolder, j, i));
                break;
            }
        }
    }
    found.sort();
    let mut paired = BTreeSet::new();
    let mut out = Vec::new();
    for (reason, a, b) in found {
        if a == b || paired.contains(&a) || paired.contains(&b) {
            continue;
        }
        let (ga, gb) = (&games[a], &games[b]);
        if dismissed.contains(&pair_key(&ga.id, &gb.id)) {
            continue;
        }
        paired.insert(a);
        paired.insert(b);
        let (keep, other) = if gb.minutes > ga.minutes {
            (gb, ga)
        } else {
            (ga, gb)
        };
        out.push(Duplicate {
            keep: keep.id.clone(),
            other: other.id.clone(),
            reason,
        });
    }
    out
}

/// The pairs the user kept both of.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Dismissed {
    pairs: BTreeSet<(String, String)>,
}

fn dismissed_path() -> PathBuf {
    Paths::get().state_dir().join("duplicates.json")
}

impl Backend {
    /// The added games that are probably one game twice. Reads a file.
    pub fn duplicate_games(&self) -> Vec<Duplicate> {
        let dismissed: Dismissed = json_store::read_or_default(&dismissed_path());
        let library = self.library();
        let userdata = self.userdata.snapshot();
        let candidates: Vec<Candidate> = library
            .all_games()
            .filter(|g| g.runner() == Runner::Sideload && !g.is_dlc)
            .map(|g| {
                // Only a prefix the game's own settings name: the defaults'
                // is shared by design.
                let prefix = self
                    .settings
                    .overrides(&g.id())
                    .0
                    .get("winePrefix")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                candidate(g, &prefix, &userdata)
            })
            .collect();
        find(&candidates, &dismissed.pairs)
    }

    /// Keep both games of a pair, and stop offering it.
    pub fn keep_duplicates(&self, a: &GameId, b: &GameId) -> Result<()> {
        let path = dismissed_path();
        let mut dismissed: Dismissed = json_store::read_or_default(&path);
        dismissed.pairs.insert(pair_key(a, b));
        json_store::write(&path, &dismissed)
    }

    /// Keep `keep` with what the user kept about `other` too, and take
    /// `other` out of the library (its files stay).
    pub async fn merge_duplicate(&self, keep: &GameId, other: &GameId) -> Result<()> {
        anyhow::ensure!(
            other.runner == Runner::Sideload && keep != other,
            "Only one added game can be merged into another"
        );
        anyhow::ensure!(
            !self.is_running(other) && !self.is_running(keep),
            "Close the game before merging it"
        );
        let store = self
            .store(Runner::Sideload)
            .context("added games aren't available")?;
        let (from, to) = (other.key(), keep.key());
        {
            let _playtime = self.sync.playtime_guard();
            let mut list = crate::sessions::read_all();
            let mut moved = false;
            for s in list.iter_mut().filter(|s| s.game == from) {
                s.game = to.clone();
                moved = true;
            }
            if moved {
                crate::sessions::replace_all(&list)?;
            }
            self.userdata.carry_over(other, keep);
        }
        store
            .uninstall(
                &other.app_name,
                RemoveArgs {
                    delete_files: false,
                    remove_prefix: false,
                    remove_settings: true,
                },
            )
            .await?;
        self.settings.remove_game(other);
        self.bus.emit(Event::LibraryChanged(Runner::Sideload));
        Ok(())
    }
}

fn candidate(
    game: &Arc<GameInfo>,
    prefix: &str,
    userdata: &crate::userdata::UserData,
) -> Candidate {
    let install = game.install.as_ref();
    let folder = install
        .map(|i| PathBuf::from(&i.install_path))
        .filter(|p| !p.as_os_str().is_empty());
    let program = install
        .map(|i| Path::new(&i.executable))
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| match &folder {
            Some(dir) if p.is_relative() => dir.join(p),
            _ => p.to_path_buf(),
        });
    Candidate {
        id: game.id(),
        title: game.title.clone(),
        program,
        folder,
        prefix: Some(PathBuf::from(prefix)).filter(|p| !p.as_os_str().is_empty()),
        minutes: userdata
            .playtime
            .get(&game.id().key())
            .map_or(0, |p| p.total_minutes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(app: &str, title: &str, program: Option<&str>, folder: Option<&str>) -> Candidate {
        Candidate {
            id: GameId::new(Runner::Sideload, app),
            title: title.into(),
            program: program.map(PathBuf::from),
            folder: folder.map(PathBuf::from),
            prefix: None,
            minutes: 0,
        }
    }

    #[test]
    fn pairs_by_program_folder_prefix_and_title() {
        let mut games = vec![
            game(
                "a",
                "Witcher 3",
                Some("/g/w3/bin/w3.exe"),
                Some("/g/w3/bin"),
            ),
            game("b", "The Witcher 3 (Lutris)", None, Some("/g/w3")),
            game("c", "Celeste", Some("/g/celeste/Celeste.exe"), None),
            game("d", "CELESTE", Some("/h/celeste.exe"), None),
            game("e", "Hades", Some("/g/hades.exe"), None),
            game("f", "Other", Some("/g/hades.exe"), None),
        ];
        games[1].minutes = 90;
        let found = find(&games, &BTreeSet::new());
        let pairs: Vec<_> = found
            .iter()
            .map(|d| {
                (
                    d.keep.app_name.as_ref(),
                    d.other.app_name.as_ref(),
                    d.reason,
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [
                ("e", "f", DuplicateReason::SameProgram),
                ("b", "a", DuplicateReason::SameFolder),
                ("c", "d", DuplicateReason::SameTitle),
            ]
        );
    }

    #[test]
    fn kept_pairs_and_unrelated_games_are_left_out() {
        let mut games = vec![
            game("a", "One", Some("/x/one.exe"), None),
            game("b", "Two", Some("/y/two.exe"), None),
        ];
        games[0].prefix = Some("/p".into());
        games[1].prefix = Some("/p".into());
        assert_eq!(find(&games, &BTreeSet::new()).len(), 1);
        let kept = BTreeSet::from([pair_key(&games[1].id, &games[0].id)]);
        assert!(find(&games, &kept).is_empty());
        games[1].prefix = None;
        assert!(find(&games, &BTreeSet::new()).is_empty());
    }
}
