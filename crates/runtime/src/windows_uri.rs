//! Links of other programs' schemes (`ddnet:[::1]:8303`, a Windows client's
//! `someapp://join/…`) that the user points at Irori: which ones Irori
//! forwards to a game's Wine prefix, and the Wine command that opens one
//! there, where the Windows program that registered the scheme takes it.

use crate::prefix::{WineCommand, WineContext};

/// Schemes never forwarded to a prefix: Irori's own links (and the ones it
/// takes from Heroic, Lutris and Nexus Mods), local files (a Windows file
/// opens through `windows_file` instead), web pages, which belong in the
/// user's browser and not in a Wine one, and a browser's internal schemes,
/// which no program registers.
const KEPT: &[&str] = &[
    "irori",
    "heroic",
    "lutris",
    "nxm",
    "file",
    "http",
    "https",
    "about",
    "blob",
    "data",
    "javascript",
];

/// Longest link forwarded. Windows programs rarely take more than a few
/// hundred characters; anything far longer is not a link a person opened.
pub const MAX_LEN: usize = 4096;

/// The scheme of an argument that starts like a URI (RFC 3986: a letter,
/// then letters, digits, `+`, `-` or `.`, then `:`), lower case. A single
/// letter is a Windows drive (`C:\Games`), not a scheme, and paths, flags
/// and commands have none.
pub fn scheme(arg: &str) -> Option<String> {
    let (scheme, _) = arg.split_once(':')?;
    let mut chars = scheme.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    (starts_with_letter
        && scheme.len() >= 2
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then(|| scheme.to_ascii_lowercase())
}

/// Whether Irori forwards this link to a game's Wine prefix: it has a scheme
/// Irori doesn't open itself, something after the scheme, and nothing a URI
/// never holds (spaces, quotes, control characters), so Wine's `start`
/// reads it as the one link it is. Reads nothing from disk.
pub fn is_forwarded_uri(arg: &str) -> bool {
    let Some(scheme) = scheme(arg) else {
        return false;
    };
    let rest = &arg[scheme.len() + 1..];
    !KEPT.contains(&scheme.as_str())
        && !rest.is_empty()
        && arg.len() <= MAX_LEN
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

/// The command that opens `uri` in a prefix: Wine's `start`, which hands it
/// to the program the prefix registered for its scheme. The link is one
/// argument and never passes through a shell.
///
/// Proton runs it with `runinprefix` and `start /wait`, so the command lasts
/// as long as the program it reaches; under umu, `UMU_CONTAINER_NSENTER`
/// joins the Steam runtime container of a game already running in that
/// prefix instead of starting a second one, so a running game's client gets
/// the link.
pub fn wine_command(wc: &WineContext, uri: &str) -> WineCommand {
    let mut cmd = if wc.is_proton() {
        let mut cmd = WineCommand::new(["start", "/wait", uri]);
        cmd.verb = "runinprefix";
        cmd
    } else {
        WineCommand::new(["start", uri])
    };
    if wc.uses_umu {
        cmd.env.insert("UMU_CONTAINER_NSENTER".into(), "1".into());
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::HostEnv;
    use crate::prefix::wine_spec;
    use irori_core::{AppSettings, GameSettings, WineInstallation, WineKind};

    #[test]
    fn forwards_other_programs_links() {
        for uri in [
            "ddnet:[::1]:8303",
            "anotherscheme:xyz",
            "someapp://join/1234?pass=abc",
            "steam://run/440",
            "Battle.Net://start",
            "x-game+launch:42",
            "mailto:me@example.com",
        ] {
            assert!(is_forwarded_uri(uri), "{uri}");
        }
    }

    #[test]
    fn keeps_links_irori_opens_itself() {
        for uri in [
            "irori://launch/gog/1",
            "heroic://launch?appName=foo&runner=legendary",
            "HEROIC://launch/foo",
            "lutris:quake",
            "nxm://skyrim/mods/1/files/2",
            "file:///home/me/setup.exe",
            "http://example.com",
            "https://example.com/page",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
        ] {
            assert!(!is_forwarded_uri(uri), "{uri}");
        }
    }

    #[test]
    fn paths_commands_and_drives_are_not_links() {
        for arg in [
            "C:\\Games\\game.exe",
            "C:/Games/game.exe",
            "c:",
            "/home/me/setup.exe",
            "./a:b",
            "setup.exe",
            "refresh",
            "--launch",
            "-l",
            "gog/123",
            "1password:",
            "-x:y",
            "ddnet:",
            "",
        ] {
            assert!(!is_forwarded_uri(arg), "{arg}");
        }
    }

    #[test]
    fn links_a_uri_never_holds_are_refused() {
        assert!(!is_forwarded_uri("ddnet:a b"));
        assert!(!is_forwarded_uri("ddnet:a\"b"));
        assert!(!is_forwarded_uri("ddnet:a\nb"));
        assert!(!is_forwarded_uri("ddnet:a\tb"));
        let long = format!("ddnet:{}", "a".repeat(MAX_LEN));
        assert!(!is_forwarded_uri(&long));
    }

    #[test]
    fn scheme_is_lower_case_and_needs_two_characters() {
        assert_eq!(scheme("DDNet:1"), Some("ddnet".into()));
        assert_eq!(scheme("a+b-c.d:1"), Some("a+b-c.d".into()));
        assert_eq!(scheme("C:\\x"), None);
        assert_eq!(scheme("9p:x"), None);
        assert_eq!(scheme("no colon"), None);
    }

    fn context(kind: WineKind, umu: bool) -> WineContext {
        let bin = match kind {
            WineKind::Proton => "/p/GE/proton",
            _ => "/usr/bin/wine",
        };
        WineContext::new(
            WineInstallation {
                bin: bin.into(),
                kind,
                name: "build".into(),
                ..Default::default()
            },
            "/pfx".into(),
            umu,
            "/usr/bin/umu-run".into(),
        )
    }

    #[test]
    fn wine_starts_the_link_as_one_argument() {
        let uri = "ddnet:[::1]:8303";
        let (app, gs, host) = (
            AppSettings::default(),
            GameSettings::default(),
            HostEnv::default(),
        );
        let spec = |wc: &WineContext| wine_spec(&app, &gs, wc, &wine_command(wc, uri), &host);

        let wine = context(WineKind::Wine, false);
        let s = spec(&wine);
        assert_eq!(s.program, std::path::PathBuf::from("/usr/bin/wine"));
        assert_eq!(s.args, ["start", uri]);
        assert!(!s.env.contains_key("UMU_CONTAINER_NSENTER"));

        let proton = context(WineKind::Proton, false);
        let s = spec(&proton);
        assert_eq!(s.args, ["runinprefix", "start", "/wait", uri]);
        assert_eq!(s.env["PROTON_VERB"], "runinprefix");
        assert!(!s.env.contains_key("UMU_CONTAINER_NSENTER"));

        let umu = context(WineKind::Proton, true);
        let s = spec(&umu);
        assert_eq!(s.program, std::path::PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(s.args, ["start", "/wait", uri]);
        assert_eq!(s.env["PROTON_VERB"], "runinprefix");
        assert_eq!(s.env["UMU_CONTAINER_NSENTER"], "1");
    }
}
