//! Keeping Irori's own entries of `shortcuts.vdf` in step with the library,
//! with the rules Sunshine's applications follow: only Irori's entries are
//! touched, and in them only Irori's fields.
//!
//! An entry is Irori's when it carries [`ID_KEY`] (written by Irori), or,
//! when an edit or Steam dropped that key, when its command starts a game
//! through Irori (`irori --launch <runner>/<app>` or an
//! `irori://launch/...` link). Irori owns [`OWN_FIELDS`]; the name, the
//! collections, the overlay and every other field stay as the user or Steam
//! left them, and so does the artwork already in Steam's `grid` folder.
//!
//! Writes go through [`write_checked`]: the file is copied to
//! `shortcuts.vdf.irori-backup` first and read back after writing; a file
//! that doesn't read back as written is replaced by the previous one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{GameId, Runner};

use super::shortcuts_file::{ShortcutEntry, ShortcutsFile, shortcut_id};
use super::vdf::{Map, Value};
use crate::util;

/// The key Irori tags its entries with: the game as `<runner>/<app>`.
pub const ID_KEY: &str = "IroriId";

/// The fields Irori writes in its own entries; everything else is the
/// user's or Steam's. The icon is Irori's only while it is Irori's file.
pub const OWN_FIELDS: [&str; 4] = ["Exe", "StartDir", "LaunchOptions", ID_KEY];

/// The game an entry of Irori's starts; `None` for entries that aren't
/// Irori's.
pub fn entry_game(entry: ShortcutEntry<'_>) -> Option<GameId> {
    if let Some(id) = entry.0.get_ci(ID_KEY).and_then(Value::as_str)
        && let Some(game) = parse_target(id)
    {
        return Some(game);
    }
    let options = entry.launch_options();
    let by_irori = entry.exe().to_ascii_lowercase().contains("irori")
        || options.to_ascii_lowercase().contains("irori");
    if !by_irori {
        return None;
    }
    let tokens = split_options(options);
    if let Some(i) = tokens.iter().position(|t| t == "--launch" || t == "-l")
        && let Some(game) = tokens.get(i + 1).and_then(|t| parse_target(t))
    {
        return Some(game);
    }
    tokens.iter().find_map(|t| {
        let rest = t.strip_prefix("irori://launch/")?;
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let (runner, app) = rest.split_once('/')?;
        let app = util::decode_component(app).unwrap_or_else(|| app.to_string());
        parse_target(&format!("{runner}/{app}"))
    })
}

/// `<runner>/<app>` as a game.
fn parse_target(target: &str) -> Option<GameId> {
    let (runner, app) = target.trim().split_once('/')?;
    let runner = Runner::from_id(runner)?;
    (!app.is_empty()).then(|| GameId::new(runner, app))
}

/// Launch options as Irori writes them: words split at blanks, `"…"`
/// quoting with `\"` and `\\` escapes.
fn split_options(options: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = options.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted && matches!(chars.peek(), Some('"' | '\\')) => {
                current.extend(chars.next());
            }
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    out.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(current);
    }
    out
}

/// What [`upsert`] did to one game's entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Added,
    /// Irori's fields of an existing entry changed.
    Updated,
    Unchanged,
}

/// Put the game `id` into `file`: append `wanted` (a complete new entry)
/// when no entry of Irori's starts it, else bring Irori's fields of that
/// entry to `wanted`'s. `icons` is Irori's icon folder: an icon there (or
/// none) is Irori's to replace, any other is the user's.
pub fn upsert(file: &mut ShortcutsFile, id: &GameId, wanted: &Map, icons: &Path) -> Upsert {
    let Some(entry) = file
        .entries_mut()
        .find(|e| entry_game(ShortcutEntry(e)).as_ref() == Some(id))
    else {
        file.push(wanted.clone());
        return Upsert::Added;
    };
    let mut changed = false;
    // Steam names an entry's art after its `appid`; one written before
    // Steam stored it gets the ID Steam computed from the old command, so
    // the art stays when the command changes.
    if entry.get_ci("appid").is_none() {
        let e = ShortcutEntry(entry);
        let id = shortcut_id(e.exe(), e.app_name());
        entry.push("appid", Value::Int32(id));
        changed = true;
    }
    for field in OWN_FIELDS {
        if let Some(value) = wanted.get_ci(field) {
            changed |= set(entry, field, value.clone());
        }
    }
    let icon = entry.str_ci("icon");
    if (icon.is_empty() || Path::new(icon).starts_with(icons))
        && let Some(value) = wanted.get_ci("icon")
    {
        changed |= set(entry, "icon", value.clone());
    }
    if changed {
        Upsert::Updated
    } else {
        Upsert::Unchanged
    }
}

/// Set `key` (matched without case, as Steam's keys vary) to `value`;
/// whether that changed anything.
fn set(map: &mut Map, key: &str, value: Value) -> bool {
    match map.get_ci_mut(key) {
        Some(current) if *current == value => false,
        Some(current) => {
            *current = value;
            true
        }
        None => {
            map.push(key, value);
            true
        }
    }
}

/// Remove Irori's entries whose game `remove` picks; returns them.
pub fn remove_own(file: &mut ShortcutsFile, mut remove: impl FnMut(&GameId) -> bool) -> Vec<Map> {
    file.remove_where(|e| entry_game(e).is_some_and(|id| remove(&id)))
}

/// The games of Irori's entries in `file`.
pub fn own_games(file: &ShortcutsFile) -> HashSet<GameId> {
    file.entries().filter_map(entry_game).collect()
}

/// `shortcuts.vdf.irori-backup` beside `path`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".irori-backup");
    PathBuf::from(name)
}

/// Write `file` to `path`: copy the current file to its backup first, then
/// read what was written back. When it doesn't read back as written, the
/// previous file is put back (or the new one removed) and an error says so.
pub async fn write_checked(path: &Path, file: &ShortcutsFile) -> Result<()> {
    let bytes = file.to_bytes();
    let previous = match tokio::fs::read(path).await {
        Ok(old) => {
            util::write_atomic(&backup_path(path), &old)
                .await
                .context("backing up Steam's shortcuts")?;
            Some(old)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };
    util::write_atomic(path, &bytes).await?;
    verify_or_restore(path, &bytes, previous.as_deref()).await
}

/// Read `path` back; when it isn't `expected` or doesn't parse, put
/// `previous` back (none: remove the file) and fail.
pub(crate) async fn verify_or_restore(
    path: &Path,
    expected: &[u8],
    previous: Option<&[u8]>,
) -> Result<()> {
    let read = tokio::fs::read(path).await;
    if matches!(&read, Ok(b) if b == expected && ShortcutsFile::parse(b).is_ok()) {
        return Ok(());
    }
    let restored = match previous {
        Some(old) => util::write_atomic(path, old).await,
        None => tokio::fs::remove_file(path).await.map_err(Into::into),
    };
    if let Err(e) = restored {
        tracing::warn!("putting back {}: {e:#}", path.display());
    }
    bail!(
        "{} didn't read back as written, so Irori put the previous file back",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::super::shortcuts_file::NewShortcut;
    use super::*;

    fn wanted(id: &GameId, exe: &str, icon: Option<&str>) -> Map {
        NewShortcut {
            app_name: "Hades".into(),
            exe: format!("\"{exe}\""),
            start_dir: "\"/opt\"".into(),
            icon: icon.map(str::to_string),
            launch_options: format!("--launch \"{}/{}\" --wait", id.runner.id(), id.app_name),
            last_play_time: 0,
            tags: vec!["Irori".into()],
            irori_id: Some(format!("{}/{}", id.runner.id(), id.app_name)),
        }
        .to_entry()
    }

    fn foreign() -> Map {
        Map::new()
            .with("AppName", Value::String("Lutris game".into()))
            .with("Exe", Value::String("\"/usr/bin/lutris\"".into()))
            .with(
                "LaunchOptions",
                Value::String("lutris:rungameid/3 --launch \"gog/1\"".into()),
            )
    }

    #[test]
    fn knows_its_entries_new_and_old() {
        let id = GameId::new(Runner::Gog, "1");
        assert_eq!(
            entry_game(ShortcutEntry(&wanted(&id, "/x/irori", None))),
            Some(id.clone())
        );
        // Written by an older Irori, without the tag.
        let old = Map::new()
            .with("Exe", Value::String("\"/usr/bin/irori\"".into()))
            .with("LaunchOptions", Value::String("--launch \"gog/1\"".into()));
        assert_eq!(entry_game(ShortcutEntry(&old)), Some(id.clone()));
        let flatpak = Map::new()
            .with("Exe", Value::String("\"flatpak\"".into()))
            .with(
                "LaunchOptions",
                Value::String("run app.irori.Irori --launch \"sideload/my app\" --wait".into()),
            );
        assert_eq!(
            entry_game(ShortcutEntry(&flatpak)),
            Some(GameId::new(Runner::Sideload, "my app"))
        );
        let link = Map::new()
            .with("Exe", Value::String("\"xdg-open\"".into()))
            .with(
                "LaunchOptions",
                Value::String("\"irori://launch/sideload/my%20app\"".into()),
            );
        assert_eq!(
            entry_game(ShortcutEntry(&link)),
            Some(GameId::new(Runner::Sideload, "my app"))
        );
        // Another program's entry, even one with a lookalike option.
        assert_eq!(entry_game(ShortcutEntry(&foreign())), None);
        let unknown = Map::new()
            .with("Exe", Value::String("\"/usr/bin/irori\"".into()))
            .with("LaunchOptions", Value::String("--launch \"nope/1\"".into()));
        assert_eq!(entry_game(ShortcutEntry(&unknown)), None);
    }

    #[test]
    fn updates_only_its_own_fields_of_its_own_entries() {
        let icons = Path::new("/data/irori/icons");
        let id = GameId::new(Runner::Gog, "1");
        let mut file = ShortcutsFile::empty();
        file.push(foreign());
        // An older Irori's entry the user renamed, filed and re-iconed.
        let mut old = Map::new()
            .with("AppName", Value::String("Hades (mine)".into()))
            .with("Exe", Value::String("\"/old/irori\"".into()))
            .with("StartDir", Value::String("\"/old\"".into()))
            .with("icon", Value::String("/home/me/hades.png".into()))
            .with("LaunchOptions", Value::String("--launch \"gog/1\"".into()))
            .with("AllowOverlay", Value::Int32(0));
        old.push(
            "tags",
            Value::Map(Map::new().with("0", Value::String("Roguelikes".into()))),
        );
        let old_id = shortcut_id("\"/old/irori\"", "Hades (mine)");
        file.push(old);
        let before_foreign = foreign();

        let new = wanted(&id, "/new/irori", Some("/data/irori/icons/gog-1.png"));
        assert_eq!(upsert(&mut file, &id, &new, icons), Upsert::Updated);
        assert_eq!(upsert(&mut file, &id, &new, icons), Upsert::Unchanged);
        let entries: Vec<_> = file.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].0, &before_foreign,
            "another program's entry stays"
        );
        let e = entries[1];
        assert_eq!(e.exe(), "\"/new/irori\"");
        assert_eq!(e.launch_options(), "--launch \"gog/1\" --wait");
        assert_eq!(e.0.str_ci(ID_KEY), "gog/1");
        // The user's name, icon, collections and overlay stay; the art
        // keeps the ID it had.
        assert_eq!(e.app_name(), "Hades (mine)");
        assert_eq!(e.0.str_ci("icon"), "/home/me/hades.png");
        assert_eq!(e.0.get_ci("AllowOverlay"), Some(&Value::Int32(0)));
        assert_eq!(
            e.0.get_ci("tags")
                .and_then(Value::as_map)
                .unwrap()
                .str_ci("0"),
            "Roguelikes"
        );
        assert_eq!(e.app_id(), Some(old_id as u32));

        // A game not there yet is appended whole.
        let other = GameId::new(Runner::Legendary, "Fortnite");
        assert_eq!(
            upsert(
                &mut file,
                &other,
                &wanted(&other, "/new/irori", None),
                icons
            ),
            Upsert::Added
        );
        assert_eq!(own_games(&file).len(), 2);

        // Removing touches Irori's entries only.
        let removed = remove_own(&mut file, |_| true);
        assert_eq!(removed.len(), 2);
        assert_eq!(file.entries().count(), 1);
        assert_eq!(file.entries().next().unwrap().0, &before_foreign);
    }

    #[test]
    fn replaces_its_own_icon_only() {
        let icons = Path::new("/data/irori/icons");
        let id = GameId::new(Runner::Gog, "1");
        let mut file = ShortcutsFile::empty();
        file.push(wanted(&id, "/x/irori", Some("/data/irori/icons/old.png")));
        let new = wanted(&id, "/x/irori", Some("/data/irori/icons/gog-1.png"));
        assert_eq!(upsert(&mut file, &id, &new, icons), Upsert::Updated);
        assert_eq!(
            file.entries().next().unwrap().0.str_ci("icon"),
            "/data/irori/icons/gog-1.png"
        );
    }

    #[tokio::test]
    async fn backs_up_and_reads_every_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shortcuts.vdf");
        let mut first = ShortcutsFile::empty();
        first.push(foreign());
        write_checked(&path, &first).await.unwrap();
        assert!(!backup_path(&path).exists(), "nothing to back up yet");

        let id = GameId::new(Runner::Gog, "1");
        let mut second = first.clone();
        second.push(wanted(&id, "/x/irori", None));
        write_checked(&path, &second).await.unwrap();
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), first.to_bytes());
        assert_eq!(std::fs::read(&path).unwrap(), second.to_bytes());

        // A write that doesn't read back as written puts the old file back.
        std::fs::write(&path, b"garbage").unwrap();
        let err = verify_or_restore(&path, &second.to_bytes(), Some(&first.to_bytes()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("previous file"), "{err:#}");
        assert_eq!(std::fs::read(&path).unwrap(), first.to_bytes());
        // With no previous file, the bad one goes.
        std::fs::write(&path, b"garbage").unwrap();
        assert!(
            verify_or_restore(&path, &second.to_bytes(), None)
                .await
                .is_err()
        );
        assert!(!path.exists());
    }

    #[test]
    fn splits_options_as_irori_writes_them() {
        assert_eq!(
            split_options(r#"run x --launch "gog/a \"b\" c" --wait"#),
            ["run", "x", "--launch", "gog/a \"b\" c", "--wait"]
        );
        assert_eq!(split_options(r#""" x"#), ["", "x"]);
    }
}
