//! Starting an engine on its own, without a game: to set it up, browse its
//! menus, or play something the library doesn't list. It starts the same
//! installation games use with the settings Irori keeps for them
//! (RetroArch's managed configuration, a managed version's portable
//! marker), so what the user changes there is what games get. The launch
//! template is left out: it names a game and the flags that play one.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::process::CommandSpec;
use serde_json::Value;

use crate::discovery::{Installation, Origin};
use crate::launch::{Vars, program_of, sync_portable_marker, xdg_data};
use crate::manifest::{EngineKind, EngineManifest};

/// Whether `manifest` has a window of its own to open. Engines that only
/// convert games into programs (AnyPS5) don't.
pub fn can_open(manifest: &EngineManifest) -> bool {
    manifest.kind != EngineKind::Relinker
}

/// The command that starts `installation` of `manifest` without a game.
/// Blocking: it writes RetroArch's settings and the portable marker.
pub fn command(
    manifest: &EngineManifest,
    installation: &Installation,
    values: &BTreeMap<String, Value>,
    own_settings: bool,
) -> Result<CommandSpec> {
    if !can_open(manifest) {
        bail!(
            "{} only converts games; it has no window to open",
            manifest.name
        );
    }
    let home = crate::dirs::home(&manifest.id);
    let engine_dir = installation
        .dir
        .clone()
        .or_else(|| installation.program.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let vars = Vars {
        rom: String::new(),
        rom_dir: String::new(),
        home: dirs::home_dir().unwrap_or_default().display().to_string(),
        engine_home: home.display().to_string(),
        engine_dir: engine_dir.display().to_string(),
        xdg_data: xdg_data(),
    };

    let mut args: Vec<OsString> = Vec::new();
    let mut shared: Vec<PathBuf> = Vec::new();
    if manifest.kind == EngineKind::Retroarch {
        let settings = crate::retroarch::managed_settings(values);
        if own_settings {
            let essentials: Vec<_> = settings
                .into_iter()
                .filter(|(k, _)| matches!(*k, "system_directory" | "libretro_directory"))
                .collect();
            let append = home.join("irori-append.cfg");
            crate::retroarch::write_config(&append, &essentials)?;
            args.extend(["--appendconfig".into(), append.into_os_string()]);
        } else {
            let config = home.join("retroarch.cfg");
            crate::retroarch::write_config(&config, &settings)?;
            args.extend(["--config".into(), config.into_os_string()]);
        }
        shared.extend([
            home.clone(),
            crate::dirs::system(),
            crate::retroarch::cores_dir(),
        ]);
    } else {
        sync_portable_marker(manifest, installation, &engine_dir, own_settings)?;
    }

    let mut spec = match installation.origin {
        Origin::Flatpak => {
            let id = installation
                .flatpak
                .clone()
                .context("the Flatpak has no application id")?;
            let mut spec = CommandSpec::new("flatpak").arg("run");
            for dir in shared.iter().filter(|d| !d.as_os_str().is_empty()) {
                spec = spec.arg(format!("--filesystem={}", dir.display()));
            }
            spec.arg(id).args(args)
        }
        _ => CommandSpec::new(program_of(&installation.program)?).args(args),
    };
    for (k, v) in &manifest.launch.env {
        spec = spec.env(k.clone(), vars.expand(v));
    }
    std::fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
    spec.cwd = Some(if engine_dir.is_dir() {
        engine_dir
    } else {
        home
    });
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation(engine: &str, origin: Origin, program: &Path) -> Installation {
        Installation {
            engine: engine.into(),
            origin,
            version: (origin == Origin::Managed).then(|| "1.0".to_string()),
            program: program.to_path_buf(),
            flatpak: (origin == Origin::Flatpak).then(|| format!("org.example.{engine}")),
            dir: None,
            size: 0,
        }
    }

    fn args(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_standalone_engine_opens_without_its_launch_template() {
        let root = crate::test_root("open-standalone");
        let program = root.join("duckstation");
        std::fs::write(&program, b"").unwrap();
        let m = crate::engine("duckstation").unwrap();
        let inst = installation("duckstation", Origin::System, &program);
        let values = m.option_values(&BTreeMap::new());
        let spec = command(&m, &inst, &values, false).unwrap();
        assert_eq!(spec.program, program);
        // No game, so neither `{rom}` nor the flags that play one.
        assert!(args(&spec).is_empty(), "{:?}", args(&spec));
        assert_eq!(spec.cwd.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn retroarch_opens_with_the_settings_games_use() {
        let _root = crate::test_root("open-retroarch");
        let m = crate::engine("retroarch").unwrap();
        let inst = installation("retroarch", Origin::Flatpak, Path::new("flatpak"));
        let values = m.option_values(&BTreeMap::new());
        let spec = command(&m, &inst, &values, false).unwrap();
        assert_eq!(spec.program, PathBuf::from("flatpak"));
        let args = args(&spec);
        assert_eq!(args[0], "run");
        let id = args
            .iter()
            .position(|a| a == "org.example.retroarch")
            .unwrap();
        assert_eq!(args[id + 1], "--config");
        assert!(!args.contains(&"-L".to_string()), "no core: {args:?}");
        let cfg = std::fs::read_to_string(crate::retroarch::home().join("retroarch.cfg")).unwrap();
        assert!(cfg.contains("system_directory = "));

        let own = command(&m, &inst, &values, true).unwrap();
        assert!(own.args.iter().any(|a| a == "--appendconfig"));
    }

    #[test]
    fn engines_that_only_convert_games_have_nothing_to_open() {
        let _root = crate::test_root("open-relinker");
        let m = crate::engine("anyps5").unwrap();
        assert!(!can_open(&m));
        let inst = installation("anyps5", Origin::System, Path::new("/bin/anyps5"));
        assert!(command(&m, &inst, &BTreeMap::new(), false).is_err());
        assert!(can_open(&crate::engine("dolphin").unwrap()));
    }
}
