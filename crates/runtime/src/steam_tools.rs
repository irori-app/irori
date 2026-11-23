//! The Wine manager's Proton builds offered to Steam as compatibility tools.
//!
//! With "Install Proton downloads into Steam" on, new downloads go straight
//! into Steam's `compatibilitytools.d`. Builds Irori installed into its own
//! folder (before the setting was on, or while Steam wasn't found) are
//! registered instead: a folder `irori-<build>` in `compatibilitytools.d`
//! holding only a `compatibilitytool.vdf` whose `install_path` points at the
//! build, so nothing is copied twice. Each folder Irori writes carries a
//! marker file; only marked folders are ever changed or removed, so tools
//! the user or ProtonUp-Qt installed are never touched. A registration
//! leaves with its build, and all of them when the setting is turned off.
//!
//! Steam reads the folder when it starts, so Steam need not be closed; a
//! running Steam lists new builds after its next start. A Flatpak or Snap
//! Steam can't read Irori's folders from its sandbox, so nothing is
//! registered there.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::{AppSettings, Paths};

use crate::locations;

/// The start of every folder Irori registers.
pub const FOLDER_PREFIX: &str = "irori-";
/// The file marking a registration as Irori's.
const MARKER: &str = ".irori-registration";

/// One build offered to Steam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// The folder in `compatibilitytools.d` (`irori-GE-Proton10-20`).
    pub folder: String,
    /// The tool's name in Steam's settings: the build's own folder name,
    /// which stays the same across updates of a `-latest` build.
    pub name: String,
    /// The build's folder (holding `proton`).
    pub install_path: PathBuf,
}

/// What a sync changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// The registrations `builds` (name, folder) need in a `compatibilitytools.d`
/// that already holds `present` (folder names of tools that aren't Irori's
/// registrations). A build already among them (installed into Steam
/// directly, or by another tool under the same name) is left out, so Steam
/// never lists one name twice. Names that can't be a folder are left out.
pub fn wanted(builds: &[(String, PathBuf)], present: &HashSet<String>) -> Vec<Registration> {
    let mut seen = HashSet::new();
    builds
        .iter()
        .filter(|(name, _)| irori_core::paths::sanitize(name) == *name && !name.is_empty())
        .filter(|(name, _)| !present.contains(name))
        .filter(|(name, _)| seen.insert(name.clone()))
        .map(|(name, path)| Registration {
            folder: format!("{FOLDER_PREFIX}{name}"),
            name: name.clone(),
            install_path: path.clone(),
        })
        .collect()
}

/// A VDF string literal: backslashes and quotes escaped.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The `compatibilitytool.vdf` of a registration.
pub fn tool_vdf(reg: &Registration) -> String {
    format!(
        "\"compatibilitytools\"\n{{\n  \"compat_tools\"\n  {{\n    {name}\n    {{\n      \"install_path\" {path}\n      \"display_name\" {display}\n      \"from_oslist\" \"windows\"\n      \"to_oslist\" \"linux\"\n    }}\n  }}\n}}\n",
        name = quoted(&reg.name),
        path = quoted(&reg.install_path.to_string_lossy()),
        display = quoted(&format!("{} (Irori)", reg.name)),
    )
}

/// Whether `dir` is a registration Irori wrote.
fn is_ours(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(FOLDER_PREFIX))
        && dir.join(MARKER).is_file()
}

/// Make `tools_dir` hold exactly `wanted` among Irori's registrations:
/// write the missing or changed ones, remove Irori's others. Blocking.
pub fn apply(tools_dir: &Path, wanted: &[Registration], app: &AppSettings) -> Result<Changes> {
    let mut changes = Changes::default();
    if let Ok(entries) = std::fs::read_dir(tools_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_ours(&path) && !wanted.iter().any(|w| w.folder == name) {
                irori_core::fs_guard::ensure_deletable(&path, app)
                    .with_context(|| format!("removing {}", path.display()))?;
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                changes.removed.push(name);
            }
        }
    }
    for reg in wanted {
        let dir = tools_dir.join(&reg.folder);
        let file = dir.join("compatibilitytool.vdf");
        let text = tool_vdf(reg);
        // A folder of that name the user made is theirs.
        if dir.exists() && !is_ours(&dir) {
            continue;
        }
        if std::fs::read_to_string(&file).is_ok_and(|t| t == text) {
            continue;
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(
            dir.join(MARKER),
            b"Written by Irori; removed with its build.\n",
        )
        .with_context(|| format!("writing {}", dir.display()))?;
        irori_core::json_store::write_bytes(&file, text.as_bytes())
            .with_context(|| format!("writing {}", file.display()))?;
        changes.added.push(reg.folder.clone());
    }
    changes.removed.sort();
    Ok(changes)
}

/// The Proton builds in Irori's own folders: name and folder.
fn irori_builds(app: &AppSettings) -> Vec<(String, PathBuf)> {
    let mut dirs = vec![
        Paths::get().proton_dir(),
        locations::proton_downloads_dir(app),
    ];
    dirs.dedup();
    let mut builds = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // Staging and rollback folders start with a dot.
            if !name.starts_with('.') && path.join("proton").is_file() {
                builds.push((name, path));
            }
        }
    }
    builds.sort();
    builds
}

/// Whether Steam at `steam` runs in a sandbox (Flatpak, Snap) that can't
/// read Irori's folders.
fn sandboxed(steam: &Path) -> bool {
    let path = steam.to_string_lossy();
    path.contains("/.var/app/com.valvesoftware.Steam") || path.contains("/snap/steam/")
}

/// Bring Steam's registrations of Irori's builds in line with the settings
/// and the builds on disk. Linux only; does nothing without a native Steam.
/// Blocking.
pub fn sync(app: &AppSettings) -> Result<Changes> {
    if !cfg!(target_os = "linux") {
        return Ok(Changes::default());
    }
    let steam = locations::steam_path(app);
    if !steam.join("steamapps").is_dir() {
        return Ok(Changes::default());
    }
    let tools_dir = steam.join("compatibilitytools.d");
    let wanted = if app.download_proton_to_steam && !sandboxed(&steam) {
        let present: HashSet<String> = std::fs::read_dir(&tools_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| !is_ours(&e.path()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        wanted(&irori_builds(app), &present)
    } else {
        Vec::new()
    };
    if wanted.is_empty() && !tools_dir.is_dir() {
        return Ok(Changes::default());
    }
    apply(&tools_dir, &wanted, app)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(root: &Path, name: &str) -> (String, PathBuf) {
        let dir = root.join("irori/proton").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proton"), "").unwrap();
        (name.to_string(), dir)
    }

    // Steam's compatibility tools are a Linux part.
    #[cfg(unix)]
    #[test]
    fn registers_builds_steam_doesnt_have_yet() {
        let dir = tempfile::tempdir().unwrap();
        let builds = vec![
            build(dir.path(), "GE-Proton10-20"),
            build(dir.path(), "GE-Proton-latest"),
            build(dir.path(), "GE-Proton9-27"),
        ];
        // Installed into Steam directly: not registered again.
        let present: HashSet<String> = ["GE-Proton9-27".to_string()].into();
        let regs = wanted(&builds, &present);
        assert_eq!(
            regs.iter().map(|r| r.folder.as_str()).collect::<Vec<_>>(),
            ["irori-GE-Proton10-20", "irori-GE-Proton-latest"]
        );
        let vdf = tool_vdf(&regs[0]);
        assert!(vdf.contains("\"GE-Proton10-20\""));
        assert!(vdf.contains("\"display_name\" \"GE-Proton10-20 (Irori)\""));
        assert!(vdf.contains(&format!("\"install_path\" \"{}\"", builds[0].1.display())));
        // Irori's VDF reader takes it back as one tool.
        let doc = crate::vdf::parse(&vdf);
        let tools = doc
            .get("compatibilitytools")
            .and_then(|c| c.get("compat_tools"))
            .unwrap();
        assert!(tools.get("GE-Proton10-20").is_some());
    }

    #[test]
    fn quotes_paths_for_vdf() {
        let reg = Registration {
            folder: "irori-x".into(),
            name: "x".into(),
            install_path: PathBuf::from(r#"/home/a "b"\c"#),
        };
        assert!(tool_vdf(&reg).contains(r#""/home/a \"b\"\\c""#));
    }

    #[test]
    fn applies_and_removes_only_irori_registrations() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("compatibilitytools.d");
        // A tool the user installed, even one named like Irori's.
        std::fs::create_dir_all(tools.join("GE-Proton9-1")).unwrap();
        std::fs::create_dir_all(tools.join("irori-mine")).unwrap();
        let builds = vec![
            build(dir.path(), "GE-Proton10-20"),
            build(dir.path(), "GE-Proton10-19"),
        ];
        let regs = wanted(&builds, &HashSet::new());
        let changes = apply(&tools, &regs, &AppSettings::default()).unwrap();
        assert_eq!(changes.added.len(), 2);
        assert!(
            tools
                .join("irori-GE-Proton10-20/compatibilitytool.vdf")
                .is_file()
        );
        // Applying again changes nothing.
        assert_eq!(
            apply(&tools, &regs, &AppSettings::default()).unwrap(),
            Changes::default()
        );
        // A removed build's registration leaves with it.
        let changes = apply(&tools, &regs[..1], &AppSettings::default()).unwrap();
        assert_eq!(changes.removed, ["irori-GE-Proton10-19"]);
        assert!(!tools.join("irori-GE-Proton10-19").exists());
        // Turning the setting off takes every registration out.
        apply(&tools, &[], &AppSettings::default()).unwrap();
        assert!(!tools.join("irori-GE-Proton10-20").exists());
        assert!(tools.join("GE-Proton9-1").is_dir());
        assert!(tools.join("irori-mine").is_dir());
    }

    #[test]
    fn sandboxed_steam_is_left_alone() {
        assert!(sandboxed(Path::new(
            "/home/u/.var/app/com.valvesoftware.Steam/.local/share/Steam"
        )));
        assert!(sandboxed(Path::new(
            "/home/u/snap/steam/common/.local/share/Steam"
        )));
        assert!(!sandboxed(Path::new("/home/u/.local/share/Steam")));
    }
}
