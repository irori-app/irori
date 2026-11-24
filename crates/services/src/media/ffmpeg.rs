//! Finding ffmpeg, which makes clip posters and plays trailers. Irori never
//! bundles it: the user's own is used, from `PATH` (and, on macOS, the
//! folders Homebrew and MacPorts install to, which a program started from
//! the Finder doesn't have on its `PATH`), else, inside Irori's Flatpak, the
//! host's. On Windows, where few people have one, the user may have Irori
//! download one ([`super::ffmpeg_download`]) into `<data>/tools/ffmpeg`,
//! which is looked in first.
//!
//! The answer, with the sound output it can play through, is kept for the
//! run; [`find_again`] looks once more after the user installed one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use irori_core::process::{self, CommandSpec, RunOptions};
use irori_core::{Paths, host};
use once_cell::sync::Lazy;
use tokio::sync::Mutex;

/// An ffmpeg this computer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    pub program: PathBuf,
    /// Runs on the Flatpak's host, through `flatpak-spawn --host`.
    pub host: bool,
    /// The output device ffmpeg plays sound through here (`pulse`, `alsa`,
    /// `audiotoolbox`), when it was built with one. ffmpeg has none for
    /// Windows, where trailers stay muted.
    pub audio_output: Option<String>,
}

impl Ffmpeg {
    /// A command running ffmpeg with `args`. A host ffmpeg ends with its
    /// `flatpak-spawn`, so stopping the command stops it.
    pub fn command<I, S>(&self, args: I) -> CommandSpec
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let spec = CommandSpec::new(&self.program).args(args);
        if self.host {
            host::tied_command(spec)
        } else {
            spec
        }
    }
}

static FOUND: Lazy<Mutex<Option<Option<Arc<Ffmpeg>>>>> = Lazy::new(|| Mutex::new(None));

/// Folders looked in beyond `PATH`.
fn extra_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    } else {
        Vec::new()
    }
}

/// Where a downloaded ffmpeg is kept.
pub fn managed_dir() -> PathBuf {
    Paths::get().tools_dir().join("ffmpeg")
}

/// The output devices of `ffmpeg -devices` that play sound, by preference
/// on this system: PulseAudio (PipeWire answers it too), then ALSA, on
/// Linux; AudioToolbox on macOS.
pub fn audio_output(devices: &str) -> Option<String> {
    let muxers: Vec<&str> = devices
        .lines()
        .skip_while(|l| l.trim() != "---")
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let flags = parts.next()?;
            let name = parts.next()?;
            // `DE`, ` E`: the second column is muxing (output).
            let output = if flags.len() == 2 {
                flags.ends_with('E')
            } else {
                flags == "E"
            };
            output.then_some(name)
        })
        .collect();
    let wanted: &[&str] = if cfg!(target_os = "macos") {
        &["audiotoolbox"]
    } else if cfg!(target_os = "linux") {
        &["pulse", "alsa"]
    } else {
        &[]
    };
    wanted
        .iter()
        .find(|w| muxers.contains(w))
        .map(|w| w.to_string())
}

/// The program's file name on this system.
fn program_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// The first of `dirs` holding an executable `name`.
fn in_dirs(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_program(p))
}

fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

async fn locate() -> Option<Ffmpeg> {
    let local = in_dirs(&[managed_dir()], program_name())
        .or_else(|| which::which("ffmpeg").ok())
        .or_else(|| in_dirs(&extra_dirs(), program_name()));
    let mut found = match local {
        Some(program) => Ffmpeg {
            program,
            host: false,
            audio_output: None,
        },
        None if host::is_flatpak() => Ffmpeg {
            program: host::find_program("ffmpeg").await?,
            host: true,
            audio_output: None,
        },
        None => return None,
    };
    let devices = found.command(["-hide_banner", "-devices"]);
    if let Ok(run) = process::run(&devices, RunOptions::default()).await {
        found.audio_output = audio_output(&format!("{}\n{}", run.stdout, run.stderr));
    }
    Some(found)
}

/// This computer's ffmpeg, looked for once per run.
pub async fn find() -> Option<Arc<Ffmpeg>> {
    let mut found = FOUND.lock().await;
    if let Some(known) = &*found {
        return known.clone();
    }
    let located = locate().await.map(Arc::new);
    match &located {
        Some(f) => tracing::info!(
            program = %f.program.display(),
            host = f.host,
            sound = f.audio_output.as_deref().unwrap_or("none"),
            "ffmpeg found"
        ),
        None => tracing::info!("no ffmpeg found"),
    }
    *found = Some(located.clone());
    located
}

/// Look for ffmpeg again (the user installed it meanwhile).
pub async fn find_again() -> Option<Arc<Ffmpeg>> {
    *FOUND.lock().await = None;
    find().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_program_in_the_extra_folders() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(program_name());
        std::fs::write(&program, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // Not executable yet: not a program.
            assert_eq!(in_dirs(&[dir.path().into()], program_name()), None);
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            in_dirs(
                &[PathBuf::from("/nowhere"), dir.path().into()],
                program_name()
            ),
            Some(program)
        );
    }

    #[test]
    fn picks_this_systems_sound_output() {
        let devices = "Devices:\n D. = Demuxing supported\n .E = Muxing supported\n ---\n \
                       DE alsa            ALSA audio output\n  E audiotoolbox    \
                       AudioToolbox output device\n D  avfoundation    AVFoundation input\n \
                       DE pulse           Pulse audio output\n  E sdl,sdl2        SDL2 output\n";
        let picked = audio_output(devices);
        if cfg!(target_os = "macos") {
            assert_eq!(picked.as_deref(), Some("audiotoolbox"));
        } else if cfg!(target_os = "linux") {
            assert_eq!(picked.as_deref(), Some("pulse"));
            let alsa_only = devices.replace("pulse", "other");
            assert_eq!(audio_output(&alsa_only).as_deref(), Some("alsa"));
        } else {
            assert_eq!(picked, None);
        }
        // Input devices don't count, nor does anything before the rule.
        assert_eq!(audio_output(" DE pulse\n D  pulse\n"), None);
        assert_eq!(audio_output("---\n D  pulse  input\n D  alsa\n"), None);
    }

    #[test]
    fn a_host_program_runs_through_flatpak_spawn() {
        let host = Ffmpeg {
            program: "/usr/bin/ffmpeg".into(),
            host: true,
            audio_output: None,
        };
        let local = Ffmpeg {
            host: false,
            ..host.clone()
        };
        assert_eq!(
            local.command(["-version"]).program,
            Path::new("/usr/bin/ffmpeg")
        );
        // Outside a Flatpak `tied_command` changes nothing either.
        if host::is_flatpak() {
            assert_eq!(
                host.command(["-version"]).program,
                Path::new("flatpak-spawn")
            );
        }
    }
}
