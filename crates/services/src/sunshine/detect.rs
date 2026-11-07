//! Finding Sunshine on this computer: its configuration folder and the port
//! of its web UI, which is one above the `port` its configuration names.

use std::path::{Path, PathBuf};

/// Sunshine's own default `port`; its web UI listens one above it.
const DEFAULT_PORT: u16 = 47989;

/// Sunshine on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SunshineInstall {
    /// The folder holding `sunshine.conf` and `apps.json`.
    pub config_dir: PathBuf,
    /// Installed from Flathub.
    pub flatpak: bool,
    /// The port of its web UI.
    pub web_port: u16,
}

/// Where Sunshine keeps its configuration, most likely first.
fn candidates() -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramW6432", "ProgramFiles"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push((PathBuf::from(dir).join("Sunshine").join("config"), false));
            }
        }
    } else if let Some(home) = dirs::home_dir() {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        out.push((config.join("sunshine"), false));
        if cfg!(target_os = "linux") {
            out.push((
                home.join(".var/app/dev.lizardbyte.app.Sunshine/config/sunshine"),
                true,
            ));
        }
    }
    out.dedup();
    out
}

/// Sunshine as its configuration shows it, if it was ever started here.
pub fn detect() -> Option<SunshineInstall> {
    candidates().into_iter().find_map(|(dir, flatpak)| {
        let conf = dir.join("sunshine.conf");
        let seen = conf.is_file() || dir.join("apps.json").is_file();
        seen.then(|| SunshineInstall {
            web_port: web_port(&std::fs::read_to_string(&conf).unwrap_or_default()),
            config_dir: dir,
            flatpak,
        })
    })
}

/// The web UI's port for the `sunshine.conf` text `conf`.
pub(crate) fn web_port(conf: &str) -> u16 {
    conf.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| key.trim() == "port")
        .filter_map(|(_, value)| value.trim().parse::<u16>().ok())
        .next_back()
        .unwrap_or(DEFAULT_PORT)
        .saturating_add(1)
}

/// Whether Sunshine's program is installed where it is usually put, even
/// before its first start.
pub fn program_installed() -> bool {
    if which::which("sunshine").is_ok() {
        return true;
    }
    let fixed: &[&str] = if cfg!(windows) {
        &[]
    } else if cfg!(target_os = "macos") {
        &["/opt/homebrew/bin/sunshine", "/usr/local/bin/sunshine"]
    } else {
        &[
            "/var/lib/flatpak/app/dev.lizardbyte.app.Sunshine",
            "/usr/bin/sunshine",
        ]
    };
    if fixed.iter().any(|p| Path::new(p).exists()) {
        return true;
    }
    if cfg!(windows) {
        return candidates()
            .iter()
            .any(|(config, _)| config.with_file_name("sunshine.exe").is_file());
    }
    dirs::home_dir().is_some_and(|home| {
        home.join(".local/share/flatpak/app/dev.lizardbyte.app.Sunshine")
            .exists()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_port_follows_the_configured_port() {
        assert_eq!(web_port(""), 47990);
        assert_eq!(web_port("# port = 1\nport = 50000\n"), 50001);
        assert_eq!(web_port("port=48000"), 48001);
        assert_eq!(web_port("port = nonsense"), 47990);
        assert_eq!(web_port("min_log_level = 2\nport = 47000"), 47001);
    }
}
