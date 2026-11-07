//! A read-only BSON decoder for the documents LiteDB 4 stores: the BSON
//! specification's types, with .NET Guids as binary subtype 4 (or the older
//! 3) in `Guid.ToByteArray` order. Every length is checked against the
//! bytes there are, and nesting is bounded, so a damaged file is an error
//! rather than a panic.

use std::fmt;

/// Why bytes could not be read as BSON.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("damaged BSON document: {0}")]
pub struct BsonError(pub &'static str);

type Result<T> = std::result::Result<T, BsonError>;

/// Documents nested deeper than this are refused (Playnite's go 3 deep).
const MAX_DEPTH: usize = 64;

/// A BSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Bson {
    Double(f64),
    String(String),
    Document(Document),
    Array(Vec<Bson>),
    Binary(Vec<u8>),
    Guid(Guid),
    ObjectId([u8; 12]),
    Bool(bool),
    /// Milliseconds since the Unix epoch, UTC.
    DateTime(i64),
    Null,
    Int32(i32),
    Int64(i64),
    /// .NET's decimal, as stored (not interpreted).
    Decimal([u8; 16]),
    MinKey,
    MaxKey,
}

impl Bson {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Bson::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Bson::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Any whole number (LiteDB stores .NET's `ulong` as `Int64`, and
    /// smaller types as `Int32`); doubles are truncated.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Bson::Int32(v) => Some(i64::from(*v)),
            Bson::Int64(v) => Some(*v),
            Bson::Double(v) if v.is_finite() => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_guid(&self) -> Option<Guid> {
        match self {
            Bson::Guid(g) => Some(*g),
            Bson::Binary(b) if b.len() == 16 => {
                let mut bytes = [0; 16];
                bytes.copy_from_slice(b);
                Some(Guid(bytes))
            }
            Bson::String(s) => Guid::parse(s),
            _ => None,
        }
    }

    pub fn as_document(&self) -> Option<&Document> {
        match self {
            Bson::Document(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Bson]> {
        match self {
            Bson::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// A BSON document: its fields in stored order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document(pub Vec<(String, Bson)>);

impl Document {
    /// The value of `key` (LiteDB's field names are case-sensitive).
    pub fn get(&self, key: &str) -> Option<&Bson> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Bson::as_str)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.get(key).and_then(Bson::as_bool).unwrap_or(false)
    }
}

/// A .NET `Guid`, in the byte order of `Guid.ToByteArray` (the first three
/// groups little-endian).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// Parse `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (any letter case, with or
    /// without braces), as .NET's `Guid.Parse` reads it.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('{').trim_end_matches('}');
        let groups: Vec<&str> = text.split('-').collect();
        if groups.len() != 5
            || groups.iter().map(|g| g.len()).ne([8, 4, 4, 4, 12])
            || !groups
                .iter()
                .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return None;
        }
        let hex: String = groups.concat();
        let mut be = [0u8; 16];
        for (i, b) in be.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        let mut bytes = be;
        bytes[0..4].reverse();
        bytes[4..6].reverse();
        bytes[6..8].reverse();
        Some(Guid(bytes))
    }
}

impl fmt::Display for Guid {
    /// Lower case with hyphens, as .NET's `Guid.ToString()` writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
        )?;
        for (i, byte) in b[8..].iter().enumerate() {
            if i == 2 {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

/// Bounds-checked reading from a byte slice.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(BsonError("a value runs past its end"))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub(crate) fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    /// A NUL-terminated name, ending before `end`.
    fn cstring(&mut self, end: usize) -> Result<String> {
        let stop = end.min(self.bytes.len());
        let rest = &self.bytes[self.pos.min(stop)..stop];
        let len = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(BsonError("a field name has no end"))?;
        let name = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(name)
    }

    /// A document's (or array's) length and where it ends.
    fn frame(&mut self) -> Result<usize> {
        let start = self.pos;
        let len = usize::try_from(self.i32()?).map_err(|_| BsonError("negative length"))?;
        let end = start
            .checked_add(len)
            .filter(|end| len >= 5 && *end <= self.bytes.len())
            .ok_or(BsonError("a document runs past its end"))?;
        Ok(end)
    }
}

/// Decode one document that fills `bytes` (trailing bytes are ignored, as
/// LiteDB ignores them).
pub fn document(bytes: &[u8]) -> Result<Document> {
    read_document(&mut Cursor::new(bytes), 0)
}

pub(crate) fn read_document(c: &mut Cursor<'_>, depth: usize) -> Result<Document> {
    let fields = read_fields(c, depth)?;
    Ok(Document(fields))
}

fn read_fields(c: &mut Cursor<'_>, depth: usize) -> Result<Vec<(String, Bson)>> {
    if depth > MAX_DEPTH {
        return Err(BsonError("documents nest too deep"));
    }
    let end = c.frame()?;
    let mut fields = Vec::new();
    while c.pos < end - 1 {
        let kind = c.u8()?;
        let name = c.cstring(end)?;
        let value = read_value(c, kind, depth)?;
        fields.push((name, value));
    }
    if c.pos != end - 1 || c.u8()? != 0 {
        return Err(BsonError("a document doesn't end where it says"));
    }
    Ok(fields)
}

fn read_value(c: &mut Cursor<'_>, kind: u8, depth: usize) -> Result<Bson> {
    Ok(match kind {
        0x01 => Bson::Double(c.f64()?),
        0x02 => {
            let len = usize::try_from(c.i32()?)
                .ok()
                .filter(|l| *l >= 1)
                .ok_or(BsonError("bad string length"))?;
            let bytes = c.take(len)?;
            if bytes[len - 1] != 0 {
                return Err(BsonError("a string has no end"));
            }
            Bson::String(String::from_utf8_lossy(&bytes[..len - 1]).into_owned())
        }
        0x03 => Bson::Document(read_document(c, depth + 1)?),
        0x04 => Bson::Array(
            read_fields(c, depth + 1)?
                .into_iter()
                .map(|(_, v)| v)
                .collect(),
        ),
        0x05 => {
            let len = usize::try_from(c.i32()?).map_err(|_| BsonError("bad binary length"))?;
            let subtype = c.u8()?;
            let bytes = c.take(len)?;
            match subtype {
                // UUID, and the older UUID subtype .NET drivers wrote, both in
                // `Guid.ToByteArray` order.
                0x03 | 0x04 if len == 16 => {
                    let mut g = [0; 16];
                    g.copy_from_slice(bytes);
                    Bson::Guid(Guid(g))
                }
                _ => Bson::Binary(bytes.to_vec()),
            }
        }
        0x07 => Bson::ObjectId(c.array()?),
        0x08 => Bson::Bool(c.u8()? != 0),
        0x09 => Bson::DateTime(c.i64()?),
        0x0A => Bson::Null,
        0x10 => Bson::Int32(c.i32()?),
        0x12 => Bson::Int64(c.i64()?),
        0x13 => Bson::Decimal(c.array()?),
        0xFF => Bson::MinKey,
        0x7F => Bson::MaxKey,
        _ => return Err(BsonError("unknown value type")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_read_and_print_as_dotnet_does() {
        let text = "cb91dfc9-b977-43bf-8e70-55f46e410fab";
        let g = Guid::parse("CB91DFC9-B977-43BF-8E70-55F46E410FAB").unwrap();
        assert_eq!(g.to_string(), text);
        // .NET's ToByteArray: the first three groups reversed.
        assert_eq!(&g.0[..4], &[0xc9, 0xdf, 0x91, 0xcb]);
        assert_eq!(&g.0[4..8], &[0x77, 0xb9, 0xbf, 0x43]);
        assert_eq!(&g.0[8..], &[0x8e, 0x70, 0x55, 0xf4, 0x6e, 0x41, 0x0f, 0xab]);
        assert_eq!(Guid::parse(&format!("{{{text}}}")), Some(g));
        assert!(Guid::parse("cb91dfc9-b977-43bf-8e70").is_none());
        assert!(Guid::parse("zb91dfc9-b977-43bf-8e70-55f46e410fab").is_none());
    }

    #[test]
    fn damaged_documents_are_errors() {
        // Too short, a length past the end, a string past the end, a
        // missing terminator, an unknown type.
        for bytes in [
            &[1u8, 0][..],
            &[50, 0, 0, 0, 0],
            &[16, 0, 0, 0, 2, b'a', 0, 100, 0, 0, 0, b'x', 0, 0, 0, 0],
            &[12, 0, 0, 0, 8, b'a', 0, 1, 0, 0, 0, 1],
            &[8, 0, 0, 0, 0x42, b'a', 0, 0],
        ] {
            assert!(document(bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(document(&[5, 0, 0, 0, 0]).unwrap(), Document::default());
    }

    #[test]
    fn deep_nesting_is_refused() {
        // A document holding a document holding … 100 levels down.
        fn nested(depth: usize) -> Vec<u8> {
            let inner = if depth == 0 {
                vec![5, 0, 0, 0, 0]
            } else {
                nested(depth - 1)
            };
            let mut body = vec![0x03, b'a', 0];
            body.extend(inner);
            body.push(0);
            let mut out = ((body.len() + 4) as i32).to_le_bytes().to_vec();
            out.extend(body);
            out
        }
        assert!(document(&nested(10)).is_ok());
        assert!(document(&nested(100)).is_err());
    }
}
