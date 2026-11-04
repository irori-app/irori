//! Starting Irori when the user logs in (`iroriStartWithSystem`): an
//! autostart desktop entry on Linux, a login launch agent on macOS and a
//! `Run` value on Windows. Started this way Irori goes straight to the tray
//! when `iroriStartWithSystemMinimized` is on (the default).
//!
//! The entry is written from the settings at startup and whenever they
//! change, so it always names the program that is running now (an
//! AppImage's own file, the Flatpak, the installed program).

#[cfg(any(not(windows), test))]
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

/// The program and arguments that start this Irori.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Variables the program needs (a relocated `IRORI_HOME`).
    pub env: Vec<(String, String)>,
}

impl Launch {
    /// The command for the running Irori, with `--minimized` when asked.
    pub fn current(minimized: bool) -> Result<Self> {
        let mut args = Vec::new();
        let program = if let Some(id) = std::env::var_os("FLATPAK_ID") {
            args.extend(["run".to_string(), id.to_string_lossy().into_owned()]);
            PathBuf::from("flatpak")
        } else if let Some(appimage) = std::env::var_os("APPIMAGE") {
            PathBuf::from(appimage)
        } else {
            std::env::current_exe().context("finding Irori's program")?
        };
        if minimized {
            args.push("--minimized".into());
        }
        let env = std::env::var("IRORI_HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(|h| vec![("IRORI_HOME".to_string(), h)])
            .unwrap_or_default();
        Ok(Self { program, args, env })
    }
}

/// Quote one word of a desktop entry's `Exec` line.
#[cfg(any(not(windows), test))]
fn exec_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@".contains(c));
    if plain {
        return word.to_string();
    }
    let mut out = String::from("\"");
    for c in word.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // `%` starts a field code; a literal one is doubled.
    out.replace('%', "%%")
}

/// The autostart desktop entry (Linux and other XDG desktops).
#[cfg(any(not(windows), test))]
pub fn desktop_entry(launch: &Launch) -> String {
    let mut words: Vec<String> = Vec::new();
    if !launch.env.is_empty() {
        words.push("env".into());
        words.extend(
            launch
                .env
                .iter()
                .map(|(k, v)| exec_word(&format!("{k}={v}"))),
        );
    }
    words.push(exec_word(&launch.program.to_string_lossy()));
    words.extend(launch.args.iter().map(|a| exec_word(a)));
    format!(
        "[Desktop Entry]\nType=Application\nName=Irori\nComment=Start Irori when you log in\nExec={}\nIcon=irori\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        words.join(" ")
    )
}

/// The login launch agent (macOS). Inside an app bundle the bundle is
/// opened, so macOS treats it as the app rather than a bare program.
#[cfg(any(not(windows), test))]
pub fn launch_agent(launch: &Launch) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let bundle = launch
        .program
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"));
    let mut argv: Vec<String> = match bundle {
        Some(app) => {
            let mut v = vec![
                "/usr/bin/open".to_string(),
                "-a".to_string(),
                app.to_string_lossy().into_owned(),
            ];
            if !launch.args.is_empty() {
                v.push("--args".into());
            }
            v
        }
        None => vec![launch.program.to_string_lossy().into_owned()],
    };
    argv.extend(launch.args.iter().cloned());
    let args: String = argv
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", escape(a)))
        .collect();
    let env: String = if launch.env.is_empty() {
        String::new()
    } else {
        let vars: String = launch
            .env
            .iter()
            .map(|(k, v)| {
                format!(
                    "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                    escape(k),
                    escape(v)
                )
            })
            .collect();
        format!("\t<key>EnvironmentVariables</key>\n\t<dict>\n{vars}\t</dict>\n")
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key>\n\t<string>{LABEL}</string>\n\t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n{env}\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>ProcessType</key>\n\t<string>Interactive</string>\n</dict>\n</plist>\n"
    )
}

/// The `Run` value (Windows): the quoted program and its arguments.
#[cfg(any(windows, test))]
pub fn run_value(launch: &Launch) -> String {
    let mut out = format!("\"{}\"", launch.program.display());
    for arg in &launch.args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// The launch agent's label and file name.
#[cfg(any(not(windows), test))]
const LABEL: &str = "app.irori.Irori.autostart";

/// Where the entry lives on this system (Windows uses the registry instead).
#[cfg(not(windows))]
fn entry_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(dirs::home_dir()?.join(format!("Library/LaunchAgents/{LABEL}.plist")))
    } else {
        Some(dirs::config_dir()?.join("autostart/irori.desktop"))
    }
}

/// Write the entry for `launch`, or remove it with `None`. Unchanged files
/// are not rewritten.
pub fn apply(launch: Option<&Launch>) -> Result<()> {
    #[cfg(windows)]
    {
        windows_run(launch)
    }
    #[cfg(not(windows))]
    {
        let path = entry_path().context("finding the autostart folder")?;
        write_entry(
            &path,
            launch.map(|l| {
                if cfg!(target_os = "macos") {
                    launch_agent(l)
                } else {
                    desktop_entry(l)
                }
            }),
        )
    }
}

/// Make `path` hold `contents`, or remove it for `None`.
#[cfg(any(not(windows), test))]
fn write_entry(path: &Path, contents: Option<String>) -> Result<()> {
    match contents {
        Some(text) => {
            if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
                return Ok(());
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
            }
            irori_core::json_store::write_bytes(path, text.as_bytes())
                .with_context(|| format!("writing {}", path.display()))
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        },
    }
}

#[cfg(windows)]
fn windows_run(launch: Option<&Launch>) -> Result<()> {
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let key = windows_registry::CURRENT_USER
        .create(RUN)
        .map_err(|e| anyhow::anyhow!("opening the Run key: {e}"))?;
    match launch {
        Some(l) => {
            let value = run_value(l);
            if key.get_string("Irori").is_ok_and(|old| old == value) {
                return Ok(());
            }
            key.set_string("Irori", &value)
                .map_err(|e| anyhow::anyhow!("writing the Run value: {e}"))
        }
        None if key.get_string("Irori").is_err() => Ok(()),
        None => key
            .remove_value("Irori")
            .map_err(|e| anyhow::anyhow!("removing the Run value: {e}")),
    }
}

/// Keep the entry in step with the settings, off the UI thread.
pub fn sync(enabled: bool, minimized: bool) {
    // Tests must never touch the real autostart entry.
    if cfg!(test) {
        return;
    }
    std::thread::Builder::new()
        .name("irori-autostart".into())
        .spawn(move || {
            let result = if enabled {
                Launch::current(minimized).and_then(|l| apply(Some(&l)))
            } else {
                apply(None)
            };
            if let Err(e) = result {
                tracing::warn!("updating the start-with-system entry: {e:#}");
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(program: &str, minimized: bool) -> Launch {
        Launch {
            program: program.into(),
            args: if minimized {
                vec!["--minimized".into()]
            } else {
                Vec::new()
            },
            env: Vec::new(),
        }
    }

    #[test]
    fn desktop_entries_quote_the_program() {
        let entry = desktop_entry(&launch("/opt/My Games/irori", true));
        assert!(entry.contains("Exec=\"/opt/My Games/irori\" --minimized\n"));
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        let plain = desktop_entry(&Launch {
            env: vec![("IRORI_HOME".into(), "/data/irori".into())],
            ..launch("/usr/bin/irori", false)
        });
        assert!(plain.contains("Exec=env IRORI_HOME=/data/irori /usr/bin/irori\n"));
        assert_eq!(exec_word("50%"), "\"50%%\"");
        assert_eq!(exec_word("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn launch_agents_open_the_app_bundle() {
        let agent = launch_agent(&launch(
            "/Applications/Irori.app/Contents/MacOS/irori",
            true,
        ));
        assert!(agent.contains("<string>/usr/bin/open</string>"));
        assert!(agent.contains("<string>/Applications/Irori.app</string>"));
        assert!(agent.contains("<string>--args</string>\n\t\t<string>--minimized</string>"));
        assert!(agent.contains("<key>RunAtLoad</key>\n\t<true/>"));
        let bare = launch_agent(&launch("/usr/local/bin/irori", false));
        assert!(bare.contains("<string>/usr/local/bin/irori</string>\n\t</array>"));
        assert!(!bare.contains("open"));
    }

    #[test]
    fn run_values_quote_the_program() {
        assert_eq!(
            run_value(&launch(
                r"C:\Users\Me\AppData\Local\Programs\Irori\irori.exe",
                true
            )),
            r#""C:\Users\Me\AppData\Local\Programs\Irori\irori.exe" --minimized"#
        );
    }

    #[test]
    fn writes_rewrites_and_removes_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autostart/irori.desktop");
        write_entry(&path, Some("one".into())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one");
        write_entry(&path, Some("two".into())).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        write_entry(&path, None).unwrap();
        assert!(!path.exists());
        // Removing what isn't there is fine.
        write_entry(&path, None).unwrap();
    }
}
