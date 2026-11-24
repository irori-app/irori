//! Restoring a backup into the game's Wine prefix as it is now.
//!
//! `ludusavi restore` writes each file back to the absolute path it was
//! backed up from, which is wrong whenever the prefix moved: a reinstall in
//! another library, a recreated prefix, another Wine user name, or a backup
//! made on Windows. Ludusavi has no `--wine-prefix` for restores, but it
//! reads redirects from its configuration, so a restore runs with a copy of
//! the user's configuration (`--config`, a throwaway folder) holding
//! redirects into the current prefix. Ludusavi's own configuration is never
//! written.
//!
//! [`plan`] decides, from the paths a preview reports, where each file goes:
//! - the prefix holding most of the files moves into the game's current one
//!   (its Wine user renamed to the current one); a stale copy from another
//!   prefix is left out rather than written over the real one;
//! - Windows paths (a backup made on Windows) go into the current prefix's
//!   `drive_c`, or are left out when the game has no prefix;
//! - the prefix's registry files (`user.reg`, `system.reg`) are left out
//!   unless asked for: restoring them rolls back every program in the
//!   prefix, not just the game.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a file of a backup is, for the restore preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    /// A save of the game.
    Save,
    /// A Wine prefix's registry file.
    Registry,
    /// A file of another Wine prefix than the one restored into.
    OtherPrefix,
    /// A Windows path with no place on this computer.
    NoPlace,
}

/// One file of a restore, with where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Where it was backed up from, as Ludusavi reports it.
    pub original: String,
    /// Where it is written.
    pub target: String,
    pub bytes: u64,
    pub kind: FileKind,
    pub included: bool,
}

/// A path prefix Ludusavi rewrites when restoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub source: String,
    pub target: String,
}

/// Where a restore writes, decided before it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Longest source first, so a Wine user's folder is matched before its
    /// prefix.
    pub redirects: Vec<Redirect>,
    pub files: Vec<PlannedFile>,
    /// The prefix the saves came from, when they move into another one.
    pub moved_from: Option<String>,
}

impl Plan {
    /// The files written.
    pub fn included(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files.iter().filter(|f| f.included)
    }

    /// Files of `kind` left out.
    pub fn left_out(&self, kind: FileKind) -> usize {
        self.files
            .iter()
            .filter(|f| !f.included && f.kind == kind)
            .count()
    }

    /// The paths to switch off in Ludusavi's `restore.toggledPaths`: both
    /// the original and the redirected path of every file left out, so the
    /// toggle holds whichever Ludusavi compares.
    pub fn toggled_off(&self) -> BTreeSet<String> {
        self.files
            .iter()
            .filter(|f| !f.included)
            .flat_map(|f| [f.original.clone(), f.target.clone()])
            .collect()
    }
}

/// `path` with forward slashes and no trailing one, as Ludusavi writes
/// paths.
fn norm(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_end_matches('/').to_string()
}

/// The prefix a path lies in (everything before its `drive_c`), the Wine
/// user it belongs to (`drive_c/users/<user>`), and the rest after
/// `drive_c`.
fn in_prefix(path: &str) -> Option<(String, Option<String>, String)> {
    let parts: Vec<&str> = path.split('/').collect();
    let at = parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case("drive_c"))?;
    let prefix = parts[..at].join("/");
    let rest = parts[at + 1..].join("/");
    let user = match parts.get(at + 1..at + 3) {
        Some([users, name]) if users.eq_ignore_ascii_case("users") => Some(name.to_string()),
        _ => None,
    };
    Some((prefix, user, rest))
}

/// A Windows drive path (`C:/…`): its drive letter and the rest.
fn windows_path(path: &str) -> Option<(char, &str)> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (path.get(1..3) == Some(":/")).then(|| (letter.to_ascii_lowercase(), &path[3..]))
}

/// Whether `rest` (a path after `drive_c`'s parent) is a registry file at
/// the prefix's root.
fn is_registry_file(prefix_rest: &str) -> bool {
    matches!(
        prefix_rest.to_ascii_lowercase().as_str(),
        "user.reg" | "system.reg" | "userdef.reg"
    )
}

/// Rewrite `path` with the first redirect whose source it starts with (at a
/// folder boundary).
pub fn redirect(path: &str, redirects: &[Redirect]) -> String {
    for r in redirects {
        if path == r.source {
            return r.target.clone();
        }
        if let Some(rest) = path.strip_prefix(&r.source)
            && rest.starts_with('/')
        {
            return format!("{}{rest}", r.target);
        }
    }
    path.to_string()
}

/// Where each of `files` (path and size, as a preview reports them) goes
/// when restored into `current` (the game's Wine prefix, the folder holding
/// `drive_c`; `None` for a native game) whose Wine user is `user`.
/// `windows_host`: this is Windows, where every path is already in place.
/// `registry`: restore the prefix's registry files too.
pub fn plan(
    files: &[(String, u64)],
    current: Option<&Path>,
    user: Option<&str>,
    windows_host: bool,
    registry: bool,
) -> Plan {
    let files: Vec<(String, u64)> = files.iter().map(|(p, b)| (norm(p), *b)).collect();
    if windows_host {
        return Plan {
            files: files
                .into_iter()
                .map(|(p, bytes)| PlannedFile {
                    target: p.clone(),
                    original: p,
                    bytes,
                    kind: FileKind::Save,
                    included: true,
                })
                .collect(),
            ..Default::default()
        };
    }
    let current = current.map(|c| norm(&c.to_string_lossy()));

    // The prefix most files came from, and its most common Wine user.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut prefixes = Vec::new();
    for (p, _) in &files {
        prefixes.push(in_prefix(p));
    }
    for (prefix, _, _) in prefixes.iter().flatten() {
        *counts.entry(prefix.clone()).or_default() += 1;
    }
    // A registry file sits at its prefix's root, beside `drive_c`.
    let prefixes: Vec<Option<(String, Option<String>, String)>> = files
        .iter()
        .zip(prefixes)
        .map(|((p, _), place)| {
            place.or_else(|| {
                let (parent, name) = p.rsplit_once('/')?;
                is_registry_file(name).then(|| (parent.to_string(), None, name.to_string()))
            })
        })
        .collect();
    let dominant = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(p, _)| p.clone());
    let mut users: BTreeMap<&str, usize> = BTreeMap::new();
    for (prefix, u, _) in prefixes.iter().flatten() {
        if Some(prefix) == dominant.as_ref()
            && let Some(u) = u
        {
            *users.entry(u.as_str()).or_default() += 1;
        }
    }
    let old_user = users
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(u, _)| u.to_string());

    let mut redirects = Vec::new();
    let mut moved_from = None;
    if let Some(cur) = &current {
        if let Some(dom) = &dominant {
            if let (Some(old), Some(new)) = (&old_user, user)
                && (old != new || dom != cur)
            {
                redirects.push(Redirect {
                    source: format!("{dom}/drive_c/users/{old}"),
                    target: format!("{cur}/drive_c/users/{new}"),
                });
            }
            if dom != cur {
                redirects.push(Redirect {
                    source: dom.clone(),
                    target: cur.clone(),
                });
                moved_from = Some(dom.clone());
            }
        }
        // A backup made on Windows: its user folder and its C: drive.
        let mut windows_users = BTreeSet::new();
        for (p, _) in &files {
            if let Some(('c', rest)) = windows_path(p) {
                let mut parts = rest.split('/');
                if let (Some(users), Some(name)) = (parts.next(), parts.next())
                    && users.eq_ignore_ascii_case("users")
                {
                    windows_users.insert(format!("{}/{name}", &p[..2 + users.len() + 1]));
                }
            }
        }
        if let Some(new) = user {
            for source in windows_users {
                redirects.push(Redirect {
                    source,
                    target: format!("{cur}/drive_c/users/{new}"),
                });
            }
        }
        let drives: BTreeSet<char> = files
            .iter()
            .filter_map(|(p, _)| windows_path(p))
            .map(|(d, _)| d)
            .collect();
        for drive in drives {
            redirects.push(Redirect {
                source: format!("{}:", drive.to_ascii_uppercase()),
                target: format!("{cur}/drive_{drive}"),
            });
        }
    }
    redirects.sort_by_key(|r| std::cmp::Reverse(r.source.len()));

    let planned = files
        .into_iter()
        .zip(prefixes)
        .map(|((original, bytes), place)| {
            let kind = match (&place, windows_path(&original)) {
                (Some((prefix, _, _)), _) if Some(prefix) != dominant.as_ref() => {
                    FileKind::OtherPrefix
                }
                (Some((prefix, None, rest)), _)
                    if prefix.len() + 1 + rest.len() == original.len()
                        && is_registry_file(rest) =>
                {
                    FileKind::Registry
                }
                (None, Some(_)) if current.is_none() => FileKind::NoPlace,
                _ => FileKind::Save,
            };
            let included = match kind {
                FileKind::Save => true,
                FileKind::Registry => registry,
                FileKind::OtherPrefix | FileKind::NoPlace => false,
            };
            let target = if kind == FileKind::OtherPrefix {
                original.clone()
            } else {
                redirect(&original, &redirects)
            };
            PlannedFile {
                original,
                target,
                bytes,
                kind,
                included,
            }
        })
        .collect();
    Plan {
        redirects,
        files: planned,
        moved_from,
    }
}

/// The Wine user of a prefix (the folder holding `drive_c`): `steamuser`
/// when Proton made it, else its only user, else this computer's user.
pub fn wine_user(prefix: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(prefix.join("drive_c").join("users"))
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.eq_ignore_ascii_case("public"))
        .collect();
    names.sort();
    if names.iter().any(|n| n == "steamuser") {
        return Some("steamuser".into());
    }
    if names.len() == 1 {
        return names.pop();
    }
    let me = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok();
    me.filter(|m| names.contains(m))
        .or_else(|| names.into_iter().next())
}

/// Ludusavi's configuration `config` (its `config.yaml`, read as YAML) with
/// the plan's redirects added before the user's own and the left-out files
/// of `title` switched off.
pub fn configure(config: &str, title: &str, plan: &Plan) -> anyhow::Result<String> {
    use serde_yaml_ng::{Mapping, Value};
    let mut root: Value = if config.trim().is_empty() {
        Value::Mapping(Mapping::new())
    } else {
        serde_yaml_ng::from_str(config)?
    };
    let root_map = root
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Ludusavi’s settings aren’t what Irori expects"))?;
    let mut redirects: Vec<Value> = plan
        .redirects
        .iter()
        .map(|r| {
            let mut m = Mapping::new();
            m.insert("kind".into(), "restore".into());
            m.insert("source".into(), r.source.clone().into());
            m.insert("target".into(), r.target.clone().into());
            Value::Mapping(m)
        })
        .collect();
    if let Some(Value::Sequence(own)) = root_map.get("redirects") {
        redirects.extend(own.iter().cloned());
    }
    root_map.insert("redirects".into(), Value::Sequence(redirects));

    let restore = root_map
        .entry("restore".into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !restore.is_mapping() {
        *restore = Value::Mapping(Mapping::new());
    }
    let restore = restore.as_mapping_mut().expect("a mapping");
    let toggled = restore
        .entry("toggledPaths".into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !toggled.is_mapping() {
        *toggled = Value::Mapping(Mapping::new());
    }
    let toggled = toggled.as_mapping_mut().expect("a mapping");
    let game = toggled
        .entry(title.into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !game.is_mapping() {
        *game = Value::Mapping(Mapping::new());
    }
    let game = game.as_mapping_mut().expect("a mapping");
    for path in plan.toggled_off() {
        game.insert(path.into(), false.into());
    }
    Ok(serde_yaml_ng::to_string(&root)?)
}

/// The backup folder named in Ludusavi's configuration (`backup.path`).
pub fn backup_path(config: &str) -> Option<std::path::PathBuf> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_str(config).ok()?;
    let path = root.get("backup")?.get("path")?.as_str()?.trim();
    (!path.is_empty()).then(|| irori_core::paths::expand_home(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<(String, u64)> {
        paths.iter().map(|p| (p.to_string(), 10)).collect()
    }

    #[test]
    fn moves_the_saves_of_the_main_prefix_into_the_current_one() {
        let backup = files(&[
            "/old/Stalker 2/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/a.sav",
            "/old/Stalker 2/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/b.sav",
            "/old/Stalker 2/pfx/user.reg",
            "/ancient/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/a.sav",
            "/home/me/.config/stalker/settings.ini",
        ]);
        let p = plan(
            &backup,
            Some(Path::new("/games/Stalker 2/pfx")),
            Some("steamuser"),
            false,
            false,
        );
        assert_eq!(p.moved_from.as_deref(), Some("/old/Stalker 2/pfx"));
        let targets: Vec<(&str, FileKind, bool)> = p
            .files
            .iter()
            .map(|f| (f.target.as_str(), f.kind, f.included))
            .collect();
        assert_eq!(
            targets,
            [
                (
                    "/games/Stalker 2/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/a.sav",
                    FileKind::Save,
                    true
                ),
                (
                    "/games/Stalker 2/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/b.sav",
                    FileKind::Save,
                    true
                ),
                ("/games/Stalker 2/pfx/user.reg", FileKind::Registry, false),
                (
                    "/ancient/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/a.sav",
                    FileKind::OtherPrefix,
                    false
                ),
                (
                    "/home/me/.config/stalker/settings.ini",
                    FileKind::Save,
                    true
                ),
            ]
        );
        assert_eq!(p.left_out(FileKind::Registry), 1);
        assert_eq!(p.left_out(FileKind::OtherPrefix), 1);
        // Asked for, the registry comes along.
        let p = plan(
            &backup,
            Some(Path::new("/games/Stalker 2/pfx")),
            Some("steamuser"),
            false,
            true,
        );
        assert_eq!(p.left_out(FileKind::Registry), 0);
        let off = p.toggled_off();
        assert!(off.contains("/ancient/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/a.sav"));
    }

    #[test]
    fn renames_the_wine_user_and_places_windows_paths() {
        let p = plan(
            &files(&["/p/drive_c/users/alice/Saved Games/x.sav"]),
            Some(Path::new("/p")),
            Some("bob"),
            false,
            false,
        );
        assert_eq!(p.files[0].target, "/p/drive_c/users/bob/Saved Games/x.sav");
        assert_eq!(p.moved_from, None);

        // Made on Windows.
        let backup = files(&[
            "C:/Users/Alice/AppData/Roaming/Game/save.dat",
            "C:/ProgramData/Game/global.dat",
        ]);
        let p = plan(
            &backup,
            Some(Path::new("/q/pfx")),
            Some("steamuser"),
            false,
            false,
        );
        assert_eq!(
            p.files[0].target,
            "/q/pfx/drive_c/users/steamuser/AppData/Roaming/Game/save.dat"
        );
        assert_eq!(
            p.files[1].target,
            "/q/pfx/drive_c/ProgramData/Game/global.dat"
        );
        assert!(p.files.iter().all(|f| f.included));
        // A native game has no place for them.
        let p = plan(&backup, None, None, false, false);
        assert_eq!(p.left_out(FileKind::NoPlace), 2);
        // On Windows they are already in place.
        let p = plan(&backup, None, None, true, false);
        assert!(p.redirects.is_empty() && p.files.iter().all(|f| f.included));
    }

    #[test]
    fn redirects_only_at_folder_boundaries() {
        let r = [Redirect {
            source: "/a/pfx".into(),
            target: "/b/pfx".into(),
        }];
        assert_eq!(redirect("/a/pfx/drive_c/x", &r), "/b/pfx/drive_c/x");
        assert_eq!(redirect("/a/pfx2/drive_c/x", &r), "/a/pfx2/drive_c/x");
    }

    #[test]
    fn adds_redirects_and_toggles_to_a_copy_of_ludusavis_settings() {
        let p = plan(
            &files(&["/old/pfx/drive_c/users/u/a.sav", "/old/pfx/system.reg"]),
            Some(Path::new("/new/pfx")),
            Some("u"),
            false,
            false,
        );
        let own = "manifest:\n  url: https://example.invalid/m.yaml\nredirects:\n  - kind: bidirectional\n    source: /x\n    target: /y\nrestore:\n  path: /backups\n  sort:\n    key: name\n";
        let out = configure(own, "Game", &p).unwrap();
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&out).unwrap();
        let redirects = v["redirects"].as_sequence().unwrap();
        assert_eq!(
            redirects[0]["source"].as_str(),
            Some("/old/pfx/drive_c/users/u")
        );
        assert_eq!(redirects[0]["kind"].as_str(), Some("restore"));
        assert_eq!(redirects.last().unwrap()["source"].as_str(), Some("/x"));
        assert_eq!(v["restore"]["path"].as_str(), Some("/backups"));
        assert_eq!(
            v["manifest"]["url"].as_str(),
            Some("https://example.invalid/m.yaml")
        );
        assert_eq!(
            v["restore"]["toggledPaths"]["Game"]["/new/pfx/system.reg"].as_bool(),
            Some(false)
        );
        assert_eq!(
            v["restore"]["toggledPaths"]["Game"]["/old/pfx/system.reg"].as_bool(),
            Some(false)
        );
        // No settings yet.
        assert!(configure("", "Game", &p).unwrap().contains("redirects"));
    }

    #[test]
    fn reads_the_backup_folder_from_ludusavis_settings() {
        let config = "manifest:\n  url: x\nbackup:\n  path: /games/ludusavi-backup\n";
        assert_eq!(
            backup_path(config),
            Some(std::path::PathBuf::from("/games/ludusavi-backup"))
        );
        assert_eq!(backup_path("backup: {}\n"), None);
        assert_eq!(backup_path("{not yaml"), None);
    }

    #[test]
    fn finds_the_wine_user_of_a_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let users = dir.path().join("drive_c/users");
        std::fs::create_dir_all(users.join("Public")).unwrap();
        std::fs::create_dir_all(users.join("alice")).unwrap();
        assert_eq!(wine_user(dir.path()).as_deref(), Some("alice"));
        std::fs::create_dir_all(users.join("steamuser")).unwrap();
        assert_eq!(wine_user(dir.path()).as_deref(), Some("steamuser"));
        assert_eq!(wine_user(&dir.path().join("nothing")), None);
    }
}
