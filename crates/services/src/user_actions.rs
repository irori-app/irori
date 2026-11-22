//! Running the actions the user added beside a game's Play button
//! (`iroriActions`, [`irori_core::UserAction`]): links go to the program the
//! system opens them with, programs start as programs of this computer or,
//! outside Windows, in the game's Wine prefix. An action that counts as
//! playing holds the game's Playing status while it runs and adds its time
//! to the game's playtime and session history, like a session Irori started.

use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};
use irori_core::process::{self, CommandSpec};
use irori_core::{GameId, HostPlatform, InstallPlatform, StatusKind, UserAction};

use crate::Backend;

/// How an action starts.
#[derive(Debug, Clone)]
pub enum ActionRun {
    /// Hand the link to the system's opener.
    Link(String),
    /// Run with the game's Wine or Proton in its prefix: the program, then
    /// its arguments.
    InPrefix(Vec<String>),
    /// Run as a program of this computer.
    Program(CommandSpec),
}

/// How `action` starts on `host`. A program in the prefix reads Windows
/// quoting, backslashes included; a program of this computer is started
/// from its own folder.
pub fn plan(action: &UserAction, host: HostPlatform) -> Result<ActionRun> {
    ensure!(action.is_set(), "The action has nothing to run");
    let target = action.target.trim();
    if action.is_link() {
        return Ok(ActionRun::Link(target.to_string()));
    }
    let file = irori_core::paths::expand_home(target);
    if action.in_prefix && host != HostPlatform::Windows {
        let mut argv = vec![file.to_string_lossy().into_owned()];
        argv.extend(process::split_windows_args(&action.args));
        return Ok(ActionRun::InPrefix(argv));
    }
    let spec = process::program_command(&file).args(process::split_args(&action.args));
    let spec = match file
        .parent()
        .filter(|p| file.is_absolute() && !p.as_os_str().is_empty())
    {
        Some(dir) => spec.cwd(dir),
        None => spec,
    };
    Ok(ActionRun::Program(spec))
}

/// The command that hands `url` to the program the system opens it with;
/// inside the Flatpak, the host's.
fn open_url(url: &str) -> CommandSpec {
    let local = if cfg!(target_os = "macos") {
        CommandSpec::new("/usr/bin/open").arg(url)
    } else if cfg!(windows) {
        CommandSpec::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
    } else {
        CommandSpec::new("xdg-open").arg(url)
    };
    irori_core::host::command(local)
}

impl Backend {
    /// Run the `index`-th action of `id`. Returns once it has started; an
    /// action that counts as playing keeps the game's Playing status until
    /// it exits.
    pub async fn run_user_action(&self, id: &GameId, index: usize) -> Result<()> {
        let settings = self.settings.game(id);
        let action = settings
            .actions
            .get(index)
            .cloned()
            .context("This action is no longer in the game's settings")?;
        let game = self
            .game(id)
            .context("The game is no longer in the library")?;
        let counts = action.counts_as_playing && !action.is_link();
        if counts {
            ensure!(
                !self.statuses.is_busy(id),
                "{} is busy; wait until it is done",
                game.title
            );
        }
        let run = plan(&action, HostPlatform::current())?;
        tracing::info!("running the action {} of {id}", action.label());
        match run {
            ActionRun::Link(url) => process::open_detached(&open_url(&url))
                .await
                .with_context(|| format!("Could not open {url}")),
            ActionRun::Program(spec) => {
                let mut child = process::spawn(&spec, None)
                    .with_context(|| format!("Could not start {}", action.label()))?;
                if counts {
                    let (b, id) = (self.clone(), id.clone());
                    let started = self.begin_action_session(&id);
                    self.spawn(async move {
                        let _ = child.wait().await;
                        b.end_action_session(&id, started);
                    });
                } else {
                    // Reaped so it never lingers as a zombie.
                    self.spawn(async move {
                        let _ = child.wait().await;
                    });
                }
                Ok(())
            }
            ActionRun::InPrefix(argv) => {
                if !host_runs_in_prefix(&game) {
                    bail!("{} doesn't run through Wine", game.title);
                }
                ensure!(Path::new(&argv[0]).is_file(), "{} was not found", argv[0]);
                let (b, id, label) = (self.clone(), id.clone(), action.label());
                let started = counts.then(|| self.begin_action_session(&id));
                self.spawn(async move {
                    if let Err(e) = b.run_in_prefix(&id, argv).await {
                        tracing::warn!("the action {label} of {id}: {e:#}");
                    }
                    if let Some(started) = started {
                        b.end_action_session(&id, started);
                    }
                });
                Ok(())
            }
        }
    }

    /// Mark `id` as playing, for an action that counts as playing.
    fn begin_action_session(&self, id: &GameId) -> (std::time::Instant, String) {
        self.statuses.set(id, StatusKind::Playing);
        self.userdata
            .record_launch(id, self.settings.app().max_recent_games as usize);
        (std::time::Instant::now(), irori_core::util::now_rfc3339())
    }

    /// Add the action's time to the game's playtime and history.
    fn end_action_session(&self, id: &GameId, (started, started_at): (std::time::Instant, String)) {
        let seconds = started.elapsed().as_secs();
        {
            // Sync sees both or neither.
            let _playtime = self.sync.playtime_guard();
            self.userdata.record_session(id, seconds);
            let session = crate::sessions::Session {
                game: id.key(),
                started: started_at,
                seconds,
                crashed: false,
                exit_code: None,
            };
            if let Err(e) = crate::sessions::append(&session) {
                tracing::warn!("recording the action's session of {id}: {e:#}");
            }
        }
        self.statuses.clear(id);
    }
}

/// Whether a game's prefix can take a Windows program: a Windows build run
/// through Wine.
fn host_runs_in_prefix(game: &irori_core::GameInfo) -> bool {
    game.installed_platform()
        .unwrap_or(InstallPlatform::Windows)
        .needs_wine()
        && HostPlatform::current() != HostPlatform::Windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(target: &str, args: &str, in_prefix: bool) -> UserAction {
        UserAction {
            target: target.into(),
            args: args.into(),
            in_prefix,
            ..Default::default()
        }
    }

    #[test]
    fn links_programs_and_prefix_programs_start_their_own_way() {
        assert!(matches!(
            plan(&action("https://example.org/wiki", "", true), HostPlatform::Linux).unwrap(),
            ActionRun::Link(url) if url == "https://example.org/wiki"
        ));
        let ActionRun::InPrefix(argv) = plan(
            &action(r"C:\Tools\Map.exe", r#"--level "C:\Maps\one.map""#, true),
            HostPlatform::Linux,
        )
        .unwrap() else {
            panic!("in the prefix")
        };
        assert_eq!(argv, [r"C:\Tools\Map.exe", "--level", r"C:\Maps\one.map"]);
        // Windows runs Windows programs itself.
        let native = plan(&action("/opt/tool", "a 'b c'", true), HostPlatform::Windows).unwrap();
        assert!(matches!(native, ActionRun::Program(_)));
        let ActionRun::Program(spec) = plan(
            &action("/opt/r2/r2modman", "--game ror2", false),
            HostPlatform::Linux,
        )
        .unwrap() else {
            panic!("a program")
        };
        assert_eq!(spec.program, std::path::PathBuf::from("/opt/r2/r2modman"));
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("/opt/r2")));
        assert!(plan(&action(" ", "", false), HostPlatform::Linux).is_err());
    }
}
