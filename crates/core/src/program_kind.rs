//! What kind of program a game starts, read from its first bytes: a Windows
//! program needs Wine away from Windows, a program of this system doesn't.
//!
//! A store sells a game's Windows build, but the program set for it may be
//! a native replacement (OpenJK for Jedi Academy, fallout2-ce): that one
//! runs without Wine. Only the file's own header decides; an unknown file
//! keeps going through Wine unless the game's settings say otherwise
//! (`iroriNoWine`).

use std::io::Read;
use std::path::Path;

use crate::HostPlatform;

/// A program's kind, as far as its first bytes tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// A Windows program (`MZ`) or batch file.
    Windows,
    /// A program this system runs itself: ELF on Linux, Mach-O or an app
    /// bundle on macOS, a script with a `#!` line on either.
    Native,
    /// Anything else, or a file that can't be read.
    Unknown,
}

/// The kind of a program whose file starts with `head` (at least four
/// bytes when the file has them), named `name`, on `host`.
pub fn classify(head: &[u8], name: &str, host: HostPlatform) -> ProgramKind {
    let lower = name.to_ascii_lowercase();
    if head.starts_with(b"MZ")
        || [".bat", ".cmd", ".lnk", ".msi"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    {
        return ProgramKind::Windows;
    }
    let native = match host {
        HostPlatform::Linux => head.starts_with(b"\x7fELF"),
        HostPlatform::Mac => {
            const MACH_O: [[u8; 4]; 5] = [
                [0xfe, 0xed, 0xfa, 0xce],
                [0xfe, 0xed, 0xfa, 0xcf],
                [0xce, 0xfa, 0xed, 0xfe],
                [0xcf, 0xfa, 0xed, 0xfe],
                [0xca, 0xfe, 0xba, 0xbe],
            ];
            MACH_O.iter().any(|m| head.starts_with(m))
        }
        HostPlatform::Windows => false,
    };
    let script = host != HostPlatform::Windows && head.starts_with(b"#!");
    if native || script {
        ProgramKind::Native
    } else {
        ProgramKind::Unknown
    }
}

/// [`classify`] for the file at `path` on this system: an app bundle on
/// macOS is native. Reads at most four bytes; blocking.
pub fn of_file(path: &Path) -> ProgramKind {
    let host = HostPlatform::current();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_dir() {
        let bundle = host == HostPlatform::Mac && name.to_ascii_lowercase().ends_with(".app");
        return if bundle {
            ProgramKind::Native
        } else {
            ProgramKind::Unknown
        };
    }
    let mut head = [0u8; 4];
    let read = std::fs::File::open(path)
        .and_then(|mut f| {
            let mut n = 0;
            while n < head.len() {
                match f.read(&mut head[n..])? {
                    0 => break,
                    got => n += got,
                }
            }
            Ok(n)
        })
        .unwrap_or(0);
    if read == 0 && !path.is_file() {
        return ProgramKind::Unknown;
    }
    classify(&head[..read], &name, host)
}

/// Whether a program named `name` is an AppImage, which needs FUSE to mount
/// itself unless told to unpack and run (`APPIMAGE_EXTRACT_AND_RUN`).
pub fn is_appimage(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".appimage")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_windows_programs_from_native_ones() {
        use HostPlatform::{Linux, Mac, Windows};
        let elf = b"\x7fELF\x02";
        let pe = b"MZ\x90\x00";
        let mach = [0xcf, 0xfa, 0xed, 0xfe];
        assert_eq!(classify(pe, "game.exe", Linux), ProgramKind::Windows);
        assert_eq!(classify(b"@ech", "start.bat", Linux), ProgramKind::Windows);
        assert_eq!(classify(elf, "openjk.x86_64", Linux), ProgramKind::Native);
        assert_eq!(classify(elf, "Game.AppImage", Linux), ProgramKind::Native);
        assert_eq!(
            classify(b"#!/bin/sh", "start.sh", Linux),
            ProgramKind::Native
        );
        assert_eq!(classify(&mach, "fallout2-ce", Mac), ProgramKind::Native);
        // Another system's program is no use here.
        assert_eq!(classify(elf, "openjk", Mac), ProgramKind::Unknown);
        assert_eq!(classify(&mach, "game", Linux), ProgramKind::Unknown);
        assert_eq!(
            classify(b"#!/bin/sh", "x.sh", Windows),
            ProgramKind::Unknown
        );
        assert_eq!(classify(b"", "nothing", Linux), ProgramKind::Unknown);
        assert_eq!(
            classify(b"PK\x03\x04", "game.jar", Linux),
            ProgramKind::Unknown
        );
    }

    #[test]
    fn reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let pe = dir.path().join("Game.exe");
        std::fs::write(&pe, b"MZ rest of header").unwrap();
        assert_eq!(of_file(&pe), ProgramKind::Windows);
        let script = dir.path().join("run");
        std::fs::write(&script, b"#!/bin/sh\nexec ./game\n").unwrap();
        let expected = if cfg!(windows) {
            ProgramKind::Unknown
        } else {
            ProgramKind::Native
        };
        assert_eq!(of_file(&script), expected);
        assert_eq!(of_file(&dir.path().join("missing")), ProgramKind::Unknown);
        assert!(is_appimage("OpenJK-x86_64.AppImage"));
        assert!(!is_appimage("openjk.x86_64"));
    }
}
