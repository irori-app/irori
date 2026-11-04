//! The user's play journal: a note per game per day played, with an
//! optional title and links to screenshots taken that day. Notes live in
//! `UserData::journal` (`state/library.json`) and travel with backups.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Notes of every game: `GameId::key()` → local calendar day → note.
pub type Journal = BTreeMap<String, BTreeMap<NaiveDate, JournalEntry>>;

/// Most screenshots one note links to.
pub const MAX_LINKED_SCREENSHOTS: usize = 12;

/// One day's note on one game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JournalEntry {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub text: String,
    /// Paths of the screenshots of that day the note shows, in the order the
    /// user picked them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<String>,
    /// RFC 3339 time the note was last written.
    pub updated: String,
}

impl JournalEntry {
    /// Whether the note holds nothing worth keeping.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.text.trim().is_empty() && self.screenshots.is_empty()
    }

    /// The note as it is kept: title and text trimmed, screenshots without
    /// duplicates and at most [`MAX_LINKED_SCREENSHOTS`], dated `now`.
    pub fn normalized(mut self, now: &str) -> Self {
        self.title = self.title.trim().to_string();
        self.text = self.text.trim().to_string();
        let mut seen = std::collections::HashSet::new();
        self.screenshots
            .retain(|p| !p.is_empty() && seen.insert(p.clone()));
        self.screenshots.truncate(MAX_LINKED_SCREENSHOTS);
        self.updated = now.to_string();
        self
    }
}

/// Write (or, with `None` or an empty note, remove) the note of `key` on
/// `day`, dated `now`.
pub(crate) fn apply(
    journal: &mut Journal,
    key: &str,
    day: NaiveDate,
    entry: Option<JournalEntry>,
    now: &str,
) {
    match entry.map(|e| e.normalized(now)).filter(|e| !e.is_empty()) {
        Some(entry) => {
            journal
                .entry(key.to_string())
                .or_default()
                .insert(day, entry);
        }
        None => {
            if let Some(days) = journal.get_mut(key) {
                days.remove(&day);
                if days.is_empty() {
                    journal.remove(key);
                }
            }
        }
    }
}

/// Add a backup's notes: a day's note from the backup is taken when the
/// current one is missing or was written earlier.
pub(crate) fn merge(journal: &mut Journal, backup: &Journal) {
    for (key, days) in backup {
        for (day, entry) in days {
            if entry.is_empty() {
                continue;
            }
            let current = journal.entry(key.clone()).or_default();
            let newer = current
                .get(day)
                .is_none_or(|c| crate::backup::later(&entry.updated, &c.updated));
            if newer {
                current.insert(*day, entry.clone());
            }
        }
    }
    journal.retain(|_, days| !days.is_empty());
}

/// Every note, newest day first, then by game key.
pub fn all(journal: &Journal) -> Vec<(&str, NaiveDate, &JournalEntry)> {
    let mut list: Vec<_> = journal
        .iter()
        .flat_map(|(key, days)| days.iter().map(move |(d, e)| (key.as_str(), *d, e)))
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn note(text: &str, updated: &str) -> JournalEntry {
        JournalEntry {
            text: text.into(),
            updated: updated.into(),
            ..Default::default()
        }
    }

    #[test]
    fn writing_trims_dedupes_and_dates_and_empty_notes_are_removed() {
        let mut j = Journal::new();
        let entry = JournalEntry {
            title: "  Boss down ".into(),
            text: "\nFinally beat the knight.  ".into(),
            screenshots: vec!["/a.png".into(), "/a.png".into(), String::new()],
            updated: String::new(),
        };
        apply(&mut j, "gog:1", day("2026-11-02"), Some(entry), "now");
        let kept = &j["gog:1"][&day("2026-11-02")];
        assert_eq!(kept.title, "Boss down");
        assert_eq!(kept.text, "Finally beat the knight.");
        assert_eq!(kept.screenshots, ["/a.png"]);
        assert_eq!(kept.updated, "now");

        // Clearing everything removes the note and the game's empty map.
        apply(
            &mut j,
            "gog:1",
            day("2026-11-02"),
            Some(JournalEntry {
                text: "   ".into(),
                ..Default::default()
            }),
            "later",
        );
        assert!(j.is_empty());
        apply(&mut j, "gog:1", day("2026-11-02"), None, "later");
        assert!(j.is_empty());
    }

    #[test]
    fn linked_screenshots_are_capped() {
        let entry = JournalEntry {
            screenshots: (0..40).map(|i| format!("/{i}.png")).collect(),
            ..Default::default()
        }
        .normalized("now");
        assert_eq!(entry.screenshots.len(), MAX_LINKED_SCREENSHOTS);
        assert_eq!(entry.screenshots[0], "/0.png");
    }

    #[test]
    fn restore_keeps_the_later_note_of_each_day() {
        let mut j = Journal::new();
        apply(
            &mut j,
            "gog:1",
            day("2026-11-01"),
            Some(note("current", "")),
            "2026-11-03T10:00:00Z",
        );
        apply(
            &mut j,
            "gog:1",
            day("2026-11-02"),
            Some(note("current", "")),
            "2026-11-03T10:00:00Z",
        );
        let mut backup = Journal::new();
        backup.entry("gog:1".into()).or_default().extend([
            (day("2026-11-01"), note("older", "2026-11-01T10:00:00Z")),
            (day("2026-11-02"), note("newer", "2026-11-04T10:00:00Z")),
            (day("2026-10-30"), note("only here", "2026-10-30T10:00:00Z")),
        ]);
        backup
            .entry("gog:2".into())
            .or_default()
            .insert(day("2026-10-30"), JournalEntry::default());
        merge(&mut j, &backup);
        let days = &j["gog:1"];
        assert_eq!(days[&day("2026-11-01")].text, "current");
        assert_eq!(days[&day("2026-11-02")].text, "newer");
        assert_eq!(days[&day("2026-10-30")].text, "only here");
        // Empty notes in a backup are not restored.
        assert!(!j.contains_key("gog:2"));
    }

    #[test]
    fn notes_list_newest_first_and_read_back_from_older_files() {
        let mut j = Journal::new();
        apply(&mut j, "gog:2", day("2026-10-01"), Some(note("a", "")), "t");
        apply(&mut j, "gog:1", day("2026-11-01"), Some(note("b", "")), "t");
        apply(&mut j, "gog:1", day("2026-10-01"), Some(note("c", "")), "t");
        let order: Vec<_> = all(&j)
            .into_iter()
            .map(|(_, _, e)| e.text.as_str())
            .collect();
        assert_eq!(order, ["b", "c", "a"]);

        let text = serde_json::to_string(&j).unwrap();
        assert!(text.contains(r#""2026-11-01":{"text":"b""#), "{text}");
        let back: Journal = serde_json::from_str(&text).unwrap();
        assert_eq!(back, j);
        // A note written by a later version with fields this one lacks.
        let newer: Journal =
            serde_json::from_str(r#"{"gog:1":{"2026-11-01":{"text":"x","mood":3}}}"#).unwrap();
        assert_eq!(newer["gog:1"][&day("2026-11-01")].text, "x");
    }
}
