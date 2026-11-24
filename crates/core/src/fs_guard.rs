//! Guard for recursive deletions of game, prefix and download folders. Every
//! crate that deletes a folder chosen by the user or recorded by a store
//! checks it here first, so one bad path (a library root, the home folder)
//! can never wipe unrelated data. Folders other launchers keep games and
//! prefixes in ([`OtherLaunchers`]) are never deleted either, nor a folder
//! holding one.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::{Paths, expand_home};
use crate::settings::AppSettings;

/// System directories whose contents are never deleted.
const SYSTEM_ROOTS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/root", "/run", "/sbin",
    "/snap", "/sys", "/usr", "/var",
];

fn path_contains(parent: &Path, child: &Path) -> bool {
    if cfg!(windows) {
        let normalize = |p: &Path| {
            p.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_lowercase()
        };
        Path::new(&normalize(child)).starts_with(normalize(parent))
    } else {
        child.starts_with(parent)
    }
}

/// Refuse to delete `dir` unless it is clearly a single game's (or prefix's)
/// own folder: an absolute path at least two levels deep, not the home
/// directory, one of its ancestors or direct children (`~/Downloads`,
/// `~/Games`…), not inside a system directory, and not containing any
/// `protected` path.
pub fn check_deletable(dir: &Path, home: Option<&Path>, protected: &[PathBuf]) -> Result<()> {
    let refuse = |why: &str| -> Result<()> {
        bail!(
            "Refusing to delete {} because {why}. Delete the files manually if you really want to.",
            dir.display()
        )
    };
    if !dir.is_absolute() || dir.components().any(|c| c == Component::ParentDir) {
        return refuse("it is not a plain full path");
    }
    let depth = dir
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count();
    if depth < 2 {
        return refuse("it is a top-level folder");
    }
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let in_temp = resolved.starts_with(&temp) && resolved != temp;
    let mac_system = cfg!(target_os = "macos")
        && ["/System", "/Library", "/private"]
            .iter()
            .any(|r| resolved.starts_with(r));
    let windows_system = cfg!(windows)
        && ["SystemRoot", "WINDIR", "ProgramData"]
            .iter()
            .filter_map(std::env::var_os)
            .any(|r| path_contains(Path::new(&r), &resolved));
    if !in_temp
        && (SYSTEM_ROOTS.iter().any(|root| resolved.starts_with(root))
            || mac_system
            || windows_system)
    {
        return refuse("it is a system folder");
    }
    if let Some(home) = home {
        if path_contains(dir, home) {
            return refuse("it contains your home folder");
        }
        if dir
            .parent()
            .is_some_and(|parent| path_contains(parent, home) && path_contains(home, parent))
        {
            return refuse("it is a folder directly inside your home folder");
        }
    }
    if protected.iter().any(|p| path_contains(dir, p)) {
        return refuse("it contains launcher or library folders");
    }
    Ok(())
}

/// Folders that must survive any deletion: Irori's own directories and the
/// configured library, prefix and Wine build roots.
pub fn protected_dirs(app: &AppSettings) -> Vec<PathBuf> {
    let p = Paths::get();
    let mut dirs = vec![p.config.clone(), p.data.clone(), p.cache.clone()];
    for s in [
        &app.default_install_path,
        &app.default_wine_prefix_dir,
        &app.compat_tools_path,
    ] {
        if !s.is_empty() {
            dirs.push(expand_home(s));
        }
    }
    dirs
}

/// [`check_deletable`] with the real home folder and [`protected_dirs`],
/// then [`check_other_launchers`] with the folders other launchers keep,
/// resolving symlinks first so a link cannot smuggle a protected folder in.
pub fn ensure_deletable(dir: &Path, app: &AppSettings) -> Result<()> {
    let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let home = dirs::home_dir().map(|h| std::fs::canonicalize(&h).unwrap_or(h));
    let protected: Vec<PathBuf> = protected_dirs(app)
        .into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    check_deletable(&resolved, home.as_deref(), &protected)?;
    check_other_launchers(&resolved, &OtherLaunchers::load().resolved())
}

/// Folders other launchers (Lutris, Bottles, Faugus Launcher) keep games and
/// prefixes in, with the launcher's name, read from their own data (only
/// read: the imports and the discovery in services never write there). Kept
/// in `state/other-launchers.json`, so every crate's deletion sees them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherLaunchers {
    /// What each launcher listed when it was last read, by launcher.
    #[serde(default)]
    pub found: BTreeMap<String, Vec<PathBuf>>,
    /// Folders of games imported from a launcher, kept when the launcher
    /// later forgets them: the files are still the ones it installed.
    #[serde(default)]
    pub imported: BTreeMap<PathBuf, String>,
}

/// Serializes the file's read-modify-write.
static OTHER_LAUNCHERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl OtherLaunchers {
    fn path() -> PathBuf {
        Paths::get().state_dir().join("other-launchers.json")
    }

    /// The recorded folders (nothing when none were recorded).
    pub fn load() -> Self {
        crate::json_store::read_or_default(&Self::path())
    }

    /// Replace what `launcher` lists with `dirs`.
    pub fn set_found(launcher: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        let dirs: Vec<PathBuf> = keepable(dirs);
        Self::change(|o| {
            if dirs.is_empty() {
                o.found.remove(launcher);
            } else {
                o.found.insert(launcher.to_string(), dirs);
            }
        })
    }

    /// Record the folders of games imported from `launcher`.
    pub fn note_imported(launcher: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        let dirs = keepable(dirs);
        if dirs.is_empty() {
            return Ok(());
        }
        Self::change(|o| {
            for dir in dirs {
                o.imported.insert(dir, launcher.to_string());
            }
        })
    }

    fn change(f: impl FnOnce(&mut Self)) -> Result<()> {
        let _guard = OTHER_LAUNCHERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut this = Self::load();
        let before = this.clone();
        f(&mut this);
        if this == before {
            return Ok(());
        }
        crate::json_store::write(&Self::path(), &this)
    }

    /// Every folder with its launcher, symlinks resolved.
    pub fn resolved(&self) -> Vec<(PathBuf, String)> {
        let found = self
            .found
            .iter()
            .flat_map(|(launcher, dirs)| dirs.iter().map(move |d| (d, launcher)));
        found
            .chain(self.imported.iter())
            .map(|(dir, launcher)| {
                (
                    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone()),
                    launcher.clone(),
                )
            })
            .collect()
    }
}

/// The folders worth recording: a launcher's entry pointing at the home
/// folder, a top-level or system folder would otherwise keep Irori from
/// deleting anything there.
fn keepable(dirs: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let mut out: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|d| check_deletable(d, home.as_deref(), &[]).is_ok())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Refuse to delete `dir` when another launcher keeps it or a folder inside
/// it. A folder inside one stays deletable: a launcher's game set up in a
/// whole library folder (`/mnt/games`) must not lock Irori's own games
/// there.
pub fn check_other_launchers(dir: &Path, others: &[(PathBuf, String)]) -> Result<()> {
    if let Some((folder, launcher)) = others.iter().find(|(folder, _)| path_contains(dir, folder)) {
        bail!(
            "Refusing to delete {} because {launcher} keeps a game or prefix in {}. Remove it in {launcher} instead.",
            dir.display(),
            folder.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn refuses_windows_roots_and_case_variants() {
        let home = Path::new(r"C:\Users\Player");
        let protected = vec![PathBuf::from(r"D:\Games\Irori")];
        for bad in [
            r"C:\",
            r"C:\Users",
            r"c:\users\player",
            r"C:\USERS\PLAYER\Downloads",
            r"D:\Games\Irori",
            r"C:\Users\Player\x\..\Games",
        ] {
            assert!(
                check_deletable(Path::new(bad), Some(home), &protected).is_err(),
                "{bad}"
            );
        }
        assert!(
            check_deletable(Path::new(r"D:\Games\Irori\Hades"), Some(home), &protected).is_ok()
        );
        let windows = std::env::var_os("SystemRoot").unwrap();
        assert!(
            check_deletable(&PathBuf::from(windows).join("System32"), Some(home), &[]).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn other_launchers_folders_and_whats_inside_them_stay() {
        let others = vec![
            (PathBuf::from("/home/u/Games/witcher"), "Lutris".to_string()),
            (
                PathBuf::from("/home/u/.local/share/bottles/bottles/Gaming"),
                "Bottles".to_string(),
            ),
        ];
        let check = |p: &str| check_other_launchers(Path::new(p), &others);
        for bad in [
            "/home/u/Games/witcher",
            "/home/u/.local/share/bottles/bottles",
            "/home/u/.local/share/bottles/bottles/Gaming",
        ] {
            let e = check(bad).unwrap_err().to_string();
            assert!(e.contains("Lutris") || e.contains("Bottles"), "{bad}: {e}");
        }
        for ok in [
            "/home/u/Games/witcher2",
            "/home/u/Games/Irori/Hades",
            "/home/u/Games/witcher/shadercache",
        ] {
            assert!(check(ok).is_ok(), "{ok} should be allowed");
        }
    }

    #[test]
    fn launchers_pointing_at_broad_folders_are_not_recorded() {
        let home = dirs::home_dir().unwrap();
        let kept = keepable([
            home.clone(),
            home.join("Games"),
            PathBuf::from("/"),
            home.join("Games/witcher"),
            home.join("Games/witcher"),
        ]);
        assert_eq!(kept, [home.join("Games/witcher")]);
        let text = r#"{"found":{"Lutris":["/a/b"]},"imported":{"/c/d":"Bottles"},"later":1}"#;
        let o: OtherLaunchers = serde_json::from_str(text).unwrap();
        assert_eq!(o.found["Lutris"], [PathBuf::from("/a/b")]);
        assert_eq!(o.imported[Path::new("/c/d")], "Bottles");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_broad_folders() {
        let home = Path::new("/home/u");
        let protected = vec![
            PathBuf::from("/home/u/.config/irori"),
            PathBuf::from("/home/u/Games/Irori"),
        ];
        let check = |p: &str| check_deletable(Path::new(p), Some(home), &protected);
        for bad in [
            "/",
            "/games",
            "/home",
            "/home/u",
            "/home/u/Games",
            "/home/u/Downloads",
            "/home/u/Games/Irori",
            "/usr/share/x",
            "relative/dir",
            "/home/u/a/../b",
        ] {
            assert!(check(bad).is_err(), "{bad} should be refused");
        }
        for ok in [
            "/home/u/Games/Irori/Hades",
            "/mnt/disk/games/Hades",
            "/home/u/Games/Other/x",
        ] {
            assert!(check(ok).is_ok(), "{ok} should be allowed");
        }
    }
}
