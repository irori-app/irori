//! Wine builds that can play Microsoft Store (GDK) games: Proton with
//! `xgameruntime.dll`, the Windows component those games load first (the
//! open-source Nexus Runtime, offered by the Wine manager as
//! [`crate::ReleaseKind::NexusRuntime`]).
//!
//! Such a build runs on the host, never under umu: it carries its own host
//! libraries and its Proton session picks the plain Wine route when `UMU_ID`
//! is set and `UMU_USE_STEAM` is `0` (otherwise it starts `steam.exe`).
//! Proton links the build's Wine DLLs into the prefix's `system32` itself, so
//! most of the runtime's components (`xgameruntime.dll`, its WinHTTP, DXVK)
//! reach games without copying. `libnghttp2.dll`, which that WinHTTP loads
//! for HTTP/2, is the exception: WinHTTP loads it with
//! `LOAD_LIBRARY_SEARCH_SYSTEM32`, so it must be in `system32` itself, and
//! existing prefixes never get it ([`XboxRuntime::provision_prefix`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{WineInstallation, WineKind};
use sha2::{Digest as _, Sha256};

/// What a Microsoft Store game's process needs from the build.
const XGAMERUNTIME: &str = "files/lib/wine/x86_64-windows/xgameruntime.dll";
/// The build's description of its patches and features.
const PATCH_FILE: &str = "XODUS_NEXUS_PATCH.json";
/// The build's Wine DLLs, one folder per Windows architecture.
const WINE_DLLS: &str = "files/lib/wine";
/// The Windows architecture folder whose checksums the patch file's
/// `components` lists; other builds name theirs in `componentsByArch`.
const DEFAULT_PE: &str = "x86_64-windows";
/// DLLs the prefix's `system32` must hold as files, with the override that
/// makes Wine load that copy.
const SYSTEM32_COMPONENTS: [(&str, &str); 1] = [("libnghttp2.dll", "n")];
/// What a file Irori replaced in `system32` is renamed to.
const BACKUP_SUFFIX: &str = ".irori-backup";
/// More than any DLL of the build; a bigger file is not one.
const MAX_COMPONENT: u64 = 64 << 20;
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
        let features = read_patch(root)
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

    /// Put the build's components that must be files in the prefix's
    /// `system32` there, as Nexus Launcher does, and return the DLL
    /// overrides that load them (for `WINEDLLOVERRIDES`). `prefix` is the
    /// compatibility data folder. Blocking.
    ///
    /// A component the build doesn't ship is skipped. One the patch file
    /// lists a SHA-256 for must match it, or nothing is copied. A copy
    /// already in place is left alone; a different file there is renamed
    /// to `<name>.irori-backup` first, unless such a backup exists, which
    /// means the file is one Irori put there for an earlier build. The new
    /// file is written beside its place and renamed into it, so a symlink
    /// Proton left there is replaced rather than written through.
    pub fn provision_prefix(&self, prefix: &Path) -> Result<Vec<(&'static str, &'static str)>> {
        let system32 = prefix.join("pfx/drive_c/windows/system32");
        if !system32.is_dir() {
            tracing::warn!(
                "{} has no system32 yet; the runtime's components are copied at the next start",
                prefix.display()
            );
            return Ok(Vec::new());
        }
        let patch = read_patch(&self.root);
        let pe = pe_folder(&patch);
        let mut overrides = Vec::new();
        for (name, mode) in SYSTEM32_COMPONENTS {
            let source = self.root.join(WINE_DLLS).join(&pe).join(name);
            if !source.is_file() {
                continue;
            }
            let bytes = read_bounded(&source)?;
            if let Some(expected) = expected_sha256(&patch, &pe, name) {
                let actual = hex::encode(Sha256::digest(&bytes));
                if !actual.eq_ignore_ascii_case(&expected) {
                    bail!(
                        "The runtime's {name} doesn't match its checksum. Reinstall the runtime in the Wine manager."
                    );
                }
            }
            place(&system32, name, &bytes)?;
            overrides.push((name.trim_end_matches(".dll"), mode));
        }
        Ok(overrides)
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

/// The build's patch file, when it has a readable one.
fn read_patch(root: &Path) -> Option<serde_json::Value> {
    std::fs::read(root.join(PATCH_FILE))
        .ok()
        .filter(|bytes| bytes.len() < 1 << 20)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// The Windows architecture folder of the build's games: the one
/// `componentsByArch` names (ARM builds), else x86-64's.
fn pe_folder(patch: &Option<serde_json::Value>) -> String {
    patch
        .as_ref()
        .and_then(|p| p["componentsByArch"].as_object())
        .and_then(|arches| arches.keys().next())
        .filter(|pe| pe.ends_with("-windows") && !pe.contains(['/', '\\', '.']))
        .cloned()
        .unwrap_or_else(|| DEFAULT_PE.into())
}

/// The SHA-256 the patch file lists for a component of the `pe` folder.
fn expected_sha256(patch: &Option<serde_json::Value>, pe: &str, name: &str) -> Option<String> {
    let patch = patch.as_ref()?;
    let listed = match patch["componentsByArch"].get(pe) {
        Some(components) => components,
        None if pe == DEFAULT_PE => &patch["components"],
        None => return None,
    };
    listed[name].as_str().map(str::to_string)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let size = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    if size > MAX_COMPONENT {
        bail!("{} is too large to be a DLL", path.display());
    }
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Put `bytes` at `system32/name` unless the file there already holds them.
fn place(system32: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let target = system32.join(name);
    if std::fs::read(&target).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        let backup = system32.join(format!("{name}{BACKUP_SUFFIX}"));
        if std::fs::symlink_metadata(&backup).is_err() {
            std::fs::rename(&target, &backup)
                .with_context(|| format!("keeping {} aside", target.display()))?;
        } else {
            // Irori's copy for an earlier build; the user's file is the backup.
            std::fs::remove_file(&target)
                .with_context(|| format!("replacing {}", target.display()))?;
        }
    }
    let temporary = system32.join(format!("{name}.irori-new"));
    let _ = std::fs::remove_file(&temporary);
    let written = std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, &target))
        .with_context(|| format!("copying {name} into {}", system32.display()));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
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

    /// A runtime whose `libnghttp2.dll` holds `dll`, with the patch file's
    /// `components` listing `listed` for it, and a prefix with `system32`.
    fn runtime_with_nghttp2(
        dir: &Path,
        dll: &[u8],
        listed: Option<&str>,
    ) -> (XboxRuntime, PathBuf, PathBuf) {
        let root = dir.join("runtime");
        let patch = match listed {
            Some(sum) => format!(
                r#"{{"features":["wine-winhttp-http2-v1"],"components":{{"libnghttp2.dll":"{sum}"}}}}"#
            ),
            None => r#"{"features":[]}"#.into(),
        };
        build(&root, true, Some(&patch));
        std::fs::write(
            root.join(WINE_DLLS).join(DEFAULT_PE).join("libnghttp2.dll"),
            dll,
        )
        .unwrap();
        let prefix = dir.join("compat");
        let system32 = prefix.join("pfx/drive_c/windows/system32");
        std::fs::create_dir_all(&system32).unwrap();
        (XboxRuntime::at(&root).unwrap(), prefix, system32)
    }

    fn sha256(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn nghttp2_is_copied_into_system32_and_overridden() {
        let tmp = tempfile::tempdir().unwrap();
        let (runtime, prefix, system32) =
            runtime_with_nghttp2(tmp.path(), b"MZ nghttp2", Some(&sha256(b"MZ nghttp2")));
        let overrides = runtime.provision_prefix(&prefix).unwrap();
        assert_eq!(overrides, vec![("libnghttp2", "n")]);
        let target = system32.join("libnghttp2.dll");
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ nghttp2");
        assert!(!system32.join("libnghttp2.dll.irori-backup").exists());
        assert!(!system32.join("libnghttp2.dll.irori-new").exists());

        // Already in place: nothing changes.
        runtime.provision_prefix(&prefix).unwrap();
        assert!(!system32.join("libnghttp2.dll.irori-backup").exists());
    }

    #[test]
    fn a_different_file_is_kept_aside_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (runtime, prefix, system32) = runtime_with_nghttp2(tmp.path(), b"MZ new", None);
        let target = system32.join("libnghttp2.dll");
        let backup = system32.join("libnghttp2.dll.irori-backup");
        std::fs::write(&target, b"MZ the user's").unwrap();
        runtime.provision_prefix(&prefix).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"MZ the user's");

        // A newer build replaces Irori's copy and keeps the user's backup.
        std::fs::write(
            runtime
                .root
                .join(WINE_DLLS)
                .join(DEFAULT_PE)
                .join("libnghttp2.dll"),
            b"MZ newer",
        )
        .unwrap();
        runtime.provision_prefix(&prefix).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ newer");
        assert_eq!(std::fs::read(&backup).unwrap(), b"MZ the user's");
    }

    #[test]
    fn a_wrong_checksum_copies_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (runtime, prefix, system32) =
            runtime_with_nghttp2(tmp.path(), b"MZ tampered", Some(&sha256(b"MZ real")));
        let err = runtime.provision_prefix(&prefix).unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert!(!system32.join("libnghttp2.dll").exists());
    }

    #[test]
    fn without_the_file_or_a_prefix_nothing_is_done() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("runtime");
        build(&root, true, None);
        let runtime = XboxRuntime::at(&root).unwrap();
        let prefix = tmp.path().join("compat");
        let system32 = prefix.join("pfx/drive_c/windows/system32");
        std::fs::create_dir_all(&system32).unwrap();
        assert!(runtime.provision_prefix(&prefix).unwrap().is_empty());
        assert!(std::fs::read_dir(&system32).unwrap().next().is_none());

        let (runtime, _, _) = runtime_with_nghttp2(&tmp.path().join("b"), b"MZ", None);
        let fresh = tmp.path().join("fresh");
        assert!(runtime.provision_prefix(&fresh).unwrap().is_empty());
        assert!(!fresh.exists());
    }

    #[test]
    fn arm_builds_name_their_folder_and_checksums() {
        let patch = serde_json::json!({
            "components": {"libnghttp2.dll": "x86"},
            "componentsByArch": {"aarch64-windows": {"libnghttp2.dll": "arm"}},
        });
        let patch = Some(patch);
        assert_eq!(pe_folder(&patch), "aarch64-windows");
        assert_eq!(
            expected_sha256(&patch, "aarch64-windows", "libnghttp2.dll").as_deref(),
            Some("arm")
        );
        let x86 = Some(serde_json::json!({"components": {"libnghttp2.dll": "x86"}}));
        assert_eq!(pe_folder(&x86), DEFAULT_PE);
        assert_eq!(
            expected_sha256(&x86, DEFAULT_PE, "libnghttp2.dll").as_deref(),
            Some("x86")
        );
        assert_eq!(pe_folder(&None), DEFAULT_PE);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_replaced_not_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let (runtime, prefix, system32) = runtime_with_nghttp2(tmp.path(), b"MZ ours", None);
        let elsewhere = tmp.path().join("other-runtime.dll");
        std::fs::write(&elsewhere, b"MZ other").unwrap();
        std::os::unix::fs::symlink(&elsewhere, system32.join("libnghttp2.dll")).unwrap();
        runtime.provision_prefix(&prefix).unwrap();
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"MZ other");
        assert_eq!(
            std::fs::read(system32.join("libnghttp2.dll")).unwrap(),
            b"MZ ours"
        );
    }
}
