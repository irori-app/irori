//! The hash RetroAchievements identifies a game file by: an MD5 over what
//! rcheevos (the library its emulators share) reads for the game's system.
//! A file whose system has no [`Method`] here, or a compressed image Irori
//! can't read (CHD, CSO, PBP), has no hash and so no achievements.
//!
//! Hashing is blocking file I/O: callers run it off the async runtime.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use md5::{Digest as _, Md5};

use crate::disc::{Track, extension};

/// Largest cartridge read whole; anything bigger is not a cartridge.
const MAX_CART: u64 = 64 * 1024 * 1024;
/// Largest Nintendo DS code read (rcheevos' own limit).
const MAX_NDS_CODE: u64 = 16 * 1024 * 1024;
/// Bytes of a Nintendo DS icon.
const NDS_ICON: usize = 0xA00;

/// How a system's games are hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The whole file.
    Whole,
    /// Without the 16-byte iNES or FDS header.
    Nes,
    /// Without a 512-byte copier header.
    Snes,
    /// In big-endian (`.z64`) byte order.
    N64,
    /// Without the 64-byte `LYNX` header.
    Lynx,
    /// Without the 128-byte `ATARI7800` header.
    Atari7800,
    /// Without a 512-byte header.
    PcEngine,
    /// The header, ARM9 and ARM7 code and the icon.
    NintendoDs,
    /// The boot program's name and contents (from `SYSTEM.CNF`).
    PlayStation,
    /// The boot program's name and contents (`BOOT2` in `SYSTEM.CNF`).
    PlayStation2,
    /// `PARAM.SFO` and `EBOOT.BIN`.
    Psp,
    /// The first 512 bytes of the data track.
    SegaCd,
    /// The file name without its extension (MAME's set name).
    Arcade,
}

/// The RetroAchievements hash (lower-case hex MD5) of a game file.
pub fn hash_file(method: Method, path: &Path) -> Result<String> {
    let digest = match method {
        Method::Whole => whole(path, 0)?,
        Method::Nes => {
            let head = head::<4>(path)?;
            let skip = if head == *b"NES\x1a" || head == *b"FDS\x1a" {
                16
            } else {
                0
            };
            whole(path, skip)?
        }
        Method::Snes => {
            let skip = if size(path)? % 8192 == 512 { 512 } else { 0 };
            whole(path, skip)?
        }
        Method::PcEngine => {
            let skip = if size(path)? % 1024 == 512 { 512 } else { 0 };
            whole(path, skip)?
        }
        Method::Lynx => {
            let skip = if head::<5>(path)? == *b"LYNX\0" {
                64
            } else {
                0
            };
            whole(path, skip)?
        }
        Method::Atari7800 => {
            let head = head::<10>(path)?;
            let skip = if &head[1..10] == b"ATARI7800" { 128 } else { 0 };
            whole(path, skip)?
        }
        Method::N64 => n64(path)?,
        Method::NintendoDs => nds(path)?,
        Method::PlayStation => playstation(path, "BOOT", "cdrom:", Some("PSX.EXE"))?,
        Method::PlayStation2 => playstation(path, "BOOT2", "cdrom0:", None)?,
        Method::Psp => psp(path)?,
        Method::SegaCd => sega_cd(path)?,
        Method::Arcade => {
            let stem = path
                .file_stem()
                .context("the game file has no name")?
                .to_string_lossy()
                .to_string();
            let mut h = Md5::new();
            h.update(stem.as_bytes());
            h.finalize().to_vec()
        }
    };
    Ok(hex(&digest))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn open(path: &Path) -> Result<File> {
    File::open(path).with_context(|| format!("could not open {}", path.display()))
}

fn size(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)
        .with_context(|| format!("could not read {}", path.display()))?
        .len())
}

fn head<const N: usize>(path: &Path) -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    let mut f = open(path)?;
    let mut got = 0;
    while got < N {
        let n = f.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(buf)
}

/// MD5 of a cartridge from byte `skip` on.
fn whole(path: &Path, skip: u64) -> Result<Vec<u8>> {
    let len = size(path)?;
    if len > MAX_CART {
        bail!("{} is too large for a cartridge", path.display());
    }
    let mut f = BufReader::new(open(path)?);
    f.seek(SeekFrom::Start(skip))?;
    let mut h = Md5::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_vec())
}

/// N64 images come in three byte orders; rcheevos hashes the big-endian one.
fn n64(path: &Path) -> Result<Vec<u8>> {
    if size(path)? > MAX_CART {
        bail!("{} is too large for a cartridge", path.display());
    }
    let mut data =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    match data.first() {
        // .v64: 16-bit words swapped.
        Some(0x37) => data
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .for_each(|w| w.swap(0, 1)),
        // .n64: 32-bit words little-endian.
        Some(0x40) => data
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|w| w.reverse()),
        _ => {}
    }
    Ok(Md5::digest(&data).to_vec())
}

fn nds(path: &Path) -> Result<Vec<u8>> {
    let mut f = open(path)?;
    let mut header = [0u8; 0x200];
    f.read_exact(&mut header)
        .context("the file is too short for a Nintendo DS game")?;
    let mut base = 0u64;
    // SuperCard images carry a 512-byte loader before the game.
    if header[..4] == [0x2e, 0, 0, 0xea] && header[0xb0..0xb4] == [0x44, 0x46, 0x96, 0] {
        base = 0x200;
        f.seek(SeekFrom::Start(base))?;
        f.read_exact(&mut header)?;
    }
    let word =
        |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes")) as u64;
    let (arm9_at, arm9_len) = (word(0x20), word(0x2c));
    let (arm7_at, arm7_len) = (word(0x30), word(0x3c));
    let icon_at = word(0x68);
    if arm9_len + arm7_len > MAX_NDS_CODE {
        bail!("the Nintendo DS game's code is larger than expected");
    }
    let mut h = Md5::new();
    h.update(&header[..0x160]);
    let mut part = |at: u64, len: usize| -> Result<()> {
        let mut buf = vec![0u8; len];
        f.seek(SeekFrom::Start(base + at))?;
        f.read_exact(&mut buf)
            .context("the Nintendo DS game is cut off")?;
        h.update(&buf);
        Ok(())
    };
    part(arm9_at, arm9_len as usize)?;
    part(arm7_at, arm7_len as usize)?;
    part(icon_at, NDS_ICON)?;
    Ok(h.finalize().to_vec())
}

/// The boot program named on `key`'s line of `SYSTEM.CNF`, without the
/// device prefix and version suffix (`SLUS_123.45`).
pub(crate) fn boot_program(cnf: &str, key: &str, device: &str) -> Option<String> {
    cnf.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(key)?;
        // "BOOT" must not match "BOOT2".
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        let rest =
            if rest.len() >= device.len() && rest[..device.len()].eq_ignore_ascii_case(device) {
                &rest[device.len()..]
            } else {
                rest
            };
        let rest = rest.trim_start_matches(['\\', '/']);
        let name: String = rest
            .chars()
            .take_while(|c| *c != ';' && !c.is_whitespace())
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

fn playstation(path: &Path, key: &str, device: &str, fallback: Option<&str>) -> Result<Vec<u8>> {
    let mut track = open_disc(path)?;
    let program = match track.find("SYSTEM.CNF")? {
        Some((lba, len)) => {
            let cnf = track.read(lba, (len as usize).min(2048))?;
            boot_program(&String::from_utf8_lossy(&cnf), key, device)
                .context("the disc's SYSTEM.CNF names no program")?
        }
        None => fallback.context("the disc has no SYSTEM.CNF")?.to_string(),
    };
    let (lba, mut len) = track
        .find(&program)?
        .with_context(|| format!("the disc has no {program}"))?;
    if key == "BOOT" {
        // A PlayStation program's header gives its real size.
        let first = track.sector(lba)?;
        if &first[..8] == b"PS-X EXE" {
            len = u32::from_le_bytes(first[28..32].try_into().expect("4 bytes")) + 2048;
        }
    }
    let mut h = Md5::new();
    h.update(program.as_bytes());
    h.update(track.read(lba, len as usize)?);
    Ok(h.finalize().to_vec())
}

fn psp(path: &Path) -> Result<Vec<u8>> {
    let mut track = open_disc(path)?;
    let mut h = Md5::new();
    for file in ["PSP_GAME\\PARAM.SFO", "PSP_GAME\\SYSDIR\\EBOOT.BIN"] {
        let (lba, len) = track
            .find(file)?
            .with_context(|| format!("the disc has no {file}"))?;
        h.update(track.read(lba, len as usize)?);
    }
    Ok(h.finalize().to_vec())
}

fn sega_cd(path: &Path) -> Result<Vec<u8>> {
    let mut track = open_disc(path)?;
    let first = track.sector(0)?;
    if &first[..16] != b"SEGADISCSYSTEM  " && &first[..16] != b"SEGA SEGASATURN " {
        bail!("the disc is not a Sega CD game");
    }
    Ok(Md5::digest(&first[..512]).to_vec())
}

fn open_disc(path: &Path) -> Result<Track> {
    match extension(path).as_str() {
        "chd" | "cso" | "pbp" | "ecm" => {
            bail!("Irori can't read compressed disc images for RetroAchievements")
        }
        _ => Track::open(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disc::tests::{iso, raw_mode2};

    fn md5_hex(data: &[u8]) -> String {
        hex(&Md5::digest(data))
    }

    fn write(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn strips_cartridge_headers() {
        let dir = tempfile::tempdir().unwrap();
        let rom = vec![7u8; 4096];

        let mut nes = b"NES\x1a".to_vec();
        nes.resize(16, 0);
        nes.extend(&rom);
        let p = write(dir.path(), "a.nes", &nes);
        assert_eq!(hash_file(Method::Nes, &p).unwrap(), md5_hex(&rom));
        let p = write(dir.path(), "b.nes", &rom);
        assert_eq!(hash_file(Method::Nes, &p).unwrap(), md5_hex(&rom));

        let snes_rom = vec![3u8; 8192];
        let mut snes = vec![0u8; 512];
        snes.extend(&snes_rom);
        let p = write(dir.path(), "a.smc", &snes);
        assert_eq!(hash_file(Method::Snes, &p).unwrap(), md5_hex(&snes_rom));

        let mut lynx = b"LYNX\0".to_vec();
        lynx.resize(64, 0);
        lynx.extend(&rom);
        let p = write(dir.path(), "a.lnx", &lynx);
        assert_eq!(hash_file(Method::Lynx, &p).unwrap(), md5_hex(&rom));

        let mut a78 = vec![1u8];
        a78.extend(b"ATARI7800");
        a78.resize(128, 0);
        a78.extend(&rom);
        let p = write(dir.path(), "a.a78", &a78);
        assert_eq!(hash_file(Method::Atari7800, &p).unwrap(), md5_hex(&rom));

        let p = write(dir.path(), "a.gb", &rom);
        assert_eq!(hash_file(Method::Whole, &p).unwrap(), md5_hex(&rom));
    }

    #[test]
    fn normalises_n64_byte_order() {
        let dir = tempfile::tempdir().unwrap();
        let z64: Vec<u8> = [0x80, 0x37, 0x12, 0x40, 1, 2, 3, 4].repeat(8);
        let v64: Vec<u8> = z64.chunks(2).flat_map(|w| [w[1], w[0]]).collect();
        let n64: Vec<u8> = z64
            .chunks(4)
            .flat_map(|w| [w[3], w[2], w[1], w[0]])
            .collect();
        let want = md5_hex(&z64);
        for (name, data) in [("a.z64", &z64), ("a.v64", &v64), ("a.n64", &n64)] {
            let p = write(dir.path(), name, data);
            assert_eq!(hash_file(Method::N64, &p).unwrap(), want, "{name}");
        }
    }

    #[test]
    fn hashes_nintendo_ds_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut rom = vec![0u8; 0x4000];
        let put = |rom: &mut Vec<u8>, at: usize, v: u32| {
            rom[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        put(&mut rom, 0x20, 0x1000);
        put(&mut rom, 0x2c, 0x100);
        put(&mut rom, 0x30, 0x2000);
        put(&mut rom, 0x3c, 0x80);
        put(&mut rom, 0x68, 0x3000);
        rom[0x1000..0x1100].fill(9);
        rom[0x2000..0x2080].fill(8);
        rom[0x3000..0x3000 + NDS_ICON].fill(5);
        let mut want = Md5::new();
        want.update(&rom[..0x160]);
        want.update(&rom[0x1000..0x1100]);
        want.update(&rom[0x2000..0x2080]);
        want.update(&rom[0x3000..0x3000 + NDS_ICON]);
        let p = write(dir.path(), "a.nds", &rom);
        assert_eq!(
            hash_file(Method::NintendoDs, &p).unwrap(),
            hex(&want.finalize())
        );
    }

    #[test]
    fn reads_boot_lines() {
        let cnf = "BOOT = cdrom:\\SLUS_007.70;1\r\nTCB = 4\r\n";
        assert_eq!(
            boot_program(cnf, "BOOT", "cdrom:").as_deref(),
            Some("SLUS_007.70")
        );
        let cnf = "BOOT2 = cdrom0:\\SLES_123.45;1\nVER = 1.00\n";
        assert_eq!(
            boot_program(cnf, "BOOT2", "cdrom0:").as_deref(),
            Some("SLES_123.45")
        );
        assert_eq!(boot_program(cnf, "BOOT", "cdrom:"), None);
        let cnf = "BOOT=cdrom:\\GAME\\MAIN.EXE;1";
        assert_eq!(
            boot_program(cnf, "BOOT", "cdrom:").as_deref(),
            Some("GAME\\MAIN.EXE")
        );
    }

    #[test]
    fn hashes_playstation_discs() {
        let dir = tempfile::tempdir().unwrap();
        // A program whose header says it is 100 bytes past the header.
        let mut exe = b"PS-X EXE".to_vec();
        exe.resize(2048 + 4000, 0xaa);
        exe[28..32].copy_from_slice(&100u32.to_le_bytes());
        let image = iso(&[
            ("SYSTEM.CNF", b"BOOT = cdrom:\\SLUS_000.01;1\n"),
            ("SLUS_000.01", &exe),
        ]);
        let mut want = Md5::new();
        want.update(b"SLUS_000.01");
        want.update(&exe[..2148]);
        let want = hex(&want.finalize());
        let iso_path = write(dir.path(), "g.iso", &image);
        assert_eq!(hash_file(Method::PlayStation, &iso_path).unwrap(), want);
        let bin = write(dir.path(), "g.bin", &raw_mode2(&image));
        assert_eq!(hash_file(Method::PlayStation, &bin).unwrap(), want);
        let chd = write(dir.path(), "g.chd", b"MComprHD");
        assert!(hash_file(Method::PlayStation, &chd).is_err());
    }

    #[test]
    fn hashes_psp_discs() {
        let dir = tempfile::tempdir().unwrap();
        let image = iso(&[
            ("PSP_GAME/PARAM.SFO", b"\0PSF sfo"),
            ("PSP_GAME/EBOOT.BIN", b"wrong folder"),
        ]);
        let p = write(dir.path(), "g.iso", &image);
        // EBOOT.BIN lives in SYSDIR; one folder level is all the test image has.
        assert!(hash_file(Method::Psp, &p).is_err());
    }

    #[test]
    fn hashes_arcade_set_names() {
        let p = Path::new("/roms/arcade/sf2.zip");
        assert_eq!(hash_file(Method::Arcade, p).unwrap(), md5_hex(b"sf2"));
    }
}
