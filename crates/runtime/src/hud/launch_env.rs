//! Cross-platform HUD launch environment builder, Flatpak `run` injector,
//! gamescope `--mangoapp` coordination, and Steam launch-option snippets.

use std::collections::BTreeMap;
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
    let _ = input;
    HudLaunchPlan::default()
}

/// Inject `--filesystem`, `--env` and `--unset-env` flags immediately after
/// `flatpak run` and strictly before the Flatpak App ID. Returns `true` when
/// `spec` was a `flatpak run` command and was updated.
pub fn apply_hud_to_flatpak_command(
    spec: &mut CommandSpec,
    plan: &HudLaunchPlan,
    mangohud_dir: Option<&Path>,
) -> bool {
    let _ = (spec, plan, mangohud_dir);
    false
}

/// Convert a multi-line `MangoHud.conf` body into an inline `MANGOHUD_CONFIG`
/// string, replacing commas inside multi-value options (e.g. `fps_value=30,60`)
/// with `+` (`fps_value=30+60`).
pub fn format_mangohud_inline_config(conf: &str) -> String {
    let _ = conf;
    String::new()
}

/// Build a copy-ready Steam Launch Options snippet (`... %command%`) for games
/// started via `steam://rungameid/<id>`.
pub fn steam_launch_snippet(
    backend: HudBackend,
    resolved: &ResolvedHud,
    conf_path: &Path,
) -> Option<String> {
    let _ = (backend, resolved, conf_path);
    None
}

/// Strip HUD environment variables (`MANGOHUD*`, `DISABLE_MANGOHUD`, `MTL_HUD_*`)
/// from companion `in_prefix` tool environments.
pub fn strip_hud_env_vars(env: &mut BTreeMap<String, String>) {
    let _ = env;
}
