//! Talking to the Steam client: `steam://` links for what Steam does itself
//! (install, uninstall, verify, show a game), and its program for launches
//! with arguments. When Irori runs as a Flatpak both run on the host, where
//! Steam is (`irori_core::host`). Both are started detached from Irori
//! ([`Opener`]): an opener that starts Steam would otherwise leave Steam with
//! Irori's pipes and in a game's process group.

use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::future::BoxFuture;
use irori_core::host;
use irori_core::process::CommandSpec;
use parking_lot::Mutex;

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

/// How links and launches reach Steam.
pub(crate) enum Opener {
    /// Started detached from Irori ([`irori_core::process::open_detached`]),
    /// so a Steam it starts never depends on Irori and outlives it.
    System,
    /// Recorded and never run, for test stores: no test starts Steam.
    Recorded(Mutex<Vec<CommandSpec>>),
}

impl Opener {
    /// Run `command`; an error when the opener says it couldn't.
    pub(crate) async fn open(&self, command: &CommandSpec) -> Result<()> {
        match self {
            Opener::System => irori_core::process::open_detached(command).await,
            Opener::Recorded(opened) => {
                opened.lock().push(command.clone());
                Ok(())
            }
        }
    }

    /// Try `commands` in order until one reaches Steam; the one that did.
    pub(crate) async fn hand_over(&self, commands: Vec<CommandSpec>) -> Result<CommandSpec> {
        hand_over(
            |command| Box::pin(async move { self.open(&command).await }),
            commands,
        )
        .await
    }

    /// Hand `url` to Steam: through the system's opener, else Steam's own
    /// program for `steam_dir`.
    pub(crate) async fn open_link(&self, steam_dir: &Path, url: &str) -> Result<()> {
        let commands = link_commands(url, steam_program(steam_dir).await);
        self.hand_over(commands).await.map(drop)
    }

    /// What a test store was asked to open, in order.
    #[cfg(test)]
    pub(crate) fn opened(&self) -> Vec<CommandSpec> {
        match self {
            Opener::System => Vec::new(),
            Opener::Recorded(opened) => opened.lock().clone(),
        }
    }
}

/// The launch plan's command once Steam has the game: Steam was already
/// handed it through the [`Opener`], so this only exits at once and the
/// session follows the game's own processes.
pub(crate) fn handed_over() -> CommandSpec {
    if cfg!(windows) {
        CommandSpec::new("cmd.exe")
            .args(["/d", "/c", "exit 0"])
            .console(irori_core::process::ConsoleWindow::Hidden)
    } else {
        CommandSpec::new("true")
    }
}

/// Note in the game's log what Steam was handed, since the launch plan's
/// own command says nothing.
pub(crate) async fn log_handover(log_file: &Path, command: &CommandSpec) {
    use tokio::io::AsyncWriteExt as _;
    let line = format!("Handed to Steam: {command}\n");
    let written = async {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(log_file)
            .await?
            .write_all(line.as_bytes())
            .await
    };
    if let Err(e) = written.await {
        tracing::debug!("writing to {}: {e}", log_file.display());
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

/// The commands that can start `app_id` with `args`, in the order to try
/// them. Without arguments Steam's `rungameid` link is enough, with Steam's
/// own program as the fallback ([`link_commands`]). With them, `-applaunch`
/// passes them silently; the `steam://run` link is the fallback, for which
/// Steam asks the user to confirm the arguments.
pub(crate) async fn launch_commands(
    steam_dir: &Path,
    app_id: &str,
    args: &[String],
) -> Vec<CommandSpec> {
    launch_commands_with(steam_program(steam_dir).await, app_id, args)
}

/// [`launch_commands`], given Steam's program when found.
fn launch_commands_with(
    steam: Option<CommandSpec>,
    app_id: &str,
    args: &[String],
) -> Vec<CommandSpec> {
    if args.is_empty() {
        return link_commands(&format!("steam://rungameid/{app_id}"), steam);
    }
    let link = open_url(&run_link(app_id, args));
    match steam {
        Some(steam) => vec![steam.arg("-applaunch").arg(app_id).args(args.iter()), link],
        None => vec![link],
    }
}

/// The commands that can hand `url` to Steam, in the order to try them: the
/// system's opener, then Steam's own program with the link, which is what
/// Steam's desktop entry runs. The second is for systems whose opener
/// can't open `steam://` links (a default handler that is missing or names
/// a program that is gone).
pub(crate) fn link_commands(url: &str, steam: Option<CommandSpec>) -> Vec<CommandSpec> {
    let mut commands = vec![open_url(url)];
    commands.extend(steam.map(|steam| steam.arg(url)));
    commands
}

/// Run `commands` with `open` in order until one hands its request over;
/// the one that did. When none does, the last one's error.
pub(crate) async fn hand_over<'a>(
    open: impl Fn(CommandSpec) -> BoxFuture<'a, Result<()>>,
    commands: Vec<CommandSpec>,
) -> Result<CommandSpec> {
    let mut failed = None;
    for command in commands {
        match open(command.clone()).await {
            Ok(()) => return Ok(command),
            Err(e) => {
                tracing::warn!("{command} didn't reach Steam: {e:#}");
                failed = Some(e);
            }
        }
    }
    Err(failed.unwrap_or_else(|| anyhow::anyhow!("nothing can open Steam")))
}

/// Steam's `run` link for `app_id` with `args`. Each argument stays one
/// argument: the link quotes those with blanks or quotes.
fn run_link(app_id: &str, args: &[String]) -> String {
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
    format!("steam://run/{app_id}//{encoded}/")
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
        let plain = launch_commands(dir.path(), "620", &[]).await;
        assert_eq!(argv(&plain[0]).last().unwrap(), "steam://rungameid/620");

        let args = vec!["-novid".to_string(), "+map test".to_string()];
        let with_args = launch_commands(dir.path(), "620", &args).await;
        let v = argv(&with_args[0]);
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

    /// An opener that can't open `steam://` links (Zorin OS with a broken
    /// default handler: `xdg-open` exits 4) leaves the link to Steam's own
    /// program, as Steam's desktop entry would run it.
    #[tokio::test]
    async fn a_link_the_opener_refuses_goes_to_steams_program() {
        let steam = CommandSpec::new("/usr/bin/steam");
        let opener = open_url("steam://x").program;
        let tried = std::sync::Arc::new(Mutex::new(Vec::new()));
        // A fake opener refusing every command of `refused`, noting each.
        let refusing = |refused: std::path::PathBuf| {
            let tried = tried.clone();
            move |command: CommandSpec| -> BoxFuture<'static, Result<()>> {
                tried.lock().push(command.clone());
                let refuses = command.program == refused;
                Box::pin(async move {
                    if refuses {
                        anyhow::bail!("it couldn't open it (exit status: 4)")
                    }
                    Ok(())
                })
            }
        };

        let commands = launch_commands_with(Some(steam.clone()), "620", &[]);
        let used = hand_over(refusing(opener.clone()), commands).await.unwrap();
        assert_eq!(used.program, steam.program);
        assert_eq!(argv(&used), ["steam://rungameid/620"]);
        assert_eq!(tried.lock().len(), 2);

        // A working opener is the only one asked.
        tried.lock().clear();
        let commands = launch_commands_with(Some(steam.clone()), "620", &[]);
        let used = hand_over(refusing("nothing".into()), commands)
            .await
            .unwrap();
        assert_eq!(used.program, opener);
        assert_eq!(tried.lock().len(), 1);

        // Without Steam's program the opener's refusal is the error.
        let commands = launch_commands_with(None, "620", &[]);
        let e = hand_over(refusing(opener.clone()), commands)
            .await
            .unwrap_err();
        assert!(format!("{e:#}").contains("exit status: 4"), "{e:#}");

        // `-applaunch` that fails falls back to the `run` link.
        let args = ["-novid".to_string()];
        let commands = launch_commands_with(Some(steam.clone()), "620", &args);
        let used = hand_over(refusing(steam.program.clone()), commands)
            .await
            .unwrap();
        assert_eq!(argv(&used).last().unwrap(), "steam://run/620//-novid/");

        // Every link has the same fallback.
        let link = link_commands("steam://validate/70", Some(steam.clone()));
        assert_eq!(link.len(), 2);
        assert_eq!(argv(&link[1]), ["steam://validate/70"]);
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
        let windows = launch_commands_with(
            Some(CommandSpec::new(r"C:\Program Files (x86)\Steam\steam.exe")),
            "620",
            &args,
        )
        .remove(0);
        let linux =
            launch_commands_with(Some(CommandSpec::new("/usr/bin/steam")), "620", &args).remove(0);
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
        let link = launch_commands_with(None, "620", &expected()).remove(0);
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
