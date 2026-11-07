//! Command line and launch-URL handling.

use std::path::{Path, PathBuf};

use irori_core::{GameId, Runner};
use irori_ui::Route;

/// What the user asked for on the command line (or a forwarded second
/// instance invocation).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cli {
    pub launch: Option<GameId>,
    /// The launch came from a link, which anything on the computer or the
    /// web can open, not from Irori's own shortcuts.
    pub launch_from_link: bool,
    /// Open a game's page (`irori://game/...`).
    pub open: Option<GameId>,
    /// Open a screen (`irori://library`, `irori://settings/<section>`…).
    pub route: Option<Route>,
    /// Open the Lutris script installer at a lutris.net game
    /// (`irori://lutris/<slug>`, `lutris:<slug>`).
    pub lutris: Option<String>,
    /// A Nexus Mods "Mod manager download" link (`nxm://…`) to install.
    pub nxm: Option<String>,
    /// A Windows file opened with Irori (`.exe`, `.msi`, `.bat`, `.cmd`,
    /// `.reg`, as an absolute path or a `file://` URL) to open in a game's
    /// Wine prefix. Not on Windows, which opens them itself.
    pub windows_file: Option<PathBuf>,
    pub big_picture: bool,
    /// Stay until the launched game's session (or Big Picture) ends, and
    /// end it when this invocation is ended (`--wait`, for Sunshine).
    pub wait: bool,
    /// Start hidden in the tray.
    pub minimized: bool,
    pub version: bool,
    pub help: bool,
}

impl Cli {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Self {
        let mut cli = Cli::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--launch" | "-l" => cli.launch = it.next().as_deref().and_then(parse_game_ref),
                "--big-picture" | "--console" => cli.big_picture = true,
                "--minimized" | "--tray" => cli.minimized = true,
                "--wait" => cli.wait = true,
                "--version" | "-V" => cli.version = true,
                "--help" | "-h" => cli.help = true,
                s if s.starts_with("nxm://") => cli.nxm = Some(s.to_string()),
                s if s.starts_with("file://") => {
                    cli.windows_file = url::Url::parse(s)
                        .ok()
                        .and_then(|u| u.to_file_path().ok())
                        .filter(|p| opens_windows_file(p));
                }
                s if s.contains("://") => cli.apply_url(s),
                s if s.starts_with("lutris:") => cli.lutris = lutris_slug(&s["lutris:".len()..]),
                s if opens_windows_file(Path::new(s)) => cli.windows_file = Some(s.into()),
                _ => {}
            }
        }
        cli
    }

    /// Apply `irori://launch/<runner>/<app>`, `irori://game/<runner>/<app>`,
    /// and Heroic's `heroic://launch/<runner>/<app>` or
    /// `heroic://launch?appName=..&runner=..` forms.
    fn apply_url(&mut self, raw: &str) {
        let Ok(url) = url::Url::parse(raw) else {
            return;
        };
        if !matches!(url.scheme(), "irori" | "heroic") {
            return;
        }
        let verb = url.host_str().unwrap_or_default();
        let mut path: Vec<String> = url
            .path_segments()
            .map(|s| s.filter(|p| !p.is_empty()).map(decode).collect())
            .unwrap_or_default();
        let query = |key: &str| {
            url.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };
        let game = match (path.len(), query("appName")) {
            (_, Some(app)) => {
                let runner = query("runner")
                    .as_deref()
                    .and_then(Runner::from_id)
                    .unwrap_or(Runner::Legendary);
                Some(GameId::new(runner, app))
            }
            (2.., None) => {
                let app = path.remove(1);
                Runner::from_id(&path[0]).map(|r| GameId::new(r, app))
            }
            // Old Heroic links carry only the app name.
            (1, None) => Some(GameId::new(Runner::Legendary, path.remove(0))),
            _ => None,
        };
        match verb {
            "launch" => {
                self.launch = game;
                self.launch_from_link = true;
            }
            "game" | "open" => self.open = game,
            "bigpicture" | "console" => self.big_picture = true,
            "library" => self.route = Some(Route::Library),
            "downloads" => self.route = Some(Route::Downloads),
            "updates" => self.route = Some(Route::Updates),
            "accounts" => self.route = Some(Route::Accounts),
            "stores" => self.route = Some(Route::Stores),
            "stats" => {
                // `irori://stats/year[/<year>]` opens the Year in review.
                let mut rest = url
                    .path_segments()
                    .into_iter()
                    .flatten()
                    .filter(|p| !p.is_empty());
                self.route = Some(match rest.next() {
                    Some("year") => Route::YearInReview(rest.next().and_then(|y| y.parse().ok())),
                    _ => Route::Stats,
                });
            }
            "wine" => self.route = Some(Route::WineManager),
            "emulators" => self.route = Some(Route::Emulators),
            "lutris" => {
                self.lutris = url
                    .path_segments()
                    .and_then(|mut s| s.find(|p| !p.is_empty()).map(decode))
                    .filter(|s| is_slug(s))
            }
            "settings" => {
                let section = url
                    .path_segments()
                    .and_then(|mut s| s.find(|p| !p.is_empty()).map(decode))
                    .unwrap_or_else(|| "general".to_string());
                self.route = Some(Route::Settings(section.into()));
            }
            _ => {}
        }
    }

    /// Whether this invocation asks the running instance to do something
    /// beyond showing its window.
    pub fn has_request(&self) -> bool {
        self.launch.is_some()
            || self.open.is_some()
            || self.route.is_some()
            || self.lutris.is_some()
            || self.nxm.is_some()
            || self.windows_file.is_some()
            || self.big_picture
    }
}

/// Whether Irori opens this file in a game's Wine prefix. Relative paths
/// were made absolute by [`absolute_file_args`] before parsing.
fn opens_windows_file(path: &Path) -> bool {
    cfg!(not(windows)) && path.is_absolute() && irori_runtime::windows_file::is_windows_file(path)
}

/// Whether a bare argument names a Windows file to open, not a command.
pub fn is_windows_file_arg(arg: &str) -> bool {
    cfg!(not(windows))
        && !arg.starts_with('-')
        && !arg.contains("://")
        && irori_runtime::windows_file::is_windows_file(Path::new(arg))
}

/// Make a Windows file given by a relative path absolute, against `cwd`: a
/// running instance gets the arguments with its own working folder.
pub fn absolute_file_args(args: Vec<String>, cwd: &Path) -> Vec<String> {
    args.into_iter()
        .map(|arg| {
            let path = Path::new(&arg);
            if is_windows_file_arg(&arg) && path.is_relative() {
                cwd.join(path).to_string_lossy().into_owned()
            } else {
                arg
            }
        })
        .collect()
}

/// The game slug of a Lutris link body: `<slug>`, `install/<slug>` or
/// `<slug>?revision=…` (Lutris's own `lutris:` forms that name a game).
fn lutris_slug(body: &str) -> Option<String> {
    let body = body.trim_start_matches('/');
    let path = body.split(['?', '#']).next().unwrap_or_default();
    let slug = match path.split('/').collect::<Vec<_>>().as_slice() {
        [slug] => *slug,
        ["install", slug, ..] => *slug,
        _ => return None,
    };
    is_slug(slug).then(|| slug.to_string())
}

/// A lutris.net game slug: letters, digits, `-`, `_` and `.`.
fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn parse_game_ref(s: &str) -> Option<GameId> {
    let (runner, app) = s.split_once('/')?;
    Some(GameId::new(Runner::from_id(runner)?, app))
}

fn decode(s: &str) -> String {
    url::form_urlencoded::parse(s.as_bytes())
        .map(|(k, v)| {
            if v.is_empty() {
                k.into_owned()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub const HELP: &str = "Irori — a fast game launcher for Epic, GOG, Amazon and ZOOM games

Usage: irori [options] [url | file]

Options:
  -l, --launch <runner>/<app>   Launch a game (runners: legendary, gog, nile, zoom, sideload)
      --big-picture             Start in Big Picture mode
      --wait                    With --launch or --big-picture: stay until the game
                                (or Big Picture) ends, and end it when stopped (Sunshine)
      --minimized               Start in the tray
  -V, --version                 Print the version
  -h, --help                    Print this help

URLs: irori://launch/<runner>/<app>, irori://game/<runner>/<app>, heroic://launch/<runner>/<app>,
      irori://library, irori://downloads, irori://accounts, irori://stores, irori://wine,
      irori://settings/<section>, irori://lutris/<slug>, lutris:<slug>,
      nxm://<game>/mods/<mod>/files/<file> (Nexus Mods downloads)

Files: a Windows program, installer, batch or registry file (.exe, .msi, .bat, .cmd, .reg)
       opens in the Wine prefix of a game you choose (Linux and macOS)";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags_and_urls() {
        assert_eq!(
            parse(&["--launch", "gog/1207658924"]).launch,
            Some(GameId::new(Runner::Gog, "1207658924"))
        );
        assert_eq!(
            parse(&["irori://launch/legendary/Fortnite"]).launch,
            Some(GameId::new(Runner::Legendary, "Fortnite"))
        );
        assert_eq!(
            parse(&["heroic://launch?appName=abc&runner=nile"]).launch,
            Some(GameId::new(Runner::Nile, "abc"))
        );
        assert_eq!(
            parse(&["heroic://launch/Sugar"]).launch,
            Some(GameId::new(Runner::Legendary, "Sugar"))
        );
        assert_eq!(
            parse(&["irori://game/sideload/x1"]).open,
            Some(GameId::new(Runner::Sideload, "x1"))
        );
        assert!(parse(&["--big-picture"]).big_picture);
        assert_eq!(parse(&["irori://library"]).route, Some(Route::Library));
        assert_eq!(
            parse(&["irori://settings/wine"]).route,
            Some(Route::Settings("wine".into()))
        );
        assert_eq!(
            parse(&["irori://settings"]).route,
            Some(Route::Settings("general".into()))
        );
        assert!(!parse(&["https://example.com"]).has_request());
    }

    #[test]
    fn stats_links_open_the_year_in_review() {
        let route = |link: &str| parse(&[link]).route;
        assert_eq!(route("irori://stats"), Some(Route::Stats));
        assert_eq!(route("irori://stats/year"), Some(Route::YearInReview(None)));
        assert_eq!(
            route("irori://stats/year/2025"),
            Some(Route::YearInReview(Some(2025)))
        );
        assert_eq!(
            route("irori://stats/year/soon"),
            Some(Route::YearInReview(None))
        );
    }

    #[test]
    fn nexus_links_are_kept_for_the_mod_installer() {
        let cli = parse(&["nxm://stardewvalley/mods/1/files/2?key=k&expires=1"]);
        assert_eq!(
            cli.nxm.as_deref(),
            Some("nxm://stardewvalley/mods/1/files/2?key=k&expires=1")
        );
        assert!(cli.has_request());
        assert_eq!(cli.launch, None);
    }

    #[cfg(not(windows))]
    #[test]
    fn windows_files_open_in_a_game_prefix() {
        let file = |args: &[&str]| parse(args).windows_file;
        assert_eq!(
            file(&["/home/me/Downloads/Setup.EXE"]),
            Some(PathBuf::from("/home/me/Downloads/Setup.EXE"))
        );
        // File managers may pass a `file://` URL for `%u`.
        assert_eq!(
            file(&["file:///home/me/My%20Games/patch.msi"]),
            Some(PathBuf::from("/home/me/My Games/patch.msi"))
        );
        assert!(parse(&["/tmp/fix.reg"]).has_request());
        assert_eq!(file(&["/home/me/notes.txt"]), None);
        assert_eq!(file(&["file:///home/me/notes.txt"]), None);
        // Relative paths are made absolute before parsing, never guessed.
        assert_eq!(file(&["setup.exe"]), None);
        assert_eq!(
            absolute_file_args(
                vec![
                    "setup.exe".into(),
                    "--minimized".into(),
                    "/abs/x.bat".into(),
                    "file:///abs/y.msi".into(),
                ],
                Path::new("/home/me/Downloads"),
            ),
            [
                "/home/me/Downloads/setup.exe",
                "--minimized",
                "/abs/x.bat",
                "file:///abs/y.msi"
            ]
        );
        assert!(is_windows_file_arg("setup.exe"));
        assert!(!is_windows_file_arg("refresh"));
    }

    #[test]
    fn lutris_links_open_the_script_installer() {
        let slug = |args: &[&str]| parse(args).lutris;
        assert_eq!(slug(&["irori://lutris/quake"]), Some("quake".into()));
        assert_eq!(slug(&["lutris:quake"]), Some("quake".into()));
        assert_eq!(
            slug(&["lutris:install/the-witcher-3"]),
            Some("the-witcher-3".into())
        );
        assert_eq!(slug(&["lutris:quake?revision=12"]), Some("quake".into()));
        assert!(parse(&["lutris:quake"]).has_request());
        // Not a game: Lutris's own launch links and anything path-like.
        assert_eq!(slug(&["lutris:rungameid/12"]), None);
        assert_eq!(slug(&["irori://lutris/..%2Fetc"]), None);
        assert_eq!(slug(&["lutris:"]), None);
        assert_eq!(slug(&["irori://lutris"]), None);
    }
}
