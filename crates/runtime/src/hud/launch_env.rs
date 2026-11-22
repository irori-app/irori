//! Cross-platform HUD launch environment builder, Flatpak `run` injector,
//! gamescope `--mangoapp` coordination, and Steam launch-option snippets.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use irori_core::process::CommandSpec;
use irori_core::{HudBackend, ResolvedHud};

/// Inputs needed to compute HUD environment variables and wrapper integration.
#[derive(Debug, Clone)]
pub struct HudLaunchInput<'a> {
    pub backend: HudBackend,
    pub resolved: &'a ResolvedHud,
    pub config_file: Option<&'a Path>,
    pub mangohud_dir: Option<&'a Path>,
    pub mangohud_bin_path: Option<&'a Path>,
    pub is_native_linux_game: bool,
    pub gamescope_enabled: bool,
    pub deck_game_mode: bool,
}

/// Resulting HUD mutations to apply to a launch environment and command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HudLaunchPlan {
    pub env_set: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub use_mangoapp: bool,
    pub opengl_wrapper: Option<Vec<String>>,
    pub sandbox_ro_binds: Vec<PathBuf>,
}

/// Build the pure HUD launch plan for Linux, macOS or Windows.
pub fn build_hud_launch_plan(input: &HudLaunchInput<'_>) -> HudLaunchPlan {
    let mut plan = HudLaunchPlan::default();

    if input.deck_game_mode
        || input.backend == HudBackend::Unsupported
        || matches!(input.resolved, ResolvedHud::Off)
    {
        return plan;
    }

    match (input.backend, input.resolved) {
        (HudBackend::MangoHud, ResolvedHud::Disabled) => {
            plan.env_set.insert("DISABLE_MANGOHUD".into(), "1".into());
            plan.env_set.insert("MANGOHUD".into(), "0".into());
            plan.env_remove
                .extend(["MANGOHUD_CONFIGFILE".into(), "MANGOHUD_CONFIG".into()]);
        }
        (HudBackend::MetalHud, ResolvedHud::Disabled) => {
            plan.env_set.insert("MTL_HUD_ENABLED".into(), "0".into());
            plan.env_remove.push("MTL_HUD_ALIGNMENT".into());
        }
        (HudBackend::MangoHud, ResolvedHud::MangoHudProfile(profile)) => {
            configure_mangohud_active(
                &mut plan,
                input,
                true,
                profile.opengl_legacy_wrapper,
                profile.mangohud_dlsym,
            );
        }
        (
            HudBackend::MangoHud,
            ResolvedHud::MangoHudSystemDefault {
                opengl_legacy_wrapper,
                mangohud_dlsym,
            },
        ) => {
            configure_mangohud_active(
                &mut plan,
                input,
                false,
                *opengl_legacy_wrapper,
                *mangohud_dlsym,
            );
        }
        (HudBackend::MetalHud, ResolvedHud::MetalHud(metal)) => {
            plan.env_set.insert("MTL_HUD_ENABLED".into(), "1".into());
            plan.env_set.insert(
                "MTL_HUD_ALIGNMENT".into(),
                metal.alignment.as_env_value().into(),
            );
        }
        _ => {}
    }

    plan
}

fn configure_mangohud_active(
    plan: &mut HudLaunchPlan,
    input: &HudLaunchInput<'_>,
    is_profile: bool,
    opengl_legacy_wrapper: bool,
    mangohud_dlsym: bool,
) {
    if input.gamescope_enabled {
        // Gamescope embeds MangoHud via `--mangoapp`; unset `MANGOHUD` so the
        // outer gamescope process does not render a second HUD.
        plan.use_mangoapp = true;
        plan.env_remove.extend([
            "MANGOHUD".into(),
            "MANGOHUD_CONFIG".into(),
            "DISABLE_MANGOHUD".into(),
        ]);
    } else {
        plan.env_set.insert("MANGOHUD".into(), "1".into());
        plan.env_remove
            .extend(["MANGOHUD_CONFIG".into(), "DISABLE_MANGOHUD".into()]);

        if input.is_native_linux_game
            && opengl_legacy_wrapper
            && let Some(bin) = input.mangohud_bin_path
        {
            plan.opengl_wrapper = Some(vec![bin.to_string_lossy().into_owned()]);
            if mangohud_dlsym {
                plan.env_set.insert("MANGOHUD_DLSYM".into(), "1".into());
            }
        }
    }

    if is_profile {
        if let Some(cfg) = input.config_file {
            plan.env_set.insert(
                "MANGOHUD_CONFIGFILE".into(),
                cfg.to_string_lossy().into_owned(),
            );
            plan.sandbox_ro_binds.push(cfg.to_path_buf());
        }
        if let Some(dir) = input.mangohud_dir
            && !plan.sandbox_ro_binds.iter().any(|p| p == dir)
        {
            plan.sandbox_ro_binds.push(dir.to_path_buf());
        }
    } else {
        plan.env_remove.push("MANGOHUD_CONFIGFILE".into());
    }
}

/// Inject `--filesystem`, `--env` and `--unset-env` flags immediately after
/// `flatpak run` and strictly before the Flatpak App ID. Returns `true` when
/// `spec` was a `flatpak run` command and was updated.
pub fn apply_hud_to_flatpak_command(
    spec: &mut CommandSpec,
    plan: &HudLaunchPlan,
    mangohud_dir: Option<&Path>,
) -> bool {
    let is_flatpak = spec
        .program
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "flatpak");
    let is_run = spec
        .args
        .first()
        .and_then(|a| a.to_str())
        .is_some_and(|a| a == "run");
    if !is_flatpak || !is_run {
        return false;
    }

    let mut injected: Vec<OsString> = Vec::new();
    let needs_dir =
        plan.env_set.contains_key("MANGOHUD_CONFIGFILE") || !plan.sandbox_ro_binds.is_empty();
    if needs_dir && let Some(dir) = mangohud_dir {
        injected.push(format!("--filesystem={}:ro", dir.display()).into());
    }
    for key in &plan.env_remove {
        injected.push(format!("--unset-env={key}").into());
    }
    for (key, value) in &plan.env_set {
        let effective_val = if key == "MANGOHUD_CONFIG" && value.contains('\n') {
            format_mangohud_inline_config(value)
        } else {
            value.clone()
        };
        injected.push(format!("--env={key}={effective_val}").into());
    }

    if injected.is_empty() {
        return false;
    }

    spec.args.splice(1..1, injected);
    true
}

/// Convert a multi-line `MangoHud.conf` body into an inline `MANGOHUD_CONFIG`
/// string, replacing commas inside multi-value options (e.g. `fps_value=30,60`)
/// with `+` (`fps_value=30+60`).
pub fn format_mangohud_inline_config(conf: &str) -> String {
    conf.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.replace(',', "+"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Build a copy-ready Steam Launch Options snippet (`... %command%`) for games
/// started via `steam://rungameid/<id>`.
pub fn steam_launch_snippet(
    backend: HudBackend,
    resolved: &ResolvedHud,
    conf_path: &Path,
) -> Option<String> {
    match (backend, resolved) {
        (HudBackend::MangoHud, ResolvedHud::MangoHudProfile(_)) => Some(format!(
            "MANGOHUD=1 MANGOHUD_CONFIGFILE=\"{}\" %command%",
            conf_path.display()
        )),
        (HudBackend::MangoHud, ResolvedHud::MangoHudSystemDefault { .. }) => {
            Some("MANGOHUD=1 %command%".into())
        }
        (HudBackend::MangoHud, ResolvedHud::Disabled) => {
            Some("DISABLE_MANGOHUD=1 MANGOHUD=0 %command%".into())
        }
        (HudBackend::MetalHud, ResolvedHud::MetalHud(metal)) => Some(format!(
            "MTL_HUD_ENABLED=1 MTL_HUD_ALIGNMENT={} %command%",
            metal.alignment.as_env_value()
        )),
        (HudBackend::MetalHud, ResolvedHud::Disabled) => Some("MTL_HUD_ENABLED=0 %command%".into()),
        _ => None,
    }
}

/// Strip HUD environment variables (`MANGOHUD*`, `DISABLE_MANGOHUD`, `MTL_HUD_*`)
/// from companion `in_prefix` tool environments.
pub fn strip_hud_env_vars(env: &mut BTreeMap<String, String>) {
    env.retain(|k, _| {
        !k.starts_with("MANGOHUD") && !k.starts_with("MTL_HUD_") && k != "DISABLE_MANGOHUD"
    });
}

#[cfg(test)]
mod tests {
    use irori_core::{MangoHudProfile, MetalHudAlignment, MetalHudSettings};

    use super::*;

    #[test]
    fn off_deck_mode_and_unsupported_emit_empty_plan() {
        let profile = ResolvedHud::MangoHudProfile(Box::default());
        for (backend, resolved, deck) in [
            (HudBackend::MangoHud, &ResolvedHud::Off, false),
            (HudBackend::MangoHud, &profile, true),
            (HudBackend::Unsupported, &profile, false),
        ] {
            let plan = build_hud_launch_plan(&HudLaunchInput {
                backend,
                resolved,
                config_file: Some(Path::new("/data/mangohud/global.conf")),
                mangohud_dir: Some(Path::new("/data/mangohud")),
                mangohud_bin_path: Some(Path::new("/usr/bin/mangohud")),
                is_native_linux_game: true,
                gamescope_enabled: false,
                deck_game_mode: deck,
            });
            assert_eq!(plan, HudLaunchPlan::default());
        }
    }

    #[test]
    fn linux_disabled_vs_profile_vs_system_default_precedence() {
        let disabled = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MangoHud,
            resolved: &ResolvedHud::Disabled,
            config_file: None,
            mangohud_dir: None,
            mangohud_bin_path: None,
            is_native_linux_game: false,
            gamescope_enabled: false,
            deck_game_mode: false,
        });
        assert_eq!(disabled.env_set["DISABLE_MANGOHUD"], "1");
        assert_eq!(disabled.env_set["MANGOHUD"], "0");
        assert!(disabled.env_remove.contains(&"MANGOHUD_CONFIGFILE".into()));
        assert!(disabled.env_remove.contains(&"MANGOHUD_CONFIG".into()));

        let profile = MangoHudProfile {
            opengl_legacy_wrapper: true,
            mangohud_dlsym: true,
            ..Default::default()
        };
        let resolved_profile = ResolvedHud::MangoHudProfile(Box::new(profile));
        let active = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MangoHud,
            resolved: &resolved_profile,
            config_file: Some(Path::new("/data/mangohud/global.conf")),
            mangohud_dir: Some(Path::new("/data/mangohud")),
            mangohud_bin_path: Some(Path::new("/usr/bin/mangohud")),
            is_native_linux_game: true,
            gamescope_enabled: false,
            deck_game_mode: false,
        });
        assert_eq!(active.env_set["MANGOHUD"], "1");
        assert_eq!(
            active.env_set["MANGOHUD_CONFIGFILE"],
            "/data/mangohud/global.conf"
        );
        assert_eq!(active.env_set["MANGOHUD_DLSYM"], "1");
        assert!(active.env_remove.contains(&"MANGOHUD_CONFIG".into()));
        assert!(active.env_remove.contains(&"DISABLE_MANGOHUD".into()));
        assert_eq!(
            active.opengl_wrapper,
            Some(vec!["/usr/bin/mangohud".into()])
        );
        assert_eq!(
            active.sandbox_ro_binds,
            vec![
                PathBuf::from("/data/mangohud/global.conf"),
                PathBuf::from("/data/mangohud"),
            ]
        );

        let sys_default = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MangoHud,
            resolved: &ResolvedHud::MangoHudSystemDefault {
                opengl_legacy_wrapper: false,
                mangohud_dlsym: true,
            },
            config_file: Some(Path::new("/data/mangohud/global.conf")),
            mangohud_dir: Some(Path::new("/data/mangohud")),
            mangohud_bin_path: None,
            is_native_linux_game: false,
            gamescope_enabled: false,
            deck_game_mode: false,
        });
        assert_eq!(sys_default.env_set["MANGOHUD"], "1");
        assert!(!sys_default.env_set.contains_key("MANGOHUD_CONFIGFILE"));
        assert!(
            sys_default
                .env_remove
                .contains(&"MANGOHUD_CONFIGFILE".into())
        );
        assert!(sys_default.env_remove.contains(&"MANGOHUD_CONFIG".into()));
        assert!(sys_default.env_remove.contains(&"DISABLE_MANGOHUD".into()));
    }

    #[test]
    fn gamescope_uses_mangoapp_and_prevents_double_hud() {
        let resolved = ResolvedHud::MangoHudProfile(Box::default());
        let plan = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MangoHud,
            resolved: &resolved,
            config_file: Some(Path::new("/data/mangohud/global.conf")),
            mangohud_dir: Some(Path::new("/data/mangohud")),
            mangohud_bin_path: Some(Path::new("/usr/bin/mangohud")),
            is_native_linux_game: true,
            gamescope_enabled: true,
            deck_game_mode: false,
        });
        assert!(plan.use_mangoapp);
        assert!(!plan.env_set.contains_key("MANGOHUD"));
        assert!(plan.env_remove.contains(&"MANGOHUD".into()));
        assert!(plan.env_remove.contains(&"MANGOHUD_CONFIG".into()));
        assert!(plan.env_remove.contains(&"DISABLE_MANGOHUD".into()));
        assert_eq!(
            plan.env_set["MANGOHUD_CONFIGFILE"],
            "/data/mangohud/global.conf"
        );
        assert!(plan.opengl_wrapper.is_none());
    }

    #[test]
    fn macos_metal_hud_alignment_is_hyphen_free() {
        let resolved = ResolvedHud::MetalHud(MetalHudSettings {
            alignment: MetalHudAlignment::BottomLeft,
        });
        let plan = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MetalHud,
            resolved: &resolved,
            config_file: None,
            mangohud_dir: None,
            mangohud_bin_path: None,
            is_native_linux_game: false,
            gamescope_enabled: false,
            deck_game_mode: false,
        });
        assert_eq!(plan.env_set["MTL_HUD_ENABLED"], "1");
        assert_eq!(plan.env_set["MTL_HUD_ALIGNMENT"], "bottomleft");
        assert!(!plan.env_set["MTL_HUD_ALIGNMENT"].contains('-'));

        let disabled = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MetalHud,
            resolved: &ResolvedHud::Disabled,
            config_file: None,
            mangohud_dir: None,
            mangohud_bin_path: None,
            is_native_linux_game: false,
            gamescope_enabled: false,
            deck_game_mode: false,
        });
        assert_eq!(disabled.env_set["MTL_HUD_ENABLED"], "0");
        assert!(disabled.env_remove.contains(&"MTL_HUD_ALIGNMENT".into()));
    }

    #[test]
    fn flatpak_run_injects_before_app_id_and_formats_inline_commas() {
        let resolved = ResolvedHud::MangoHudProfile(Box::default());
        let plan = build_hud_launch_plan(&HudLaunchInput {
            backend: HudBackend::MangoHud,
            resolved: &resolved,
            config_file: Some(Path::new("/data/mangohud/global.conf")),
            mangohud_dir: Some(Path::new("/data/mangohud")),
            mangohud_bin_path: None,
            is_native_linux_game: true,
            gamescope_enabled: false,
            deck_game_mode: false,
        });

        let mut spec = CommandSpec::new("flatpak").args([
            "run",
            "--filesystem=/roms",
            "org.libretro.RetroArch",
            "--fullscreen",
            "/roms/game.sfc",
        ]);
        assert!(apply_hud_to_flatpak_command(
            &mut spec,
            &plan,
            Some(Path::new("/data/mangohud"))
        ));

        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let run_idx = args.iter().position(|a| a == "run").unwrap();
        let app_idx = args
            .iter()
            .position(|a| a == "org.libretro.RetroArch")
            .unwrap();
        let fs_idx = args
            .iter()
            .position(|a| a == "--filesystem=/data/mangohud:ro")
            .unwrap();
        let env_idx = args.iter().position(|a| a == "--env=MANGOHUD=1").unwrap();
        let cfg_idx = args
            .iter()
            .position(|a| a == "--env=MANGOHUD_CONFIGFILE=/data/mangohud/global.conf")
            .unwrap();
        let unset_idx = args
            .iter()
            .position(|a| a == "--unset-env=DISABLE_MANGOHUD")
            .unwrap();

        assert!(run_idx < fs_idx && fs_idx < app_idx);
        assert!(run_idx < env_idx && env_idx < app_idx);
        assert!(run_idx < cfg_idx && cfg_idx < app_idx);
        assert!(run_idx < unset_idx && unset_idx < app_idx);

        // Inline config converts multi-value commas to `+`.
        let inline = format_mangohud_inline_config(
            "legacy_layout=false\nfps_value=30,60\nfps_color=B22222,FDFD09,39F900\nfps\n",
        );
        assert_eq!(
            inline,
            "legacy_layout=false,fps_value=30+60,fps_color=B22222+FDFD09+39F900,fps"
        );

        let mut inline_plan = HudLaunchPlan::default();
        inline_plan.env_set.insert(
            "MANGOHUD_CONFIG".into(),
            "legacy_layout=false\nfps_value=30,60\n".into(),
        );
        let mut spec2 = CommandSpec::new("flatpak").args(["run", "org.example.App"]);
        assert!(apply_hud_to_flatpak_command(&mut spec2, &inline_plan, None));
        assert!(
            spec2
                .args
                .iter()
                .any(|a| a == "--env=MANGOHUD_CONFIG=legacy_layout=false,fps_value=30+60")
        );
    }

    #[test]
    fn steam_launch_snippets_cover_linux_and_macos_modes() {
        let conf = Path::new("/home/u/.local/share/irori/mangohud/steam_123.conf");
        assert_eq!(
            steam_launch_snippet(
                HudBackend::MangoHud,
                &ResolvedHud::MangoHudProfile(Box::default()),
                conf
            ),
            Some(
                "MANGOHUD=1 MANGOHUD_CONFIGFILE=\"/home/u/.local/share/irori/mangohud/steam_123.conf\" %command%"
                    .into()
            )
        );
        assert_eq!(
            steam_launch_snippet(
                HudBackend::MangoHud,
                &ResolvedHud::MangoHudSystemDefault {
                    opengl_legacy_wrapper: false,
                    mangohud_dlsym: true
                },
                conf
            ),
            Some("MANGOHUD=1 %command%".into())
        );
        assert_eq!(
            steam_launch_snippet(HudBackend::MangoHud, &ResolvedHud::Disabled, conf),
            Some("DISABLE_MANGOHUD=1 MANGOHUD=0 %command%".into())
        );
        assert_eq!(
            steam_launch_snippet(HudBackend::MangoHud, &ResolvedHud::Off, conf),
            None
        );

        assert_eq!(
            steam_launch_snippet(
                HudBackend::MetalHud,
                &ResolvedHud::MetalHud(MetalHudSettings {
                    alignment: MetalHudAlignment::TopRight
                }),
                conf
            ),
            Some("MTL_HUD_ENABLED=1 MTL_HUD_ALIGNMENT=topright %command%".into())
        );
        assert_eq!(
            steam_launch_snippet(HudBackend::MetalHud, &ResolvedHud::Disabled, conf),
            Some("MTL_HUD_ENABLED=0 %command%".into())
        );
        assert_eq!(
            steam_launch_snippet(HudBackend::Unsupported, &ResolvedHud::Disabled, conf),
            None
        );
    }

    #[test]
    fn strip_hud_env_vars_removes_only_hud_keys() {
        let mut env = BTreeMap::from([
            ("MANGOHUD".into(), "1".into()),
            ("MANGOHUD_CONFIGFILE".into(), "/x.conf".into()),
            ("DISABLE_MANGOHUD".into(), "1".into()),
            ("MTL_HUD_ENABLED".into(), "1".into()),
            ("MTL_HUD_ALIGNMENT".into(), "topright".into()),
            ("WINEPREFIX".into(), "/pfx".into()),
            ("MTL_CAPTURE_ENABLED".into(), "1".into()),
        ]);
        strip_hud_env_vars(&mut env);
        assert_eq!(env.len(), 2);
        assert_eq!(env["WINEPREFIX"], "/pfx");
        assert_eq!(env["MTL_CAPTURE_ENABLED"], "1");
    }
}
