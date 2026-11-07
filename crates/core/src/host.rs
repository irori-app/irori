//! Programs and processes of the host system while Irori runs as a Flatpak.
//!
//! Inside the sandbox `/usr` is the runtime's, other apps' `~/.var/app`
//! folders are hidden and the host's processes can't be seen. Programs the
//! user installed (Steam, Ludusavi, another Flatpak) are therefore looked up,
//! started and followed on the host through `flatpak-spawn --host`, which the
//! manifest's `--talk-name=org.freedesktop.Flatpak` allows. Outside a Flatpak
//! every function here works on this system directly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::process::{self, CommandSpec, RunOptions};

/// Whether Irori runs inside a Flatpak sandbox.
pub fn is_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()
}

/// `spec` as it runs on the host: through `flatpak-spawn --host` inside a
/// Flatpak, unchanged otherwise. The host program outlives Irori, as a
/// client it starts (Steam) must.
pub fn command(spec: CommandSpec) -> CommandSpec {
    if is_flatpak() {
        flatpak_spawn(spec, false)
    } else {
        spec
    }
}

/// Like [`command`], but the host program ends when its `flatpak-spawn` does,
/// so cancelling the command or Irori quitting stops it too.
pub fn tied_command(spec: CommandSpec) -> CommandSpec {
    if is_flatpak() {
        flatpak_spawn(spec, true)
    } else {
        spec
    }
}

/// `spec` wrapped in `flatpak-spawn --host`. Its variables, removed
/// variables and folder become `flatpak-spawn` options, since the host
/// program starts from the host session's environment, not the sandbox's.
/// `watch_bus` ends the host program when `flatpak-spawn` exits.
pub fn flatpak_spawn(spec: CommandSpec, watch_bus: bool) -> CommandSpec {
    let mut wrapped = CommandSpec::new("flatpak-spawn").arg("--host");
    if watch_bus {
        wrapped = wrapped.arg("--watch-bus");
    }
    if let Some(dir) = &spec.cwd {
        let mut option = OsString::from("--directory=");
        option.push(dir);
        wrapped = wrapped.arg(option);
    }
    for (key, value) in &spec.env {
        wrapped = wrapped.arg(format!("--env={key}={value}"));
    }
    for key in &spec.env_remove {
        wrapped = wrapped.arg(format!("--unset-env={key}"));
    }
    wrapped.arg(spec.program).args(spec.args)
}

/// Where the host finds the program `name` (a name looked up on `PATH`, or
/// a path that must be an executable file). `None` when it has none.
pub async fn find_program(name: &str) -> Option<PathBuf> {
    if !is_flatpak() {
        return which::which(name).ok();
    }
    let spec = flatpak_spawn(
        CommandSpec::new("sh").args(["-c", r#"command -v "$1""#, "sh", name]),
        false,
    );
    let out = process::run(&spec, RunOptions::default()).await.ok()?;
    if !out.success() {
        return None;
    }
    // `command -v` names shell builtins and functions without a path.
    out.stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .map(PathBuf::from)
}

/// Whether the Flatpak app `id` is installed on the host, for this user or
/// system-wide.
pub async fn has_flatpak_app(id: &str) -> bool {
    let Some(flatpak) = find_program("flatpak").await else {
        return false;
    };
    let spec = command(CommandSpec::new(flatpak).args(["info", "--show-ref", id]));
    process::run(&spec, RunOptions::default())
        .await
        .is_ok_and(|out| out.success())
}

/// A process running on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcess {
    pub pid: u32,
    /// Its command line, arguments joined by spaces.
    pub command_line: String,
}

impl HostProcess {
    /// The command line split on spaces: an argument holding a space comes
    /// back as several.
    pub fn args(&self) -> Vec<String> {
        self.command_line
            .split_whitespace()
            .map(String::from)
            .collect()
    }
}

/// The host's processes, or only those of `pids` when given. Used inside a
/// Flatpak, whose own process list holds just the sandbox's.
pub async fn processes(pids: Option<&[u32]>) -> Result<Vec<HostProcess>> {
    let mut ps = CommandSpec::new("ps").args(["-ww", "-o", "pid=", "-o", "args="]);
    match pids {
        Some([]) => return Ok(Vec::new()),
        Some(pids) => {
            let list: Vec<String> = pids.iter().map(u32::to_string).collect();
            ps = ps.arg("-p").arg(list.join(","));
        }
        None => ps = ps.arg("-e"),
    }
    // `ps` exits with 1 when none of the processes asked for exists.
    let out = process::run(&command(ps), RunOptions::default()).await?;
    if !out.success() && !out.stdout.trim().is_empty() {
        anyhow::bail!("{}", out.error_summary());
    }
    Ok(parse_ps(&out.stdout))
}

/// Sends the host processes `pids` SIGTERM, or SIGKILL when `force`.
pub async fn signal(pids: &[u32], force: bool) -> Result<()> {
    if pids.is_empty() {
        return Ok(());
    }
    let kill = CommandSpec::new("kill")
        .arg(if force { "-KILL" } else { "-TERM" })
        .args(pids.iter().map(u32::to_string));
    process::run(&command(kill), RunOptions::default()).await?;
    Ok(())
}

/// `ps -o pid= -o args=` output: one process per line, its PID first.
fn parse_ps(stdout: &str) -> Vec<HostProcess> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            Some(HostProcess {
                pid: pid.parse().ok()?,
                command_line: rest.trim().to_string(),
            })
        })
        .filter(|p| !p.command_line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(spec: &CommandSpec) -> Vec<String> {
        std::iter::once(spec.program.to_string_lossy().into_owned())
            .chain(spec.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn wraps_a_command_for_the_host() {
        let mut spec = CommandSpec::new("/usr/bin/ludusavi")
            .args(["backup", "--api"])
            .env("LANG", "de_DE.UTF-8")
            .cwd("/home/u/Games");
        spec.env_remove.push("LD_PRELOAD".into());
        let wrapped = flatpak_spawn(spec, true);
        assert_eq!(
            argv(&wrapped),
            [
                "flatpak-spawn",
                "--host",
                "--watch-bus",
                "--directory=/home/u/Games",
                "--env=LANG=de_DE.UTF-8",
                "--unset-env=LD_PRELOAD",
                "/usr/bin/ludusavi",
                "backup",
                "--api",
            ]
        );
        // The variables travel as options, not on flatpak-spawn itself.
        assert!(wrapped.env.is_empty());
        assert!(wrapped.cwd.is_none());

        let plain = flatpak_spawn(CommandSpec::new("steam").arg("-applaunch"), false);
        assert_eq!(
            argv(&plain),
            ["flatpak-spawn", "--host", "steam", "-applaunch"]
        );
    }

    #[test]
    fn reads_the_process_list() {
        let out = "      1 /sbin/init splash\n\
                   4242 /home/u/.local/share/Steam/ubuntu12_32/reaper SteamLaunch AppId=440 -- hl2.sh\n\
                    77 \n\
                   garbage line\n";
        let list = parse_ps(out);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].pid, 1);
        assert_eq!(list[0].command_line, "/sbin/init splash");
        assert_eq!(list[1].pid, 4242);
        assert_eq!(
            list[1].args()[..3],
            [
                "/home/u/.local/share/Steam/ubuntu12_32/reaper",
                "SteamLaunch",
                "AppId=440"
            ]
        );
    }
}
