//! Finding ffmpeg, which makes clip posters. Irori never bundles it: the
//! user's own is used, from `PATH` (and, on macOS, the folders Homebrew and
//! MacPorts install to, which a program started from the Finder doesn't
//! have on its `PATH`), else, inside Irori's Flatpak, the host's.
//!
//! The answer is kept for the run; [`find_again`] looks once more after the
//! user installed it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use irori_core::host;
use irori_core::process::CommandSpec;
use once_cell::sync::Lazy;
use tokio::sync::Mutex;

/// An ffmpeg this computer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    pub program: PathBuf,
    /// Runs on the Flatpak's host, through `flatpak-spawn --host`.
    pub host: bool,
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
    let local = which::which("ffmpeg")
        .ok()
        .or_else(|| in_dirs(&extra_dirs(), program_name()));
    if let Some(program) = local {
        return Some(Ffmpeg {
            program,
            host: false,
        });
    }
    if host::is_flatpak() {
        let program = host::find_program("ffmpeg").await?;
        return Some(Ffmpeg {
            program,
            host: true,
        });
    }
    None
}

/// This computer's ffmpeg, looked for once per run.
pub async fn find() -> Option<Arc<Ffmpeg>> {
    let mut found = FOUND.lock().await;
    if let Some(known) = &*found {
        return known.clone();
    }
    let located = locate().await.map(Arc::new);
    match &located {
        Some(f) => tracing::info!(program = %f.program.display(), host = f.host, "ffmpeg found"),
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
    fn a_host_program_runs_through_flatpak_spawn() {
        let host = Ffmpeg {
            program: "/usr/bin/ffmpeg".into(),
            host: true,
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
