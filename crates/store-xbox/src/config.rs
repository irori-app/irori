//! `MicrosoftGame.config`, the manifest of a GDK game in its install folder:
//! the package identity, display name, Store and Xbox title IDs and the
//! programs it starts. Names are matched without letter case or namespace
//! prefix, as Windows does.

use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// The largest manifest read (real ones are a few kilobytes).
const MAX_SIZE: u64 = 1 << 20;

/// What a game's manifest says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameConfig {
    /// Package identity name (`Microsoft.Halo`).
    pub identity: Option<String>,
    /// Package version (`1.2.3.0`).
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub display_name: Option<String>,
    /// The Store product ID (`9NBLGGH4R315`).
    pub store_id: Option<String>,
    /// The Xbox title ID, hexadecimal as the manifest writes it.
    pub title_id: Option<String>,
    /// The programs, in the manifest's order; the first is the game.
    pub executables: Vec<String>,
}

impl GameConfig {
    /// The manifest in `dir` (any letter case of its name), `None` when the
    /// folder has none or it can't be read.
    pub fn read(dir: &Path) -> Option<Self> {
        let path = find(dir)?;
        let file = std::fs::File::open(path).ok()?;
        let mut text = String::new();
        use std::io::Read;
        file.take(MAX_SIZE).read_to_string(&mut text).ok()?;
        Self::parse(&text)
    }

    /// Parse a manifest's text; `None` when it isn't XML.
    pub fn parse(text: &str) -> Option<Self> {
        let mut reader = Reader::from_str(text);
        let mut config = GameConfig::default();
        let mut text_of: Option<&'static str> = None;
        loop {
            match reader.read_event().ok()? {
                Event::Start(e) | Event::Empty(e) => {
                    text_of = None;
                    match local(&e).as_str() {
                        "identity" => {
                            config.identity = attr(&e, "name");
                            config.version = attr(&e, "version");
                            config.publisher = attr(&e, "publisher");
                        }
                        "shellvisuals" => {
                            config.display_name = config
                                .display_name
                                .take()
                                .or_else(|| attr(&e, "defaultdisplayname"));
                        }
                        "executable" => {
                            if let Some(name) = attr(&e, "name").filter(|n| safe_relative(n)) {
                                config.executables.push(name);
                            }
                        }
                        "storeid" => text_of = Some("storeid"),
                        "titleid" => text_of = Some("titleid"),
                        _ => {}
                    }
                }
                Event::Text(t) => {
                    let value = Some(
                        t.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .trim()
                            .to_string(),
                    );
                    match (text_of, value) {
                        (Some("storeid"), Some(v)) if !v.is_empty() => config.store_id = Some(v),
                        (Some("titleid"), Some(v)) if !v.is_empty() => config.title_id = Some(v),
                        _ => {}
                    }
                }
                Event::End(_) => text_of = None,
                Event::Eof => break,
                _ => {}
            }
        }
        Some(config)
    }

    /// The Xbox title ID as a number.
    pub fn title_id_number(&self) -> Option<u32> {
        let id = self.title_id.as_deref()?.trim();
        let hex = id
            .strip_prefix("0x")
            .or_else(|| id.strip_prefix("0X"))
            .unwrap_or(id);
        u32::from_str_radix(hex, 16).ok().filter(|n| *n != 0)
    }

    /// The program that starts the game.
    pub fn executable(&self) -> Option<&str> {
        self.executables.first().map(String::as_str)
    }
}

/// `MicrosoftGame.config` in `dir`, whatever its letter case.
pub fn find(dir: &Path) -> Option<PathBuf> {
    let exact = dir.join("MicrosoftGame.config");
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.eq_ignore_ascii_case("MicrosoftGame.config"))
                && e.path().is_file()
        })
        .map(|e| e.path())
}

fn local(e: &BytesStart) -> String {
    e.local_name().as_ref().to_ascii_lowercase()
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.local_name().as_ref().to_ascii_lowercase();
        (key == name)
            .then(|| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.trim().to_string())
            })
            .flatten()
            .filter(|v| !v.is_empty() && v.len() < 512)
    })
}

/// A package-relative path without parent steps, roots or drive letters.
pub fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && name
            .split(['\\', '/'])
            .filter(|c| !c.is_empty())
            .all(|c| c != "." && c != ".." && !c.contains(':'))
        && !name.starts_with(['\\', '/'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_identity_programs_and_ids_without_case_or_prefix() {
        let config = GameConfig::parse(
            r#"<?xml version="1.0"?>
            <Game configVersion="1" xmlns:x="urn:x">
              <x:Identity Name="Example.Game" Version="1.2.3.0" Publisher="CN=Example"/>
              <ShellVisuals DefaultDisplayName="Example &amp; Co"/>
              <ExecutableList>
                <Executable Name="Bin\Game.exe" Id="Game"/>
                <Executable Name="..\evil.exe"/>
                <Executable Name="Tools/Editor.exe"/>
              </ExecutableList>
              <StoreId> 9ZZZZZZZZZZZ </StoreId>
              <TitleId>1A2B3C4D</TitleId>
            </Game>"#,
        )
        .unwrap();
        assert_eq!(config.identity.as_deref(), Some("Example.Game"));
        assert_eq!(config.version.as_deref(), Some("1.2.3.0"));
        assert_eq!(config.display_name.as_deref(), Some("Example & Co"));
        assert_eq!(config.executables, ["Bin\\Game.exe", "Tools/Editor.exe"]);
        assert_eq!(config.store_id.as_deref(), Some("9ZZZZZZZZZZZ"));
        assert_eq!(config.title_id_number(), Some(0x1A2B3C4D));
    }

    #[test]
    fn broken_or_empty_manifests() {
        assert!(GameConfig::parse("<Game><unclosed").is_none());
        let empty = GameConfig::parse("<Game/>").unwrap();
        assert!(empty.executable().is_none());
        assert!(empty.title_id_number().is_none());
    }

    #[test]
    fn finds_the_manifest_in_any_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("microsoftgame.CONFIG"),
            r#"<Game><ExecutableList><Executable Name="g.exe"/></ExecutableList></Game>"#,
        )
        .unwrap();
        assert_eq!(
            GameConfig::read(dir.path()).unwrap().executable(),
            Some("g.exe")
        );
        assert!(GameConfig::read(&dir.path().join("none")).is_none());
    }

    #[test]
    fn relative_paths_stay_inside_the_package() {
        for bad in [
            "",
            "..\\x.exe",
            "C:\\x.exe",
            "/x.exe",
            "\\x.exe",
            "a/../../x",
        ] {
            assert!(!safe_relative(bad), "{bad}");
        }
        assert!(safe_relative("bin\\x64\\game.exe"));
    }
}
