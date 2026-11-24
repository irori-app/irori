//! Reading LaunchBox's `Metadata.xml` as a stream.
//!
//! The file is over half a gigabyte: a flat list of `<Game>`,
//! `<GameAlternateName>` and `<GameImage>` records under `<LaunchBox>`,
//! joined by `DatabaseID`. It is read one event at a time and only what the
//! box art needs is kept: games on the platforms the caller names, their
//! alternate names and their front boxes. Every field is capped, every list
//! is capped, and a run of text without markup longer than
//! [`MAX_TEXT_RUN`] stops the read, so no input makes the reader hold much
//! more than the index it builds.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufReader, Read};

use anyhow::{Context as _, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

use super::{BoxArt, LaunchBoxGame, SystemIndex};

/// The longest field value kept; longer names and file names are dropped.
const MAX_FIELD: usize = 512;
/// The most games kept across every platform.
const MAX_GAMES: usize = 400_000;
/// The most boxes kept across every game.
const MAX_BOXES: usize = 1_500_000;
/// The most alternate names kept per game.
const MAX_ALTERNATES: usize = 16;
/// The most boxes kept per game.
const MAX_BOXES_PER_GAME: usize = 64;
/// The longest run of bytes without a `<`: one text node (an `Overview`) is
/// a few kilobytes; a file with a longer one is not LaunchBox's.
pub(crate) const MAX_TEXT_RUN: usize = 1 << 20;

/// Image types that are the front of a box.
const FRONT: &str = "Box - Front";
const FRONT_RECONSTRUCTED: &str = "Box - Front - Reconstructed";

/// A record being read.
#[derive(Default)]
struct Record {
    kind: Kind,
    name: String,
    id: Option<u32>,
    platform: String,
    file: String,
    image_type: String,
    region: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Kind {
    #[default]
    Other,
    Game,
    Alternate,
    Image,
}

/// A game kept while reading.
struct Kept {
    system: usize,
    game: LaunchBoxGame,
}

/// Read `Metadata.xml` from `input`, keeping the games whose LaunchBox
/// platform `platforms` maps to a system id. Returns each system's games
/// that have at least one front box, sorted by name.
pub fn read(
    input: impl Read,
    platforms: &HashMap<String, String>,
) -> Result<BTreeMap<String, SystemIndex>> {
    let systems: Vec<&String> = {
        let mut s: Vec<&String> = platforms.values().collect();
        s.sort();
        s.dedup();
        s
    };
    let system_of = |platform: &str| {
        platforms
            .get(platform)
            .and_then(|id| systems.iter().position(|s| *s == id))
    };

    let mut reader =
        Reader::from_reader(BufReader::with_capacity(64 * 1024, BoundedRuns::new(input)));
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut depth = 0usize;
    let mut record = Record::default();
    // The field of `record` being read, while inside one we keep.
    let mut field: Option<&'static str> = None;
    let mut value = String::new();

    let mut games: HashMap<u32, Kept> = HashMap::new();
    // Games on platforms nobody asked for: their images are dropped at once.
    let mut elsewhere: HashSet<u32> = HashSet::new();
    // Records naming a game not read yet (LaunchBox lists games first, but
    // nothing promises it): joined at the end.
    let mut early_boxes: Vec<(u32, BoxArt)> = Vec::new();
    let mut early_names: Vec<(u32, String)> = Vec::new();
    let mut boxes = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .context("LaunchBox's games database isn't valid XML")?;
        match event {
            Event::Start(e) => {
                depth += 1;
                let name = e.local_name();
                match depth {
                    2 => {
                        record = Record {
                            kind: match name.as_ref() {
                                "Game" => Kind::Game,
                                "GameAlternateName" => Kind::Alternate,
                                "GameImage" => Kind::Image,
                                _ => Kind::Other,
                            },
                            ..Default::default()
                        };
                    }
                    3 if record.kind != Kind::Other => {
                        field = wanted_field(record.kind, name.as_ref());
                        value.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if field.is_some() => push_capped(&mut value, &t.xml10_content()),
            Event::CData(t) if field.is_some() => push_capped(&mut value, t.as_ref()),
            Event::GeneralRef(r) if field.is_some() => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    let mut tmp = [0u8; 4];
                    push_capped(&mut value, c.encode_utf8(&mut tmp));
                } else if let Some(s) =
                    quick_xml::escape::resolve_predefined_entity(&r.xml10_content())
                {
                    push_capped(&mut value, s);
                }
            }
            Event::End(_) => {
                if depth == 3
                    && let Some(f) = field.take()
                {
                    let v = value.trim();
                    // A field at the cap was cut: better nothing than a wrong name.
                    if value.len() < MAX_FIELD {
                        match f {
                            "name" => record.name = v.to_string(),
                            "id" => record.id = v.parse().ok(),
                            "platform" => record.platform = v.to_string(),
                            "file" => record.file = v.to_string(),
                            "type" => record.image_type = v.to_string(),
                            "region" => record.region = v.to_string(),
                            _ => {}
                        }
                    }
                } else if depth == 2 {
                    let done = std::mem::take(&mut record);
                    finish(
                        done,
                        &system_of,
                        &mut games,
                        &mut elsewhere,
                        &mut early_boxes,
                        &mut early_names,
                        &mut boxes,
                    );
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    for (id, art) in early_boxes {
        if let Some(kept) = games.get_mut(&id)
            && kept.game.boxes.len() < MAX_BOXES_PER_GAME
        {
            kept.game.boxes.push(art);
        }
    }
    for (id, name) in early_names {
        if let Some(kept) = games.get_mut(&id) {
            add_alternate(&mut kept.game, name);
        }
    }

    let mut out: BTreeMap<String, SystemIndex> = BTreeMap::new();
    for kept in games.into_values() {
        if kept.game.boxes.is_empty() {
            continue;
        }
        out.entry(systems[kept.system].clone())
            .or_default()
            .games
            .push(kept.game);
    }
    for index in out.values_mut() {
        index
            .games
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    }
    Ok(out)
}

/// The field of a `kind` record an element holds, if it is one we keep.
fn wanted_field(kind: Kind, element: &str) -> Option<&'static str> {
    match (kind, element) {
        (Kind::Game, "Name") | (Kind::Alternate, "AlternateName") => Some("name"),
        (Kind::Game, "Platform") => Some("platform"),
        (Kind::Image, "FileName") => Some("file"),
        (Kind::Image, "Type") => Some("type"),
        (Kind::Image, "Region") => Some("region"),
        (_, "DatabaseID") => Some("id"),
        _ => None,
    }
}

fn push_capped(value: &mut String, s: &str) {
    if value.len() >= MAX_FIELD {
        return;
    }
    for c in s.chars() {
        if value.len() + c.len_utf8() > MAX_FIELD {
            // Marks the value as cut: `MAX_FIELD` bytes means too long.
            while value.len() < MAX_FIELD {
                value.push(' ');
            }
            return;
        }
        value.push(c);
    }
}

fn add_alternate(game: &mut LaunchBoxGame, name: String) {
    if !name.is_empty()
        && name != game.name
        && game.alternate.len() < MAX_ALTERNATES
        && !game.alternate.contains(&name)
    {
        game.alternate.push(name);
    }
}

/// An image's file name as LaunchBox's image server serves it: one plain
/// name, so it can't make the address point anywhere else.
fn plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn finish(
    record: Record,
    system_of: &dyn Fn(&str) -> Option<usize>,
    games: &mut HashMap<u32, Kept>,
    elsewhere: &mut HashSet<u32>,
    early_boxes: &mut Vec<(u32, BoxArt)>,
    early_names: &mut Vec<(u32, String)>,
    boxes: &mut usize,
) {
    let Some(id) = record.id else { return };
    match record.kind {
        Kind::Game => {
            if record.name.is_empty() || games.contains_key(&id) {
                return;
            }
            match system_of(&record.platform) {
                Some(system) if games.len() < MAX_GAMES => {
                    games.insert(
                        id,
                        Kept {
                            system,
                            game: LaunchBoxGame {
                                id,
                                name: record.name,
                                alternate: Vec::new(),
                                boxes: Vec::new(),
                            },
                        },
                    );
                }
                _ => {
                    elsewhere.insert(id);
                }
            }
        }
        Kind::Alternate => {
            if let Some(kept) = games.get_mut(&id) {
                add_alternate(&mut kept.game, record.name);
            } else if !elsewhere.contains(&id) && early_names.len() < MAX_GAMES {
                early_names.push((id, record.name));
            }
        }
        Kind::Image => {
            let reconstructed = match record.image_type.as_str() {
                FRONT => false,
                FRONT_RECONSTRUCTED => true,
                _ => return,
            };
            if !plain_file_name(&record.file) || *boxes >= MAX_BOXES || elsewhere.contains(&id) {
                return;
            }
            let art = BoxArt {
                file: record.file,
                region: Some(record.region).filter(|r| !r.is_empty()),
                reconstructed,
            };
            if let Some(kept) = games.get_mut(&id) {
                if kept.game.boxes.len() < MAX_BOXES_PER_GAME {
                    kept.game.boxes.push(art);
                    *boxes += 1;
                }
            } else {
                early_boxes.push((id, art));
                *boxes += 1;
            }
        }
        Kind::Other => {}
    }
}

/// A reader that fails once more than [`MAX_TEXT_RUN`] bytes pass without a
/// `<`, which bounds the size of any one event the XML reader buffers.
struct BoundedRuns<R> {
    inner: R,
    run: usize,
}

impl<R> BoundedRuns<R> {
    fn new(inner: R) -> Self {
        Self { inner, run: 0 }
    }
}

impl<R: Read> Read for BoundedRuns<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        let read = &out[..n];
        // The run this read ends, and the one it leaves open.
        let first = read.iter().position(|b| *b == b'<').unwrap_or(n);
        let longest = self.run + first;
        self.run = match read.iter().rposition(|b| *b == b'<') {
            Some(last) => n - last - 1,
            None => longest,
        };
        if longest > MAX_TEXT_RUN || self.run > MAX_TEXT_RUN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a text in LaunchBox's games database is far too long",
            ));
        }
        Ok(n)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FIXTURE: &str = r#"<?xml version="1.0" standalone="yes"?>
<LaunchBox>
  <Game>
    <Name>Super Mario Bros. 3</Name>
    <Overview>Bowser &amp; his children are back.</Overview>
    <DatabaseID>10</DatabaseID>
    <Platform>Nintendo Entertainment System</Platform>
    <Genres />
  </Game>
  <Game>
    <Name>Sonic &amp; Knuckles</Name>
    <DatabaseID>20</DatabaseID>
    <Platform>Sega Genesis</Platform>
  </Game>
  <Game>
    <Name>Halo: Combat Evolved</Name>
    <DatabaseID>1</DatabaseID>
    <Platform>Windows</Platform>
  </Game>
  <Game>
    <Name>Boxless</Name>
    <DatabaseID>30</DatabaseID>
    <Platform>Nintendo Entertainment System</Platform>
  </Game>
  <Platform><Name>Nintendo Entertainment System</Name></Platform>
  <GameAlternateName>
    <AlternateName>Super Mario Bros. 3 (Famicom)</AlternateName>
    <DatabaseID>10</DatabaseID>
    <Region>Japan</Region>
  </GameAlternateName>
  <GameAlternateName>
    <AlternateName>Halo 1</AlternateName>
    <DatabaseID>1</DatabaseID>
  </GameAlternateName>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>aaaa-1111.jpg</FileName>
    <Type>Box - Front</Type>
    <Region>North America</Region>
    <CRC32>1</CRC32>
  </GameImage>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>bbbb-2222.png</FileName>
    <Type>Box - Front</Type>
    <Region>Japan</Region>
  </GameImage>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>cccc-3333.png</FileName>
    <Type>Box - Front - Reconstructed</Type>
  </GameImage>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>dddd-4444.png</FileName>
    <Type>Screenshot - Gameplay</Type>
  </GameImage>
  <GameImage>
    <DatabaseID>10</DatabaseID>
    <FileName>../escape.png</FileName>
    <Type>Box - Front</Type>
  </GameImage>
  <GameImage>
    <DatabaseID>20</DatabaseID>
    <FileName>eeee-5555.jpg</FileName>
    <Type>Box - Front</Type>
    <Region>Europe</Region>
  </GameImage>
  <GameImage>
    <DatabaseID>1</DatabaseID>
    <FileName>ffff-6666.jpg</FileName>
    <Type>Box - Front</Type>
  </GameImage>
</LaunchBox>"#;

    pub(crate) fn platforms() -> HashMap<String, String> {
        [
            ("Nintendo Entertainment System", "nes"),
            ("Nintendo Famicom Disk System", "nes"),
            ("Sega Genesis", "genesis"),
        ]
        .into_iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect()
    }

    #[test]
    fn keeps_front_boxes_of_the_named_platforms() {
        let out = read(FIXTURE.as_bytes(), &platforms()).unwrap();
        assert_eq!(out.keys().collect::<Vec<_>>(), ["genesis", "nes"]);

        let nes = &out["nes"].games;
        // "Boxless" has no box and Halo is on a platform nobody asked for.
        assert_eq!(nes.len(), 1);
        let smb3 = &nes[0];
        assert_eq!(smb3.id, 10);
        assert_eq!(smb3.name, "Super Mario Bros. 3");
        assert_eq!(smb3.alternate, ["Super Mario Bros. 3 (Famicom)"]);
        let files: Vec<&str> = smb3.boxes.iter().map(|b| b.file.as_str()).collect();
        // The screenshot and the file name with a path are left out.
        assert_eq!(files, ["aaaa-1111.jpg", "bbbb-2222.png", "cccc-3333.png"]);
        assert_eq!(smb3.boxes[0].region.as_deref(), Some("North America"));
        assert!(!smb3.boxes[0].reconstructed);
        assert_eq!(smb3.boxes[2].region, None);
        assert!(smb3.boxes[2].reconstructed);

        // Entities are read.
        assert_eq!(out["genesis"].games[0].name, "Sonic & Knuckles");
    }

    #[test]
    fn images_listed_before_their_game_still_join_it() {
        let xml = r#"<LaunchBox>
  <GameImage><DatabaseID>7</DatabaseID><FileName>x.png</FileName><Type>Box - Front</Type></GameImage>
  <GameAlternateName><AlternateName>Rockman</AlternateName><DatabaseID>7</DatabaseID></GameAlternateName>
  <Game><Name>Mega Man</Name><DatabaseID>7</DatabaseID><Platform>Nintendo Famicom Disk System</Platform></Game>
</LaunchBox>"#;
        let out = read(xml.as_bytes(), &platforms()).unwrap();
        let game = &out["nes"].games[0];
        assert_eq!(game.boxes.len(), 1);
        assert_eq!(game.alternate, ["Rockman"]);
    }

    #[test]
    fn cut_fields_are_dropped() {
        let long = "A".repeat(MAX_FIELD + 10);
        let xml = format!(
            "<LaunchBox><Game><Name>{long}</Name><DatabaseID>1</DatabaseID>\
             <Platform>Sega Genesis</Platform></Game>\
             <GameImage><DatabaseID>1</DatabaseID><FileName>a.png</FileName>\
             <Type>Box - Front</Type></GameImage></LaunchBox>"
        );
        let out = read(xml.as_bytes(), &platforms()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn a_text_without_end_stops_the_read() {
        let mut xml = String::from("<LaunchBox><Game><Overview>");
        xml.push_str(&"x".repeat(MAX_TEXT_RUN + 1));
        xml.push_str("</Overview></Game></LaunchBox>");
        let err = read(xml.as_bytes(), &platforms()).unwrap_err();
        assert!(format!("{err:#}").contains("far too long"), "{err:#}");
    }

    #[test]
    fn bounded_runs_count_across_reads() {
        let data = b"<a>xxxx<b>yy";
        let mut r = BoundedRuns::new(io::BufReader::with_capacity(3, &data[..]));
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        // "b>yy" since the last `<`.
        assert_eq!(r.run, 4);
    }

    #[test]
    fn broken_xml_is_an_error() {
        assert!(read("<LaunchBox><Game></LaunchBox>".as_bytes(), &platforms()).is_err());
    }
}
