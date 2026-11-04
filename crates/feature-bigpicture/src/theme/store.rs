//! The user's own themes: one `<id>.irori-theme.json` per theme in the
//! config folder's `bigpicture/themes`, and which theme is in use
//! (`iroriUi.bigPictureTheme`). Ids are made here from theme names and
//! checked before they become file names, so nothing read from a file or a
//! setting can point outside that folder.
//!
//! Every function here touches the disk: call them off the UI thread.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_ui::i18n::t;

use super::{EXTENSION, MAX_FILE_BYTES, Theme, ThemeError, gallery};

/// Longest theme id.
const MAX_ID: usize = 40;

/// Which theme Big Picture uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThemeRef {
    BuiltIn(String),
    User(String),
}

impl ThemeRef {
    /// Read the setting; anything unknown is Irori's own theme.
    pub fn parse(setting: &str) -> ThemeRef {
        match setting.split_once(':') {
            Some(("user", id)) if is_valid_id(id) => ThemeRef::User(id.into()),
            Some(("builtin", id)) if gallery::get(id).is_some() => ThemeRef::BuiltIn(id.into()),
            _ => ThemeRef::BuiltIn(gallery::DEFAULT_ID.into()),
        }
    }

    /// The setting's value.
    pub fn setting(&self) -> String {
        match self {
            ThemeRef::BuiltIn(id) if id == gallery::DEFAULT_ID => String::new(),
            ThemeRef::BuiltIn(id) => format!("builtin:{id}"),
            ThemeRef::User(id) => format!("user:{id}"),
        }
    }
}

/// A theme the user made or imported.
#[derive(Debug, Clone, PartialEq)]
pub struct UserTheme {
    pub id: String,
    pub theme: Theme,
}

/// Whether `id` may name a theme file: lower-case letters, digits and
/// inner dashes.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An id for a theme called `name` that none of `taken` has.
pub fn new_id<'a>(name: &str, taken: impl IntoIterator<Item = &'a str> + Clone) -> String {
    let mut base = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let mut base: String = base
        .trim_end_matches('-')
        .chars()
        .take(MAX_ID - 3)
        .collect();
    base = base.trim_end_matches('-').to_owned();
    if base.is_empty() {
        base = "theme".into();
    }
    let free = |id: &str| !taken.clone().into_iter().any(|t| t == id);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| free(id))
        .expect("some suffix is free")
}

/// Where the user's themes live.
pub fn themes_dir() -> PathBuf {
    irori_core::Paths::get()
        .config
        .join("bigpicture")
        .join("themes")
}

fn file_of(dir: &Path, id: &str) -> Option<PathBuf> {
    is_valid_id(id).then(|| dir.join(format!("{id}{EXTENSION}")))
}

/// Read a theme file of at most [`MAX_FILE_BYTES`].
pub fn read_file(path: &Path) -> Result<Theme> {
    let size = std::fs::metadata(path)
        .with_context(|| t("bigpicture.theme.error.read"))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(ThemeError::TooLarge.into());
    }
    let text = std::fs::read_to_string(path).with_context(|| t("bigpicture.theme.error.read"))?;
    Ok(Theme::from_json(&text)?)
}

/// Every readable theme in `dir`, by name. A file that doesn't read is
/// left out (and logged), so one broken file never hides the others.
pub fn load(dir: &Path) -> Vec<UserTheme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<UserTheme> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(EXTENSION)?;
            if !is_valid_id(id) || !entry.file_type().ok()?.is_file() {
                return None;
            }
            match read_file(&entry.path()) {
                Ok(theme) => Some(UserTheme {
                    id: id.to_owned(),
                    theme,
                }),
                Err(e) => {
                    tracing::warn!("Big Picture theme {name}: {e:#}");
                    None
                }
            }
        })
        .collect();
    out.sort_by(|a, b| {
        a.theme
            .name
            .to_lowercase()
            .cmp(&b.theme.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Write `theme` as the user's theme `id`.
pub fn save(dir: &Path, id: &str, theme: &Theme) -> Result<()> {
    let path = file_of(dir, id).context("invalid theme id")?;
    irori_core::json_store::write_bytes(&path, theme.to_json().as_bytes())
        .with_context(|| t("bigpicture.theme.error.save"))
}

/// Remove the user's theme `id`: its one file, nothing else.
pub fn delete(dir: &Path, id: &str) -> Result<()> {
    let path = file_of(dir, id).context("invalid theme id")?;
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| t("bigpicture.theme.error.delete"))
        }
        _ => Ok(()),
    }
}

/// Read the theme file at `path` and keep a copy among the user's themes,
/// under a new id made from its name.
pub fn import(dir: &Path, path: &Path) -> Result<UserTheme> {
    let theme = read_file(path)?;
    let taken: Vec<String> = load(dir).into_iter().map(|u| u.id).collect();
    let id = new_id(&theme.name, taken.iter().map(String::as_str));
    save(dir, &id, &theme)?;
    Ok(UserTheme { id, theme })
}

/// Write `theme` to `path`, adding the theme ending when it is missing.
/// Returns where it was written.
pub fn export(theme: &Theme, path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let path = if name.ends_with(EXTENSION) {
        path.to_path_buf()
    } else {
        let stem = name.strip_suffix(".json").unwrap_or(name);
        path.with_file_name(format!("{stem}{EXTENSION}"))
    };
    irori_core::json_store::write_bytes(&path, theme.to_json().as_bytes())
        .with_context(|| t("bigpicture.theme.error.save"))?;
    Ok(path)
}

/// The name to suggest when exporting `theme`.
pub fn export_name(theme: &Theme) -> String {
    format!("{}{EXTENSION}", new_id(&theme.name, std::iter::empty()))
}
