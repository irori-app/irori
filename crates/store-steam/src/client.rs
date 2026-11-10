//! Talking to the Steam client: `steam://` links for what Steam does itself
//! (install, uninstall, verify, show a game), and its program for launches
//! with arguments. When Irori runs as a Flatpak both run on the host, where
//! Steam is (`irori_core::host`).

use std::path::{Path, PathBuf};

use irori_core::host;
use irori_core::process::CommandSpec;

/// Flatpak Steam's application ID.
const FLATPAK_ID: &str = "com.valvesoftware.Steam";

/// The command that hands `url` to the program registered for it. Inside a
/// Flatpak the host's `xdg-open` takes it, so Steam gets the link without
/// the portal asking which program should open it.
pub(crate) fn open_url(url: &str) -> CommandSpec {
    host::command(local_open_url(url))
}

fn local_open_url(url: &str) -> CommandSpec {
    if cfg!(target_os = "macos") {
        CommandSpec::new("/usr/bin/open").arg(url)
    } else if cfg!(windows) {
        CommandSpec::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
    } else {
        CommandSpec::new("xdg-open").arg(url)
    }
}

/// The Steam program for `steam_dir`, as a command to add arguments to.
/// `None` when it can't be found; `steam://` links still work then.
pub(crate) async fn steam_program(steam_dir: &Path) -> Option<CommandSpec> {
    if cfg!(target_os = "linux") {
        if steam_dir.components().any(|c| c.as_os_str() == FLATPAK_ID) {
            let flatpak = host::find_program("flatpak").await?;
            return Some(host::command(
                CommandSpec::new(flatpak).arg("run").arg(FLATPAK_ID),
            ));
        }
        let steam = host::find_program("steam").await?;
        return Some(host::command(CommandSpec::new(steam)));
    }
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        vec![steam_dir.join("steam.exe")]
    } else {
        vec![
            PathBuf::from("/Applications/Steam.app/Contents/MacOS/steam_osx"),
            irori_core::paths::expand_home("~/Applications/Steam.app/Contents/MacOS/steam_osx"),
        ]
    };
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(CommandSpec::new)
}

/// The arguments a Steam game gets: the game's launch options in Irori, then
/// `extra`. They are split by the rules of the program that reads them
/// (`split_program_args`): a Windows build, which every game is on Windows
/// and a game Steam runs through Proton is elsewhere, reads Windows
/// quoting, so `C:\Users\me\saves` keeps its backslashes on every system.
pub(crate) fn game_args(
    launcher_args: &str,
    extra: &[String],
    windows_program: bool,
) -> Vec<String> {
    let mut args = irori_core::process::split_program_args(windows_program, launcher_args);
    args.extend(extra.iter().cloned());
    args
}

/// The command starting `app_id` with `args`. Without arguments Steam's
/// `rungameid` link is enough. With them, `-applaunch` passes them silently;
/// the `steam://run` link is the fallback, for which Steam asks the user to
/// confirm the arguments.
pub(crate) async fn launch_command(steam_dir: &Path, app_id: &str, args: &[String]) -> CommandSpec {
    if args.is_empty() {
        return open_url(&format!("steam://rungameid/{app_id}"));
    }
    launch_command_with(steam_program(steam_dir).await, app_id, args)
}

/// [`launch_command`] with arguments, given Steam's program when found.
/// Each argument stays one argument: the program gets them as a list, and
/// the link quotes those with blanks or quotes.
fn launch_command_with(steam: Option<CommandSpec>, app_id: &str, args: &[String]) -> CommandSpec {
    if let Some(steam) = steam {
        return steam.arg("-applaunch").arg(app_id).args(args.iter());
    }
    let joined = args
        .iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '"') {
                irori_core::process::windows_arg(a)
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let encoded: String = url::form_urlencoded::byte_serialize(joined.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    open_url(&format!("steam://run/{app_id}//{encoded}/"))
}

/// A request to the Steam client about one game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientLink {
    Install,
    Uninstall,
    Verify,
    Details,
}

impl ClientLink {
    pub(crate) fn url(self, app_id: &str) -> String {
        match self {
            ClientLink::Install => format!("steam://install/{app_id}"),
            ClientLink::Uninstall => format!("steam://uninstall/{app_id}"),
            ClientLink::Verify => format!("steam://validate/{app_id}"),
            ClientLink::Details => format!("steam://nav/games/details/{app_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[tokio::test]
    async fn launches_through_links_or_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let plain = launch_command(dir.path(), "620", &[]).await;
        assert_eq!(argv(&plain).last().unwrap(), "steam://rungameid/620");

        let args = vec!["-novid".to_string(), "+map test".to_string()];
        let with_args = launch_command(dir.path(), "620", &args).await;
        let v = argv(&with_args);
        if steam_program(dir.path()).await.is_some() {
            assert_eq!(
                &v[v.len() - 4..],
                ["-applaunch", "620", "-novid", "+map test"]
            );
        } else {
            assert_eq!(
                v.last().unwrap(),
                "steam://run/620//-novid%20%22%2Bmap%20test%22/"
            );
        }
    }

    /// Launch options with a Windows path and a quoted value, as a user
    /// types them for a Windows build.
    const OPTIONS: &str = r#"-savedir C:\Users\me\saves "+connect 10.0.0.2:27015""#;

    fn expected() -> Vec<String> {
        [
            "-savedir",
            r"C:\Users\me\saves",
            "+connect 10.0.0.2:27015",
            "-novid",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn windows_builds_get_their_launch_options_as_written_everywhere() {
        let extra = ["-novid".to_string()];
        let args = game_args(OPTIONS, &extra, true);
        assert_eq!(args, expected());
        // The command line steam.exe gets on Windows, and the one Steam's
        // program gets elsewhere, carry the same arguments.
        let windows = launch_command_with(
            Some(CommandSpec::new(r"C:\Program Files (x86)\Steam\steam.exe")),
            "620",
            &args,
        );
        let linux = launch_command_with(Some(CommandSpec::new("/usr/bin/steam")), "620", &args);
        assert_eq!(
            windows.program,
            std::path::PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe")
        );
        let mut want = vec!["-applaunch".to_string(), "620".to_string()];
        want.extend(expected());
        assert_eq!(argv(&windows), want);
        assert_eq!(argv(&linux), want);
        // Quoted the way Windows programs read their command line, each
        // argument comes back whole.
        let command_line = argv(&windows)
            .iter()
            .map(|a| irori_core::process::windows_arg(a))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            irori_core::process::split_program_args(true, &command_line),
            want
        );
        // A native build reads this system's quoting.
        if !cfg!(windows) {
            assert_eq!(game_args("-x 'two words'", &[], false), ["-x", "two words"]);
        }
    }

    #[test]
    fn the_link_keeps_each_argument_whole() {
        let link = launch_command_with(None, "620", &expected());
        let link = argv(&link).last().unwrap().clone();
        let encoded = link
            .strip_prefix("steam://run/620//")
            .and_then(|l| l.strip_suffix('/'))
            .unwrap();
        let joined: String = url::form_urlencoded::parse(format!("a={encoded}").as_bytes())
            .next()
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            irori_core::process::split_program_args(true, &joined),
            expected()
        );
    }

    #[test]
    fn links_name_the_app() {
        assert_eq!(ClientLink::Install.url("70"), "steam://install/70");
        assert_eq!(ClientLink::Uninstall.url("70"), "steam://uninstall/70");
        assert_eq!(ClientLink::Verify.url("70"), "steam://validate/70");
        assert_eq!(
            ClientLink::Details.url("70"),
            "steam://nav/games/details/70"
        );
    }
}
