//! A small reader for Windows executables (PE): the machine type and the
//! DLLs the program imports, directly or delay-loaded. That is all ReShade's
//! own installer looks at to pick the DLL it installs as.
//!
//! Executables are untrusted input and can be hundreds of megabytes, so the
//! reader seeks to the few structures it needs, bounds every count and
//! offset, and never reads a whole file.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context as _, Result, bail};

/// Processor architecture of an executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    X86,
    X64,
    Arm64,
    Other,
}

impl Machine {
    fn from_raw(raw: u16) -> Self {
        match raw {
            0x014c => Machine::X86,
            0x8664 => Machine::X64,
            0xaa64 => Machine::Arm64,
            _ => Machine::Other,
        }
    }
}

/// What a PE file says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: Machine,
    /// Imported DLL names in lower case, direct imports first, without
    /// duplicates.
    pub imports: Vec<String>,
}

/// Import descriptors read at most; real programs import a few dozen DLLs.
const MAX_DESCRIPTORS: usize = 4096;
/// Sections read at most (the format allows 96 in practice).
const MAX_SECTIONS: usize = 96;
/// Longest DLL name read.
const MAX_NAME: usize = 256;

/// Random access to the bytes of a file.
trait Source {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

impl Source for &[u8] {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start >= self.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(len).min(self.len());
        Ok(self[start..end].to_vec())
    }
}

impl Source for File {
    fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::with_capacity(len);
        self.by_ref().take(len as u64).read_to_end(&mut buf)?;
        Ok(buf)
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

#[derive(Debug, Clone, Copy)]
struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

struct Image<S> {
    src: S,
    sections: Vec<Section>,
    image_base: u64,
}

impl<S: Source> Image<S> {
    /// File offset of a relative virtual address.
    fn offset(&self, rva: u32) -> Option<u64> {
        self.sections.iter().find_map(|s| {
            let size = s.virtual_size.max(s.raw_size);
            let inside = rva >= s.virtual_address && rva - s.virtual_address < size;
            let delta = rva.checked_sub(s.virtual_address)?;
            (inside && delta < s.raw_size).then(|| u64::from(s.raw_offset) + u64::from(delta))
        })
    }

    fn read_rva(&mut self, rva: u32, len: usize) -> Option<Vec<u8>> {
        let at = self.offset(rva)?;
        let bytes = self.src.read_at(at, len).ok()?;
        (bytes.len() == len).then_some(bytes)
    }

    fn name_at(&mut self, rva: u32) -> Option<String> {
        let at = self.offset(rva)?;
        let bytes = self.src.read_at(at, MAX_NAME).ok()?;
        let end = bytes.iter().position(|&b| b == 0)?;
        let name = std::str::from_utf8(&bytes[..end]).ok()?;
        (!name.is_empty() && name.is_ascii()).then(|| name.to_ascii_lowercase())
    }
}

/// Read the machine type and imports of the executable at `path`.
pub fn read_file(path: &Path) -> Result<PeInfo> {
    let file = File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
    parse(file).with_context(|| format!("{} is not a Windows program", path.display()))
}

/// [`read_file`] over bytes in memory.
pub fn read_bytes(bytes: &[u8]) -> Result<PeInfo> {
    parse(bytes)
}

fn parse<S: Source>(mut src: S) -> Result<PeInfo> {
    let dos = src.read_at(0, 64)?;
    if dos.get(..2) != Some(b"MZ") {
        bail!("no DOS header");
    }
    let pe_at = u64::from(u32_at(&dos, 0x3c).context("truncated DOS header")?);
    // COFF header (24 bytes with the signature), then the optional header.
    let coff = src.read_at(pe_at, 24)?;
    if coff.get(..4) != Some(b"PE\0\0") {
        bail!("no PE signature");
    }
    let machine = Machine::from_raw(u16_at(&coff, 4).context("truncated COFF header")?);
    let sections = usize::from(u16_at(&coff, 6).context("truncated COFF header")?);
    let optional_size = usize::from(u16_at(&coff, 20).context("truncated COFF header")?);
    let optional = src.read_at(pe_at + 24, optional_size)?;
    if optional.len() != optional_size {
        bail!("truncated optional header");
    }
    let (image_base, dirs_count_at, dirs_at) = match u16_at(&optional, 0) {
        Some(0x10b) => (u64::from(u32_at(&optional, 28).unwrap_or(0)), 92, 96),
        Some(0x20b) => (u64_at(&optional, 24).unwrap_or(0), 108, 112),
        _ => bail!("unknown optional header"),
    };
    let dirs_count = u32_at(&optional, dirs_count_at).unwrap_or(0) as usize;
    let directory = |index: usize| -> Option<(u32, u32)> {
        if index >= dirs_count {
            return None;
        }
        let at = dirs_at + index * 8;
        let rva = u32_at(&optional, at)?;
        let size = u32_at(&optional, at + 4)?;
        (rva != 0).then_some((rva, size))
    };
    let imports_dir = directory(1);
    let delay_dir = directory(13);

    let table_at = pe_at + 24 + optional_size as u64;
    let count = sections.min(MAX_SECTIONS);
    let table = src.read_at(table_at, count * 40)?;
    let sections = table
        .as_chunks::<40>()
        .0
        .iter()
        .filter_map(|s| {
            Some(Section {
                virtual_size: u32_at(s, 8)?,
                virtual_address: u32_at(s, 12)?,
                raw_size: u32_at(s, 16)?,
                raw_offset: u32_at(s, 20)?,
            })
        })
        .collect();
    let mut image = Image {
        src,
        sections,
        image_base,
    };

    let mut imports: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if !imports.contains(&name) {
            imports.push(name);
        }
    };
    if let Some((rva, _)) = imports_dir {
        for i in 0..MAX_DESCRIPTORS {
            let Some(desc) = image.read_rva(rva.saturating_add((i * 20) as u32), 20) else {
                break;
            };
            if desc.iter().all(|&b| b == 0) {
                break;
            }
            if let Some(name) = u32_at(&desc, 12).and_then(|n| image.name_at(n)) {
                push(name);
            }
        }
    }
    if let Some((rva, _)) = delay_dir {
        for i in 0..MAX_DESCRIPTORS {
            let Some(desc) = image.read_rva(rva.saturating_add((i * 32) as u32), 32) else {
                break;
            };
            if desc.iter().all(|&b| b == 0) {
                break;
            }
            let attributes = u32_at(&desc, 0).unwrap_or(0);
            let Some(mut name) = u32_at(&desc, 4) else {
                break;
            };
            // Old linkers wrote virtual addresses rather than RVAs here.
            if attributes & 1 == 0 && u64::from(name) >= image.image_base && image.image_base > 0 {
                name = u32::try_from(u64::from(name) - image.image_base).unwrap_or(0);
            }
            if let Some(name) = image.name_at(name) {
                push(name);
            }
        }
    }
    Ok(PeInfo { machine, imports })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal PE file built in the test: one `.idata` section holding the
    /// import descriptors, the delay-load descriptors and the DLL names.
    pub(crate) fn build_pe(machine: u16, imports: &[&str], delayed: &[&str]) -> Vec<u8> {
        let pe32_plus = machine != 0x014c;
        let optional_size: usize = if pe32_plus { 240 } else { 224 };
        let pe_at = 0x80usize;
        let headers_end = pe_at + 24 + optional_size + 40;
        let raw_offset = headers_end.next_multiple_of(0x200);
        let section_rva = 0x1000u32;

        // Section layout: descriptors, delay descriptors, then names.
        let desc_size = (imports.len() + 1) * 20;
        let delay_size = (delayed.len() + 1) * 32;
        let names_at = desc_size + delay_size;
        let mut section = vec![0u8; names_at];
        let mut names = Vec::new();
        let mut name_rvas = Vec::new();
        for name in imports.iter().chain(delayed) {
            name_rvas.push(section_rva + (names_at + names.len()) as u32);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        for (i, rva) in name_rvas.iter().take(imports.len()).enumerate() {
            section[i * 20 + 12..i * 20 + 16].copy_from_slice(&rva.to_le_bytes());
        }
        for (i, rva) in name_rvas.iter().skip(imports.len()).enumerate() {
            let at = desc_size + i * 32;
            section[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
            section[at + 4..at + 8].copy_from_slice(&rva.to_le_bytes());
        }
        section.extend_from_slice(&names);
        let raw_size = section.len().next_multiple_of(0x200);
        section.resize(raw_size, 0);

        let mut out = vec![0u8; raw_offset];
        out[..2].copy_from_slice(b"MZ");
        out[0x3c..0x40].copy_from_slice(&(pe_at as u32).to_le_bytes());
        out[pe_at..pe_at + 4].copy_from_slice(b"PE\0\0");
        out[pe_at + 4..pe_at + 6].copy_from_slice(&machine.to_le_bytes());
        out[pe_at + 6..pe_at + 8].copy_from_slice(&1u16.to_le_bytes());
        out[pe_at + 20..pe_at + 22].copy_from_slice(&(optional_size as u16).to_le_bytes());
        let opt = pe_at + 24;
        let magic: u16 = if pe32_plus { 0x20b } else { 0x10b };
        out[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
        let (count_at, dirs_at) = if pe32_plus { (108, 112) } else { (92, 96) };
        out[opt + count_at..opt + count_at + 4].copy_from_slice(&16u32.to_le_bytes());
        let dir = |out: &mut Vec<u8>, index: usize, rva: u32, size: u32| {
            let at = opt + dirs_at + index * 8;
            out[at..at + 4].copy_from_slice(&rva.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&size.to_le_bytes());
        };
        if !imports.is_empty() {
            dir(&mut out, 1, section_rva, desc_size as u32);
        }
        if !delayed.is_empty() {
            dir(
                &mut out,
                13,
                section_rva + desc_size as u32,
                delay_size as u32,
            );
        }
        let sec = opt + optional_size;
        out[sec..sec + 6].copy_from_slice(b".idata");
        out[sec + 8..sec + 12].copy_from_slice(&(raw_size as u32).to_le_bytes());
        out[sec + 12..sec + 16].copy_from_slice(&section_rva.to_le_bytes());
        out[sec + 16..sec + 20].copy_from_slice(&(raw_size as u32).to_le_bytes());
        out[sec + 20..sec + 24].copy_from_slice(&(raw_offset as u32).to_le_bytes());
        out.extend_from_slice(&section);
        out
    }

    #[test]
    fn reads_machine_and_imports_of_64_bit_programs() {
        let pe = build_pe(
            0x8664,
            &["KERNEL32.dll", "d3d11.dll", "DXGI.dll"],
            &["XInput1_4.dll"],
        );
        let info = read_bytes(&pe).unwrap();
        assert_eq!(info.machine, Machine::X64);
        assert_eq!(
            info.imports,
            ["kernel32.dll", "d3d11.dll", "dxgi.dll", "xinput1_4.dll"]
        );
    }

    #[test]
    fn reads_32_bit_programs() {
        let pe = build_pe(0x014c, &["d3d9.dll", "USER32.dll"], &[]);
        let info = read_bytes(&pe).unwrap();
        assert_eq!(info.machine, Machine::X86);
        assert_eq!(info.imports, ["d3d9.dll", "user32.dll"]);
    }

    #[test]
    fn reads_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Game.exe");
        std::fs::write(&path, build_pe(0x8664, &["opengl32.dll"], &[])).unwrap();
        assert_eq!(read_file(&path).unwrap().imports, ["opengl32.dll"]);
    }

    #[test]
    fn refuses_what_is_not_a_program() {
        assert!(read_bytes(b"").is_err());
        assert!(read_bytes(b"#!/bin/sh\necho hi\n").is_err());
        let mut pe = build_pe(0x8664, &["d3d9.dll"], &[]);
        pe.truncate(0x90);
        assert!(read_bytes(&pe).is_err());
    }

    #[test]
    fn survives_broken_tables() {
        // Import directory pointing outside every section: no imports, no panic.
        let mut pe = build_pe(0x8664, &["d3d9.dll"], &[]);
        let opt = 0x80 + 24;
        pe[opt + 112 + 8..opt + 112 + 12].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
        let info = read_bytes(&pe).unwrap();
        assert!(info.imports.is_empty());
    }
}
