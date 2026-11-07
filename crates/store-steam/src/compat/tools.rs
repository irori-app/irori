//! The compatibility tools Steam offers: Valve's Proton builds, installed
//! like games in the Steam libraries, and the tools in `compatibilitytools.d`
//! folders (GE-Proton and other community builds, Irori's downloads among
//! them when "Install Proton downloads into Steam" is on).

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use irori_runtime::vdf::{self, Vdf};

use crate::scan::{self, state};

/// A tool a Steam game can be mapped to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatTool {
    /// The name `CompatToolMapping` records (`proton_9`, `GE-Proton10-4`).
    pub name: String,
    /// The name Steam shows.
    pub display_name: String,
    /// Where it comes from.
    pub source: ToolSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// One of Valve's Proton builds, installed by Steam.
    Valve,
    /// A folder of a `compatibilitytools.d` directory.
    Custom(PathBuf),
}

/// Flatpak Steam's application ID, part of its folder's path.
const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// Folders Steam reads compatibility tools from, besides its own
/// `compatibilitytools.d`. Flatpak Steam sees only its own folder.
fn tool_dirs(steam_dir: &Path, extra: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = vec![steam_dir.join("compatibilitytools.d")];
    let flatpak = steam_dir.components().any(|c| c.as_os_str() == FLATPAK_ID);
    if !flatpak {
        if let Some(extra) = extra {
            dirs.extend(
                extra
                    .split(':')
                    .filter(|p| !p.trim().is_empty())
                    .map(PathBuf::from),
            );
        }
        dirs.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
        dirs.push(PathBuf::from("/usr/local/share/steam/compatibilitytools.d"));
    }
    dirs
}

/// Every tool installed for `steam_dir`: Valve's first (Experimental and
/// Hotfix, then newest version first), then the others by name, newest
/// first. A name appears once. Blocking.
pub(crate) fn installed_tools(steam_dir: &Path) -> Vec<CompatTool> {
    let extra = std::env::var("STEAM_EXTRA_COMPAT_TOOLS_PATHS").ok();
    let mut valve = valve_tools(steam_dir);
    valve.sort_by(valve_order);
    let mut custom = Vec::new();
    for dir in tool_dirs(steam_dir, extra.as_deref()) {
        custom.extend(custom_tools(&dir));
    }
    custom.sort_by(|a, b| natural_cmp(&b.display_name, &a.display_name));
    let mut seen = HashSet::new();
    valve
        .into_iter()
        .chain(custom)
        .filter(|t| seen.insert(t.name.clone()))
        .collect()
}

/// Valve's Proton builds installed in any Steam library.
fn valve_tools(steam_dir: &Path) -> Vec<CompatTool> {
    let mut tools = Vec::new();
    for library in scan::libraries(steam_dir) {
        let Ok(entries) = std::fs::read_dir(library.join("steamapps")) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            if let Some(tool) = std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|text| valve_tool(&text))
            {
                tools.push(tool);
            }
        }
    }
    tools
}

/// The Proton build an app manifest describes, if it is a fully installed
/// one.
pub(crate) fn valve_tool(manifest: &str) -> Option<CompatTool> {
    let doc = vdf::parse(manifest);
    let app = doc.get("AppState")?;
    let name = app.get("name")?.as_str()?.trim();
    let flags: u64 = app.get("StateFlags")?.as_str()?.trim().parse().ok()?;
    if flags & state::FULLY_INSTALLED == 0 {
        return None;
    }
    Some(CompatTool {
        name: valve_internal_name(name)?,
        display_name: name.to_string(),
        source: ToolSource::Valve,
    })
}

/// The name Steam's mapping uses for one of Valve's Proton builds, from the
/// name its app has: "Proton 9.0" is `proton_9`, "Proton 5.13" `proton_513`,
/// "Proton Experimental" `proton_experimental`. The runtimes Valve also
/// names "Proton …" (EasyAntiCheat, BattlEye) aren't tools.
pub(crate) fn valve_internal_name(app_name: &str) -> Option<String> {
    let rest = app_name.strip_prefix("Proton ")?;
    // "Proton 9.0 (Beta)" maps like "Proton 9.0".
    let rest = rest.split('(').next().unwrap_or_default().trim();
    let version: Vec<&str> = rest.split('.').collect();
    if version.len() <= 2
        && version
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        let digits = match version.as_slice() {
            [major, "0"] => major.to_string(),
            parts => parts.concat(),
        };
        return Some(format!("proton_{digits}"));
    }
    matches!(rest, "Experimental" | "Hotfix" | "Next")
        .then(|| format!("proton_{}", rest.to_lowercase()))
}

/// Experimental, Hotfix and Next first, then versions newest first.
fn valve_order(a: &CompatTool, b: &CompatTool) -> Ordering {
    let rank = |tool: &CompatTool| match tool.name.as_str() {
        "proton_experimental" => 0,
        "proton_hotfix" => 1,
        "proton_next" => 2,
        _ => 3,
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| natural_cmp(&b.display_name, &a.display_name))
}

/// The tools of each folder in `dir` that has a `compatibilitytool.vdf`.
fn custom_tools(dir: &Path) -> Vec<CompatTool> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut tools = Vec::new();
    for entry in entries.flatten() {
        let folder = entry.path();
        let Ok(text) = std::fs::read_to_string(folder.join("compatibilitytool.vdf")) else {
            continue;
        };
        tools.extend(parse_tool_file(&text, &folder));
    }
    tools
}

/// The tools a `compatibilitytool.vdf` declares (one file may declare
/// several).
pub(crate) fn parse_tool_file(text: &str, folder: &Path) -> Vec<CompatTool> {
    let doc = vdf::parse(text);
    let Some(tools) = doc
        .get("compatibilitytools")
        .and_then(|d| d.get("compat_tools"))
    else {
        return Vec::new();
    };
    tools
        .entries()
        .iter()
        .filter(|(name, value)| !name.trim().is_empty() && matches!(value, Vdf::Obj(_)))
        .map(|(name, value)| CompatTool {
            name: name.trim().to_string(),
            display_name: value
                .get("display_name")
                .and_then(Vdf::as_str)
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .unwrap_or(name.trim())
                .to_string(),
            source: ToolSource::Custom(folder.to_path_buf()),
        })
        .collect()
}

/// Compare with runs of digits as numbers, so "GE-Proton10-4" comes after
/// "GE-Proton9-27".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_valves_builds_as_steam_does() {
        for (app, name) in [
            ("Proton 9.0", Some("proton_9")),
            ("Proton 9.0 (Beta)", Some("proton_9")),
            ("Proton 10.0", Some("proton_10")),
            ("Proton 8.0", Some("proton_8")),
            ("Proton 6.3", Some("proton_63")),
            ("Proton 5.13", Some("proton_513")),
            ("Proton 4.11", Some("proton_411")),
            ("Proton 3.7", Some("proton_37")),
            ("Proton Experimental", Some("proton_experimental")),
            ("Proton Hotfix", Some("proton_hotfix")),
            ("Proton EasyAntiCheat Runtime", None),
            ("Proton BattlEye Runtime", None),
            ("Portal 2", None),
        ] {
            assert_eq!(valve_internal_name(app).as_deref(), name, "{app}");
        }
    }

    fn manifest(id: &str, name: &str, flags: u32) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"name\"\t\t\"{name}\"\n\t\"StateFlags\"\t\t\"{flags}\"\n\t\"installdir\"\t\t\"{name}\"\n}}\n"
        )
    }

    #[test]
    fn lists_valve_and_custom_tools_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path().join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        for (id, name, flags) in [
            ("2805730", "Proton 9.0", 4),
            ("1493710", "Proton Experimental", 4),
            ("1580130", "Proton 6.3", 4),
            ("3658110", "Proton 10.0", 4),
            ("1887720", "Proton 7.0", 1026), // still downloading
            ("1826330", "Proton EasyAntiCheat Runtime", 4),
            ("620", "Portal 2", 4),
        ] {
            std::fs::write(
                steam.join(format!("steamapps/appmanifest_{id}.acf")),
                manifest(id, name, flags),
            )
            .unwrap();
        }
        let tools_dir = steam.join("compatibilitytools.d");
        for (folder, file) in [
            (
                "GE-Proton9-27",
                "\"compatibilitytools\"\n{\n  \"compat_tools\"\n  {\n    \"GE-Proton9-27\" // Internal name\n    {\n      \"install_path\" \".\"\n      \"display_name\" \"GE-Proton9-27\"\n      \"from_oslist\"  \"windows\"\n      \"to_oslist\"    \"linux\"\n    }\n  }\n}\n",
            ),
            (
                "GE-Proton10-4",
                "\"compatibilitytools\" { \"compat_tools\" { \"GE-Proton10-4\" { \"display_name\" \"GE-Proton10-4\" } } }",
            ),
            (
                "luxtorpeda",
                "\"compatibilitytools\" { \"compat_tools\" { \"luxtorpeda\" { \"display_name\" \"Luxtorpeda\" } \"luxtorpeda_dev\" { } } }",
            ),
            ("broken", "garbage {"),
        ] {
            std::fs::create_dir_all(tools_dir.join(folder)).unwrap();
            std::fs::write(tools_dir.join(folder).join("compatibilitytool.vdf"), file).unwrap();
        }
        std::fs::create_dir_all(tools_dir.join("empty")).unwrap();

        let tools: Vec<CompatTool> = installed_tools(&steam)
            .into_iter()
            .filter(|t| match &t.source {
                ToolSource::Valve => true,
                ToolSource::Custom(p) => p.starts_with(dir.path()),
            })
            .collect();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "proton_experimental",
                "proton_10",
                "proton_9",
                "proton_63",
                "luxtorpeda_dev",
                "luxtorpeda",
                "GE-Proton10-4",
                "GE-Proton9-27",
            ]
        );
        assert_eq!(tools[1].display_name, "Proton 10.0");
        assert_eq!(tools[5].display_name, "Luxtorpeda");
        assert_eq!(tools[4].display_name, "luxtorpeda_dev");
        assert_eq!(
            tools[7].source,
            ToolSource::Custom(tools_dir.join("GE-Proton9-27"))
        );
    }

    #[test]
    fn flatpak_steam_reads_only_its_own_folder() {
        let flatpak = Path::new("/home/u/.var/app/com.valvesoftware.Steam/data/Steam");
        assert_eq!(
            tool_dirs(flatpak, Some("/opt/tools")),
            vec![flatpak.join("compatibilitytools.d")]
        );
        let native = Path::new("/home/u/.local/share/Steam");
        let dirs = tool_dirs(native, Some("/opt/a:/opt/b"));
        assert_eq!(dirs[0], native.join("compatibilitytools.d"));
        assert_eq!(dirs[1], PathBuf::from("/opt/a"));
        assert_eq!(dirs[2], PathBuf::from("/opt/b"));
        assert!(dirs.contains(&PathBuf::from("/usr/share/steam/compatibilitytools.d")));
    }

    #[test]
    fn natural_order_reads_numbers() {
        assert_eq!(
            natural_cmp("GE-Proton10-4", "GE-Proton9-27"),
            Ordering::Greater
        );
        assert_eq!(
            natural_cmp("GE-Proton9-27", "GE-Proton9-3"),
            Ordering::Greater
        );
        assert_eq!(natural_cmp("a", "A"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }
}
