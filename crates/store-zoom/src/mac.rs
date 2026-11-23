//! Mac builds from ZOOM Platform. A disk image is mounted read-only with
//! macOS's `hdiutil` (never shown in Finder, never opened) and what it holds
//! is copied out with `ditto`, which keeps an app bundle's links, modes and
//! extended attributes; zips and tarballs go through the native extractor.
//! The game is the `.app` found in the install folder afterwards, and it is
//! launched as that bundle's own executable.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

/// Apps in a game's folder that are never the game.
static NOT_GAME_APP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(un)?install|setup|uninstall|crash ?report|launcher ?update")
        .expect("app filter regex")
});

/// How deep below the install folder an app is looked for.
const APP_DEPTH: usize = 3;

/// Whether a downloaded file is a disk image.
pub fn is_disk_image(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dmg"))
}

/// Whether a downloaded file is an installer package, which would run
/// scripts as the system installer: Irori doesn't open those.
pub fn is_installer_package(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pkg") || e.eq_ignore_ascii_case("mpkg"))
}

fn is_app(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// What to copy out of a mounted disk image at `mount`: its apps when it has
/// any at the top, else everything visible (a game folder). Hidden files,
/// the volume's own folders and links (the usual one to `/Applications`)
/// stay behind.
pub fn disk_image_items(mount: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(mount) else {
        return Vec::new();
    };
    let mut visible: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
        .map(|e| e.path())
        .collect();
    visible.sort();
    let apps: Vec<PathBuf> = visible
        .iter()
        .filter(|p| is_app(p) && p.is_dir())
        .cloned()
        .collect();
    if apps.is_empty() { visible } else { apps }
}

fn folded(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The game's app in `root`, relative to it: among the shallowest apps that
/// aren't installers or updaters, the one named like `title`, else the
/// first. Apps inside apps (helpers) don't count. Blocking.
pub fn app_bundle(root: &Path, title: &str) -> Option<String> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<(usize, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = e.path();
            if is_app(&path) {
                out.push((depth, path));
            } else if depth < APP_DEPTH {
                walk(&path, depth + 1, out);
            }
        }
    }
    let mut apps = Vec::new();
    walk(root, 0, &mut apps);
    apps.retain(|(_, p)| {
        p.file_stem()
            .is_some_and(|n| !NOT_GAME_APP.is_match(&n.to_string_lossy()))
    });
    let shallowest = apps.iter().map(|(d, _)| *d).min()?;
    let candidates: Vec<&PathBuf> = apps
        .iter()
        .filter(|(d, _)| *d == shallowest)
        .map(|(_, p)| p)
        .collect();
    let wanted = folded(title);
    let named = candidates.iter().find(|p| {
        !wanted.is_empty()
            && p.file_stem()
                .is_some_and(|n| folded(&n.to_string_lossy()).contains(&wanted))
    });
    let chosen = named.or(candidates.first())?;
    Some(
        chosen
            .strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn app(root: &Path, rel: &str) {
        std::fs::create_dir_all(root.join(rel).join("Contents/MacOS")).unwrap();
    }

    // macOS app bundles, named with '/'.
    #[cfg(unix)]
    #[test]
    fn finds_the_games_app() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(app_bundle(root, "Anything"), None);

        app(root, "Extras/Uninstall.app");
        app(root, "Game/Hero Quest.app");
        app(root, "Game/Hero Quest.app/Contents/Helpers/Crash.app");
        app(root, "Game/Map Editor.app");
        assert_eq!(
            app_bundle(root, "Hero Quest").as_deref(),
            Some("Game/Hero Quest.app")
        );
        // Unknown title: the first of the shallowest.
        assert_eq!(
            app_bundle(root, "Something Else").as_deref(),
            Some("Game/Hero Quest.app")
        );
        // A shallower app wins over deeper ones.
        app(root, "Launch.app");
        assert_eq!(
            app_bundle(root, "Hero Quest").as_deref(),
            Some("Launch.app")
        );
    }

    #[test]
    fn takes_apps_out_of_a_disk_image_or_else_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let mount = dir.path();
        std::fs::create_dir_all(mount.join(".background")).unwrap();
        std::fs::write(mount.join(".DS_Store"), b"").unwrap();
        std::fs::create_dir_all(mount.join("Game Folder")).unwrap();
        std::fs::write(mount.join("Readme.txt"), b"hi").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/Applications", mount.join("Applications")).unwrap();
        assert_eq!(
            disk_image_items(mount),
            [mount.join("Game Folder"), mount.join("Readme.txt")]
        );
        app(mount, "Game.app");
        assert_eq!(disk_image_items(mount), [mount.join("Game.app")]);
    }

    #[test]
    fn tells_disk_images_and_packages_apart() {
        assert!(is_disk_image(Path::new("/d/Game_1.2.DMG")));
        assert!(!is_disk_image(Path::new("/d/game.zip")));
        assert!(is_installer_package(Path::new("/d/Game.pkg")));
        assert!(!is_installer_package(Path::new("/d/Game.dmg")));
    }
}
