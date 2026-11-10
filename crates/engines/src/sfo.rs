//! `PARAM.SFO`, the parameter file of PSP, PS3, PS Vita and PS4 games: a
//! small table of named values (title, title id, version). Only reading;
//! every offset is checked, so a damaged or hostile file yields nothing
//! rather than a panic.
//!
//! The layout: a 20-byte header (`\0PSF`, version, key table offset, data
//! table offset, entry count), then 16-byte index entries (key offset,
//! format, used length, maximum length, data offset), all little endian.

const MAGIC: &[u8; 4] = b"\0PSF";
const HEADER_LEN: usize = 20;
const ENTRY_LEN: usize = 16;
/// Value formats: UTF-8 without and with a terminating NUL.
const UTF8_SPECIAL: u16 = 0x0004;
const UTF8: u16 = 0x0204;

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    let v = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?);
    usize::try_from(v).ok()
}

/// The text value named `key`, if the file has one. Trailing NULs are
/// dropped; nothing else is changed.
pub fn string(bytes: &[u8], key: &str) -> Option<String> {
    if bytes.get(..4)? != MAGIC {
        return None;
    }
    let keys = u32_at(bytes, 8)?;
    let data = u32_at(bytes, 12)?;
    let count = u32_at(bytes, 16)?;
    // Entries can't outnumber the bytes that would hold them.
    if count > bytes.len() / ENTRY_LEN {
        return None;
    }
    (0..count).find_map(|i| {
        let entry = HEADER_LEN + i * ENTRY_LEN;
        let name_at = keys.checked_add(usize::from(u16_at(bytes, entry)?))?;
        let rest = bytes.get(name_at..)?;
        let name = &rest[..rest.iter().position(|&b| b == 0)?];
        if name != key.as_bytes() {
            return None;
        }
        let format = u16_at(bytes, entry + 2)?;
        if format != UTF8 && format != UTF8_SPECIAL {
            return None;
        }
        let used = u32_at(bytes, entry + 4)?;
        let max = u32_at(bytes, entry + 8)?;
        // A NUL-terminated value is read to its NUL within the field: some
        // tools write the used length in characters, not bytes, which cuts
        // a title with accents short or inside a letter.
        let len = if format == UTF8 { used.max(max) } else { used };
        let start = data.checked_add(u32_at(bytes, entry + 12)?)?;
        let field = |len: usize| bytes.get(start..start.checked_add(len)?);
        let value = field(len).or_else(|| field(used))?;
        let value = match value.iter().position(|&b| b == 0) {
            Some(end) => &value[..end],
            None => value,
        };
        String::from_utf8(value.to_vec()).ok()
    })
}

/// The game's title (`TITLE`) as a library would show it: line breaks
/// (PS3 titles often have them) and runs of spaces become one space, and
/// trademark signs are dropped.
pub fn title(bytes: &[u8]) -> Option<String> {
    let raw = string(bytes, "TITLE")?;
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, '™' | '®'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Builds `PARAM.SFO` files for tests: `(key, value)` pairs, text only.
#[cfg(any(test, feature = "test-support"))]
pub fn build(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut data = Vec::new();
    let mut index = Vec::new();
    for (key, value) in entries {
        let key_at = keys.len() as u16;
        keys.extend_from_slice(key.as_bytes());
        keys.push(0);
        let data_at = data.len() as u32;
        let len = value.len() as u32 + 1;
        // Values are padded to a multiple of four, as real files are.
        let max = len.div_ceil(4) * 4;
        data.extend_from_slice(value.as_bytes());
        data.resize(data_at as usize + max as usize, 0);
        index.extend_from_slice(&key_at.to_le_bytes());
        index.extend_from_slice(&UTF8.to_le_bytes());
        index.extend_from_slice(&len.to_le_bytes());
        index.extend_from_slice(&max.to_le_bytes());
        index.extend_from_slice(&data_at.to_le_bytes());
    }
    while keys.len() % 4 != 0 {
        keys.push(0);
    }
    let keys_at = (HEADER_LEN + index.len()) as u32;
    let data_at = keys_at + keys.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&0x0101u32.to_le_bytes());
    out.extend_from_slice(&keys_at.to_le_bytes());
    out.extend_from_slice(&data_at.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend(index);
    out.extend(keys);
    out.extend(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_titles_and_other_values() {
        let file = build(&[
            ("CATEGORY", "DG"),
            ("TITLE", "Demon's Souls™"),
            ("TITLE_ID", "BLUS30443"),
        ]);
        assert_eq!(title(&file).as_deref(), Some("Demon's Souls"));
        assert_eq!(string(&file, "TITLE_ID").as_deref(), Some("BLUS30443"));
        assert!(string(&file, "VERSION").is_none());

        let multiline = build(&[("TITLE", "METAL GEAR SOLID 4\nGUNS OF THE PATRIOTS")]);
        assert_eq!(
            title(&multiline).as_deref(),
            Some("METAL GEAR SOLID 4 GUNS OF THE PATRIOTS")
        );
        assert!(title(&build(&[("TITLE", "  \n ")])).is_none());
    }

    #[test]
    fn reads_titles_with_accents_whose_length_counts_characters() {
        let accented = build(&[("TITLE", "Ōkami Café"), ("TITLE_ID", "PCSE00001")]);
        assert_eq!(title(&accented).as_deref(), Some("Ōkami Café"));
        // The used length written as 11 (characters and the NUL) where the
        // value takes 13 bytes.
        let mut short = accented.clone();
        short[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&11u32.to_le_bytes());
        assert_eq!(title(&short).as_deref(), Some("Ōkami Café"));
        assert_eq!(string(&short, "TITLE_ID").as_deref(), Some("PCSE00001"));
    }

    #[test]
    fn refuses_damaged_files() {
        let file = build(&[("TITLE", "Sample Game")]);
        assert!(title(b"").is_none());
        assert!(title(b"\0PSF").is_none());
        let mut wrong_magic = file.clone();
        wrong_magic[1] = b'X';
        assert!(title(&wrong_magic).is_none());
        // Every truncation fails cleanly instead of reading past the end.
        for len in 0..file.len() - 4 {
            let _ = title(&file[..len]);
        }
        // A huge entry count or offset is refused, not followed.
        let mut count = file.clone();
        count[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(title(&count).is_none());
        let mut offset = file.clone();
        offset[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(title(&offset).is_none());
        // Integer values are not text.
        let mut integer = file;
        integer[HEADER_LEN + 2..HEADER_LEN + 4].copy_from_slice(&0x0404u16.to_le_bytes());
        assert!(title(&integer).is_none());
    }
}
