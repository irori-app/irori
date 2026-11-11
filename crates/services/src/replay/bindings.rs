//! Where the clip and screenshot shortcuts can come from on this desktop,
//! and, where Irori can't listen for keys itself, the exact configuration
//! lines that bind `irori --save-clip` and `irori --screenshot` in the
//! user's compositor or window manager. Pure: everything it needs is passed
//! in, so every desktop's choice and every line is tested here.

use std::path::{Path, PathBuf};

/// The keys Irori asks for, or grabs itself.
pub const CLIP_KEYS: &str = "Alt+F10";
pub const SCREENSHOT_KEYS: &str = "Alt+F12";

/// The command-line flags a binding runs (`crates/irori/src/cli.rs`).
pub const CLIP_FLAG: &str = "--save-clip";
pub const SCREENSHOT_FLAG: &str = "--screenshot";

/// The compositors and window managers whose configuration Irori can write
/// a binding for. Their names are never translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    Sway,
    I3,
    Hyprland,
    River,
    Niri,
    Wayfire,
    Labwc,
    Openbox,
    /// bspwm and others that take their keys from sxhkd.
    Sxhkd,
}

impl Compositor {
    /// The project's own name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sway => "Sway",
            Self::I3 => "i3",
            Self::Hyprland => "Hyprland",
            Self::River => "river",
            Self::Niri => "niri",
            Self::Wayfire => "Wayfire",
            Self::Labwc => "labwc",
            Self::Openbox => "Openbox",
            Self::Sxhkd => "sxhkd",
        }
    }

    /// The file the lines go in.
    pub fn config_file(self) -> &'static str {
        match self {
            Self::Sway => "~/.config/sway/config",
            Self::I3 => "~/.config/i3/config",
            Self::Hyprland => "~/.config/hypr/hyprland.conf",
            Self::River => "~/.config/river/init",
            Self::Niri => "~/.config/niri/config.kdl",
            Self::Wayfire => "~/.config/wayfire.ini",
            Self::Labwc => "~/.config/labwc/rc.xml",
            Self::Openbox => "~/.config/openbox/rc.xml",
            Self::Sxhkd => "~/.config/sxhkd/sxhkdrc",
        }
    }
}

/// The graphical session Irori runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// An X11 session, where a program may grab keys from the X server.
    /// Never under Wayland: XWayland only sees keys while one of its own
    /// windows has the focus.
    pub x11: bool,
    pub compositor: Option<Compositor>,
}

impl Session {
    /// Read the session from its environment variables.
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| var(name).is_some_and(|v| !v.trim().is_empty());
        let kind = var("XDG_SESSION_TYPE").unwrap_or_default().to_lowercase();
        let x11 = match kind.as_str() {
            "x11" => true,
            "wayland" => false,
            _ => !set("WAYLAND_DISPLAY") && set("DISPLAY"),
        };
        let desktops = var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase()
            .split(':')
            .map(str::to_string)
            .collect::<Vec<_>>();
        let named = |name: &str| desktops.iter().any(|d| d == name);
        let compositor = if set("HYPRLAND_INSTANCE_SIGNATURE") || named("hyprland") {
            Some(Compositor::Hyprland)
        } else if set("NIRI_SOCKET") || named("niri") {
            Some(Compositor::Niri)
        } else if set("SWAYSOCK") || named("sway") {
            Some(Compositor::Sway)
        } else if named("i3") || (x11 && set("I3SOCK")) {
            Some(Compositor::I3)
        } else if named("river") {
            Some(Compositor::River)
        } else if named("wayfire") {
            Some(Compositor::Wayfire)
        } else if named("labwc") {
            Some(Compositor::Labwc)
        } else if named("openbox") {
            Some(Compositor::Openbox)
        } else if named("bspwm") {
            Some(Compositor::Sxhkd)
        } else {
            None
        };
        Self { x11, compositor }
    }
}

/// Ways to get the shortcuts, tried in order until one works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The desktop portal's GlobalShortcuts (KDE Plasma, GNOME, Hyprland).
    Portal,
    /// Irori grabs the keys from the X server (`XGrabKey`).
    X11Grab,
    /// The user binds Irori's commands in their compositor. Always works.
    Commands,
}

/// The sources to try on this system and session. macOS and Windows have
/// none: their keyboard shortcuts while playing are the in-game overlay's.
pub fn sources(linux: bool, session: &Session) -> Vec<Source> {
    match (linux, session.x11) {
        (false, _) => Vec::new(),
        (true, true) => vec![Source::Portal, Source::X11Grab, Source::Commands],
        (true, false) => vec![Source::Portal, Source::Commands],
    }
}

/// How the user's binding starts Irori: its argument list before the flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program(pub Vec<String>);

impl Program {
    /// Inside Irori's Flatpak, `flatpak run`; from an AppImage, the AppImage
    /// (the program inside is in a folder that goes away); `irori` when that
    /// is what `PATH` finds; else the program's full path.
    pub fn find(
        flatpak_id: Option<&str>,
        appimage: Option<&Path>,
        exe: &Path,
        on_path: Option<&Path>,
    ) -> Self {
        if let Some(id) = flatpak_id.filter(|id| !id.is_empty()) {
            return Self(vec!["flatpak".into(), "run".into(), id.into()]);
        }
        if let Some(appimage) = appimage {
            return Self(vec![appimage.to_string_lossy().into_owned()]);
        }
        if on_path.is_some_and(|found| found == exe) {
            return Self(vec!["irori".into()]);
        }
        Self(vec![exe.to_string_lossy().into_owned()])
    }

    /// This process.
    pub fn current() -> Self {
        let exe = std::env::current_exe()
            .ok()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .unwrap_or_else(|| PathBuf::from("irori"));
        let on_path = which("irori").map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        Self::find(
            std::env::var("FLATPAK_ID").ok().as_deref(),
            std::env::var_os("APPIMAGE").map(PathBuf::from).as_deref(),
            &exe,
            on_path.as_deref(),
        )
    }

    /// The full command for `flag`, as a shell reads it.
    pub fn shell(&self, flag: &str) -> String {
        self.0
            .iter()
            .map(|a| shell_quote(a))
            .chain([flag.to_string()])
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The full command for `flag`, as an argument list.
    fn argv<'a>(&'a self, flag: &'a str) -> impl Iterator<Item = &'a str> {
        self.0.iter().map(String::as_str).chain([flag])
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Single quotes around anything a shell would split or expand.
fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:+@%,".contains(&b));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// What the settings show where Irori can't listen for keys itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The compositor the lines are for, if Irori knows it.
    pub compositor: Option<Compositor>,
    /// The lines to paste, or for an unknown desktop the two commands.
    pub lines: String,
}

impl Binding {
    pub fn new(compositor: Option<Compositor>, program: &Program) -> Self {
        let clip = program.shell(CLIP_FLAG);
        let shot = program.shell(SCREENSHOT_FLAG);
        let lines = match compositor {
            Some(Compositor::Sway | Compositor::I3) => {
                format!("bindsym Mod1+F10 exec {clip}\nbindsym Mod1+F12 exec {shot}")
            }
            Some(Compositor::Hyprland) => {
                format!("bind = ALT, F10, exec, {clip}\nbind = ALT, F12, exec, {shot}")
            }
            Some(Compositor::River) => format!(
                "riverctl map normal Alt F10 spawn {}\nriverctl map normal Alt F12 spawn {}",
                shell_quote(&clip),
                shell_quote(&shot)
            ),
            Some(Compositor::Niri) => {
                let kdl = |flag| {
                    program
                        .argv(flag)
                        .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                format!(
                    "binds {{\n    Alt+F10 {{ spawn {}; }}\n    Alt+F12 {{ spawn {}; }}\n}}",
                    kdl(CLIP_FLAG),
                    kdl(SCREENSHOT_FLAG)
                )
            }
            Some(Compositor::Wayfire) => format!(
                "[command]\nbinding_irori_clip = <alt> KEY_F10\ncommand_irori_clip = {clip}\n\
                 binding_irori_screenshot = <alt> KEY_F12\ncommand_irori_screenshot = {shot}"
            ),
            Some(Compositor::Labwc) => format!(
                "<keybind key=\"A-F10\"><action name=\"Execute\" command=\"{}\" /></keybind>\n\
                 <keybind key=\"A-F12\"><action name=\"Execute\" command=\"{}\" /></keybind>",
                xml_escape(&clip),
                xml_escape(&shot)
            ),
            Some(Compositor::Openbox) => format!(
                "<keybind key=\"A-F10\"><action name=\"Execute\"><command>{}</command></action></keybind>\n\
                 <keybind key=\"A-F12\"><action name=\"Execute\"><command>{}</command></action></keybind>",
                xml_escape(&clip),
                xml_escape(&shot)
            ),
            Some(Compositor::Sxhkd) => format!("alt + F10\n\t{clip}\n\nalt + F12\n\t{shot}"),
            None => format!("{clip}\n{shot}"),
        };
        Self { compositor, lines }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn session(vars: &[(&str, &str)]) -> Session {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Session::detect(|name| vars.get(name).cloned())
    }

    #[test]
    fn sessions_are_told_apart() {
        let x11 = |desktop: &str| {
            session(&[
                ("XDG_SESSION_TYPE", "x11"),
                ("DISPLAY", ":0"),
                ("XDG_CURRENT_DESKTOP", desktop),
            ])
        };
        let wayland = |vars: &[(&str, &str)]| {
            let mut all = vec![
                ("XDG_SESSION_TYPE", "wayland"),
                ("WAYLAND_DISPLAY", "wayland-1"),
            ];
            all.extend_from_slice(vars);
            session(&all)
        };
        assert_eq!(
            wayland(&[("SWAYSOCK", "/run/user/1000/sway.sock")]).compositor,
            Some(Compositor::Sway)
        );
        assert_eq!(
            wayland(&[("XDG_CURRENT_DESKTOP", "sway")]).compositor,
            Some(Compositor::Sway)
        );
        assert_eq!(
            wayland(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")]).compositor,
            Some(Compositor::Hyprland)
        );
        assert_eq!(
            wayland(&[("NIRI_SOCKET", "/run/user/1000/niri.sock")]).compositor,
            Some(Compositor::Niri)
        );
        assert_eq!(
            wayland(&[("XDG_CURRENT_DESKTOP", "river")]).compositor,
            Some(Compositor::River)
        );
        assert_eq!(
            wayland(&[("XDG_CURRENT_DESKTOP", "Wayfire")]).compositor,
            Some(Compositor::Wayfire)
        );
        assert_eq!(
            wayland(&[("XDG_CURRENT_DESKTOP", "labwc:wlroots")]).compositor,
            Some(Compositor::Labwc)
        );
        assert_eq!(wayland(&[("XDG_CURRENT_DESKTOP", "KDE")]).compositor, None);
        assert!(!wayland(&[]).x11);
        assert!(x11("i3").x11);
        assert_eq!(x11("i3").compositor, Some(Compositor::I3));
        assert_eq!(x11("bspwm").compositor, Some(Compositor::Sxhkd));
        assert_eq!(x11("openbox").compositor, Some(Compositor::Openbox));
        assert_eq!(x11("XFCE").compositor, None);
        // Without XDG_SESSION_TYPE, the displays decide.
        assert!(session(&[("DISPLAY", ":0")]).x11);
        assert!(!session(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")]).x11);
        assert!(!session(&[]).x11);
    }

    #[test]
    fn x11_grabs_before_commands_and_wayland_never_grabs() {
        let x11 = Session {
            x11: true,
            compositor: None,
        };
        let wayland = Session {
            x11: false,
            compositor: Some(Compositor::Sway),
        };
        assert_eq!(
            sources(true, &x11),
            [Source::Portal, Source::X11Grab, Source::Commands]
        );
        assert_eq!(sources(true, &wayland), [Source::Portal, Source::Commands]);
        assert!(sources(false, &x11).is_empty());
        assert!(sources(false, &wayland).is_empty());
    }

    #[test]
    fn the_program_is_named_the_way_a_binding_can_start_it() {
        let exe = Path::new("/usr/bin/irori");
        assert_eq!(
            Program::find(Some("app.irori.Irori"), None, exe, Some(exe)).shell(CLIP_FLAG),
            "flatpak run app.irori.Irori --save-clip"
        );
        assert_eq!(
            Program::find(
                None,
                Some(Path::new("/home/a/Apps/Irori 1.0.AppImage")),
                exe,
                None
            )
            .shell(SCREENSHOT_FLAG),
            "'/home/a/Apps/Irori 1.0.AppImage' --screenshot"
        );
        assert_eq!(
            Program::find(None, None, exe, Some(exe)).shell(CLIP_FLAG),
            "irori --save-clip"
        );
        let local = Path::new("/home/a/irori/irori");
        assert_eq!(
            Program::find(None, None, local, Some(exe)).shell(CLIP_FLAG),
            "/home/a/irori/irori --save-clip"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn every_compositor_gets_lines_it_reads() {
        let irori = Program(vec!["irori".into()]);
        let lines = |c| Binding::new(Some(c), &irori).lines;
        assert_eq!(
            lines(Compositor::Sway),
            "bindsym Mod1+F10 exec irori --save-clip\nbindsym Mod1+F12 exec irori --screenshot"
        );
        assert_eq!(lines(Compositor::I3), lines(Compositor::Sway));
        assert_eq!(
            lines(Compositor::Hyprland),
            "bind = ALT, F10, exec, irori --save-clip\nbind = ALT, F12, exec, irori --screenshot"
        );
        assert_eq!(
            lines(Compositor::River),
            "riverctl map normal Alt F10 spawn 'irori --save-clip'\n\
             riverctl map normal Alt F12 spawn 'irori --screenshot'"
        );
        assert_eq!(
            lines(Compositor::Niri),
            "binds {\n    Alt+F10 { spawn \"irori\" \"--save-clip\"; }\n    \
             Alt+F12 { spawn \"irori\" \"--screenshot\"; }\n}"
        );
        assert!(
            lines(Compositor::Wayfire)
                .starts_with("[command]\nbinding_irori_clip = <alt> KEY_F10\n")
        );
        assert!(
            lines(Compositor::Wayfire).contains("command_irori_screenshot = irori --screenshot")
        );
        assert_eq!(
            lines(Compositor::Labwc).lines().next(),
            Some(
                r#"<keybind key="A-F10"><action name="Execute" command="irori --save-clip" /></keybind>"#
            )
        );
        assert!(lines(Compositor::Openbox).contains("<command>irori --screenshot</command>"));
        assert_eq!(
            lines(Compositor::Sxhkd),
            "alt + F10\n\tirori --save-clip\n\nalt + F12\n\tirori --screenshot"
        );
        assert_eq!(
            Binding::new(None, &irori).lines,
            "irori --save-clip\nirori --screenshot"
        );

        // A Flatpak's or a spaced path's command survives each syntax.
        let flatpak = Program(vec![
            "flatpak".into(),
            "run".into(),
            "app.irori.Irori".into(),
        ]);
        assert!(
            Binding::new(Some(Compositor::Niri), &flatpak)
                .lines
                .contains(r#"spawn "flatpak" "run" "app.irori.Irori" "--save-clip";"#)
        );
        let spaced = Program(vec!["/opt/My Games/irori".into()]);
        assert!(
            Binding::new(Some(Compositor::River), &spaced)
                .lines
                .contains(r"spawn ''\''/opt/My Games/irori'\'' --save-clip'")
        );
        let quoted = Program(vec!["/opt/a\"b/irori".into()]);
        assert!(
            Binding::new(Some(Compositor::Labwc), &quoted)
                .lines
                .contains("command=\"'/opt/a&quot;b/irori' --save-clip\"")
        );
    }
}
