//! Rockstar Games Launcher: installed titles from the registry.
//!
//! Each installed title has a key under
//! `HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Rockstar Games\<title>` with its
//! folder in `InstallFolder`. The launcher and Social Club have keys there
//! too and are skipped. Titles not in [`TITLES`] are still listed, named after
//! their key.
//!
//! Games launch through the launcher:
//! `Launcher.exe -launchTitleInFolder "<install folder>"`.
//!
//! Copies of these titles bought on Steam or the Epic Games Store start
//! through the launcher too ([`Title::needs_launcher`], [`store_title`]).
//!
//! The launcher keeps no readable record of the account's titles: its
//! settings and profiles are encrypted, and the title list it logs is the
//! whole catalogue. So Irori lists installed titles only. Whether someone is
//! signed in comes from the launcher's log and profile folders ([`sign_in`]).

use std::path::{Path, PathBuf};

use crate::{
    InstalledGame, LaunchSpec, Launcher, Registry, ScanReport, SignIn, drive_c, registry,
    retain_present,
};

/// The registry key holding one subkey per title (both registry views are read).
pub const ROCKSTAR_KEY: &str = r"SOFTWARE\Rockstar Games";

/// The value holding a title's install folder.
pub const INSTALL_FOLDER: &str = "InstallFolder";

/// Subkeys of [`ROCKSTAR_KEY`] that are Rockstar's own software.
const NOT_GAMES: &[&str] = &[
    "Launcher",
    "Rockstar Games Launcher",
    "Rockstar Games Social Club",
    "Social Club",
];

/// A Rockstar title Irori knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Title {
    /// Irori's id for the title.
    pub id: &'static str,
    /// The registry key names the title may be installed under.
    pub keys: &'static [&'static str],
    pub title: &'static str,
    /// Other names stores sell the title under.
    pub aliases: &'static [&'static str],
    /// The title's program, relative to its install folder.
    pub executable: &'static str,
    /// The title's Steam app id, for art and compatibility reports.
    pub steam_app_id: &'static str,
    /// Copies bought on Steam or the Epic Games Store start through the
    /// Rockstar Games Launcher too, so it must be in their Wine prefix.
    pub needs_launcher: bool,
}

/// Known Rockstar titles: everything the Rockstar Games Launcher sells, and
/// the older Rockstar PC games stores still sell.
pub const TITLES: &[Title] = &[
    Title {
        id: "gta5",
        keys: &["Grand Theft Auto V", "Grand Theft Auto V Legacy"],
        title: "Grand Theft Auto V Legacy",
        aliases: &[
            "Grand Theft Auto V",
            "GTA V",
            "Grand Theft Auto V: Premium Edition",
        ],
        executable: "PlayGTAV.exe",
        steam_app_id: "271590",
        needs_launcher: true,
    },
    Title {
        id: "gta5-enhanced",
        keys: &["Grand Theft Auto V Enhanced"],
        title: "Grand Theft Auto V Enhanced",
        aliases: &[],
        executable: "PlayGTAV.exe",
        steam_app_id: "3240220",
        needs_launcher: true,
    },
    Title {
        id: "rdr2",
        keys: &["Red Dead Redemption 2"],
        title: "Red Dead Redemption 2",
        aliases: &[
            "Red Dead Redemption 2: Ultimate Edition",
            "Red Dead Redemption 2: Special Edition",
        ],
        executable: "RDR2.exe",
        steam_app_id: "1174180",
        needs_launcher: true,
    },
    Title {
        id: "rdr",
        keys: &["Red Dead Redemption"],
        title: "Red Dead Redemption",
        aliases: &["Red Dead Redemption and Undead Nightmare"],
        executable: "RDR.exe",
        steam_app_id: "2668510",
        needs_launcher: true,
    },
    Title {
        id: "gta4",
        keys: &["Grand Theft Auto IV", "GTAIV"],
        title: "Grand Theft Auto IV: The Complete Edition",
        aliases: &["Grand Theft Auto IV"],
        executable: "GTAIV.exe",
        steam_app_id: "12210",
        needs_launcher: true,
    },
    Title {
        id: "lanoire",
        keys: &["L.A. Noire"],
        title: "L.A. Noire",
        aliases: &["L.A. Noire: The Complete Edition"],
        executable: "LANoire.exe",
        steam_app_id: "110800",
        needs_launcher: true,
    },
    Title {
        id: "lanoirevr",
        keys: &["L.A. Noire: The VR Case Files", "L.A. Noire VR"],
        title: "L.A. Noire: The VR Case Files",
        aliases: &[],
        executable: "LANoireVR.exe",
        steam_app_id: "722230",
        needs_launcher: true,
    },
    Title {
        id: "mp3",
        keys: &["Max Payne 3"],
        title: "Max Payne 3",
        aliases: &["Max Payne 3: Complete Edition"],
        executable: "MaxPayne3.exe",
        steam_app_id: "204100",
        needs_launcher: true,
    },
    Title {
        id: "bully",
        keys: &["Bully Scholarship Edition", "Bully: Scholarship Edition"],
        title: "Bully: Scholarship Edition",
        aliases: &["Bully"],
        executable: "Bully.exe",
        steam_app_id: "12200",
        needs_launcher: false,
    },
    Title {
        id: "gta3-de",
        keys: &[
            "Grand Theft Auto III - The Definitive Edition",
            "GTA III - Definitive Edition",
        ],
        title: "Grand Theft Auto III – The Definitive Edition",
        aliases: &[],
        executable: r"Gameface\Binaries\Win64\LibertyCity.exe",
        steam_app_id: "1546970",
        needs_launcher: true,
    },
    Title {
        id: "gtavc-de",
        keys: &[
            "Grand Theft Auto Vice City - The Definitive Edition",
            "GTA Vice City - Definitive Edition",
        ],
        title: "Grand Theft Auto: Vice City – The Definitive Edition",
        aliases: &[],
        executable: r"Gameface\Binaries\Win64\ViceCity.exe",
        steam_app_id: "1546990",
        needs_launcher: true,
    },
    Title {
        id: "gtasa-de",
        keys: &[
            "Grand Theft Auto San Andreas - The Definitive Edition",
            "GTA San Andreas - Definitive Edition",
        ],
        title: "Grand Theft Auto: San Andreas – The Definitive Edition",
        aliases: &[],
        executable: r"Gameface\Binaries\Win64\SanAndreas.exe",
        steam_app_id: "1547000",
        needs_launcher: true,
    },
    Title {
        id: "gta3",
        keys: &["Grand Theft Auto III"],
        title: "Grand Theft Auto III",
        aliases: &[],
        executable: "gta3.exe",
        steam_app_id: "12100",
        needs_launcher: false,
    },
    Title {
        id: "gtavc",
        keys: &["Grand Theft Auto Vice City", "Grand Theft Auto: Vice City"],
        title: "Grand Theft Auto: Vice City",
        aliases: &[],
        executable: "gta-vc.exe",
        steam_app_id: "12110",
        needs_launcher: false,
    },
    Title {
        id: "gtasa",
        keys: &[
            "Grand Theft Auto San Andreas",
            "Grand Theft Auto: San Andreas",
        ],
        title: "Grand Theft Auto: San Andreas",
        aliases: &[],
        executable: "gta_sa.exe",
        steam_app_id: "12120",
        needs_launcher: false,
    },
    Title {
        id: "mc2",
        keys: &["Midnight Club 2", "Midnight Club II"],
        title: "Midnight Club II",
        aliases: &["Midnight Club 2"],
        executable: "mc2.exe",
        steam_app_id: "12160",
        needs_launcher: false,
    },
    Title {
        id: "manhunt",
        keys: &["Manhunt"],
        title: "Manhunt",
        aliases: &[],
        executable: "manhunt.exe",
        steam_app_id: "12130",
        needs_launcher: false,
    },
];

/// The known title filed under this registry key name (case-insensitive).
pub fn title_for_key(key: &str) -> Option<&'static Title> {
    TITLES
        .iter()
        .find(|t| t.keys.iter().any(|k| k.eq_ignore_ascii_case(key)))
}

/// The known title with this [`Title::id`].
pub fn title_by_id(id: &str) -> Option<&'static Title> {
    TITLES.iter().find(|t| t.id == id)
}

/// The known title a store (Epic, Steam) sells as `title`, or whose program
/// is `executable` when the store names Rockstar as its developer or
/// publisher. Titles compare ignoring case, spaces and punctuation, so
/// "Grand Theft Auto III - The Definitive Edition" matches the en dash.
pub fn store_title(title: &str, executable: &str, makers: &[&str]) -> Option<&'static Title> {
    let wanted = normalize(title);
    if !wanted.is_empty()
        && let Some(t) = TITLES.iter().find(|t| {
            std::iter::once(t.title)
                .chain(t.aliases.iter().copied())
                .chain(t.keys.iter().copied())
                .any(|name| normalize(name) == wanted)
        })
    {
        return Some(t);
    }
    let by_rockstar = makers
        .iter()
        .any(|m| m.to_ascii_lowercase().contains("rockstar"));
    let program = file_name(executable);
    if !by_rockstar || program.is_empty() {
        return None;
    }
    TITLES
        .iter()
        .find(|t| file_name(t.executable).eq_ignore_ascii_case(program))
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `Gameface\Binaries\Win64\SanAndreas.exe` → `SanAndreas.exe`.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or_default()
}

/// The launcher's folder under a user's Documents.
pub const DOCUMENTS_DIR: &str = "Rockstar Games/Launcher";

/// The launcher's log of its last run, in [`DOCUMENTS_DIR`]. Rewritten each
/// time the launcher starts.
pub const LOG_FILE: &str = "launcher.log";

/// Whether the launcher's log says someone signed in: the last sign-in or
/// sign-out event wins, and `None` means the log mentions neither. The log
/// names the account; nothing of it is kept.
pub fn sign_in_from_log(text: &str) -> Option<SignIn> {
    let mut state = None;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !(lower.contains("presence event") || lower.contains("is now signed")) {
            continue;
        }
        if lower.contains("signed in") {
            state = Some(SignIn::SignedIn);
        } else if lower.contains("signed out") {
            state = Some(SignIn::SignedOut);
        }
    }
    state
}

/// Whether someone is signed in to the launcher, from the user folders
/// `documents` (each a Documents folder): its log of the last run, else a
/// profile folder, which the launcher and Social Club make at the first
/// sign-in and keep while they remember the account.
pub fn sign_in(documents: &[PathBuf]) -> SignIn {
    let mut seen_launcher = false;
    let mut profile = false;
    for docs in documents {
        let dir = docs.join(DOCUMENTS_DIR);
        if !dir.is_dir() {
            continue;
        }
        seen_launcher = true;
        if let Ok(text) = std::fs::read(dir.join(LOG_FILE))
            && let Some(state) = sign_in_from_log(&String::from_utf8_lossy(&text))
        {
            return state;
        }
        profile |= [
            dir.join("Profiles"),
            docs.join("Rockstar Games/Social Club/Profiles"),
        ]
        .iter()
        .any(|p| has_subfolder(p));
    }
    match (profile, seen_launcher) {
        (true, _) => SignIn::SignedIn,
        (false, true) => SignIn::SignedOut,
        (false, false) => SignIn::Unknown,
    }
}

fn has_subfolder(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().any(|e| e.path().is_dir()))
}

/// How the Rockstar Games Launcher starts the title in `install_folder`.
pub fn launch_spec(install_folder: &str) -> LaunchSpec {
    LaunchSpec::LauncherArgs(vec![
        "-launchTitleInFolder".to_string(),
        install_folder.to_string(),
    ])
}

/// The titles the registry (`HKEY_LOCAL_MACHINE`) records as installed.
pub fn installed_games(registry: &Registry) -> Vec<InstalledGame> {
    let mut games: Vec<InstalledGame> = Vec::new();
    for key in registry.machine_subkeys(ROCKSTAR_KEY) {
        if NOT_GAMES.iter().any(|n| n.eq_ignore_ascii_case(&key)) {
            continue;
        }
        let Some(folder) = registry
            .machine_string(&format!(r"{ROCKSTAR_KEY}\{key}"), INSTALL_FOLDER)
            .map(str::trim)
            .filter(|f| !f.is_empty())
        else {
            continue;
        };
        let known = title_for_key(&key);
        let id = known.map_or_else(|| slug(&key), |t| t.id.to_string());
        if id.is_empty() || games.iter().any(|g| g.id == id) {
            continue;
        }
        games.push(InstalledGame {
            launcher: Launcher::Rockstar,
            id,
            title: known.map_or_else(|| key.clone(), |t| t.title.to_string()),
            install_dir: PathBuf::from(folder),
            executable: known.map(|t| t.executable.to_string()),
            launch: launch_spec(folder),
        });
    }
    games
}

/// `Some Title: Edition` → `some-title-edition`, the id of titles Irori doesn't know.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// The Rockstar titles installed in the Wine prefix at `prefix`.
pub fn scan_prefix(prefix: &Path) -> ScanReport {
    let mut warnings = Vec::new();
    let registry = registry::load_machine(prefix, &mut warnings);
    let mut report = scan_drive(&drive_c(prefix), &registry);
    report.warnings.splice(0..0, warnings);
    report
}

/// The Rockstar titles installed on `drive` (`drive_c`, or `C:\`), with
/// `registry` holding `HKEY_LOCAL_MACHINE`.
pub fn scan_drive(drive: &Path, registry: &Registry) -> ScanReport {
    let mut report = ScanReport {
        games: installed_games(registry),
        warnings: Vec::new(),
    };
    retain_present(drive, &mut report);
    report
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SYSTEM_REG: &str = r#"WINE REGISTRY Version 2
;; All keys relative to \\Machine

[Software\\Wow6432Node\\Rockstar Games\\Grand Theft Auto V] 1700000000
#time=1da0000000000000
"InstallFolder"="C:\\Program Files\\Rockstar Games\\Grand Theft Auto V"
"Language"="en-US"
"Telemetry"=hex:00,01,02,\
  03

[Software\\Wow6432Node\\Rockstar Games\\Launcher] 1700000000
"InstallFolder"="C:\\Program Files\\Rockstar Games\\Launcher"

[Software\\Wow6432Node\\Rockstar Games\\Rockstar Games Social Club] 1700000000
"InstallFolder"="C:\\Program Files\\Rockstar Games\\Social Club"

[Software\\Rockstar Games\\Some New Game: Deluxe] 1700000000
"InstallFolder"="D:\\Rockstar\\Some New Game"

[Software\\Wow6432Node\\Rockstar Games\\Uninstalled Thing] 1700000000
"Version"="1.0"
"#;

    #[test]
    fn lists_titles_with_an_install_folder() {
        let games = installed_games(&Registry::parse(SYSTEM_REG));
        assert_eq!(games.len(), 2, "{games:?}");

        let gta = &games[0];
        assert_eq!(gta.app_name(), "rockstar:gta5");
        assert_eq!(gta.title, "Grand Theft Auto V Legacy");
        assert_eq!(
            gta.install_dir,
            PathBuf::from(r"C:\Program Files\Rockstar Games\Grand Theft Auto V")
        );
        assert_eq!(gta.executable.as_deref(), Some("PlayGTAV.exe"));
        assert_eq!(
            gta.launch.launcher_args(),
            [
                "-launchTitleInFolder",
                r"C:\Program Files\Rockstar Games\Grand Theft Auto V"
            ]
        );

        let other = &games[1];
        assert_eq!(other.id, "some-new-game-deluxe");
        assert_eq!(other.title, "Some New Game: Deluxe");
        assert_eq!(other.executable, None);
    }

    #[test]
    fn every_title_is_complete_and_distinct() {
        for (ix, t) in TITLES.iter().enumerate() {
            assert!(!t.keys.is_empty() && !t.executable.is_empty(), "{t:?}");
            assert!(
                !t.steam_app_id.is_empty() && t.steam_app_id.bytes().all(|b| b.is_ascii_digit()),
                "{t:?}"
            );
            assert_eq!(title_by_id(t.id), Some(t));
            for other in &TITLES[ix + 1..] {
                assert_ne!(t.id, other.id);
                assert_ne!(t.steam_app_id, other.steam_app_id);
                for key in t.keys {
                    assert!(
                        !other.keys.iter().any(|k| k.eq_ignore_ascii_case(key)),
                        "{key} is filed under two titles"
                    );
                }
            }
        }
    }

    #[test]
    fn recognises_store_copies() {
        let id =
            |title: &str, exe: &str, makers: &[&str]| store_title(title, exe, makers).map(|t| t.id);
        assert_eq!(id("Grand Theft Auto V", "", &[]), Some("gta5"));
        assert_eq!(
            id("Grand Theft Auto V Enhanced", "", &[]),
            Some("gta5-enhanced")
        );
        assert_eq!(
            id(
                "Grand Theft Auto: San Andreas - The Definitive Edition",
                "",
                &[]
            ),
            Some("gtasa-de")
        );
        assert_eq!(id("RED DEAD REDEMPTION 2", "", &[]), Some("rdr2"));
        // A renamed edition is still found by its program, but only when
        // the store says Rockstar made it.
        assert_eq!(
            id(
                "Red Dead Redemption 2: Story Mode",
                "RDR2.exe",
                &["Rockstar Games"]
            ),
            Some("rdr2")
        );
        assert_eq!(
            id(
                "Some Remaster",
                r"Gameface\Binaries\Win64\ViceCity.exe",
                &["Grove Street Games", "Rockstar Games"]
            ),
            Some("gtavc-de")
        );
        assert_eq!(id("Not Red Dead", "RDR2.exe", &["Someone Else"]), None);
        assert_eq!(id("Hades", "Hades.exe", &["Supergiant Games"]), None);
        assert_eq!(id("", "", &["Rockstar Games"]), None);
    }

    #[test]
    fn reads_sign_in_from_the_log() {
        let log = "\
[2026-01-02 10:00:00.000] [DISPLAY] [Main ] [launcher] Time to start up: 900 ms
[2026-01-02 10:00:05.000] [DISPLAY] [Main ] [launcher] Presence Event - Signed In
[2026-01-02 10:00:05.000] [DISPLAY] [Main ] [launcher] User 'someone' is now signed in
";
        assert_eq!(sign_in_from_log(log), Some(SignIn::SignedIn));
        let out = format!(
            "{log}[2026-01-02 11:00:00.000] [DISPLAY] [Main ] [launcher] Presence Event - Signed Out\n"
        );
        assert_eq!(sign_in_from_log(&out), Some(SignIn::SignedOut));
        assert_eq!(
            sign_in_from_log("[x] [DISPLAY] [Workr] [titlemanager] Title list updated.\n"),
            None
        );
    }

    #[test]
    fn sign_in_from_the_users_folders() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let drive = tmp.path();
        let docs = drive.join("users/steamuser/Documents");
        let launcher = docs.join(DOCUMENTS_DIR);

        // Never ran: nothing to tell.
        std::fs::create_dir_all(&docs).expect("mkdir");
        assert_eq!(crate::user_documents(drive), std::slice::from_ref(&docs));
        assert_eq!(sign_in(&crate::user_documents(drive)), SignIn::Unknown);

        // Ran, nobody signed in.
        std::fs::create_dir_all(&launcher).expect("mkdir");
        assert_eq!(sign_in(std::slice::from_ref(&docs)), SignIn::SignedOut);

        // A remembered profile, and no log.
        std::fs::create_dir_all(docs.join("Rockstar Games/Social Club/Profiles/0A1B"))
            .expect("mkdir");
        assert_eq!(sign_in(std::slice::from_ref(&docs)), SignIn::SignedIn);

        // The log of the last run wins over the profile.
        std::fs::write(
            launcher.join(LOG_FILE),
            "[t] [DISPLAY] [Main ] [launcher] Presence Event - Signed Out\n",
        )
        .expect("write");
        assert_eq!(sign_in(&[docs]), SignIn::SignedOut);
    }
}
