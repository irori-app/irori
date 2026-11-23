//! "How it starts" for a game's settings: the command, wrappers and
//! variables its next launch gets, from the same facts and composition the
//! launch uses ([`irori_runtime::preview_launch`]), worked out without
//! creating the prefix, downloading or writing anything. Secrets (tokens,
//! keys, passwords) are masked here, before anything reaches the screen or
//! the clipboard.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use irori_core::process::{is_secret_var, split_program_args};
use irori_core::{GameId, InstallPlatform};
use irori_runtime::{LaunchPreview, Target};

use crate::Backend;

/// What a masked value shows.
pub const MASK: &str = "••••••";

/// How a game's next launch starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HowItStarts {
    /// Wrappers, the Wine layer, the program and its arguments, in order.
    pub command: Vec<String>,
    /// The variables Irori sets, by name; secret ones masked.
    pub env: Vec<(String, String)>,
    /// How many values were masked.
    pub masked: usize,
    /// The Wine prefix of a Windows game.
    pub prefix: Option<PathBuf>,
    /// The game runs in a bubblewrap sandbox.
    pub sandboxed: bool,
    /// A Windows build starts its native program without Wine.
    pub without_wine: bool,
    /// An Intel Mac program starts through Rosetta (`arch -x86_64`).
    pub rosetta: bool,
    /// The program isn't known before the store builds the launch (a store
    /// game without a chosen program), so the command names the install
    /// folder instead.
    pub program_from_store: bool,
    /// Another program starts the game with its own settings (Steam, the
    /// other launchers): Irori's variables and wrappers don't apply.
    pub handed_over: bool,
}

impl HowItStarts {
    /// One line a shell runs: the variables, then the command, each value
    /// quoted. Masked values stay masked.
    pub fn as_command(&self) -> String {
        let mut words: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", irori_runtime::wrappers::shell_join(&[v.clone()])))
            .collect();
        words.push(irori_runtime::wrappers::shell_join(&self.command));
        words.join(" ")
    }
}

/// The preview's command and masked variables.
pub(crate) fn assemble(
    preview: LaunchPreview,
    program: Option<PathBuf>,
    install_folder: Option<&str>,
    args: Vec<String>,
) -> HowItStarts {
    let mut command: Vec<String> = preview.wrappers.into_iter().flatten().collect();
    command.extend(preview.runner);
    let program_from_store = program.is_none();
    command.push(match program {
        Some(p) => p.to_string_lossy().into_owned(),
        None => install_folder.unwrap_or_default().to_string(),
    });
    command.extend(args);
    let mut masked = 0;
    let env = preview
        .env
        .into_iter()
        .map(|(k, v)| {
            if is_secret_var(&k) && !v.is_empty() {
                masked += 1;
                (k, MASK.to_string())
            } else {
                (k, v)
            }
        })
        .collect();
    HowItStarts {
        command: command.into_iter().filter(|w| !w.is_empty()).collect(),
        env,
        masked,
        prefix: preview.prefix,
        sandboxed: preview.sandboxed,
        without_wine: preview.without_wine,
        rosetta: false,
        program_from_store,
        handed_over: false,
    }
}

impl Backend {
    /// How `id`'s next launch starts (see the module notes).
    pub async fn how_it_starts(&self, id: &GameId) -> Result<HowItStarts> {
        let game = self.game(id).context("unknown game")?;
        if !id.runner.manages_installs() {
            return Ok(HowItStarts {
                handed_over: true,
                ..Default::default()
            });
        }
        let app = self.settings.app();
        let mut settings = self.settings.game(id);
        let platform = game
            .installed_platform()
            .unwrap_or(InstallPlatform::Windows);
        settings.sandbox = self.sandbox_of(id);
        let target = Target {
            id,
            game: &game,
            platform,
        };
        let mut preview = irori_runtime::preview_launch(&app, &settings, target).await?;
        // What the launch adds for ReShade and mods, read the same way.
        if !preview.runner.is_empty() {
            let mut env: std::collections::BTreeMap<String, String> =
                preview.env.into_iter().collect();
            self.reshade_launch_env(id, &mut env).await;
            self.mods_launch_env(id, &game, &mut env).await;
            preview.env = env.into_iter().collect();
        }
        let windows = platform == InstallPlatform::Windows && !preview.without_wine;
        let mut args = split_program_args(windows, &settings.launcher_args);
        let mut program = irori_runtime::without_wine::launch_program(&settings, &game);
        // An Intel Mac program goes through Rosetta, as the launch does.
        let mut rosetta = false;
        if platform == InstallPlatform::Mac
            && cfg!(all(target_os = "macos", target_arch = "aarch64"))
            && let Some(p) = program.clone()
        {
            let (folder, force) = (game.install_path(), settings.rosetta);
            let spec = irori_core::process::CommandSpec::new(p).args(args.clone());
            let (spec, wrapped) = self
                .spawn_blocking(move || {
                    irori_runtime::rosetta::wrap(spec, folder.as_deref(), force)
                })
                .await?;
            if wrapped {
                rosetta = true;
                program = Some(spec.program.clone());
                args = spec
                    .args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
            }
        }
        let folder = game.install.as_ref().map(|i| i.install_path.as_str());
        let mut how = assemble(preview, program, folder, args);
        how.rosetta = rosetta;
        Ok(how)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_and_the_command_reads_in_order() {
        let preview = LaunchPreview {
            env: vec![
                ("EPIC_TOKEN".into(), "abc".into()),
                ("STORE".into(), "egl".into()),
                ("API_KEY".into(), String::new()),
            ],
            wrappers: vec![vec!["gamemoderun".into()], vec!["env".into(), "-u".into()]],
            runner: vec!["/p/proton".into(), "waitforexitandrun".into()],
            prefix: Some(PathBuf::from("/pfx")),
            sandboxed: false,
            without_wine: false,
        };
        let how = assemble(
            preview,
            Some(PathBuf::from("/games/x/Game.exe")),
            Some("/games/x"),
            vec!["-windowed".into()],
        );
        assert_eq!(
            how.command,
            [
                "gamemoderun",
                "env",
                "-u",
                "/p/proton",
                "waitforexitandrun",
                "/games/x/Game.exe",
                "-windowed"
            ]
        );
        assert_eq!(how.env[0], ("EPIC_TOKEN".to_string(), MASK.to_string()));
        assert_eq!(how.env[1].1, "egl");
        assert_eq!(how.masked, 1);
        let line = how.as_command();
        assert!(!line.contains("abc"));
        assert!(line.starts_with("EPIC_TOKEN="));
        assert!(line.ends_with("/games/x/Game.exe -windowed"));
    }

    #[test]
    fn without_a_known_program_the_command_names_the_folder() {
        let how = assemble(LaunchPreview::default(), None, Some("/games/x"), Vec::new());
        assert!(how.program_from_store);
        assert_eq!(how.command, ["/games/x"]);
    }
}
