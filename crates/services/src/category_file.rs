//! Categories alone as a file, to share them or carry them to another
//! computer without a whole backup: manual categories with their games,
//! smart categories with their rules, and the looks (pin, colour) of manual
//! ones. Games are named by `GameId::key()`, which is the same on every
//! computer.
//!
//! Importing merges, as restoring a backup does: a manual category gains
//! the file's games, a smart category or look already here stays as it is
//! now, and a name another kind of category already uses here is skipped,
//! since names are unique across both kinds. A whole Irori backup can be
//! imported too: its categories are taken and nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use irori_core::{json_store, util};
use serde::{Deserialize, Serialize};

use crate::{Backend, CategoryLook, SmartCategory, UserData};

/// Marks a file as Irori's categories.
const FORMAT: &str = "irori-categories";
/// Newest layout this version reads and writes.
const VERSION: u32 = 1;

/// The contents of a categories file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CategoriesFile {
    pub format: String,
    pub version: u32,
    /// RFC 3339.
    pub created: String,
    /// Manual categories: name → game keys.
    pub categories: BTreeMap<String, BTreeSet<String>>,
    /// Smart categories by id.
    pub smart_categories: BTreeMap<String, SmartCategory>,
    /// Pins and colours of manual categories.
    pub category_looks: BTreeMap<String, CategoryLook>,
}

/// What an import changed, for telling the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CategoriesImport {
    /// Categories of either kind that weren't here.
    pub added: usize,
    /// Manual categories here that gained games.
    pub extended: usize,
    /// Categories left out because another kind uses their name here.
    pub skipped: usize,
}

/// The user's categories as a file.
pub fn categories_file(user: &UserData) -> CategoriesFile {
    CategoriesFile {
        format: FORMAT.into(),
        version: VERSION,
        created: util::now_rfc3339(),
        categories: user.categories.clone(),
        smart_categories: user.smart_categories.clone(),
        category_looks: user
            .category_looks
            .iter()
            .filter(|(name, _)| user.categories.contains_key(*name))
            .map(|(n, l)| (n.clone(), l.clone()))
            .collect(),
    }
}

/// Read a categories file, or the categories of an Irori backup.
pub fn parse(text: &str) -> Result<CategoriesFile> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("This file is not an Irori categories file")?;
    match value.get("format").and_then(|f| f.as_str()) {
        Some(FORMAT) => {
            let file: CategoriesFile = serde_json::from_value(value)
                .context("This file is not an Irori categories file")?;
            if file.version > VERSION {
                bail!("This file was made by a newer Irori; update Irori to import it");
            }
            Ok(file)
        }
        Some("irori-backup") => {
            let backup: crate::Backup =
                serde_json::from_value(value).context("This Irori backup could not be read")?;
            let Some(library) = backup.library else {
                bail!("This backup holds no categories");
            };
            Ok(CategoriesFile {
                format: FORMAT.into(),
                version: VERSION,
                created: backup.created,
                categories: library.categories,
                smart_categories: library.smart_categories,
                category_looks: library.category_looks,
            })
        }
        _ => bail!("This file is not an Irori categories file"),
    }
}

/// Merge a file's categories into the user's (see the module notes).
pub fn merge(d: &mut UserData, file: &CategoriesFile) -> CategoriesImport {
    let mut result = CategoriesImport::default();
    let smart_names: BTreeSet<String> = d
        .smart_categories
        .values()
        .map(|s| s.name.to_lowercase())
        .collect();
    for (name, games) in &file.categories {
        if smart_names.contains(&name.to_lowercase()) {
            result.skipped += 1;
            continue;
        }
        match d.categories.get_mut(name) {
            Some(here) => {
                let before = here.len();
                here.extend(games.iter().cloned());
                if here.len() > before {
                    result.extended += 1;
                }
            }
            None => {
                d.categories.insert(name.clone(), games.clone());
                result.added += 1;
            }
        }
    }
    for (name, look) in &file.category_looks {
        if d.categories.contains_key(name) {
            d.category_looks
                .entry(name.clone())
                .or_insert_with(|| look.clone());
        }
    }
    for (id, smart) in &file.smart_categories {
        if d.smart_categories.contains_key(id) {
            continue;
        }
        let taken = d
            .categories
            .keys()
            .chain(d.smart_categories.values().map(|s| &s.name))
            .any(|n| n.to_lowercase() == smart.name.to_lowercase());
        if taken {
            result.skipped += 1;
            continue;
        }
        d.smart_categories.insert(id.clone(), smart.clone());
        result.added += 1;
    }
    result
}

impl Backend {
    /// Write the user's categories to `path`; returns how many there are.
    pub fn export_categories(&self, path: &Path) -> Result<usize> {
        let file = categories_file(&self.userdata.snapshot());
        let count = file.categories.len() + file.smart_categories.len();
        let text = serde_json::to_string_pretty(&file)?;
        json_store::write_bytes(path, text.as_bytes())
            .with_context(|| format!("Cannot write {}", path.display()))?;
        Ok(count)
    }

    /// Merge the categories of a categories file or backup at `path`.
    pub fn import_categories(&self, path: &Path) -> Result<CategoriesImport> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        let file = parse(&text)?;
        let mut result = CategoriesImport::default();
        self.userdata.update_if(None, |d| {
            result = merge(d, &file);
            result.added + result.extended > 0
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smart(name: &str) -> SmartCategory {
        serde_json::from_value(serde_json::json!({
            "name": name, "match": "all",
            "rules": [{"rule": "favourite"}], "leaveOut": []
        }))
        .unwrap()
    }

    #[test]
    fn a_file_carries_categories_to_another_computer() {
        let mut here = UserData::default();
        here.categories
            .insert("RPG".into(), ["gog:1".to_string()].into());
        here.categories.insert("Empty".into(), BTreeSet::new());
        here.smart_categories.insert("s-1".into(), smart("Short"));
        here.category_looks.insert(
            "RPG".into(),
            CategoryLook {
                pinned: true,
                ..Default::default()
            },
        );
        // A look left behind by a deleted category stays out of the file.
        here.category_looks
            .insert("Gone".into(), CategoryLook::default());
        here.favourites.insert("gog:1".into());

        let text = serde_json::to_string(&categories_file(&here)).unwrap();
        let file = parse(&text).unwrap();
        assert!(!file.category_looks.contains_key("Gone"));

        let mut there = UserData::default();
        there
            .categories
            .insert("RPG".into(), ["legendary:x".to_string()].into());
        let result = merge(&mut there, &file);
        assert_eq!(
            result,
            CategoriesImport {
                added: 2,
                extended: 1,
                skipped: 0
            }
        );
        assert_eq!(there.categories["RPG"].len(), 2);
        assert!(there.categories["Empty"].is_empty());
        assert_eq!(there.smart_categories["s-1"].name, "Short");
        assert!(there.category_looks["RPG"].pinned);
        // Nothing but categories travels.
        assert!(there.favourites.is_empty());

        // Importing again changes nothing.
        assert_eq!(merge(&mut there, &file), CategoriesImport::default());
    }

    #[test]
    fn names_stay_unique_across_both_kinds() {
        let mut file = CategoriesFile {
            format: FORMAT.into(),
            version: VERSION,
            ..Default::default()
        };
        file.categories.insert("Short".into(), BTreeSet::new());
        file.smart_categories.insert("s-2".into(), smart("rpg"));
        let mut d = UserData::default();
        d.categories.insert("RPG".into(), BTreeSet::new());
        d.smart_categories.insert("s-1".into(), smart("Short"));
        let result = merge(&mut d, &file);
        assert_eq!(result.skipped, 2);
        assert_eq!(result.added, 0);
        assert!(!d.smart_categories.contains_key("s-2"));
    }

    #[test]
    fn reads_the_categories_of_a_backup() {
        let text = serde_json::json!({
            "format": "irori-backup",
            "version": 1,
            "library": {
                "favourites": ["gog:1"],
                "categories": {"RPG": ["gog:1"]}
            }
        })
        .to_string();
        let file = parse(&text).unwrap();
        assert_eq!(file.categories["RPG"].len(), 1);

        let no_library = r#"{"format": "irori-backup", "version": 1}"#;
        assert!(parse(no_library).is_err());
        assert!(parse(r#"{"format": "something-else"}"#).is_err());
        assert!(parse("not json").is_err());
        let newer = r#"{"format": "irori-categories", "version": 99}"#;
        assert!(parse(newer).is_err());
    }
}
