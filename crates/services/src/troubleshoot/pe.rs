//! A headers-only reader of Windows programs (PE files): which DLLs a
//! program imports when it starts, whether it is a .NET program, and its
//! machine. It reads the headers, the section table and the import names
//! with small seeks, never the whole file, and gives up on anything that
//! doesn't look like a PE file instead of trusting its sizes.

#![forbid(unsafe_code)]

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// More import descriptors than any real program has; a damaged table
/// stops here instead of walking the whole file.
const MAX_IMPORTS: usize = 512;
/// More sections than any real program has.
const MAX_SECTIONS: u16 = 96;
/// Longest DLL name read.
const MAX_NAME: usize = 256;

const IMAGE_FILE_MACHINE_I386: u16 = 0x014c;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
const PE32: u16 = 0x10b;
const PE32_PLUS: u16 = 0x20b;
/// Data directories: imports and the .NET (CLR) header.
const DIR_IMPORT: usize = 1;
const DIR_CLR: usize = 14;

/// The processor a program is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    X86,
    X64,
    Arm64,
    Other(u16),
}

/// What a program's headers say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: Machine,
    /// DLLs imported at start, lower case, in the order listed.
    pub imports: Vec<String>,
    /// The program has a .NET header (it runs in the .NET runtime).
    pub dotnet: bool,
}

/// Read `path`'s headers; `None` when it isn't a readable PE file.
pub fn read_file(path: &Path) -> Option<PeInfo> {
    let mut file = std::fs::File::open(path).ok()?;
    parse(&mut file)
}

/// Read a PE file's headers from `r`; `None` when it isn't one.
pub fn parse<R: Read + Seek>(r: &mut R) -> Option<PeInfo> {
    let dos = read_at(r, 0, 64)?;
    if &dos[..2] != b"MZ" {
        return None;
    }
    let pe = u64::from(u32_at(&dos, 0x3c)?);
    let coff = read_at(r, pe, 24)?;
    if &coff[..4] != b"PE\0\0" {
        return None;
    }
    let machine = match u16_at(&coff, 4)? {
        IMAGE_FILE_MACHINE_I386 => Machine::X86,
        IMAGE_FILE_MACHINE_AMD64 => Machine::X64,
        IMAGE_FILE_MACHINE_ARM64 => Machine::Arm64,
        other => Machine::Other(other),
    };
    let sections = u16_at(&coff, 6)?;
    let optional_size = u16_at(&coff, 20)?;
    if sections == 0 || sections > MAX_SECTIONS || optional_size < 96 {
        return None;
    }
    let optional = read_at(r, pe + 24, usize::from(optional_size))?;
    let (count_at, dirs_at) = match u16_at(&optional, 0)? {
        PE32 => (92, 96),
        PE32_PLUS => (108, 112),
        _ => return None,
    };
    let dir_count = u32_at(&optional, count_at)? as usize;
    let dir = |ix: usize| -> Option<(u32, u32)> {
        if ix >= dir_count {
            return None;
        }
        let at = dirs_at + ix * 8;
        Some((u32_at(&optional, at)?, u32_at(&optional, at + 4)?))
    };
    let table = read_at(
        r,
        pe + 24 + u64::from(optional_size),
        usize::from(sections) * 40,
    )?;
    let sections: Vec<Section> = table
        .as_chunks::<40>()
        .0
        .iter()
        .filter_map(|s| {
            Some(Section {
                virtual_address: u32_at(s, 12)?,
                raw_size: u32_at(s, 16)?,
                raw_pointer: u32_at(s, 20)?,
            })
        })
        .collect();
    let offset = |rva: u32| sections.iter().find_map(|s| s.offset(rva));

    let dotnet = dir(DIR_CLR).is_some_and(|(rva, size)| rva != 0 && size != 0);
    let mut imports = Vec::new();
    if let Some((rva, size)) = dir(DIR_IMPORT).filter(|(rva, _)| *rva != 0)
        && let Some(start) = offset(rva)
    {
        // The table ends with an all-zero descriptor; `size` bounds it too
        // when it is set.
        let limit = if size == 0 {
            MAX_IMPORTS
        } else {
            (size as usize / 20).min(MAX_IMPORTS)
        };
        for ix in 0..limit {
            let Some(desc) = read_at(r, start + ix as u64 * 20, 20) else {
                break;
            };
            if desc.iter().all(|b| *b == 0) {
                break;
            }
            let name_rva = u32_at(&desc, 12)?;
            let Some(name) = offset(name_rva).and_then(|at| read_name(r, at)) else {
                continue;
            };
            let name = name.to_ascii_lowercase();
            if !imports.contains(&name) {
                imports.push(name);
            }
        }
    }
    Some(PeInfo {
        machine,
        imports,
        dotnet,
    })
}

struct Section {
    virtual_address: u32,
    raw_size: u32,
    raw_pointer: u32,
}

impl Section {
    /// File offset of `rva` when this section holds it.
    fn offset(&self, rva: u32) -> Option<u64> {
        // Past the raw data the section is zeros in memory only.
        let inside = rva.checked_sub(self.virtual_address)?;
        (inside < self.raw_size).then(|| u64::from(self.raw_pointer) + u64::from(inside))
    }
}

/// A NUL-terminated ASCII name at `at`.
fn read_name<R: Read + Seek>(r: &mut R, at: u64) -> Option<String> {
    r.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = vec![0u8; MAX_NAME];
    let n = read_up_to(r, &mut buf)?;
    let end = buf[..n].iter().position(|b| *b == 0)?;
    let name = &buf[..end];
    let plausible = !name.is_empty()
        && name
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b' '));
    plausible.then(|| String::from_utf8_lossy(name).into_owned())
}

/// Exactly `len` bytes at `at`.
fn read_at<R: Read + Seek>(r: &mut R, at: u64, len: usize) -> Option<Vec<u8>> {
    r.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Fill as much of `buf` as the file has.
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> Option<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(filled)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A minimal PE file for tests: one section holding the import table and
/// the names, optionally a .NET header directory.
#[cfg(test)]
pub(crate) fn build_test_pe(machine: u16, plus: bool, dlls: &[&str], dotnet: bool) -> Vec<u8> {
    let put16 =
        |v: &mut Vec<u8>, at: usize, x: u16| v[at..at + 2].copy_from_slice(&x.to_le_bytes());
    let put32 =
        |v: &mut Vec<u8>, at: usize, x: u32| v[at..at + 4].copy_from_slice(&x.to_le_bytes());
    let pe = 0x40usize;
    let optional_size: u16 = if plus { 240 } else { 224 };
    let table = pe + 24 + usize::from(optional_size);
    let raw = 0x200usize;
    let va = 0x1000u32;
    let mut f = vec![0u8; raw + 0x400];
    f[..2].copy_from_slice(b"MZ");
    put32(&mut f, 0x3c, pe as u32);
    f[pe..pe + 4].copy_from_slice(b"PE\0\0");
    put16(&mut f, pe + 4, machine);
    put16(&mut f, pe + 6, 1);
    put16(&mut f, pe + 20, optional_size);
    let opt = pe + 24;
    put16(&mut f, opt, if plus { PE32_PLUS } else { PE32 });
    let (count_at, dirs_at) = if plus { (108, 112) } else { (92, 96) };
    put32(&mut f, opt + count_at, 16);
    let descriptors = (dlls.len() + 1) * 20;
    put32(&mut f, opt + dirs_at + DIR_IMPORT * 8, va);
    put32(
        &mut f,
        opt + dirs_at + DIR_IMPORT * 8 + 4,
        descriptors as u32,
    );
    if dotnet {
        put32(&mut f, opt + dirs_at + DIR_CLR * 8, va + 0x300);
        put32(&mut f, opt + dirs_at + DIR_CLR * 8 + 4, 72);
    }
    f[table..table + 6].copy_from_slice(b".idata");
    put32(&mut f, table + 8, 0x400);
    put32(&mut f, table + 12, va);
    put32(&mut f, table + 16, 0x400);
    put32(&mut f, table + 20, raw as u32);
    let mut name_at = descriptors;
    for (ix, dll) in dlls.iter().enumerate() {
        put32(&mut f, raw + ix * 20 + 12, va + name_at as u32);
        f[raw + name_at..raw + name_at + dll.len()].copy_from_slice(dll.as_bytes());
        name_at += dll.len() + 1;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_the_imports_of_a_64_bit_program() {
        let file = build_test_pe(
            IMAGE_FILE_MACHINE_AMD64,
            true,
            &[
                "KERNEL32.dll",
                "MSVCP140.dll",
                "d3dcompiler_47.dll",
                "MSVCP140.dll",
            ],
            false,
        );
        let info = parse(&mut Cursor::new(file)).unwrap();
        assert_eq!(info.machine, Machine::X64);
        assert_eq!(
            info.imports,
            ["kernel32.dll", "msvcp140.dll", "d3dcompiler_47.dll"]
        );
        assert!(!info.dotnet);
    }

    #[test]
    fn tells_32_bit_and_dotnet_programs() {
        let file = build_test_pe(IMAGE_FILE_MACHINE_I386, false, &["mscoree.dll"], true);
        let info = parse(&mut Cursor::new(file)).unwrap();
        assert_eq!(info.machine, Machine::X86);
        assert_eq!(info.imports, ["mscoree.dll"]);
        assert!(info.dotnet);
    }

    #[test]
    fn refuses_what_isnt_a_program() {
        assert!(parse(&mut Cursor::new(b"#!/bin/sh\necho hi\n".to_vec())).is_none());
        assert!(parse(&mut Cursor::new(b"MZ".to_vec())).is_none());
        // A header pointing past the end of the file.
        let mut file = build_test_pe(IMAGE_FILE_MACHINE_AMD64, true, &["a.dll"], false);
        file[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(parse(&mut Cursor::new(file)).is_none());
        // A truncated import table keeps what could be read.
        let file = build_test_pe(IMAGE_FILE_MACHINE_AMD64, true, &["a.dll"], false);
        let cut = file[..0x200 + 20].to_vec();
        let info = parse(&mut Cursor::new(cut)).unwrap();
        assert!(info.imports.is_empty());
    }

    #[test]
    fn reads_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.exe");
        std::fs::write(
            &path,
            build_test_pe(IMAGE_FILE_MACHINE_AMD64, true, &["VCRUNTIME140.dll"], false),
        )
        .unwrap();
        assert_eq!(read_file(&path).unwrap().imports, ["vcruntime140.dll"]);
        assert!(read_file(&dir.path().join("missing.exe")).is_none());
    }
}
