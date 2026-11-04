//! Signing emulators in to RetroAchievements with the user's account, by
//! writing the settings each one documents for it:
//!
//! - RetroArch: `cheevos_*` in the `retroarch.cfg` Irori runs it with.
//! - PCSX2: `[Achievements]` in `inis/PCSX2.ini`, the token in
//!   `inis/secrets.ini`.
//! - PPSSPP: `[Achievements]` in `PSP/SYSTEM/ppsspp.ini`, the token in
//!   `ppsspp_retroachievements.dat` beside it.
//! - Dolphin: `[Achievements]` in `Config/RetroAchievements.ini`.
//! - DuckStation keeps its token encrypted with a key of this computer, so
//!   Irori only turns achievements on and the user signs in there.
//!
//! The token is the one RetroAchievements gives emulators at sign-in (not
//! the Web API key). Existing settings are kept; only these keys change.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::discovery::{Installation, Origin};
use crate::host::Os;
use irori_core::json_store;

/// Engines Irori can sign in or turn achievements on for.
pub const SUPPORTED: [&str; 5] = ["retroarch", "pcsx2", "ppsspp", "dolphin", "duckstation"];

/// The account an emulator signs in with.
#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    /// The emulators' token (`dorequest.php?r=login2`).
    pub token: String,
    /// Hardcore mode: no save states, rewinding or cheats.
    pub hardcore: bool,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("username", &self.username)
            .field("hardcore", &self.hardcore)
            .finish_non_exhaustive()
    }
}

/// What signing an engine in did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Signed in; the settings file written.
    SignedIn(PathBuf),
    /// Achievements turned on; the user signs in inside the emulator.
    Enabled(PathBuf),
}

/// Folders the emulators keep their settings under, resolved once so
/// tests can point them at a temporary folder.
#[derive(Debug, Clone)]
pub struct Roots {
    pub os: Os,
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, else `~/.config`.
    pub xdg_config: PathBuf,
    /// The user's Documents folder.
    pub documents: PathBuf,
    /// Irori's folder for RetroArch's settings.
    pub retroarch_home: PathBuf,
}

impl Roots {
    pub fn current() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Self {
            os: Os::current(),
            documents: dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
            xdg_config,
            retroarch_home: crate::retroarch::home(),
            home,
        }
    }

    /// A Flatpak's `~/.config`.
    fn flatpak_config(&self, app: &str) -> PathBuf {
        self.home.join(".var/app").join(app).join("config")
    }
}

/// Whether a managed Windows install runs in portable mode (its settings
/// beside the program), as Irori sets them up.
fn portable_dir(install: &Installation, roots: &Roots) -> Option<PathBuf> {
    (roots.os == Os::Windows && install.origin == Origin::Managed)
        .then(|| install.dir.clone())
        .flatten()
}

/// Settings folder of PCSX2 (holding `inis/`).
fn pcsx2_root(install: &Installation, roots: &Roots) -> PathBuf {
    if let Some(dir) = portable_dir(install, roots) {
        return dir;
    }
    match (roots.os, install.origin) {
        (Os::Linux, Origin::Flatpak) => roots.flatpak_config("net.pcsx2.PCSX2").join("PCSX2"),
        (Os::Linux, _) => roots.xdg_config.join("PCSX2"),
        (Os::Macos, _) => roots.home.join("Library/Application Support/PCSX2"),
        (Os::Windows, _) => roots.documents.join("PCSX2"),
    }
}

/// PPSSPP's `PSP/SYSTEM` folder.
fn ppsspp_system(install: &Installation, roots: &Roots) -> PathBuf {
    let root = if let Some(dir) = portable_dir(install, roots) {
        dir.join("memstick")
    } else {
        match (roots.os, install.origin) {
            (Os::Linux, Origin::Flatpak) => {
                roots.flatpak_config("org.ppsspp.PPSSPP").join("ppsspp")
            }
            (Os::Windows, _) => roots.documents.join("PPSSPP"),
            _ => roots.xdg_config.join("ppsspp"),
        }
    };
    root.join("PSP").join("SYSTEM")
}

/// Dolphin's folder of `.ini` files.
fn dolphin_config(install: &Installation, roots: &Roots) -> PathBuf {
    if let Some(dir) = portable_dir(install, roots) {
        return dir.join("User").join("Config");
    }
    match (roots.os, install.origin) {
        (Os::Linux, Origin::Flatpak) => roots
            .flatpak_config("org.DolphinEmu.dolphin-emu")
            .join("dolphin-emu"),
        (Os::Linux, _) => roots.xdg_config.join("dolphin-emu"),
        (Os::Macos, _) => roots
            .home
            .join("Library/Application Support/Dolphin/Config"),
        (Os::Windows, _) => roots.documents.join("Dolphin Emulator").join("Config"),
    }
}

/// DuckStation's `settings.ini`.
fn duckstation_settings(install: &Installation, roots: &Roots) -> PathBuf {
    if let Some(dir) = portable_dir(install, roots) {
        return dir.join("settings.ini");
    }
    let root = match (roots.os, install.origin) {
        (Os::Linux, Origin::Flatpak) => roots
            .flatpak_config("org.duckstation.DuckStation")
            .join("duckstation"),
        (Os::Linux, _) => {
            // DuckStation reads XDG_CONFIG_HOME only when it is set.
            if std::env::var_os("XDG_CONFIG_HOME").is_some() {
                roots.xdg_config.join("duckstation")
            } else {
                roots.home.join(".local/share/duckstation")
            }
        }
        (Os::Macos, _) => roots.home.join("Library/Application Support/DuckStation"),
        (Os::Windows, _) => roots.documents.join("DuckStation"),
    };
    root.join("settings.ini")
}

/// Set `key = value` in `[section]` of an INI text, keeping everything
/// else; the section is added when missing.
pub fn set_ini(text: &str, section: &str, pairs: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = format!("[{section}]");
    let start = match lines.iter().position(|l| l.trim() == header) {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len() - 1
        }
    };
    for (key, value) in pairs {
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |p| start + 1 + p);
        let line = format!("{key} = {value}");
        let found = lines[start + 1..end].iter().position(|l| {
            l.split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        });
        match found {
            Some(i) => lines[start + 1 + i] = line,
            None => {
                // After the section's last setting, before blank lines.
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, line);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn update_ini(path: &Path, section: &str, pairs: &[(&str, String)]) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let text = set_ini(&text, section, pairs);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    json_store::write_bytes(path, text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

fn flag(on: bool, style: Bool) -> String {
    match (style, on) {
        (Bool::Lower, true) => "true".into(),
        (Bool::Lower, false) => "false".into(),
        (Bool::Title, true) => "True".into(),
        (Bool::Title, false) => "False".into(),
    }
}

#[derive(Clone, Copy)]
enum Bool {
    Lower,
    Title,
}

/// Sign the engine `engine` (installed as `install`) in with `login`.
/// Blocking: writes files.
pub fn sign_in(
    engine: &str,
    install: &Installation,
    login: &Login,
    roots: &Roots,
) -> Result<Outcome> {
    let now = irori_core::util::unix_now().to_string();
    let user = login.username.trim().to_string();
    match engine {
        "retroarch" => {
            let cfg = roots.retroarch_home.join("retroarch.cfg");
            if let Some(dir) = cfg.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::retroarch::write_config(
                &cfg,
                &[
                    ("cheevos_enable", "true".into()),
                    ("cheevos_username", user),
                    ("cheevos_token", login.token.clone()),
                    // RetroArch signs in with the token; a stale password
                    // would replace it.
                    ("cheevos_password", String::new()),
                    (
                        "cheevos_hardcore_mode_enable",
                        flag(login.hardcore, Bool::Lower),
                    ),
                ],
            )?;
            Ok(Outcome::SignedIn(cfg))
        }
        "pcsx2" => {
            let inis = pcsx2_root(install, roots).join("inis");
            let main = inis.join("PCSX2.ini");
            update_ini(
                &main,
                "Achievements",
                &[
                    ("Enabled", flag(true, Bool::Lower)),
                    ("ChallengeMode", flag(login.hardcore, Bool::Lower)),
                    ("Username", user),
                    ("LoginTimestamp", now),
                ],
            )?;
            update_ini(
                &inis.join("secrets.ini"),
                "Achievements",
                &[("Token", login.token.clone())],
            )?;
            Ok(Outcome::SignedIn(main))
        }
        "ppsspp" => {
            let system = ppsspp_system(install, roots);
            let ini = system.join("ppsspp.ini");
            update_ini(
                &ini,
                "Achievements",
                &[
                    ("AchievementsEnable", flag(true, Bool::Title)),
                    (
                        "AchievementsChallengeMode",
                        flag(login.hardcore, Bool::Title),
                    ),
                    ("AchievementsUserName", user),
                ],
            )?;
            json_store::write_bytes(
                &system.join("ppsspp_retroachievements.dat"),
                login.token.as_bytes(),
            )?;
            Ok(Outcome::SignedIn(ini))
        }
        "dolphin" => {
            let ini = dolphin_config(install, roots).join("RetroAchievements.ini");
            update_ini(
                &ini,
                "Achievements",
                &[
                    ("Enabled", flag(true, Bool::Title)),
                    ("HardcoreEnabled", flag(login.hardcore, Bool::Title)),
                    ("Username", user),
                    ("ApiToken", login.token.clone()),
                ],
            )?;
            Ok(Outcome::SignedIn(ini))
        }
        "duckstation" => {
            let ini = duckstation_settings(install, roots);
            update_ini(
                &ini,
                "Cheevos",
                &[
                    ("Enabled", flag(true, Bool::Lower)),
                    ("ChallengeMode", flag(login.hardcore, Bool::Lower)),
                ],
            )?;
            Ok(Outcome::Enabled(ini))
        }
        other => anyhow::bail!("Irori can't sign {other} in to RetroAchievements"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(origin: Origin, dir: Option<PathBuf>) -> Installation {
        Installation {
            engine: "x".into(),
            origin,
            version: None,
            program: PathBuf::from("/bin/x"),
            flatpak: None,
            dir,
            size: 0,
        }
    }

    fn roots(tmp: &Path, os: Os) -> Roots {
        Roots {
            os,
            home: tmp.join("home"),
            xdg_config: tmp.join("home/.config"),
            documents: tmp.join("home/Documents"),
            retroarch_home: tmp.join("irori/retroarch"),
        }
    }

    fn login() -> Login {
        Login {
            username: "player".into(),
            token: "TOKEN".into(),
            hardcore: true,
        }
    }

    #[test]
    fn ini_updates_keep_other_settings() {
        let text =
            "[UI]\nTheme = dark\n\n[Achievements]\nEnabled = false\nOther = 1\n\n[Next]\nA = b\n";
        let out = set_ini(
            text,
            "Achievements",
            &[("Enabled", "true".into()), ("Username", "p".into())],
        );
        assert_eq!(
            out,
            "[UI]\nTheme = dark\n\n[Achievements]\nEnabled = true\nOther = 1\nUsername = p\n\n[Next]\nA = b\n"
        );
        assert_eq!(
            set_ini("", "Cheevos", &[("Enabled", "true".into())]),
            "[Cheevos]\nEnabled = true\n"
        );
    }

    #[test]
    fn writes_each_emulator_where_it_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path(), Os::Linux);
        let sys = install(Origin::System, None);

        let Outcome::SignedIn(cfg) = sign_in("retroarch", &sys, &login(), &r).unwrap() else {
            panic!()
        };
        let text = std::fs::read_to_string(cfg).unwrap();
        assert!(text.contains("cheevos_token = \"TOKEN\""));
        assert!(text.contains("cheevos_hardcore_mode_enable = \"true\""));

        sign_in("pcsx2", &sys, &login(), &r).unwrap();
        let inis = tmp.path().join("home/.config/PCSX2/inis");
        assert!(
            std::fs::read_to_string(inis.join("PCSX2.ini"))
                .unwrap()
                .contains("Username = player")
        );
        assert!(
            std::fs::read_to_string(inis.join("secrets.ini"))
                .unwrap()
                .contains("Token = TOKEN")
        );

        let flatpak = install(Origin::Flatpak, None);
        sign_in("ppsspp", &flatpak, &login(), &r).unwrap();
        let system = tmp
            .path()
            .join("home/.var/app/org.ppsspp.PPSSPP/config/ppsspp/PSP/SYSTEM");
        assert_eq!(
            std::fs::read_to_string(system.join("ppsspp_retroachievements.dat")).unwrap(),
            "TOKEN"
        );
        assert!(
            std::fs::read_to_string(system.join("ppsspp.ini"))
                .unwrap()
                .contains("AchievementsEnable = True")
        );

        sign_in("dolphin", &sys, &login(), &r).unwrap();
        let ini = tmp
            .path()
            .join("home/.config/dolphin-emu/RetroAchievements.ini");
        assert!(
            std::fs::read_to_string(ini)
                .unwrap()
                .contains("ApiToken = TOKEN")
        );

        assert!(matches!(
            sign_in("duckstation", &flatpak, &login(), &r).unwrap(),
            Outcome::Enabled(_)
        ));
        assert!(sign_in("mgba", &sys, &login(), &r).is_err());
    }

    #[test]
    fn portable_windows_installs_keep_settings_beside_the_program() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path(), Os::Windows);
        let dir = tmp.path().join("engines/dolphin/2509");
        let managed = install(Origin::Managed, Some(dir.clone()));
        let Outcome::SignedIn(ini) = sign_in("dolphin", &managed, &login(), &r).unwrap() else {
            panic!()
        };
        assert_eq!(ini, dir.join("User/Config/RetroAchievements.ini"));
        let other = install(Origin::Windows, None);
        let Outcome::SignedIn(ini) = sign_in("pcsx2", &other, &login(), &r).unwrap() else {
            panic!()
        };
        assert!(ini.starts_with(tmp.path().join("home/Documents/PCSX2")));
        assert!(!format!("{:?}", login()).contains("TOKEN"));
    }
}
