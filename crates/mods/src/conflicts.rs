//! Which files several enabled mods provide, and which one the game gets.

use std::collections::{BTreeMap, HashMap};

/// Conflicts between the enabled mods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conflicts {
    /// Files provided by more than one mod, sorted by path.
    pub files: Vec<FileConflict>,
    /// Per mod id.
    pub by_mod: BTreeMap<String, ModConflicts>,
}

/// One file several mods provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileConflict {
    pub path: String,
    /// Mod ids in load order; the last one wins.
    pub mods: Vec<String>,
}

impl FileConflict {
    pub fn winner(&self) -> &str {
        self.mods.last().map_or("", String::as_str)
    }
}

/// How one mod fares against the others.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModConflicts {
    /// Mods this one beats, with the number of files.
    pub wins_over: BTreeMap<String, usize>,
    /// Mods that beat this one, with the number of files.
    pub loses_to: BTreeMap<String, usize>,
    /// Game files this mod replaces.
    pub game_files: usize,
    /// Every file of this mod is overridden: it has no effect.
    pub hidden: bool,
}

impl Conflicts {
    pub fn of(&self, id: &str) -> Option<&ModConflicts> {
        self.by_mod.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Conflicts between `mods` (enabled, in load order, each with its files
/// relative to the game folder). `game_has` tells whether the game folder
/// has a file of its own at a path. Paths compare without letter case.
pub fn find(mods: &[(&str, &[String])], game_has: impl Fn(&str) -> bool) -> Conflicts {
    let mut providers: HashMap<String, (String, Vec<usize>)> = HashMap::new();
    for (ix, (_, files)) in mods.iter().enumerate() {
        for file in files.iter() {
            let entry = providers
                .entry(file.to_lowercase())
                .or_insert_with(|| (file.clone(), Vec::new()));
            if entry.1.last() != Some(&ix) {
                entry.1.push(ix);
            }
        }
    }
    let mut out = Conflicts::default();
    let mut won = vec![0usize; mods.len()];
    for (path, owners) in providers.values() {
        let winner = *owners.last().expect("every path has a provider");
        won[winner] += 1;
        if game_has(path) {
            out.by_mod
                .entry(mods[winner].0.to_string())
                .or_default()
                .game_files += 1;
        }
        if owners.len() < 2 {
            continue;
        }
        for &loser in &owners[..owners.len() - 1] {
            let (w, l) = (mods[winner].0.to_string(), mods[loser].0.to_string());
            *out.by_mod
                .entry(w.clone())
                .or_default()
                .wins_over
                .entry(l.clone())
                .or_default() += 1;
            *out.by_mod
                .entry(l)
                .or_default()
                .loses_to
                .entry(w)
                .or_default() += 1;
        }
        out.files.push(FileConflict {
            path: path.clone(),
            mods: owners.iter().map(|&i| mods[i].0.to_string()).collect(),
        });
    }
    for (ix, (id, files)) in mods.iter().enumerate() {
        if !files.is_empty() && won[ix] == 0 {
            out.by_mod.entry(id.to_string()).or_default().hidden = true;
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
