//! `irori list --json`: the installed library as JSON for other programs.
//!
//! Tools that work on a game's files or Wine prefix (SCSKiller compiles
//! shader caches ahead of time, for example) need to know where Irori keeps
//! each game and how it runs it, without reading every store's own files.
//! This is that contract, and it is versioned: fields are only ever added
//! within a version, so readers ignore what they don't know.
//!
//! Only games Irori installs and runs itself are listed. Steam's games and
//! third-party launchers' games run through their own clients, and the ROM
//! library runs through emulators, so none of them has a prefix of Irori's.

use std::path::{Path, PathBuf};

use irori_core::settings::{GameSettings, WineInstallation, WineKind};
use irori_core::{GameInfo, HostPlatform, InstallPlatform, OpContext, Runner};
use irori_services::Backend;
use serde::Serialize;

/// The format's version. Bump it only for a change an older reader would
/// misread (a field removed or its meaning changed).
pub const VERSION: u32 = 1;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub version: u32,
    pub games: Vec<ExportedGame>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportedGame {
    /// `<runner>/<app>`, as `irori --launch` takes it.
    pub id: String,
    pub runner: &'static str,
    pub app_name: String,
    pub title: String,
    /// `windows`, `linux` or `osx` (Heroic's spelling): the installed build.
    pub platform: String,
    pub install_path: PathBuf,
    /// The game's program, absolute; `None` when the store names none.
    pub executable: Option<PathBuf>,
    /// The arguments the user added (`launcherArgs`), as typed.
    pub arguments: String,
    /// Starts the game the way Irori's Play button does. Its processes carry
    /// `IRORI_GAME_ID=<id>`.
    pub launch_command: Vec<String>,
    /// How the game runs under Wine; `None` when it runs natively.
    pub wine: Option<ExportedWine>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportedWine {
    /// The prefix as Irori passes it (`WINEPREFIX`, or `STEAM_COMPAT_DATA_PATH`
    /// for Proton, whose `drive_c` is then under `pfx/`).
    pub prefix: PathBuf,
    /// `wine`, `proton`, `crossover` or `toolkit`.
    pub kind: &'static str,
    pub name: String,
    /// The build's folder: the folder holding the `proton` script, or the
    /// Wine build's root (the parent of `bin/`).
    pub build_path: Option<PathBuf>,
    /// The program Irori runs: `bin/wine` or the `proton` script.
    pub bin: PathBuf,
    /// Whether Proton runs through umu-run.
    pub umu: bool,
    /// The variables the user set for the game (`enviromentOptions`), in order.
    pub environment: Vec<(String, String)>,
}

/// Whether `runner`'s games belong in the export (see the module docs).
pub fn listed(runner: Runner) -> bool {
    runner.manages_installs() && runner != Runner::Emulation
}

/// One installed game's entry. `wine` is the build Irori would run it with,
/// `None` when it runs natively or no build is available; `irori` is the
/// command that starts Irori.
pub fn entry(
    game: &GameInfo,
    settings: &GameSettings,
    prefix: Option<PathBuf>,
    wine: Option<&WineInstallation>,
    umu: bool,
    irori: &[String],
) -> Option<ExportedGame> {
    let install = game.install.as_ref().filter(|_| game.is_installed)?;
    if game.browser_url.is_some() {
        return None;
    }
    let install_path = PathBuf::from(&install.install_path);
    let executable = (!install.executable.trim().is_empty()).then(|| {
        let exe = Path::new(install.executable.trim());
        if exe.is_absolute() {
            exe.to_path_buf()
        } else {
            install_path.join(exe)
        }
    });
    let id = game.id();
    let key = format!("{}/{}", id.runner.id(), id.app_name);
    let wine = match (prefix, wine) {
        (Some(prefix), Some(wine)) => Some(ExportedWine {
            prefix,
            kind: kind_name(wine.kind),
            name: wine.name.clone(),
            build_path: build_path(wine),
            bin: PathBuf::from(&wine.bin),
            umu: umu && wine.is_proton(),
            environment: settings
                .environment_options
                .iter()
                .filter(|v| !v.key.trim().is_empty())
                .map(|v| (v.key.trim().to_owned(), v.value.clone()))
                .collect(),
        }),
        _ => None,
    };
    Some(ExportedGame {
        id: key.clone(),
        runner: id.runner.id(),
        app_name: id.app_name.to_string(),
        title: game.title.clone(),
        platform: install.platform.as_str().to_owned(),
        install_path,
        executable,
        arguments: settings.launcher_args.clone(),
        launch_command: irori
            .iter()
            .cloned()
            .chain(["--launch".to_owned(), key])
            .collect(),
        wine,
    })
}

fn kind_name(kind: WineKind) -> &'static str {
    match kind {
        WineKind::Wine => "wine",
        WineKind::Proton => "proton",
        WineKind::Crossover => "crossover",
        WineKind::Toolkit => "toolkit",
    }
}

/// The folder a build lives in: the `proton` script's folder for Proton,
/// the parent of `bin/` for Wine.
fn build_path(wine: &WineInstallation) -> Option<PathBuf> {
    let bin = Path::new(&wine.bin);
    let parent = bin.parent()?;
    if wine.is_proton() {
        return Some(parent.to_path_buf());
    }
    match parent.file_name() {
        Some(name) if name == "bin" => parent.parent().map(Path::to_path_buf),
        _ => Some(parent.to_path_buf()),
    }
}

/// The command that starts Irori from outside: inside its Flatpak the
/// program's own path means nothing to the host, so `flatpak run`.
fn launcher() -> Vec<String> {
    if irori_core::host::is_flatpak() {
        let app = std::env::var("FLATPAK_ID").unwrap_or_else(|_| "app.irori.Irori".into());
        return vec!["flatpak".into(), "run".into(), app];
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("irori"));
    vec![exe.display().to_string()]
}

/// Every installed game in the export, with its Wine build resolved as a
/// launch would (without downloading one).
pub async fn collect(backend: &Backend) -> Export {
    let app = backend.settings.app();
    let irori = launcher();
    let host = HostPlatform::current();
    let mut games = Vec::new();
    let library = backend.library();
    for game in library.all_games() {
        let id = game.id();
        // Hidden games behind the PIN stay hidden from other tools too.
        if !listed(id.runner) || game.is_dlc || backend.concealed(&id) {
            continue;
        }
        let settings = backend.settings.game(&id);
        let platform = game
            .installed_platform()
            .unwrap_or(InstallPlatform::Windows);
        let (prefix, wine, umu) = if host.needs_wine(platform) {
            match irori_runtime::resolve_wine(&app, &settings, &OpContext::detached(), false).await
            {
                Ok(wine) => {
                    let effective = GameSettings {
                        wine_version: wine.clone(),
                        ..settings.clone()
                    };
                    let prefix = irori_runtime::prefix_path(&app, &effective, game);
                    let umu = irori_runtime::runtimes::is_umu_supported(&effective, false);
                    (Some(prefix), Some(wine), umu)
                }
                Err(e) => {
                    tracing::warn!("{}: no Wine build for the export: {e:#}", game.title);
                    (None, None, false)
                }
            }
        } else {
            (None, None, false)
        };
        if let Some(entry) = entry(game, &settings, prefix, wine.as_ref(), umu, &irori) {
            games.push(entry);
        }
    }
    games.sort_by_key(|a| a.title.to_lowercase());
    Export {
        version: VERSION,
        games,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::InstalledInfo;
    use irori_core::settings::EnvVar;

    fn game(runner: Runner, exe: &str) -> GameInfo {
        GameInfo {
            app_name: "Quail".into(),
            runner: Some(runner),
            title: "Quail Quest".into(),
            is_installed: true,
            install: Some(InstalledInfo {
                install_path: "/games/Quail".into(),
                executable: exe.into(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn proton() -> WineInstallation {
        WineInstallation {
            bin: "/steam/compatibilitytools.d/GE-Proton10-1/proton".into(),
            name: "GE-Proton10-1".into(),
            kind: WineKind::Proton,
            ..Default::default()
        }
    }

    #[test]
    fn a_proton_game_names_its_prefix_build_and_variables() {
        let settings = GameSettings {
            launcher_args: "-dx11".into(),
            environment_options: vec![
                EnvVar {
                    key: "DXVK_ASYNC".into(),
                    value: "1".into(),
                },
                EnvVar {
                    key: " ".into(),
                    value: "ignored".into(),
                },
            ],
            ..Default::default()
        };
        let entry = entry(
            &game(Runner::Legendary, "Binaries/Quail.exe"),
            &settings,
            Some("/prefixes/Quail Quest".into()),
            Some(&proton()),
            true,
            &["/usr/bin/irori".to_owned()],
        )
        .expect("listed");
        assert_eq!(entry.id, "legendary/Quail");
        assert_eq!(entry.platform, "windows");
        assert_eq!(
            entry.executable.as_deref(),
            Some(Path::new("/games/Quail/Binaries/Quail.exe"))
        );
        assert_eq!(
            entry.launch_command,
            ["/usr/bin/irori", "--launch", "legendary/Quail"]
        );
        let wine = entry.wine.expect("runs under Wine");
        assert_eq!(wine.kind, "proton");
        assert!(wine.umu);
        assert_eq!(
            wine.build_path.as_deref(),
            Some(Path::new("/steam/compatibilitytools.d/GE-Proton10-1"))
        );
        assert_eq!(wine.environment, [("DXVK_ASYNC".into(), "1".into())]);
        let json = serde_json::to_value(Export {
            version: VERSION,
            games: vec![],
        })
        .unwrap();
        assert_eq!(json["version"], 1);
    }

    #[test]
    fn a_wine_build_root_is_the_parent_of_bin() {
        let wine = WineInstallation {
            bin: "/wine/wine-ge-8/bin/wine".into(),
            kind: WineKind::Wine,
            ..Default::default()
        };
        assert_eq!(build_path(&wine), Some(PathBuf::from("/wine/wine-ge-8")));
    }

    #[test]
    fn native_games_have_no_wine_and_absolute_programs_stay() {
        let entry = entry(
            &game(Runner::Gog, "/opt/quail/start.sh"),
            &GameSettings::default(),
            None,
            None,
            false,
            &["irori".to_owned()],
        )
        .expect("listed");
        assert!(entry.wine.is_none());
        assert_eq!(
            entry.executable.as_deref(),
            Some(Path::new("/opt/quail/start.sh"))
        );
    }

    #[test]
    fn uninstalled_and_browser_games_are_left_out() {
        let mut g = game(Runner::Sideload, "x.exe");
        g.is_installed = false;
        assert!(entry(&g, &GameSettings::default(), None, None, false, &[]).is_none());
        let mut g = game(Runner::Sideload, "");
        g.browser_url = Some("https://example.com".into());
        assert!(entry(&g, &GameSettings::default(), None, None, false, &[]).is_none());
    }

    #[test]
    fn client_run_and_emulated_games_are_not_listed() {
        assert!(listed(Runner::Legendary));
        assert!(listed(Runner::Sideload));
        assert!(!listed(Runner::Steam));
        assert!(!listed(Runner::Launchers));
        assert!(!listed(Runner::Emulation));
    }
}
