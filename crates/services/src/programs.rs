//! Programs started alongside a game (`iroriLaunchPrograms`): trainers,
//! overlays, voice chat, scripts. [`plan`] turns the settings into commands;
//! [`Programs`] starts them after the game, writes what happens to the game
//! log, and at the end of the session stops the ones that stop with it.
//!
//! Windows programs run with the game's own Wine or Proton build and prefix,
//! prepared for the game, so they see the same `C:` drive. Steam games run
//! in Steam's Proton prefix, which only `protontricks-launch` can reach.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use irori_core::process::{self, CancellationToken, CommandSpec};
use irori_core::{GameInfo, HostPlatform, LaunchEnvironment, LaunchProgram, WineKind};
use tokio::task::JoinHandle;

/// A program ready to start.
#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub label: String,
    pub spec: CommandSpec,
    pub delay: Duration,
    pub stop_with_game: bool,
    pub in_prefix: bool,
}

/// Why a program in the list doesn't start this time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Skip {
    /// No program is set.
    NoProgram,
    /// It should run in the game's prefix, but the game has none (a native
    /// game, or one another launcher runs).
    NoPrefix,
    /// A Steam game's Proton prefix needs `protontricks-launch`, which isn't
    /// installed, or Steam made no prefix for the game.
    SteamPrefix,
}

impl Skip {
    /// For the game log, which stays English.
    fn reason(self) -> &'static str {
        match self {
            Skip::NoProgram => "no program is set",
            Skip::NoPrefix => {
                "it runs in the game's Wine prefix, and this game doesn't run in one Irori prepares"
            }
            Skip::SteamPrefix => {
                "Steam's Proton prefix needs protontricks-launch, and either it isn't installed or Steam hasn't made a prefix for this game yet"
            }
        }
    }
}

/// What [`plan`] works from.
pub(crate) struct PlanInput<'a> {
    pub programs: &'a [LaunchProgram],
    /// What the runtime prepared for the game (its Wine layer, variables).
    pub environment: &'a LaunchEnvironment,
    pub host: HostPlatform,
    /// Variables naming the game (`IRORI_GAME_TITLE`, …).
    pub identity: BTreeMap<String, String>,
    /// For a Steam game with a Proton prefix: `protontricks-launch` and its
    /// arguments up to the program (`… --appid <id>`).
    pub steam_launch: Option<Vec<String>>,
}

/// The programs to start and the ones skipped, with why.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    pub start: Vec<Planned>,
    pub skipped: Vec<(String, Skip)>,
}

/// Commands for the enabled programs of a launch.
pub(crate) fn plan(input: &PlanInput<'_>) -> Plan {
    let mut out = Plan::default();
    for program in input.programs.iter().filter(|p| p.enabled) {
        let label = program.label();
        let path = program.path.trim();
        if path.is_empty() {
            out.skipped.push((label, Skip::NoProgram));
            continue;
        }
        let args = if input.host == HostPlatform::Windows {
            process::split_windows_args(&program.args)
        } else {
            process::split_args(&program.args)
        };
        // Windows runs Windows programs itself.
        let in_prefix = program.in_prefix && input.host != HostPlatform::Windows;
        let file = irori_core::paths::expand_home(path);
        let spec = if !in_prefix {
            let mut spec = process::program_command(&file)
                .args(args)
                .envs(input.identity.clone());
            if let Some(wine) = &input.environment.wine {
                spec = spec.env("WINEPREFIX", wine.prefix.to_string_lossy().into_owned());
            }
            spec
        } else if let Some(wine) = &input.environment.wine {
            // `waitforexitandrun` would wait for the game to end first;
            // `run` joins the session the game started.
            let mut argv = vec![wine.bin.to_string_lossy().into_owned()];
            let mut env = input.environment.env.clone();
            if wine.uses_umu {
                env.insert("PROTON_VERB".into(), "run".into());
            } else if wine.kind == WineKind::Proton {
                argv.push("run".into());
            }
            argv.push(file.to_string_lossy().into_owned());
            argv.extend(args);
            CommandSpec::new(argv.remove(0)).args(argv).envs(env)
        } else if let Some(launch) = &input.steam_launch {
            let mut argv = launch.clone();
            argv.push(file.to_string_lossy().into_owned());
            argv.extend(args);
            CommandSpec::new(argv.remove(0))
                .args(argv)
                .envs(input.identity.clone())
        } else {
            let skip = if input.identity_is_steam() {
                Skip::SteamPrefix
            } else {
                Skip::NoPrefix
            };
            out.skipped.push((label, skip));
            continue;
        };
        let spec = match file
            .parent()
            .filter(|p| file.is_absolute() && !p.as_os_str().is_empty())
        {
            Some(dir) => spec.cwd(dir),
            None => spec,
        };
        out.start.push(Planned {
            label,
            spec,
            delay: Duration::from_secs(program.delay_seconds.min(LaunchProgram::MAX_DELAY).into()),
            stop_with_game: program.stop_with_game,
            in_prefix,
        });
    }
    out
}

impl PlanInput<'_> {
    fn identity_is_steam(&self) -> bool {
        self.identity.get("IRORI_RUNNER").map(String::as_str) == Some("steam")
    }
}

/// Variables that name the game, for its scripts and programs.
pub(crate) fn identity_env(game: &GameInfo) -> BTreeMap<String, String> {
    let runner = game.runner().id().to_string();
    [
        ("IRORI_GAME_TITLE", game.title.clone()),
        ("IRORI_APP_NAME", game.app_name.to_string()),
        ("IRORI_RUNNER", runner.clone()),
        ("HEROIC_GAME_TITLE", game.title.clone()),
        ("HEROIC_APP_NAME", game.app_name.to_string()),
        ("HEROIC_APP_RUNNER", runner),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// How the end of a session waits for the game's prefix to empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrefixWait {
    /// Not at all.
    None,
    /// Until the prefix's last process exits (`wineserver -w`).
    Wineserver,
    /// While a process runs from the game's folder. `wineserver -w` would
    /// also wait for the programs started alongside, which only stop once
    /// the session ends.
    Folder(String),
}

/// How to wait for a game whose launcher exits before it does
/// (`track_wineserver`), given whether programs started alongside still run
/// in its prefix.
pub(crate) fn prefix_wait(
    track_wineserver: bool,
    needs_wine: bool,
    prefix_programs_running: bool,
    install_path: Option<&str>,
) -> PrefixWait {
    if !track_wineserver || !needs_wine {
        return PrefixWait::None;
    }
    if !prefix_programs_running {
        return PrefixWait::Wineserver;
    }
    match install_path.filter(|p| !p.trim().is_empty()) {
        Some(path) => PrefixWait::Folder(path.to_string()),
        None => PrefixWait::None,
    }
}

/// Whether the end of a session stops a program that is still running.
pub(crate) fn stops_at_end(stop_with_game: bool) -> bool {
    stop_with_game
}

/// The programs of one session.
pub(crate) struct Programs {
    /// Cancelled when the session ends: programs still waiting for their
    /// delay never start, and running ones that stop with the game stop.
    end: CancellationToken,
    /// Programs running in the game's prefix right now.
    in_prefix: Arc<AtomicUsize>,
    /// Tasks of programs that stop with the game, awaited at the end so
    /// they are gone before the after-launch script runs.
    stopping: Vec<JoinHandle<()>>,
}

impl Programs {
    /// Start the planned programs (each after its delay) and log the skipped.
    /// Call from inside the backend runtime.
    pub(crate) fn start(plan: Plan, log_file: &Path) -> Self {
        for (label, skip) in &plan.skipped {
            append_log(
                log_file,
                &format!("Not starting {label}: {}\n", skip.reason()),
            );
        }
        let end = CancellationToken::new();
        let in_prefix = Arc::new(AtomicUsize::new(0));
        let mut stopping = Vec::new();
        for planned in plan.start {
            let task = tokio::spawn(run(
                planned.clone(),
                log_file.to_path_buf(),
                end.clone(),
                in_prefix.clone(),
            ));
            if stops_at_end(planned.stop_with_game) {
                stopping.push(task);
            }
        }
        Self {
            end,
            in_prefix,
            stopping,
        }
    }

    /// Whether a program started alongside still runs in the game's prefix.
    pub(crate) fn running_in_prefix(&self) -> bool {
        self.in_prefix.load(Ordering::SeqCst) > 0
    }

    /// End the session: cancel the programs still waiting and stop the ones
    /// that stop with the game. Returns once they have exited.
    pub(crate) async fn end(self) {
        self.end.cancel();
        for task in self.stopping {
            let _ = task.await;
        }
    }
}

async fn run(
    planned: Planned,
    log_file: PathBuf,
    end: CancellationToken,
    in_prefix: Arc<AtomicUsize>,
) {
    let label = &planned.label;
    if !planned.delay.is_zero() {
        tokio::select! {
            _ = tokio::time::sleep(planned.delay) => {}
            _ = end.cancelled() => {
                append_log(&log_file, &format!("Not starting {label}: the game ended first\n"));
                return;
            }
        }
    }
    append_log(&log_file, &format!("Starting {label}: {}\n", planned.spec));
    let mut child = match process::spawn(&planned.spec, Some(&log_file)) {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!("starting {label}: {e:#}");
            append_log(&log_file, &format!("Could not start {label}: {e:#}\n"));
            return;
        }
    };
    if planned.in_prefix {
        in_prefix.fetch_add(1, Ordering::SeqCst);
    }
    let pid = child.id().unwrap_or_default();
    tracing::info!("started {label} with the game (pid {pid})");
    let status = tokio::select! {
        status = child.wait() => status.ok(),
        _ = end.cancelled(), if planned.stop_with_game => {
            append_log(&log_file, &format!("Stopping {label}: the game ended\n"));
            process::terminate(&mut child).await;
            None
        }
    };
    if planned.in_prefix {
        in_prefix.fetch_sub(1, Ordering::SeqCst);
    }
    if let Some(status) = status {
        let code = status
            .code()
            .map_or_else(|| "a signal".to_string(), |c| c.to_string());
        append_log(&log_file, &format!("{label} exited with {code}\n"));
    }
}

fn append_log(path: &Path, text: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::WineLaunch;

    fn program(path: &str) -> LaunchProgram {
        LaunchProgram {
            path: path.into(),
            ..Default::default()
        }
    }

    fn wine(kind: WineKind, uses_umu: bool) -> LaunchEnvironment {
        LaunchEnvironment {
            env: [("WINEPREFIX".to_string(), "/p".to_string())].into(),
            wrappers: vec![vec!["gamescope".into(), "--".into()]],
            wine: Some(WineLaunch {
                bin: PathBuf::from(if uses_umu { "/umu-run" } else { "/proton" }),
                build_bin: PathBuf::from("/proton"),
                kind,
                prefix: PathBuf::from("/p"),
                uses_umu,
                proton_verb: "waitforexitandrun",
            }),
        }
    }

    fn input<'a>(
        programs: &'a [LaunchProgram],
        environment: &'a LaunchEnvironment,
    ) -> PlanInput<'a> {
        PlanInput {
            programs,
            environment,
            host: HostPlatform::Linux,
            identity: [("IRORI_RUNNER".to_string(), "gog".to_string())].into(),
            steam_launch: None,
        }
    }

    fn argv(spec: &CommandSpec) -> Vec<String> {
        std::iter::once(spec.program.to_string_lossy().into_owned())
            .chain(spec.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn native_programs_run_on_their_own_with_the_game_named() {
        let mut p = program("/opt/voice/voice");
        // Double quotes group an argument under both POSIX and Windows rules.
        p.args = "--minimized --room \"my room\"".into();
        p.delay_seconds = 5;
        let env = wine(WineKind::Wine, false);
        let plan = plan(&input(std::slice::from_ref(&p), &env));
        let started = &plan.start[0];
        assert_eq!(
            argv(&started.spec),
            ["/opt/voice/voice", "--minimized", "--room", "my room"]
        );
        assert_eq!(started.spec.env["IRORI_RUNNER"], "gog");
        assert_eq!(started.spec.env["WINEPREFIX"], "/p");
        // Without a drive letter "/opt/voice/voice" isn't absolute on Windows.
        if cfg!(unix) {
            assert_eq!(started.spec.cwd.as_deref(), Some(Path::new("/opt/voice")));
        }
        assert_eq!(started.delay, Duration::from_secs(5));
        assert!(!started.in_prefix && started.stop_with_game);
    }

    #[test]
    fn windows_programs_join_the_games_prefix_without_its_wrappers() {
        let mut p = program("/games/Trainer.exe");
        p.in_prefix = true;
        let programs = [p];

        let env = wine(WineKind::Proton, false);
        let started = &plan(&input(&programs, &env)).start[0];
        assert_eq!(
            argv(&started.spec),
            ["/proton", "run", "/games/Trainer.exe"]
        );
        assert_eq!(started.spec.env["WINEPREFIX"], "/p");
        assert!(started.in_prefix);

        let env = wine(WineKind::Proton, true);
        let started = &plan(&input(&programs, &env)).start[0];
        assert_eq!(argv(&started.spec), ["/umu-run", "/games/Trainer.exe"]);
        assert_eq!(started.spec.env["PROTON_VERB"], "run");

        let mut env = wine(WineKind::Wine, false);
        env.wine.as_mut().unwrap().bin = PathBuf::from("/usr/bin/wine");
        let started = &plan(&input(&programs, &env)).start[0];
        assert_eq!(argv(&started.spec), ["/usr/bin/wine", "/games/Trainer.exe"]);
        assert!(!started.spec.env.contains_key("PROTON_VERB"));
    }

    #[test]
    fn skips_what_cannot_run_and_what_is_off() {
        let mut off = program("/bin/true");
        off.enabled = false;
        let mut in_prefix = program("C:\\x.exe");
        in_prefix.in_prefix = true;
        let programs = [off, program("  "), in_prefix.clone()];
        let native_game = LaunchEnvironment::default();
        let plan_native = plan(&input(&programs, &native_game));
        assert!(plan_native.start.is_empty());
        assert_eq!(
            plan_native
                .skipped
                .iter()
                .map(|(_, s)| *s)
                .collect::<Vec<_>>(),
            [Skip::NoProgram, Skip::NoPrefix]
        );

        // A Steam game reaches its Proton prefix through protontricks-launch.
        let mut steam = input(std::slice::from_ref(&in_prefix), &native_game);
        steam.identity.insert("IRORI_RUNNER".into(), "steam".into());
        assert_eq!(plan(&steam).skipped[0].1, Skip::SteamPrefix);
        steam.steam_launch = Some(vec![
            "protontricks-launch".into(),
            "--appid".into(),
            "620".into(),
        ]);
        let started = &plan(&steam).start[0];
        assert_eq!(
            argv(&started.spec),
            ["protontricks-launch", "--appid", "620", "C:\\x.exe"]
        );
    }

    #[test]
    fn windows_hosts_run_every_program_natively() {
        let mut p = program("C:\\Tools\\trainer.exe");
        p.in_prefix = true;
        p.args = r#"-x "a b""#.into();
        p.delay_seconds = 100_000;
        let env = LaunchEnvironment::default();
        let mut windows = input(std::slice::from_ref(&p), &env);
        windows.host = HostPlatform::Windows;
        let plan = plan(&windows);
        assert_eq!(plan.start.len(), 1);
        assert!(!plan.start[0].in_prefix);
        assert_eq!(plan.start[0].spec.args.len(), 2);
        assert_eq!(
            plan.start[0].delay,
            Duration::from_secs(LaunchProgram::MAX_DELAY.into())
        );
    }

    #[test]
    fn programs_in_the_prefix_change_how_the_session_waits() {
        assert_eq!(prefix_wait(false, true, true, Some("/g")), PrefixWait::None);
        assert_eq!(
            prefix_wait(true, false, false, Some("/g")),
            PrefixWait::None
        );
        assert_eq!(
            prefix_wait(true, true, false, Some("/g")),
            PrefixWait::Wineserver
        );
        assert_eq!(
            prefix_wait(true, true, true, Some("/g")),
            PrefixWait::Folder("/g".into())
        );
        assert_eq!(prefix_wait(true, true, true, Some(" ")), PrefixWait::None);
        assert!(stops_at_end(true) && !stops_at_end(false));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn starts_after_the_delay_and_stops_with_the_game() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("game.log");
        std::fs::write(&log, "").unwrap();
        let mut keep = program("sleep");
        keep.name = "Kept".into();
        keep.args = "2".into();
        keep.stop_with_game = false;
        let mut stop = program("sleep");
        stop.name = "Stopped".into();
        stop.args = "30".into();
        let mut late = program("sleep");
        late.name = "Late".into();
        late.delay_seconds = 60;
        let programs = [keep, stop, late];
        let env = LaunchEnvironment::default();
        let session = Programs::start(plan(&input(&programs, &env)), &log);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!session.running_in_prefix());
        session.end().await;
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("Starting Kept"), "{text}");
        assert!(text.contains("Stopping Stopped: the game ended"), "{text}");
        assert!(!text.contains("Stopping Kept"), "{text}");
        assert!(
            text.contains("Not starting Late: the game ended first"),
            "{text}"
        );
    }
}
