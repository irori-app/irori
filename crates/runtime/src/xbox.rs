//! Wine builds that can play Microsoft Store (GDK) games: Proton with
//! `xgameruntime.dll`, the Windows component those games load first (the
//! open-source Nexus Runtime, offered by the Wine manager as
//! [`crate::ReleaseKind::NexusRuntime`]).
//!
//! Such a build runs on the host, never under umu: it carries its own host
//! libraries and its Proton session picks the plain Wine route when `UMU_ID`
//! is set and `UMU_USE_STEAM` is `0` (otherwise it starts `steam.exe`).
//! Proton links every DLL of the build into the prefix's `system32` itself, so
//! the runtime's components (`xgameruntime.dll`, its WinHTTP with HTTP/2,
//! DXVK) reach games without copying.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use irori_core::{WineInstallation, WineKind};

/// What a Microsoft Store game's process needs from the build.
const XGAMERUNTIME: &str = "files/lib/wine/x86_64-windows/xgameruntime.dll";
/// The build's description of its patches and features.
const PATCH_FILE: &str = "XODUS_NEXUS_PATCH.json";
/// `UMU_ID` value: any non-empty value selects the plain Wine route.
const UMU_ID: &str = "irori-xbox";

/// A Proton build with `xgameruntime.dll`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XboxRuntime {
    /// The build's root (holds `proton` and `files/`).
    pub root: PathBuf,
    /// Feature names from the build's patch file (`cloud-saves-pr28`,
    /// `xodus-tcp-bridge-v1`, `xuser-offline-identity-v2`, …); empty when it
    /// has none.
    pub features: Vec<String>,
}

impl XboxRuntime {
    /// The build behind `wine` when it can play Microsoft Store games.
    /// Reads two small files; call off the UI thread.
    pub fn inspect(wine: &WineInstallation) -> Option<Self> {
        if wine.kind != WineKind::Proton {
            return None;
        }
        let root = Path::new(&wine.bin).parent()?.to_path_buf();
        Self::at(&root)
    }

    /// The build at `root`, when it has `xgameruntime.dll`.
    pub fn at(root: &Path) -> Option<Self> {
        if !root.join(XGAMERUNTIME).is_file() || !root.join("proton").is_file() {
            return None;
        }
        let features = std::fs::read(root.join(PATCH_FILE))
            .ok()
            .filter(|bytes| bytes.len() < 1 << 20)
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|patch| {
                patch["features"].as_array().map(|list| {
                    list.iter()
                        .filter_map(|f| f.as_str())
                        .filter(|f| f.len() < 80)
                        .map(str::to_string)
                        .collect()
                })
            })
            .unwrap_or_default();
        Some(Self {
            root: root.to_path_buf(),
            features,
        })
    }

    /// Whether the build has a feature its patch file names.
    pub fn has(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// The in-game account service reaches Irori over the keyed loopback
    /// bridge, which saves and Store licences need.
    pub fn has_service_bridge(&self) -> bool {
        self.has("xodus-tcp-bridge-v1")
    }

    /// The runtime may sign in from its cached identity when Irori allows an
    /// offline launch.
    pub fn supports_offline(&self) -> bool {
        self.has("xuser-offline-identity-v2")
    }

    /// The Windows-path form of a host path inside the prefix's `Z:` drive.
    pub fn windows_path(path: &Path) -> String {
        format!("Z:{}", path.to_string_lossy().replace('/', "\\"))
    }

    /// Variables a game's Proton session needs on top of Irori's Proton
    /// environment: the plain Wine route, and caches kept with the prefix.
    /// `prefix` is the compatibility data folder (`STEAM_COMPAT_DATA_PATH`).
    pub fn launch_env(&self, prefix: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let mut set = |k: &str, v: String| {
            env.insert(k.to_string(), v);
        };
        set("UMU_ID", UMU_ID.into());
        set("UMU_USE_STEAM", "0".into());
        set("SteamAppId", "0".into());
        set("SteamGameId", "0".into());
        // The session is Python; the build's folder is not Irori's to write.
        set("PYTHONDONTWRITEBYTECODE", "1".into());
        let shaders = prefix.join("shadercache");
        set(
            "STEAM_COMPAT_SHADER_PATH",
            shaders.to_string_lossy().into_owned(),
        );
        set(
            "VKD3D_SHADER_CACHE_PATH",
            shaders.join("vkd3d").to_string_lossy().into_owned(),
        );
        // Proton's media converters need their fallback files, or they fail
        // to set up and crash the game on exit.
        let media = self.root.join("files/share/media");
        set(
            "MEDIACONV_BLANK_VIDEO_FILE",
            media.join("blank.mkv").to_string_lossy().into_owned(),
        );
        set(
            "MEDIACONV_BLANK_AUDIO_FILE",
            media.join("blank.ptna").to_string_lossy().into_owned(),
        );
        set(
            "ESPEAK_DATA_PATH",
            self.root.join("files/share").to_string_lossy().into_owned(),
        );
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(dir: &Path, dll: bool, patch: Option<&str>) -> WineInstallation {
        std::fs::create_dir_all(dir.join("files/lib/wine/x86_64-windows")).unwrap();
        std::fs::write(dir.join("proton"), "#!/usr/bin/env python3\n").unwrap();
        if dll {
            std::fs::write(dir.join(XGAMERUNTIME), b"MZ").unwrap();
        }
        if let Some(patch) = patch {
            std::fs::write(dir.join(PATCH_FILE), patch).unwrap();
        }
        WineInstallation {
            bin: dir.join("proton").to_string_lossy().into_owned(),
            name: "test".into(),
            kind: WineKind::Proton,
            ..Default::default()
        }
    }

    #[test]
    fn only_proton_with_xgameruntime_qualifies() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = build(&tmp.path().join("plain"), false, None);
        assert!(XboxRuntime::inspect(&plain).is_none());
        let wine = WineInstallation {
            kind: WineKind::Wine,
            ..build(&tmp.path().join("wine"), true, None)
        };
        assert!(XboxRuntime::inspect(&wine).is_none());
        let nexus = build(
            &tmp.path().join("nexus"),
            true,
            Some(r#"{"features":["xodus-tcp-bridge-v1","cloud-saves-pr28",3]}"#),
        );
        let runtime = XboxRuntime::inspect(&nexus).unwrap();
        assert!(runtime.has_service_bridge());
        assert!(!runtime.supports_offline());
        assert_eq!(runtime.features.len(), 2);
    }

    #[test]
    fn unreadable_patch_file_means_no_features() {
        let tmp = tempfile::tempdir().unwrap();
        let nexus = build(tmp.path(), true, Some("not json"));
        assert!(XboxRuntime::inspect(&nexus).unwrap().features.is_empty());
    }

    #[test]
    fn launch_env_pins_the_plain_wine_route() {
        let runtime = XboxRuntime {
            root: "/rt".into(),
            features: vec![],
        };
        let env = runtime.launch_env(Path::new("/pfx"));
        assert_eq!(env["UMU_USE_STEAM"], "0");
        assert!(!env["UMU_ID"].is_empty());
        // Joined like the code does, so the test also holds where the
        // separator is `\` (the crate's tests run on Windows too).
        let joined = |p: PathBuf| p.to_string_lossy().into_owned();
        assert_eq!(
            env["STEAM_COMPAT_SHADER_PATH"],
            joined(Path::new("/pfx").join("shadercache"))
        );
        assert_eq!(
            env["MEDIACONV_BLANK_VIDEO_FILE"],
            joined(Path::new("/rt").join("files/share/media").join("blank.mkv"))
        );
        assert_eq!(
            XboxRuntime::windows_path(Path::new("/games/Halo/x.exe")),
            "Z:\\games\\Halo\\x.exe"
        );
    }
}
