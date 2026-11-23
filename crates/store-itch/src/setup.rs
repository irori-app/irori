//! Windows setup programs uploaded to itch.io: Inno Setup and NSIS
//! installers and MSI packages. Copying one in as "the game" leaves a
//! setup program to start; instead it runs silently into the install
//! folder, through Wine (or directly on Windows), and the program it
//! installed is found afterwards. Detection reads a bounded part of the
//! file; everything here is pure apart from that read.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

/// A setup program Irori knows how to run silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setup {
    Inno,
    Nsis,
    Msi,
}

/// How much of a file is searched for an installer's marks: Inno Setup and
/// NSIS write theirs near the start of the program.
const SNIFF_BYTES: u64 = 8 << 20;

/// The setup program `path` (named `name` on itch.io) is, if any.
pub(crate) fn detect(path: &Path, name: &str) -> Option<Setup> {
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
pub(crate) fn windows_path(path: &Path, native: bool) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    if native { text } else { format!("Z:{text}") }
}

/// The command line that installs `setup` (at `file`) silently into `dir`,
/// writing its log to `log`. `native` says Windows runs it, not Wine.
pub(crate) fn command(
    setup: Setup,
    file: &Path,
    dir: &Path,
    log: &Path,
    native: bool,
) -> Vec<String> {
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

#[cfg(test)]
mod tests {
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
        assert_eq!(classify("game.exe", b"MZ just a game"), None);
        assert_eq!(classify("readme.txt", b"Inno Setup"), None);
        assert_eq!(
            classify("game.msi", &[0xD0, 0xCF, 0x11, 0xE0, 0xA1]),
            Some(Setup::Msi)
        );
        assert_eq!(classify("game.msi", b"not ole"), None);
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
}
