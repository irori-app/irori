//! Windows setup programs: Inno Setup and NSIS installers and MSI packages.
//!
//! Shared across itch.io installs and direct/web downloads so a setup program
//! can be detected reliably (without mistaking bundled uninstallers or
//! redistributables for game installers) and executed silently either directly
//! on Windows or inside a Wine prefix.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use irori_core::process::CommandSpec;

/// A setup program Irori knows how to run silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    Inno,
    Nsis,
    Msi,
}

/// How much of a file is searched for an installer's marks: Inno Setup and
/// NSIS write theirs near the start of the program.
const SNIFF_BYTES: u64 = 8 << 20;

/// Whether `name` looks like a bundled uninstaller or runtime redistributable
/// (such as `unins000.exe`, `vcredist_x64.exe`, `vc_redist.x64.exe`, or
/// `DXSETUP.exe`) that must never be mistaken for a game installer.
pub fn is_ignored_name(name: &str) -> bool {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    base.starts_with("unins")
        || base.contains("uninstall")
        || base.contains("vcredist")
        || base.contains("vc_redist")
        || base.contains("dxsetup")
        || base.contains("dxwebsetup")
}

/// The setup program `path` (named `name`) is, if any.
///
/// Uninstallers and runtime redistributables (`unins*.exe`, `vcredist*`,
/// `vc_redist*`, `dxsetup*`) always return `None` even when they are built
/// with Inno Setup or NSIS.
pub fn detect(path: &Path, name: &str) -> Option<Setup> {
    if is_ignored_name(name)
        || path
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(is_ignored_name)
    {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let mut head = Vec::new();
    File::open(path)
        .ok()?
        .take(SNIFF_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    classify(&lower, &head)
}

fn classify(name: &str, head: &[u8]) -> Option<Setup> {
    if is_ignored_name(name) {
        return None;
    }
    // An MSI is an OLE compound file.
    if name.ends_with(".msi") && head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return Some(Setup::Msi);
    }
    if !name.ends_with(".exe") || !head.starts_with(b"MZ") {
        return None;
    }
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    if has(b"Inno Setup") {
        Some(Setup::Inno)
    } else if has(b"NullsoftInst") || has(b"Nullsoft.NSIS") {
        Some(Setup::Nsis)
    } else {
        None
    }
}

/// `path` as the setup program sees it: under Wine, on drive `Z:`, which
/// Wine maps to `/`.
pub fn windows_path(path: &Path, native: bool) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    if native { text } else { format!("Z:{text}") }
}

/// The command line that installs `setup` (at `file`) silently into `dir`,
/// writing its log to `log`. `native` says Windows runs it, not Wine.
pub fn command(setup: Setup, file: &Path, dir: &Path, log: &Path, native: bool) -> Vec<String> {
    let file_arg = |p: &Path| {
        if native {
            p.to_string_lossy().into_owned()
        } else {
            windows_path(p, false)
        }
    };
    match setup {
        Setup::Inno => vec![
            file.to_string_lossy().into_owned(),
            "/VERYSILENT".into(),
            "/SUPPRESSMSGBOXES".into(),
            "/NORESTART".into(),
            "/NOICONS".into(),
            "/SP-".into(),
            format!("/DIR={}", windows_path(dir, native)),
            format!("/LOG={}", windows_path(log, native)),
        ],
        // NSIS takes the folder last and unquoted, spaces and all.
        Setup::Nsis => vec![
            file.to_string_lossy().into_owned(),
            "/S".into(),
            format!("/D={}", windows_path(dir, native)),
        ],
        // Most packages install under TARGETDIR or INSTALLDIR; both are
        // given, so either kind lands in the install folder.
        Setup::Msi => vec![
            "msiexec".into(),
            "/i".into(),
            file_arg(file),
            "/qn".into(),
            "/norestart".into(),
            "/l*v".into(),
            windows_path(log, native),
            format!("TARGETDIR={}", windows_path(dir, native)),
            format!("INSTALLDIR={}", windows_path(dir, native)),
        ],
    }
}

/// A [`CommandSpec`] that installs `setup` (at `file`) silently into `dir`,
/// writing its log to `log`.
///
/// On native Windows, NSIS requires `/D=<path>` to be the final argument with
/// no surrounding quotes even when `<path>` contains spaces, so it is attached
/// via [`CommandSpec::raw_arg`].
pub fn command_spec(
    setup: Setup,
    file: &Path,
    dir: &Path,
    log: &Path,
    native: bool,
) -> CommandSpec {
    match setup {
        Setup::Nsis if native => CommandSpec::new(file)
            .arg("/S")
            .raw_arg(format!("/D={}", windows_path(dir, true))),
        _ => {
            let argv = command(setup, file, dir, log, native);
            let mut iter = argv.into_iter();
            let program = iter.next().unwrap_or_default();
            CommandSpec::new(program).args(iter)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn setup_programs_are_told_by_their_marks() {
        let inno = [
            b"MZ\x90\x00".as_slice(),
            b"...Inno Setup Setup Data (6.2.0)",
        ]
        .concat();
        assert_eq!(classify("setup.exe", &inno), Some(Setup::Inno));
        let nsis = [b"MZ".as_slice(), b"\x00\x00NullsoftInst"].concat();
        assert_eq!(classify("game-setup.exe", &nsis), Some(Setup::Nsis));
        let nsis_alt = [b"MZ".as_slice(), b"\x00Nullsoft.NSIS"].concat();
        assert_eq!(classify("installer.exe", &nsis_alt), Some(Setup::Nsis));
        assert_eq!(classify("game.exe", b"MZ just a game"), None);
        assert_eq!(classify("readme.txt", b"Inno Setup"), None);
        assert_eq!(
            classify("game.msi", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1]),
            Some(Setup::Msi)
        );
        assert_eq!(classify("game.msi", b"not ole"), None);
    }

    #[test]
    fn ignores_uninstallers_and_redistributables() {
        let inno = [
            b"MZ\x90\x00".as_slice(),
            b"...Inno Setup Setup Data (6.2.0)",
        ]
        .concat();
        let nsis = [b"MZ".as_slice(), b"\x00\x00NullsoftInst"].concat();
        let tmp = tempfile::tempdir().unwrap();

        for name in [
            "unins000.exe",
            "unins001.exe",
            "Uninstall.exe",
            "game_uninstall.exe",
            "vcredist_x64.exe",
            "VC_redist.x64.exe",
            "DXSETUP.exe",
            "dxwebsetup.exe",
        ] {
            assert_eq!(classify(&name.to_ascii_lowercase(), &inno), None, "{name}");
            assert_eq!(classify(&name.to_ascii_lowercase(), &nsis), None, "{name}");
            let path = tmp.path().join(name);
            std::fs::write(&path, &inno).unwrap();
            assert_eq!(detect(&path, name), None, "{name}");
        }

        // A real game installer next to them is still detected.
        let setup_path = tmp.path().join("setup_game.exe");
        std::fs::write(&setup_path, &inno).unwrap();
        assert_eq!(detect(&setup_path, "setup_game.exe"), Some(Setup::Inno));
    }

    #[test]
    fn silent_command_lines() {
        let (file, dir, log) = (
            Path::new("/g/.itch-download/7/setup.exe"),
            Path::new("/g/My Game"),
            Path::new("/g/.itch-download/7/setup.log"),
        );
        let inno = command(Setup::Inno, file, dir, log, false);
        assert_eq!(inno[0], "/g/.itch-download/7/setup.exe");
        assert!(inno.contains(&r"/DIR=Z:\g\My Game".to_string()));
        assert!(inno.contains(&"/VERYSILENT".to_string()));
        let nsis = command(Setup::Nsis, file, dir, log, false);
        assert_eq!(nsis.last().unwrap(), r"/D=Z:\g\My Game");
        let msi = command(Setup::Msi, Path::new("/g/x/game.msi"), dir, log, false);
        assert_eq!(&msi[..3], ["msiexec", "/i", r"Z:\g\x\game.msi"]);
        assert!(msi.contains(&r"TARGETDIR=Z:\g\My Game".to_string()));
        let native = command(
            Setup::Nsis,
            Path::new(r"C:\dl\setup.exe"),
            Path::new(r"C:\Games\My Game"),
            log,
            true,
        );
        assert_eq!(native.last().unwrap(), r"/D=C:\Games\My Game");
    }

    #[test]
    fn native_nsis_command_spec_uses_raw_arg_for_dir_with_spaces() {
        let spec = command_spec(
            Setup::Nsis,
            Path::new(r"C:\dl\setup.exe"),
            Path::new(r"C:\Games\My Cool Game"),
            Path::new(r"C:\dl\setup.log"),
            true,
        );
        assert_eq!(spec.program, Path::new(r"C:\dl\setup.exe"));
        assert_eq!(spec.args, vec![OsString::from("/S")]);
        assert_eq!(
            spec.raw_args,
            vec![OsString::from(r"/D=C:\Games\My Cool Game")]
        );

        // Under Wine, `/D=` stays in regular args (passed to Wine's argv).
        let wine_spec = command_spec(
            Setup::Nsis,
            Path::new("/dl/setup.exe"),
            Path::new("/Games/My Cool Game"),
            Path::new("/dl/setup.log"),
            false,
        );
        assert!(wine_spec.raw_args.is_empty());
        assert_eq!(
            wine_spec.args,
            vec![
                OsString::from("/S"),
                OsString::from(r"/D=Z:\Games\My Cool Game"),
            ]
        );
    }
}
