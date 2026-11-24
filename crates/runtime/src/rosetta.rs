//! Rosetta 2 on Apple silicon Macs: which programs need it, starting a
//! native Mac game's Intel code through `arch -x86_64`, and installing
//! Rosetta through Apple's own `softwareupdate` when the user asks.
//!
//! An Intel-only Mac program started from an arm64 process isn't translated
//! when it is spawned with a preferred architecture the binary lacks: it
//! hangs or fails to start, so games are started through `arch -x86_64`,
//! which execs the program in place (same process, so tracking and stopping
//! still work). Rosetta is never installed silently; installing it accepts
//! Apple's licence, which the UI says before the user agrees.
//!
//! The Mach-O header reader is pure, bounded and runs on every system; the
//! rest does nothing outside Apple silicon Macs.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::RosettaMissing;
use irori_core::process::{CommandSpec, RunOptions};

/// `arch(1)`, which ships with macOS.
const ARCH: &str = "/usr/bin/arch";
/// Apple's updater, which installs Rosetta on request.
const SOFTWAREUPDATE: &str = "/usr/sbin/softwareupdate";

/// A processor family a Mach-O file holds code for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachArch {
    X86_64,
    Arm64,
    Other,
}

const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
/// More slices than any real universal binary carries; a header claiming more
/// isn't one (Java class files share the fat magic).
const MAX_SLICES: u32 = 16;
/// Enough header for [`MAX_SLICES`] 64-bit fat entries.
const HEADER_BYTES: usize = 8 + 32 * MAX_SLICES as usize;

fn arch_of(cpu: u32) -> MachArch {
    match cpu {
        CPU_TYPE_X86_64 => MachArch::X86_64,
        CPU_TYPE_ARM64 => MachArch::Arm64,
        _ => MachArch::Other,
    }
}

/// The architectures of a Mach-O file from its first bytes: one for a thin
/// 64-bit file, each slice's for a universal one. `None` when the bytes
/// aren't a Mach-O program this reader knows (32-bit files never run on
/// current macOS).
pub fn mach_o_arches(header: &[u8]) -> Option<Vec<MachArch>> {
    let word = |at: usize, big: bool| -> Option<u32> {
        let b: [u8; 4] = header.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    match word(0, true)? {
        // MH_MAGIC_64 as stored by little-endian machines.
        0xcffa_edfe => Some(vec![arch_of(word(4, false)?)]),
        // FAT_MAGIC and FAT_MAGIC_64: big-endian headers.
        magic @ (0xcafe_babe | 0xcafe_babf) => {
            let count = word(4, true)?;
            if count == 0 || count > MAX_SLICES {
                return None;
            }
            let entry = if magic == 0xcafe_babe { 20 } else { 32 };
            (0..count as usize)
                .map(|i| word(8 + i * entry, true).map(arch_of))
                .collect()
        }
        _ => None,
    }
}

/// [`mach_o_arches`] of the file at `path`, reading only its header.
pub fn file_arches(path: &Path) -> Option<Vec<MachArch>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(HEADER_BYTES);
    file.by_ref()
        .take(HEADER_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    mach_o_arches(&buf)
}

/// Whether code with these architectures goes through Rosetta on Apple
/// silicon: it has Intel code, and either no Apple silicon code or the user
/// asked for the Intel code (`force`).
pub fn wants_rosetta(arches: &[MachArch], force: bool) -> bool {
    arches.contains(&MachArch::X86_64) && (force || !arches.contains(&MachArch::Arm64))
}

/// Where in a launch command the game's own Mac program is: `0` for the
/// command's program, `n` for its `n`-th argument, with the program's path
/// (an app bundle resolved to its executable). Wrappers (`env`, a user's
/// script) come before the game and are Mac programs too, so the first one
/// inside the game's folder `folder` wins, else the first one at all.
fn game_program(command: &CommandSpec, folder: Option<&Path>) -> Option<(usize, PathBuf)> {
    let programs: Vec<(usize, &Path, PathBuf)> = std::iter::once(command.program.as_os_str())
        .chain(command.args.iter().map(|a| a.as_os_str()))
        .enumerate()
        .filter_map(|(i, part)| {
            let path = Path::new(part);
            if !path.is_absolute() {
                return None;
            }
            let program = if path.extension().is_some_and(|e| e == "app") && path.is_dir() {
                irori_core::platform::bundle_executable(path).ok()?
            } else if path.is_file() {
                path.to_path_buf()
            } else {
                return None;
            };
            file_arches(&program).map(|_| (i, path, program))
        })
        .collect();
    let inside = folder.and_then(|folder| programs.iter().find(|(_, p, _)| p.starts_with(folder)));
    inside
        .or(programs.first())
        .map(|(i, _, program)| (*i, program.clone()))
}

/// `command` with the game's program started through `arch -x86_64` when it
/// needs Rosetta (or `force` asks for its Intel code), else unchanged.
/// Pure apart from reading the program's header; see [`for_launch`].
pub fn wrap(command: CommandSpec, folder: Option<&Path>, force: bool) -> (CommandSpec, bool) {
    let Some((at, program)) = game_program(&command, folder) else {
        return (command, false);
    };
    if !file_arches(&program).is_some_and(|a| wants_rosetta(&a, force)) {
        return (command, false);
    }
    let mut parts: Vec<std::ffi::OsString> = std::iter::once(command.program.clone().into())
        .chain(command.args.iter().cloned())
        .collect();
    // An app bundle only resolves to its executable as the command's
    // program; once `arch` comes first it is an argument.
    parts[at] = program.into_os_string();
    parts.splice(at..at, [ARCH.into(), "-x86_64".into()]);
    let program = PathBuf::from(parts.remove(0));
    (
        CommandSpec {
            program,
            args: parts,
            ..command
        },
        true,
    )
}

/// Whether Rosetta can run Intel code here: it can always on Intel Macs, and
/// on Apple silicon only once installed.
pub async fn installed() -> bool {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return true;
    }
    irori_core::process::run(
        &CommandSpec::new(ARCH).args(["-x86_64", "/usr/bin/true"]),
        RunOptions::default(),
    )
    .await
    .is_ok_and(|r| r.success())
}

/// Fail with [`RosettaMissing`] naming `what` unless Rosetta can run here.
pub async fn ensure(what: &str) -> Result<()> {
    if installed().await {
        return Ok(());
    }
    Err(RosettaMissing {
        what: what.to_string(),
    }
    .into())
}

/// A native Mac game's launch command as it must run on this Mac: through
/// `arch -x86_64` when its program is Intel-only (or `force` and it has
/// Intel code), failing with [`RosettaMissing`] when that is needed and
/// Rosetta isn't there. `folder` is the game's install folder. Unchanged
/// anywhere but Apple silicon Macs.
pub async fn for_launch(
    command: CommandSpec,
    folder: Option<PathBuf>,
    force: bool,
    title: &str,
) -> Result<CommandSpec> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Ok(command);
    }
    let (command, wrapped) =
        tokio::task::spawn_blocking(move || wrap(command, folder.as_deref(), force))
            .await
            .context("Could not read the game's program")?;
    if wrapped {
        ensure(title).await?;
    }
    Ok(command)
}

/// Install Rosetta 2 with Apple's `softwareupdate`, accepting Apple's
/// licence for it: only after the user agreed to that in Irori.
pub async fn install() -> Result<()> {
    anyhow::ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "Rosetta is only for Apple silicon Macs"
    );
    let result = irori_core::process::run(
        &CommandSpec::new(SOFTWAREUPDATE).args(["--install-rosetta", "--agree-to-license"]),
        RunOptions::default(),
    )
    .await
    .context("Could not start softwareupdate")?;
    anyhow::ensure!(
        result.success() && installed().await,
        "softwareupdate couldn't install Rosetta: {}",
        result
            .stderr
            .lines()
            .chain(result.stdout.lines())
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no reason given")
            .trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin(cpu: u32) -> Vec<u8> {
        let mut b = vec![0xcf, 0xfa, 0xed, 0xfe];
        b.extend(cpu.to_le_bytes());
        b.extend([0; 24]);
        b
    }

    fn fat(cpus: &[u32]) -> Vec<u8> {
        let mut b = 0xcafe_babe_u32.to_be_bytes().to_vec();
        b.extend((cpus.len() as u32).to_be_bytes());
        for cpu in cpus {
            b.extend(cpu.to_be_bytes());
            b.extend([0; 16]);
        }
        b
    }

    #[test]
    fn reads_thin_and_universal_headers() {
        assert_eq!(
            mach_o_arches(&thin(CPU_TYPE_X86_64)),
            Some(vec![MachArch::X86_64])
        );
        assert_eq!(
            mach_o_arches(&thin(CPU_TYPE_ARM64)),
            Some(vec![MachArch::Arm64])
        );
        assert_eq!(
            mach_o_arches(&fat(&[CPU_TYPE_X86_64, CPU_TYPE_ARM64])),
            Some(vec![MachArch::X86_64, MachArch::Arm64])
        );
        let mut fat64 = 0xcafe_babf_u32.to_be_bytes().to_vec();
        fat64.extend(1u32.to_be_bytes());
        fat64.extend(CPU_TYPE_X86_64.to_be_bytes());
        fat64.extend([0; 28]);
        assert_eq!(mach_o_arches(&fat64), Some(vec![MachArch::X86_64]));
    }

    #[test]
    fn refuses_what_isnt_a_mac_program() {
        assert_eq!(mach_o_arches(b"#!/bin/sh\necho hi\n"), None);
        assert_eq!(mach_o_arches(b"\x7fELF\x02\x01\x01"), None);
        assert_eq!(mach_o_arches(&[0xcf, 0xfa]), None);
        // A Java class file: the fat magic with a version where the count is.
        let mut class = 0xcafe_babe_u32.to_be_bytes().to_vec();
        class.extend([0, 0, 0, 52]);
        assert_eq!(mach_o_arches(&class), None);
        // A universal header cut short.
        let mut cut = fat(&[CPU_TYPE_X86_64, CPU_TYPE_ARM64]);
        cut.truncate(20);
        assert_eq!(mach_o_arches(&cut), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_this_macs_own_programs() {
        // macOS's own programs are universal: Apple silicon and Intel code.
        let arches = file_arches(Path::new("/usr/bin/true")).unwrap();
        assert!(arches.contains(&MachArch::Arm64), "{arches:?}");
        assert!(!wants_rosetta(&arches, false));
    }

    #[test]
    fn rosetta_for_intel_only_code_or_when_asked() {
        use MachArch::*;
        assert!(wants_rosetta(&[X86_64], false));
        assert!(!wants_rosetta(&[Arm64], false));
        assert!(!wants_rosetta(&[Arm64], true));
        assert!(!wants_rosetta(&[X86_64, Arm64], false));
        assert!(wants_rosetta(&[X86_64, Arm64], true));
    }

    #[test]
    fn wraps_the_game_program_even_behind_a_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let intel = dir.path().join("Game");
        std::fs::write(&intel, thin(CPU_TYPE_X86_64)).unwrap();
        let universal = dir.path().join("Universal");
        std::fs::write(&universal, fat(&[CPU_TYPE_X86_64, CPU_TYPE_ARM64])).unwrap();

        let (cmd, wrapped) = wrap(CommandSpec::new(&intel).arg("-windowed"), None, false);
        assert!(wrapped);
        assert_eq!(cmd.program, PathBuf::from(ARCH));
        assert_eq!(
            cmd.args,
            [
                "-x86_64".into(),
                intel.clone().into_os_string(),
                "-windowed".into()
            ]
        );

        let (cmd, wrapped) = wrap(
            CommandSpec::new("/usr/bin/env")
                .arg("FOO=1")
                .arg(&intel)
                .env("K", "V"),
            Some(dir.path()),
            false,
        );
        assert!(wrapped);
        assert_eq!(cmd.program, PathBuf::from("/usr/bin/env"));
        assert_eq!(cmd.args[1], std::ffi::OsString::from(ARCH));
        assert_eq!(cmd.env["K"], "V");

        let (cmd, wrapped) = wrap(CommandSpec::new(&universal), None, false);
        assert!(!wrapped);
        assert_eq!(cmd.program, universal);
        let (_, wrapped) = wrap(CommandSpec::new(&universal), None, true);
        assert!(wrapped);
    }

    #[test]
    fn resolves_an_app_bundle_before_wrapping_it() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Game.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let exe = bundle.join("Contents/MacOS/Game");
        std::fs::write(&exe, thin(CPU_TYPE_X86_64)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut info = plist::Dictionary::new();
        info.insert(
            "CFBundleExecutable".into(),
            plist::Value::String("Game".into()),
        );
        plist::Value::Dictionary(info)
            .to_file_xml(bundle.join("Contents/Info.plist"))
            .unwrap();
        let (cmd, wrapped) = wrap(CommandSpec::new(&bundle), Some(dir.path()), false);
        if cfg!(unix) {
            assert!(wrapped);
            assert_eq!(cmd.program, PathBuf::from(ARCH));
            assert_eq!(cmd.args[1], exe.into_os_string());
        }
    }

    #[test]
    fn leaves_commands_without_a_mac_program_alone() {
        let (cmd, wrapped) = wrap(CommandSpec::new("steam").arg("-applaunch"), None, true);
        assert!(!wrapped);
        assert_eq!(cmd.program, PathBuf::from("steam"));
    }
}
