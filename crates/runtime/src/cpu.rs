//! The CPU a program is built for, and whether this computer can run it.
//!
//! Nearly every store game, and every Wine build made for x86-64, is an x86
//! program. On Arm64 Linux those run only through an x86 emulator the kernel
//! hands them to (binfmt_misc: FEX, Box64 or qemu-user register themselves
//! there), which then also covers the programs a game's scripts and launchers
//! start. Arm64 Proton builds (GE-Proton, Proton-CachyOS) carry FEX inside
//! Wine for the Windows programs they run, so they need nothing more.
//!
//! Irori never guesses: a program it can't read, or a system whose
//! binfmt_misc it can't see (inside a Flatpak, say), counts as runnable.

use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use irori_core::X86EmulatorMissing;
use irori_core::process::CommandSpec;
use parking_lot::Mutex;

/// ELF machine numbers (`e_machine`).
pub const EM_386: u16 = 3;
pub const EM_X86_64: u16 = 62;
pub const EM_AARCH64: u16 = 183;

const BINFMT_DIR: &str = "/proc/sys/fs/binfmt_misc";
/// How long an answer about the registered emulators holds, so installing
/// FEX while Irori runs is noticed at the next launch.
const SUPPORT_TTL: Duration = Duration::from_secs(5);

/// Programs that run x86 programs themselves when a user puts them in front
/// of a game as a wrapper; a command with one needs no registered emulator.
const EMULATORS: [&str; 6] = [
    "FEXInterpreter",
    "FEXLoader",
    "FEX",
    "box64",
    "box86",
    "qemu-x86_64",
];

/// How this computer runs x86 programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X86Support {
    /// It is an x86 computer.
    Native,
    /// An emulator is registered with the kernel for x86-64 programs.
    Emulated,
    /// The registrations are readable and none takes x86-64 programs.
    Missing,
    /// The registrations can't be read here.
    Unknown,
}

/// Whether this computer is Arm64 Linux, the one host where x86 programs
/// need an emulator Irori doesn't manage. (Windows on Arm and macOS with
/// Rosetta run them themselves.)
pub fn is_arm64_linux() -> bool {
    cfg!(all(target_os = "linux", target_arch = "aarch64"))
}

/// The machine a little- or big-endian ELF file is built for; `None` for
/// anything that isn't ELF (a script, a Windows program) or can't be read.
pub fn elf_machine(path: &Path) -> Option<u16> {
    let mut header = [0u8; 20];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .ok()?;
    if header[..4] != *b"\x7fELF" {
        return None;
    }
    let bytes = [header[18], header[19]];
    match header[5] {
        1 => Some(u16::from_le_bytes(bytes)),
        2 => Some(u16::from_be_bytes(bytes)),
        _ => None,
    }
}

/// Whether `machine` is 32- or 64-bit x86.
pub fn is_x86(machine: u16) -> bool {
    machine == EM_386 || machine == EM_X86_64
}

/// Whether a program for `machine` runs with `support`.
pub fn can_run(machine: u16, support: X86Support) -> bool {
    !is_x86(machine) || support != X86Support::Missing
}

/// Whether the program at `path` runs on this computer; a file that isn't
/// an ELF program counts as runnable.
pub fn runs_here(path: &Path) -> bool {
    if !is_arm64_linux() {
        return true;
    }
    elf_machine(path).is_none_or(|machine| can_run(machine, x86_support()))
}

static SUPPORT: Mutex<Option<(Instant, X86Support)>> = Mutex::new(None);

/// How this computer runs x86 programs, read from binfmt_misc on Arm64
/// Linux and kept for a few seconds.
pub fn x86_support() -> X86Support {
    if !is_arm64_linux() {
        return X86Support::Native;
    }
    if let Some((at, support)) = *SUPPORT.lock()
        && at.elapsed() < SUPPORT_TTL
    {
        return support;
    }
    let support = read_support(Path::new(BINFMT_DIR));
    *SUPPORT.lock() = Some((Instant::now(), support));
    support
}

/// The registrations in a binfmt_misc folder. Without its `status` file the
/// folder isn't mounted (as in a Flatpak's own /proc), so nothing is known.
fn read_support(dir: &Path) -> X86Support {
    if !dir.join("status").is_file() {
        return X86Support::Unknown;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return X86Support::Unknown;
    };
    let found = entries.flatten().any(|entry| {
        let name = entry.file_name();
        name != "status"
            && name != "register"
            && std::fs::read_to_string(entry.path())
                .is_ok_and(|text| X86_64_HEADERS.iter().any(|h| binfmt_matches(&text, h)))
    });
    if found {
        X86Support::Emulated
    } else {
        X86Support::Missing
    }
}

/// The first 20 bytes of an x86-64 Linux program and of a position-independent
/// one (`ET_EXEC`, `ET_DYN`), which registrations are matched against.
const X86_64_HEADERS: [[u8; 20]; 2] = [
    [
        0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0x3e, 0,
    ],
    [
        0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0x3e, 0,
    ],
];

/// Whether an enabled binfmt_misc registration (its file's text) takes a
/// program starting with `header`: `magic` at `offset`, compared under
/// `mask`. Registrations by file extension never match.
fn binfmt_matches(entry: &str, header: &[u8]) -> bool {
    let mut lines = entry.lines();
    if lines.next().map(str::trim) != Some("enabled") {
        return false;
    }
    let (mut offset, mut magic, mut mask) = (0usize, None, None);
    for line in lines {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "offset" => offset = value.trim().parse().unwrap_or(0),
            "magic" => magic = decode_hex(value.trim()),
            "mask" => mask = decode_hex(value.trim()),
            _ => {}
        }
    }
    let Some(magic) = magic.filter(|m| !m.is_empty()) else {
        return false;
    };
    let mask = mask.unwrap_or_else(|| vec![0xff; magic.len()]);
    magic.iter().enumerate().all(|(i, &want)| {
        let bits = mask.get(i).copied().unwrap_or(0xff);
        header
            .get(offset + i)
            .is_some_and(|&have| have & bits == want & bits)
    })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The first x86 program a command starts directly: its program or an
/// argument that is a path to one. Nothing when one of them is an emulator
/// itself (a FEX or Box64 wrapper the user added).
fn first_x86_program(command: &CommandSpec) -> Option<PathBuf> {
    let parts = std::iter::once(command.program.as_os_str())
        .chain(command.args.iter().map(|a| a.as_os_str()));
    let mut found = None;
    for part in parts {
        let path = Path::new(part);
        if path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| EMULATORS.contains(&name))
        {
            return None;
        }
        if found.is_none() && path.is_absolute() && elf_machine(path).is_some_and(is_x86) {
            found = Some(path.to_path_buf());
        }
    }
    found
}

/// Refuse a command that starts an x86 program this computer can't run.
/// Only Arm64 Linux without a registered x86 emulator ever refuses.
pub fn check_command(command: &CommandSpec) -> Result<(), X86EmulatorMissing> {
    if !is_arm64_linux() || x86_support() != X86Support::Missing {
        return Ok(());
    }
    match first_x86_program(command) {
        Some(program) => Err(X86EmulatorMissing { program }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FEX's own registration, as `FEXInterpreter` writes it.
    const FEX: &str = "enabled\ninterpreter /usr/bin/FEXInterpreter\nflags: POCF\noffset 0\nmagic 7f454c4602010100000000000000000002003e00\nmask fffffffffffefe00fffffffffffffffffeffffff\n";
    /// Box64's, from its systemd-binfmt file.
    const BOX64: &str = "enabled\ninterpreter /usr/local/bin/box64\nflags: \noffset 0\nmagic 7f454c4602010100000000000000000002003e00\nmask fffffffffffefe00fffffffffffffffffeffffff\n";
    const I386_ONLY: &str = "enabled\ninterpreter /usr/bin/qemu-i386\nflags: F\noffset 0\nmagic 7f454c4601010100000000000000000002000300\nmask fffffffffffefe00fffffffffffffffffeffffff\n";

    fn write_elf(path: &Path, class: u8, endian: u8, machine: u16) {
        let mut header = vec![0x7f, b'E', b'L', b'F', class, endian, 1, 0];
        header.resize(18, 0);
        header[16] = 2;
        header.extend(if endian == 2 {
            machine.to_be_bytes()
        } else {
            machine.to_le_bytes()
        });
        header.resize(64, 0);
        std::fs::write(path, header).unwrap();
    }

    #[test]
    fn reads_the_machine_of_programs() {
        let dir = tempfile::tempdir().unwrap();
        let (x64, x86, arm, be, script) = (
            dir.path().join("game.x86_64"),
            dir.path().join("game.x86"),
            dir.path().join("game.arm64"),
            dir.path().join("big"),
            dir.path().join("start.sh"),
        );
        write_elf(&x64, 2, 1, EM_X86_64);
        write_elf(&x86, 1, 1, EM_386);
        write_elf(&arm, 2, 1, EM_AARCH64);
        write_elf(&be, 2, 2, EM_AARCH64);
        std::fs::write(&script, "#!/bin/sh\nexec ./game.x86_64\n").unwrap();
        assert_eq!(elf_machine(&x64), Some(EM_X86_64));
        assert_eq!(elf_machine(&x86), Some(EM_386));
        assert_eq!(elf_machine(&arm), Some(EM_AARCH64));
        assert_eq!(elf_machine(&be), Some(EM_AARCH64));
        assert_eq!(elf_machine(&script), None);
        assert_eq!(elf_machine(&dir.path().join("absent")), None);
    }

    #[test]
    fn x86_programs_need_an_emulator_only_where_none_is_registered() {
        assert!(!can_run(EM_X86_64, X86Support::Missing));
        assert!(!can_run(EM_386, X86Support::Missing));
        assert!(can_run(EM_AARCH64, X86Support::Missing));
        assert!(can_run(EM_X86_64, X86Support::Emulated));
        // Not knowing is never a reason to refuse a game.
        assert!(can_run(EM_X86_64, X86Support::Unknown));
        assert!(can_run(EM_X86_64, X86Support::Native));
    }

    #[test]
    fn fex_and_box64_registrations_take_x86_64_programs() {
        for entry in [FEX, BOX64] {
            assert!(
                X86_64_HEADERS.iter().all(|h| binfmt_matches(entry, h)),
                "{entry}"
            );
        }
        assert!(!X86_64_HEADERS.iter().any(|h| binfmt_matches(I386_ONLY, h)));
        let disabled = FEX.replacen("enabled", "disabled", 1);
        assert!(!binfmt_matches(&disabled, &X86_64_HEADERS[0]));
        let by_extension = "enabled\ninterpreter /usr/bin/wine\nflags: \nextension .exe\n";
        assert!(!binfmt_matches(by_extension, &X86_64_HEADERS[0]));
    }

    #[test]
    fn support_comes_from_the_mounted_registrations() {
        let dir = tempfile::tempdir().unwrap();
        // Not mounted: no status file.
        assert_eq!(read_support(dir.path()), X86Support::Unknown);
        std::fs::write(dir.path().join("status"), "enabled\n").unwrap();
        std::fs::write(dir.path().join("register"), "").unwrap();
        std::fs::write(dir.path().join("qemu-i386"), I386_ONLY).unwrap();
        assert_eq!(read_support(dir.path()), X86Support::Missing);
        std::fs::write(dir.path().join("FEX-x86_64"), FEX).unwrap();
        assert_eq!(read_support(dir.path()), X86Support::Emulated);
    }

    #[test]
    fn finds_the_x86_program_a_command_starts() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.x86_64");
        let native = dir.path().join("gamemoderun");
        write_elf(&game, 2, 1, EM_X86_64);
        write_elf(&native, 2, 1, EM_AARCH64);
        let command =
            CommandSpec::new(&native).args([game.as_os_str(), OsStr::new("--fullscreen")]);
        assert_eq!(first_x86_program(&command), Some(game.clone()));

        let script = dir.path().join("start.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        assert_eq!(first_x86_program(&CommandSpec::new(&script)), None);

        // A user's own FEX wrapper runs it already.
        let wrapped = CommandSpec::new("/usr/bin/FEXInterpreter").args([game.as_os_str()]);
        assert_eq!(first_x86_program(&wrapped), None);
    }
}
