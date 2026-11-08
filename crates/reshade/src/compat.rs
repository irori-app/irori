//! ReShade's compatibility list, `Compatibility.ini` on the `list` branch of
//! crosire/reshade-shaders: the file ReShade's own installer reads. Each
//! section is an executable's file name; Irori uses `Banned` (the game blocks
//! or bans ReShade), `RenderApi` (the API to install for, when imports don't
//! tell), `ForceInstallApi` and `InstallTarget` (the folder, relative to the
//! executable, the DLL must go to).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::Api;

/// Where ReShade's installer reads the list from.
pub const COMPATIBILITY_URL: &str =
    "https://raw.githubusercontent.com/crosire/reshade-shaders/list/Compatibility.ini";

/// The entry of one executable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompatEntry {
    pub banned: bool,
    /// The `RenderApi` value, upper case (`D3D11`, `OPENGL`, …).
    pub render_api: Option<String>,
    /// Use `render_api` even when the imports name another API.
    pub force_api: bool,
    /// The folder the DLL goes to, relative to the executable's folder.
    pub install_target: Option<PathBuf>,
}

impl CompatEntry {
    /// The API `render_api` names, when Irori can install for it.
    pub fn api(&self) -> Option<Api> {
        Some(match self.render_api.as_deref()? {
            "D3D8" => Api::D3D8,
            "D3D9" => Api::D3D9,
            "D3D10" | "D3D11" | "D3D12" | "DXGI" => Api::Dxgi,
            "OPENGL" => Api::OpenGl,
            "VULKAN" => Api::Vulkan,
            "DDRAW" => Api::DDraw,
            _ => return None,
        })
    }
}

/// The parsed list, keyed by lower-case executable name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompatList {
    entries: HashMap<String, CompatEntry>,
}

impl CompatList {
    /// Parse the INI text. Lines that are neither a section, a `key=value`
    /// nor a comment are skipped; keys match without letter case, as in
    /// ReShade's own parser (the file writes both `RenderApi` and
    /// `RenderAPI`).
    pub fn parse(text: &str) -> Self {
        let mut entries: HashMap<String, CompatEntry> = HashMap::new();
        let mut current: Option<String> = None;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = section.trim().to_lowercase();
                current = (!name.is_empty()).then(|| {
                    entries.entry(name.clone()).or_default();
                    name
                });
                continue;
            }
            let (Some(section), Some((key, value))) = (&current, line.split_once('=')) else {
                continue;
            };
            let Some(entry) = entries.get_mut(section) else {
                continue;
            };
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "banned" => entry.banned = value == "1",
                "renderapi" => entry.render_api = Some(value.to_ascii_uppercase()),
                "forceinstallapi" => entry.force_api = value == "1",
                "installtarget" => entry.install_target = install_target(value),
                _ => {}
            }
        }
        Self { entries }
    }

    /// The entry of an executable file name (`Game.exe`), any letter case.
    pub fn get(&self, exe_name: &str) -> Option<&CompatEntry> {
        self.entries.get(&exe_name.to_lowercase())
    }

    /// Whether ReShade marks the executable as banned.
    pub fn is_banned(&self, exe_name: &str) -> bool {
        self.get(exe_name).is_some_and(|e| e.banned)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `bin\x64_retail` as a relative path, or nothing when the value would
/// leave the game's folder.
fn install_target(value: &str) -> Option<PathBuf> {
    let unified = value.replace('\\', "/");
    if unified.contains(':') {
        return None;
    }
    irori_core::archive_guard::relative_entry_path(std::path::Path::new(&unified))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}# A Hat In Time\r\n[HatinTimeGame.exe]\r\nRenderApi=D3D9\r\n\r\n\
# Apex Legends\n[ApexLauncher.exe]\nBanned=1\n[r5apex.exe]\nBanned=1\nRenderApi=D3D11\n\n\
# Detroit\n[DetroitBecomeHuman.exe]\nRenderAPI=VULKAN\n\n\
[F1_22.exe]\nRenderApi=D3D12\nForceInstallApi=1\n\n\
[Titanfall2.exe]\nRenderApi=D3D11\nInstallTarget=bin\\x64_retail\n\n\
[Evil.exe]\nInstallTarget=..\\..\\Windows\n\
[Nope.exe]\nInstallTarget=C:\\Windows\n\
stray line\n";

    #[test]
    fn parses_reshades_list() {
        let list = CompatList::parse(SAMPLE);
        assert_eq!(list.len(), 8);
        let hat = list.get("hatintimegame.EXE").unwrap();
        assert_eq!(hat.api(), Some(Api::D3D9));
        assert!(!hat.banned);
        assert!(list.is_banned("ApexLauncher.exe"));
        assert!(list.is_banned("R5Apex.exe"));
        assert_eq!(list.get("r5apex.exe").unwrap().api(), Some(Api::Dxgi));
        assert_eq!(
            list.get("DetroitBecomeHuman.exe").unwrap().api(),
            Some(Api::Vulkan)
        );
        let f1 = list.get("F1_22.exe").unwrap();
        assert!(f1.force_api);
        assert_eq!(f1.api(), Some(Api::Dxgi));
        assert_eq!(
            list.get("Titanfall2.exe").unwrap().install_target,
            Some(PathBuf::from("bin").join("x64_retail"))
        );
        assert!(!list.is_banned("Hades.exe"));
        assert!(list.get("Hades.exe").is_none());
    }

    #[test]
    fn install_targets_stay_in_the_game_folder() {
        let list = CompatList::parse(SAMPLE);
        assert_eq!(list.get("Evil.exe").unwrap().install_target, None);
        assert_eq!(list.get("Nope.exe").unwrap().install_target, None);
    }
}
