//! Program icons as cover stand-ins: a game without cover art (an added
//! Windows app, an import from Lutris, Bottles or Faugus Launcher, an itch.io
//! upload without a cover) shows the icon of its Windows program
//! (`irori_extras::program_icon`) on its stand-in cover, as Lutris does.
//!
//! Icons are read off the library rebuild, on a blocking thread, after it:
//! each is a PNG under `<cache>/program-icons/`, named by a digest of the
//! program's path, size and modified time, so a program that changes gets a
//! new file (and the art store a new address). What was found for each game,
//! no icon included, is kept in `<cache>/program-icons.json`, so the rebuild
//! itself reads no file and a program without an icon isn't read again
//! until it changes. The user's own art and the store's always win; the
//! `iroriUi.programIconArt` setting turns this off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use irori_core::{GameInfo, Paths, json_store};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// What one game's program gave.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Found {
    program: PathBuf,
    size: u64,
    /// Modified time, in seconds since the Unix epoch.
    modified: u64,
    /// The icon's PNG, or `None` when the program has none.
    icon: Option<PathBuf>,
}

/// Reads a program's icon as PNG bytes; [`irori_extras::program_icon::png_of`]
/// outside tests.
type Extract = fn(&Path) -> Option<Vec<u8>>;

pub struct ProgramIcons {
    /// By game key.
    found: RwLock<HashMap<String, Found>>,
    busy: AtomicBool,
    root: PathBuf,
    extract: Extract,
}

fn wants_icon(game: &GameInfo) -> bool {
    game.cover_art().is_none() && game.system.is_none()
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

impl ProgramIcons {
    pub fn load() -> Self {
        Self::under(
            Paths::get().cache.clone(),
            irori_extras::program_icon::png_of,
        )
    }

    fn under(root: PathBuf, extract: Extract) -> Self {
        Self {
            found: RwLock::new(json_store::read_or_default(
                &root.join("program-icons.json"),
            )),
            busy: AtomicBool::new(false),
            root,
            extract,
        }
    }

    fn index(&self) -> PathBuf {
        self.root.join("program-icons.json")
    }

    fn dir(&self) -> PathBuf {
        self.root.join("program-icons")
    }

    /// Give a game without cover art the icon its program was found to
    /// have, when it still starts that program. Reads no file.
    pub fn complete(&self, game: &mut GameInfo) {
        if !wants_icon(game) {
            return;
        }
        let Some(program) = irori_extras::program_icon::windows_program(game) else {
            return;
        };
        let found = self.found.read();
        if let Some(found) = found.get(&game.id().key())
            && found.program == program
            && let Some(icon) = &found.icon
        {
            game.program_icon = Some(file_url(icon));
        }
    }

    /// Read the icons of the games without cover art whose program is new
    /// or changed since it was last read. Blocking: run it on a blocking
    /// thread. Returns whether anything changed. One round runs at a time.
    pub fn refresh(&self, games: &[Arc<GameInfo>]) -> bool {
        let wanted: Vec<(String, PathBuf)> = games
            .iter()
            .filter(|g| wants_icon(g))
            .filter_map(|g| {
                Some((
                    g.id().key(),
                    irori_extras::program_icon::windows_program(g)?,
                ))
            })
            .collect();
        if wanted.is_empty() || self.busy.swap(true, Ordering::AcqRel) {
            return false;
        }
        let (mut changed, mut noted) = (false, false);
        for (key, program) in wanted {
            let Ok(meta) = std::fs::metadata(&program) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let known = self.found.read().get(&key).cloned();
            if let Some(k) = &known
                && k.program == program
                && k.size == meta.len()
                && k.modified == modified
                && k.icon.as_deref().is_none_or(Path::exists)
            {
                continue;
            }
            let icon = self.icon_of(&program, meta.len(), modified);
            let found = Found {
                program,
                size: meta.len(),
                modified,
                icon,
            };
            // A program without an icon is noted too, so it isn't read again;
            // only a change of what covers show needs a rebuild.
            if known.as_ref().and_then(|k| k.icon.as_ref()) != found.icon.as_ref() {
                changed = true;
            }
            if let Some(old) = known.and_then(|k| k.icon)
                && Some(&old) != found.icon.as_ref()
            {
                let _ = std::fs::remove_file(old);
            }
            self.found.write().insert(key, found);
            noted = true;
        }
        if noted {
            let snapshot = self.found.read().clone();
            if let Err(e) = json_store::write(&self.index(), &snapshot) {
                tracing::warn!("saving program icons: {e:#}");
            }
        }
        self.busy.store(false, Ordering::Release);
        changed
    }

    /// The program's icon as a PNG in the cache, read once per path, size
    /// and modified time.
    fn icon_of(&self, program: &Path, size: u64, modified: u64) -> Option<PathBuf> {
        let mut hash = Sha256::new();
        hash.update(program.as_os_str().as_encoded_bytes());
        hash.update(size.to_le_bytes());
        hash.update(modified.to_le_bytes());
        let digest = hash.finalize();
        let name: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        let target = self.dir().join(format!("{name}.png"));
        if target.is_file() {
            return Some(target);
        }
        let png = (self.extract)(program)?;
        match json_store::write_bytes(&target, &png) {
            Ok(()) => Some(target),
            Err(e) => {
                tracing::warn!("saving the icon of {}: {e:#}", program.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstalledInfo, Runner};

    fn game(app: &str, folder: &Path, exe: &str) -> GameInfo {
        GameInfo {
            runner: Some(Runner::Sideload),
            app_name: app.into(),
            install: Some(InstalledInfo {
                install_path: folder.display().to_string(),
                executable: exe.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn icon_png(program: &Path) -> Option<Vec<u8>> {
        let bytes = std::fs::read(program).ok()?;
        bytes
            .starts_with(b"MZ")
            .then(|| b"png of the icon".to_vec())
    }

    #[test]
    fn games_without_art_get_their_programs_icon() {
        let dir = tempfile::tempdir().unwrap();
        let games = dir.path().join("games");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join("a.exe"), b"MZ program").unwrap();
        std::fs::write(games.join("b.exe"), b"no icon here").unwrap();
        let icons = ProgramIcons::under(dir.path().join("cache"), icon_png);
        let a = game("a", &games, "a.exe");
        let b = game("b", &games, "b.exe");
        let mut covered = game("c", &games, "a.exe");
        covered.art_cover = Some("https://x/cover.jpg".into());
        let native = game("d", &games, "start.sh");
        let list: Vec<Arc<GameInfo>> = [&a, &b, &covered, &native]
            .into_iter()
            .map(|g| Arc::new(g.clone()))
            .collect();

        assert!(icons.refresh(&list));
        let mut shown = a.clone();
        icons.complete(&mut shown);
        let url = shown.program_icon.clone().unwrap();
        let path = url.strip_prefix("file://").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"png of the icon");
        for g in [&b, &covered, &native] {
            let mut g = g.clone();
            icons.complete(&mut g);
            assert_eq!(g.program_icon, None, "{}", g.app_name);
        }

        // Nothing changed: nothing is read again, no rebuild is asked for.
        assert!(!icons.refresh(&list));
        // The index survives a restart.
        let again = ProgramIcons::under(dir.path().join("cache"), |_| None);
        let mut shown = a.clone();
        again.complete(&mut shown);
        assert_eq!(shown.program_icon.as_deref(), Some(url.as_str()));

        // A game that starts another program no longer shows the old icon.
        let mut moved = game("a", &games, "b.exe");
        icons.complete(&mut moved);
        assert_eq!(moved.program_icon, None);
    }

    #[test]
    fn a_changed_program_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.exe"), b"plain").unwrap();
        let icons = ProgramIcons::under(dir.path().join("cache"), icon_png);
        let a = game("a", dir.path(), "a.exe");
        let list = vec![Arc::new(a.clone())];
        assert!(!icons.refresh(&list), "no icon: covers stay as they are");
        std::fs::write(dir.path().join("a.exe"), b"MZ a longer program").unwrap();
        assert!(icons.refresh(&list));
        let mut shown = a;
        icons.complete(&mut shown);
        assert!(shown.program_icon.is_some());
    }
}
