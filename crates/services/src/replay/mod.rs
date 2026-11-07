//! The replay buffer: while a game Irori started runs, gpu-screen-recorder
//! keeps the last moments of play in memory, and a clip of them is saved on
//! request (a controller chord, the tray, the command palette or a global
//! shortcut). Linux only.
//!
//! Irori never bundles the recorder. It runs the one on `PATH`, else its
//! Flatpak, and the settings say how to install it when neither is there.
//! The recorder is its own process for the length of one session: Irori
//! itself does no capturing or encoding.
//!
//! A save sends the recorder SIGUSR1. It writes `Replay_<date>_<time>.mp4`
//! into the game's clip folder and prints that path, which Irori renames to
//! `<game> <date> <time>.mp4` and announces. The recorder process is found by
//! its output folder, which belongs to the session, so the Flatpak's process
//! inside its sandbox is found the same way as a native one.

mod shortcut;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::NaiveDateTime;
use irori_core::process::{CancellationToken, CommandSpec, RunOptions, Stream};
use irori_core::settings::ReplaySettings;
use irori_core::{GameId, Notice, NoticeLevel};
use parking_lot::Mutex;

use crate::Backend;

pub use shortcut::ShortcutState;

/// The recorder's Flathub id.
pub const FLATPAK_ID: &str = "com.dec05eba.gpu_screen_recorder";
/// The recorder's program name, on `PATH` and inside its Flatpak.
const PROGRAM: &str = "gpu-screen-recorder";
/// Clips are MP4: every player and video site takes them.
const CONTAINER: &str = "mp4";
/// How long the recorder gets to stop before its process group is ended.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// A gpu-screen-recorder Irori can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorder {
    /// A program on `PATH`.
    Native(PathBuf),
    /// The Flathub package.
    Flatpak,
}

impl Recorder {
    /// The recorder on this system: on `PATH` first, then the Flatpak (user
    /// or system install). `None` on other systems. Blocking.
    pub fn detect() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        let native = which::which_in(PROGRAM, std::env::var_os("PATH"), &cwd)
            .ok()
            .filter(|p| !p.to_string_lossy().contains("/flatpak/exports/bin/"));
        if let Some(program) = native {
            return Some(Self::Native(program));
        }
        let home = dirs::home_dir().unwrap_or_default();
        [
            home.join(".local/share/flatpak/app"),
            PathBuf::from("/var/lib/flatpak/app"),
        ]
        .iter()
        .any(|root| root.join(FLATPAK_ID).join("current").exists())
        .then_some(Self::Flatpak)
    }

    /// The command that runs the recorder with `args`, letting a Flatpak
    /// write to `dir`.
    pub fn command(&self, args: Vec<String>, dir: &Path) -> CommandSpec {
        match self {
            Self::Native(program) => CommandSpec::new(program).args(args),
            Self::Flatpak => CommandSpec::new("flatpak")
                .arg("run")
                .arg(format!("--filesystem={}", dir.display()))
                .arg(format!("--command={PROGRAM}"))
                .arg(FLATPAK_ID)
                .args(args),
        }
    }
}

/// The bitrate, in kbit/s, of a quality preset. A constant bitrate keeps the
/// buffer's memory the same whatever is on screen, as the recorder advises
/// for replays.
pub fn bitrate_kbps(quality: &str) -> u32 {
    match quality {
        "medium" => 10_000,
        "high" => 20_000,
        "ultra" => 60_000,
        _ => 35_000,
    }
}

/// About how much memory the buffer holds, in MiB.
pub fn buffer_mib(settings: &ReplaySettings) -> u64 {
    // Audio adds 128 kbit/s at most; the encoded video dominates.
    let kbps = u64::from(bitrate_kbps(&settings.quality)) + 128;
    kbps * 1000 / 8 * u64::from(settings.seconds.max(1)) / (1024 * 1024)
}

/// The recorder's arguments for a replay buffer writing into `dir`.
pub fn args(settings: &ReplaySettings, dir: &Path) -> Vec<String> {
    let capture = if settings.capture == "portal" {
        "portal"
    } else {
        "screen"
    };
    let mut args: Vec<String> = vec![
        "-w".into(),
        capture.into(),
        "-c".into(),
        CONTAINER.into(),
        "-f".into(),
        "60".into(),
        "-r".into(),
        settings.seconds.clamp(5, 1800).to_string(),
        "-bm".into(),
        "cbr".into(),
        "-q".into(),
        bitrate_kbps(&settings.quality).to_string(),
        "-v".into(),
        "no".into(),
    ];
    if capture == "portal" {
        // Ask what to record once, not at every game.
        args.extend(["-restore-portal-session".into(), "yes".into()]);
    }
    match settings.audio.as_str() {
        "none" => {}
        "desktop_and_mic" => args.extend(["-a".into(), "default_output|default_input".into()]),
        _ => args.extend(["-a".into(), "default_output".into()]),
    }
    args.extend(["-o".into(), dir.to_string_lossy().into_owned()]);
    args
}

/// Where clips go: the chosen folder, else `Videos/Irori`.
pub fn clips_root(settings: &ReplaySettings) -> PathBuf {
    let chosen = settings.folder.trim();
    if !chosen.is_empty() {
        return irori_core::paths::expand_home(chosen);
    }
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    dirs::video_dir()
        .unwrap_or_else(|| home.join("Videos"))
        .join("Irori")
}

/// The folder of one game's clips.
pub fn game_folder(root: &Path, title: &str) -> PathBuf {
    root.join(irori_core::paths::sanitize(title.trim()))
}

/// The file name of a clip of `title` saved at local time `at`, which the
/// album reads the time back from.
pub fn clip_name(title: &str, at: NaiveDateTime, ext: &str) -> String {
    let title = irori_core::paths::sanitize(title.trim());
    format!("{title} {}.{ext}", at.format("%Y-%m-%d %H-%M-%S"))
}

/// `name` in `dir`, numbered when a file of that name is already there.
pub(crate) fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

/// What the recorder printed on its standard output.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Output {
    Saved(PathBuf),
    SaveFailed,
    Other,
}

fn parse_output(line: &str) -> Output {
    let line = line.trim();
    if line.starts_with("gsr error:") && line.contains("replay") {
        Output::SaveFailed
    } else if line.starts_with('/') && line.ends_with(&format!(".{CONTAINER}")) {
        Output::Saved(PathBuf::from(line))
    } else {
        Output::Other
    }
}

/// The replay buffer of the session it records.
struct Active {
    id: GameId,
    title: String,
    dir: PathBuf,
    seconds: u32,
    cancel: CancellationToken,
    /// The user asked it to stop (the session ended): an exit is no failure.
    stopping: Arc<std::sync::atomic::AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

/// The replay buffer's state, owned by the backend.
#[derive(Default)]
pub(crate) struct ReplayState {
    active: Mutex<Option<Active>>,
    pub(crate) shortcut: shortcut::Shortcut,
}

/// A replay buffer that is recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayInfo {
    pub game: GameId,
    pub title: String,
    pub seconds: u32,
}

impl Backend {
    /// The replay buffer recording now, if any.
    pub fn replay(&self) -> Option<ReplayInfo> {
        self.replay.active.lock().as_ref().map(|a| ReplayInfo {
            game: a.id.clone(),
            title: a.title.clone(),
            seconds: a.seconds,
        })
    }

    /// Start a replay buffer for a session of `title`, when the game's
    /// settings turn it on and nothing records yet. Problems are told as a
    /// notice; the game plays regardless. Looking for the recorder takes a
    /// lookup on `PATH` and two `stat`s, so the buffer is known to record
    /// (or not) when this returns.
    pub(crate) fn start_replay(&self, id: &GameId, title: &str, settings: &ReplaySettings) {
        if !settings.enabled || !cfg!(target_os = "linux") || self.replay.active.lock().is_some() {
            return;
        }
        let b = self.clone();
        self.spawn(async move { b.listen_for_clip_shortcut().await });
        let not_recording = |message: Notice| message.game(id.clone());
        let Some(recorder) = Recorder::detect() else {
            self.bus.notice(not_recording(
                Notice::new(NoticeLevel::Warning, "The replay buffer isn't recording").keys(
                    "shell.notice.replay_not_recording",
                    Some("shell.notice.replay_missing"),
                    &[],
                ),
            ));
            return;
        };
        if let Err(e) = self.run_replay(recorder, id, title, settings) {
            tracing::warn!("replay buffer for {id}: {e:#}");
            self.bus.notice(not_recording(
                Notice::new(NoticeLevel::Warning, "The replay buffer isn't recording")
                    .message(format!("{e:#}"))
                    .keys("shell.notice.replay_not_recording", None, &[]),
            ));
        }
    }

    fn run_replay(
        &self,
        recorder: Recorder,
        id: &GameId,
        title: &str,
        settings: &ReplaySettings,
    ) -> Result<()> {
        let dir = game_folder(&clips_root(settings), title);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        let spec = recorder.command(args(settings, &dir), &dir);
        let cancel = CancellationToken::new();
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let b = self.clone();
        let (game, name) = (id.clone(), title.to_owned());
        let on_line = move |stream: Stream, line: &str| {
            if stream != Stream::Stdout {
                return;
            }
            match parse_output(line) {
                Output::Saved(path) => b.clip_saved(&game, &name, path),
                Output::SaveFailed => b.bus.notice(
                    Notice::new(NoticeLevel::Error, "The clip wasn't saved")
                        .keys("shell.notice.clip_failed", None, &[])
                        .game(game.clone()),
                ),
                Output::Other => {}
            }
        };
        // Held until the buffer is registered, so a recorder that fails at
        // once finds it to clear.
        let mut active = self.replay.active.lock();
        let task = {
            let (b, id, cancel, stopping) =
                (self.clone(), id.clone(), cancel.clone(), stopping.clone());
            self.spawn(async move {
                let options = RunOptions::default().on_line(on_line).cancel(cancel);
                let result = irori_core::process::run(&spec, options).await;
                let stopped = stopping.load(std::sync::atomic::Ordering::Relaxed);
                {
                    let mut active = b.replay.active.lock();
                    if active.as_ref().is_some_and(|a| a.id == id) {
                        *active = None;
                    }
                }
                let reason = match result {
                    Ok(r) if stopped || r.success() => return,
                    Ok(r) => r.error_summary(),
                    Err(e) => format!("{e:#}"),
                };
                tracing::warn!("the replay buffer of {id} stopped: {reason}");
                b.bus.notice(
                    Notice::new(NoticeLevel::Warning, "The replay buffer stopped")
                        .message(reason)
                        .keys("shell.notice.replay_stopped", None, &[])
                        .game(id),
                );
            })
        };
        tracing::info!("replay buffer of {id} records into {}", dir.display());
        *active = Some(Active {
            id: id.clone(),
            title: title.to_owned(),
            dir,
            seconds: settings.seconds,
            cancel,
            stopping,
            task,
        });
        Ok(())
    }

    /// Stop the replay buffer of `id`'s session without saving.
    pub(crate) async fn stop_replay(&self, id: &GameId) {
        let active = {
            let mut active = self.replay.active.lock();
            match active.as_ref() {
                Some(a) if a.id == *id => active.take(),
                _ => None,
            }
        };
        let Some(active) = active else { return };
        active
            .stopping
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // SIGINT stops a replay buffer without saving it.
        let dir = active.dir.clone();
        let _ = self
            .spawn_blocking(move || signal_recorder(&dir, sysinfo::Signal::Interrupt))
            .await;
        let mut task = active.task;
        if tokio::time::timeout(STOP_GRACE, &mut task).await.is_err() {
            active.cancel.cancel();
            let _ = task.await;
        }
    }

    /// Save the replay buffer as a clip. The result arrives as a notice:
    /// the clip, or why there is none.
    pub fn save_clip(&self) {
        let b = self.clone();
        self.spawn(async move {
            let Some(dir) = b.replay.active.lock().as_ref().map(|a| a.dir.clone()) else {
                b.bus.notice(
                    Notice::new(NoticeLevel::Info, "The replay buffer isn't recording").keys(
                        "shell.notice.replay_not_recording",
                        Some("shell.notice.replay_turn_on"),
                        &[],
                    ),
                );
                return;
            };
            let sent = b
                .spawn_blocking(move || signal_recorder(&dir, sysinfo::Signal::User1))
                .await
                .unwrap_or(false);
            if !sent {
                b.bus.notice(
                    Notice::new(NoticeLevel::Error, "The clip wasn't saved").keys(
                        "shell.notice.clip_failed",
                        None,
                        &[],
                    ),
                );
            }
        });
    }

    /// Give the clip the recorder wrote its final name, and say so.
    fn clip_saved(&self, id: &GameId, title: &str, path: PathBuf) {
        let at = chrono::Local::now().naive_local();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let target = free_path(&dir, &clip_name(title, at, CONTAINER));
        let path = match std::fs::rename(&path, &target) {
            Ok(()) => target,
            Err(e) => {
                tracing::warn!("renaming the clip {}: {e}", path.display());
                path
            }
        };
        tracing::info!("clip of {id} saved to {}", path.display());
        let shown = path.display().to_string();
        self.bus.notice(
            Notice::new(NoticeLevel::Success, format!("Clip of {title} saved"))
                .message(shown)
                .keys("shell.notice.clip_saved", None, &[("title", title)])
                .game(id.clone()),
        );
    }
}

/// Send `signal` to the recorder writing into `dir`. Returns whether one was
/// found. Blocking.
fn signal_recorder(dir: &Path, signal: sysinfo::Signal) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let dir = dir.as_os_str();
    let mut found = false;
    for process in sys.processes().values() {
        if is_recorder_for(process.cmd(), dir) {
            found |= process.kill_with(signal).unwrap_or(false);
        }
    }
    found
}

/// Whether a command line is the recorder's own (not `flatpak run` or
/// `bwrap`, whose arguments hold the same words) writing into `dir`.
fn is_recorder_for(cmd: &[std::ffi::OsString], dir: &std::ffi::OsStr) -> bool {
    let program = cmd
        .first()
        .and_then(|p| Path::new(p).file_name())
        .is_some_and(|name| name == PROGRAM);
    program && cmd.windows(2).any(|w| w[0] == "-o" && w[1] == dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn settings() -> ReplaySettings {
        ReplaySettings {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn records_the_screen_at_a_constant_bitrate_with_the_desktop_sound() {
        let a = args(&settings(), Path::new("/clips/Hades"));
        assert_eq!(
            a.join(" "),
            "-w screen -c mp4 -f 60 -r 60 -bm cbr -q 35000 -v no -a default_output -o /clips/Hades"
        );
    }

    #[test]
    fn audio_capture_and_length_follow_the_settings() {
        let s = ReplaySettings {
            seconds: 99_999,
            quality: "medium".into(),
            audio: "desktop_and_mic".into(),
            capture: "portal".into(),
            ..settings()
        };
        let a = args(&s, Path::new("/c"));
        let text = a.join(" ");
        assert!(text.starts_with("-w portal "));
        assert!(text.contains(" -r 1800 "));
        assert!(text.contains(" -q 10000 "));
        assert!(text.contains(" -restore-portal-session yes "));
        // One argument, so both sources mix into one track.
        assert!(a.contains(&"default_output|default_input".to_string()));
        let silent = args(
            &ReplaySettings {
                audio: "none".into(),
                capture: "anything".into(),
                ..settings()
            },
            Path::new("/c"),
        );
        assert!(!silent.contains(&"-a".to_string()));
        assert_eq!(silent[1], "screen");
    }

    #[test]
    fn the_flatpak_may_write_to_the_clip_folder() {
        let spec = Recorder::Flatpak.command(vec!["-r".into(), "60".into()], Path::new("/v/H"));
        assert_eq!(spec.program, PathBuf::from("flatpak"));
        let args: Vec<_> = spec.args.iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            args,
            [
                "run",
                "--filesystem=/v/H",
                "--command=gpu-screen-recorder",
                FLATPAK_ID,
                "-r",
                "60"
            ]
        );
        let native = Recorder::Native("/usr/bin/gpu-screen-recorder".into())
            .command(vec!["-r".into()], Path::new("/v"));
        assert_eq!(
            native.program,
            PathBuf::from("/usr/bin/gpu-screen-recorder")
        );
        assert_eq!(native.args, [OsString::from("-r")]);
    }

    #[test]
    fn clips_are_named_after_the_game_and_the_moment() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 4)
            .unwrap()
            .and_hms_opt(19, 15, 5)
            .unwrap();
        assert_eq!(
            clip_name("Hades II", at, "mp4"),
            "Hades II 2026-11-04 19-15-05.mp4"
        );
        assert_eq!(
            clip_name(" Half-Life: Alyx ", at, "mp4"),
            "Half-Life_ Alyx 2026-11-04 19-15-05.mp4"
        );
        assert_eq!(
            game_folder(Path::new("/v"), "../.."),
            PathBuf::from("/v/.._..")
        );
        assert_eq!(game_folder(Path::new("/v"), ".."), PathBuf::from("/v/_"));
    }

    #[test]
    fn a_taken_name_is_numbered() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_path(dir.path(), "a b.mp4"), dir.path().join("a b.mp4"));
        std::fs::write(dir.path().join("a b.mp4"), "").unwrap();
        std::fs::write(dir.path().join("a b (2).mp4"), "").unwrap();
        assert_eq!(
            free_path(dir.path(), "a b.mp4"),
            dir.path().join("a b (3).mp4")
        );
    }

    #[test]
    fn reads_saved_paths_and_failures_from_the_recorder() {
        assert_eq!(
            parse_output("/home/u/Videos/Irori/Hades/Replay_2026-11-04_19-15-05.mp4\n"),
            Output::Saved("/home/u/Videos/Irori/Hades/Replay_2026-11-04_19-15-05.mp4".into())
        );
        assert_eq!(
            parse_output("gsr error: Failed to save replay"),
            Output::SaveFailed
        );
        assert_eq!(parse_output("update fps: 60"), Output::Other);
    }

    #[test]
    fn finds_the_recorder_not_its_sandbox() {
        let dir = std::ffi::OsStr::new("/v/Hades");
        let cmd = |words: &[&str]| -> Vec<OsString> { words.iter().map(OsString::from).collect() };
        assert!(is_recorder_for(
            &cmd(&[
                "/app/bin/gpu-screen-recorder",
                "-w",
                "screen",
                "-o",
                "/v/Hades"
            ]),
            dir
        ));
        assert!(!is_recorder_for(
            &cmd(&["bwrap", "--args", "gpu-screen-recorder", "-o", "/v/Hades"]),
            dir
        ));
        assert!(!is_recorder_for(
            &cmd(&["gpu-screen-recorder", "-o", "/v/Hades II"]),
            dir
        ));
    }

    #[test]
    fn memory_follows_length_and_quality() {
        assert_eq!(buffer_mib(&settings()), 251);
        let short = ReplaySettings {
            seconds: 30,
            quality: "medium".into(),
            ..settings()
        };
        assert_eq!(buffer_mib(&short), 36);
    }
}
