//! Plan imports of games set up in Bottles and Faugus Launcher, two Wine
//! launchers whose games Irori can run itself. Like the Lutris import
//! ([`crate::lutris_import`]), this only reads: each launcher's own files
//! are opened read-only, nothing is written there, and every game that is
//! not imported comes back with the reason. The plans use the Lutris
//! import's shapes ([`LutrisPlan`], [`PlannedGame`], [`ImportKind`]), so the
//! shell applies all three the same way.

pub mod bottles;
pub mod faugus;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lutris_import::{ImportKind, PlannedGame};
pub use crate::lutris_import::{LutrisPlan, SkipReason};

/// The folders every Lutris, Bottles and Faugus Launcher installation of
/// this user keeps games, prefixes and Wine builds in, by launcher, read
/// from their own data (only read). Reads files: call it off the async
/// runtime.
pub fn found_folders() -> Vec<(&'static str, Vec<PathBuf>)> {
    let mut lutris = Vec::new();
    for dirs in crate::lutris_import::LutrisDirs::discover() {
        lutris.push(dirs.data_dir.join("runners"));
        match crate::lutris_import::plan(&dirs) {
            Ok(plan) => lutris.extend(plan.folders()),
            Err(e) => tracing::debug!("reading Lutris' folders: {e:#}"),
        }
    }
    let mut bottles = Vec::new();
    for dirs in bottles::BottlesDirs::discover() {
        bottles.push(dirs.runners_dir.clone());
        match bottles::plan(&dirs) {
            Ok((plan, found)) => {
                bottles.extend(found.into_iter().map(|b| b.path));
                bottles.extend(plan.folders());
            }
            Err(e) => tracing::debug!("reading Bottles' folders: {e:#}"),
        }
    }
    let mut faugus = Vec::new();
    for file in faugus::discover() {
        match faugus::plan(&file) {
            Ok(plan) => faugus.extend(plan.folders()),
            Err(e) => tracing::debug!("reading Faugus Launcher's folders: {e:#}"),
        }
    }
    vec![
        ("Lutris", lutris),
        ("Bottles", bottles),
        ("Faugus Launcher", faugus),
    ]
}

/// The XDG data or config folder under `home`, honouring `var`.
fn xdg(home: &Path, var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(default))
}

/// A planned game with only a name, a kind and a cover.
fn planned(name: String, id: String, runner: &str, kind: ImportKind) -> PlannedGame {
    PlannedGame {
        lutris_id: 0,
        name,
        slug: id,
        runner: runner.to_string(),
        kind,
        favourite: false,
        hidden: false,
        categories: Vec::new(),
        playtime_minutes: 0,
        last_played: None,
        cover: None,
        commands: Default::default(),
    }
}

/// `KEY=value` words of an environment string (`DXVK_ASYNC=1 FOO="a b"`);
/// other words are left out.
fn env_words(text: &str) -> BTreeMap<String, String> {
    irori_core::process::split_args(text)
        .into_iter()
        .filter_map(|w| {
            let (k, v) = w.split_once('=')?;
            let valid = !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !k.starts_with(|c: char| c.is_ascii_digit());
            valid.then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}
