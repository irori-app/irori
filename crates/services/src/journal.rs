//! The user's play journal: notes on each game, several a day, each at the
//! time of day it is about (a note about the whole day has none). A note may
//! be about one capture (a screenshot or clip) and link others taken that
//! day; a moment marked in game ("Mark this moment") is a note waiting for
//! words. Notes live in `UserData::journal` (`state/library.json`), travel
//! with backups and sync (one register per note, see `sync::merge`), and are
//! found by [`search`].
//!
//! Irori before several notes a day kept one note per day under the
//! library file's `journal` key. Those notes are read from there and become
//! whole-day notes ([`migrate`]); the new shape is written under
//! `journal_notes`, so an older Irori still reads the rest of the file.

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};

/// Notes of every game: `GameId::key()` → local calendar day → that day's
/// notes, in [`order`].
pub type Journal = BTreeMap<String, BTreeMap<NaiveDate, Vec<JournalEntry>>>;

/// The journal as Irori kept it before several notes a day: one note per
/// game and day.
pub type LegacyJournal = BTreeMap<String, BTreeMap<NaiveDate, JournalEntry>>;

/// Most screenshots one note links to.
pub const MAX_LINKED_SCREENSHOTS: usize = 12;

/// Prefix of the ids notes from the one-note-a-day journal get, followed by
/// their day, so the same old note gets the same id on every computer.
const DAY_NOTE_ID: &str = "day-";

fn is_false(b: &bool) -> bool {
    !*b
}

/// One note on one game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JournalEntry {
    /// Names the note among its game's notes, through edits and on every
    /// computer. Given when the note is first written.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The local time of day the note is about; `None` for a note about the
    /// whole day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<NaiveTime>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub text: String,
    /// Paths of the screenshots of that day the note shows, in the order the
    /// user picked them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<String>,
    /// The one screenshot or clip the note is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
    /// Marked in game to be written later.
    #[serde(skip_serializing_if = "is_false")]
    pub marked: bool,
    /// RFC 3339 time the note was last written.
    pub updated: String,
}

impl JournalEntry {
    /// Whether the note holds nothing worth keeping. A marked moment is
    /// kept until it is deleted, words or not.
    pub fn is_empty(&self) -> bool {
        self.title.trim().is_empty()
            && self.text.trim().is_empty()
            && self.screenshots.is_empty()
            && self.capture.is_none()
            && !self.marked
    }

    /// A marked moment that has no words yet.
    pub fn is_waiting(&self) -> bool {
        self.marked && self.title.trim().is_empty() && self.text.trim().is_empty()
    }

    /// The note as it is kept: title and text trimmed, screenshots without
    /// duplicates (or the capture) and at most [`MAX_LINKED_SCREENSHOTS`],
    /// dated `now`.
    pub fn normalized(mut self, now: &str) -> Self {
        self.title = self.title.trim().to_string();
        self.text = self.text.trim().to_string();
        self.capture = self.capture.filter(|c| !c.is_empty());
        let mut seen = std::collections::HashSet::new();
        if let Some(capture) = &self.capture {
            seen.insert(capture.clone());
        }
        self.screenshots
            .retain(|p| !p.is_empty() && seen.insert(p.clone()));
        self.screenshots.truncate(MAX_LINKED_SCREENSHOTS);
        self.updated = now.to_string();
        self
    }

    /// Whether every word of `query` (folded, see [`crate::search::fold`])
    /// is in the note's title or text.
    pub fn matches(&self, folded_query: &str) -> bool {
        if folded_query.is_empty() {
            return true;
        }
        let haystack = crate::search::fold(&format!("{} {}", self.title, self.text));
        folded_query
            .split(' ')
            .filter(|w| !w.is_empty())
            .all(|w| haystack.contains(w))
    }

    /// Every picture the note shows: its capture, then the linked ones.
    pub fn pictures(&self) -> impl Iterator<Item = &String> {
        self.capture.iter().chain(&self.screenshots)
    }
}

/// Most characters a "Where I left off" note keeps.
pub const MAX_LEFT_OFF: usize = 280;

/// Where the user left off in a game: one short note, shown under its
/// status, replaced as they go.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeftOff {
    pub text: String,
    /// RFC 3339 time it was written.
    pub updated: String,
}

impl LeftOff {
    /// `text` as it is kept, dated `now`: trimmed and cut to
    /// [`MAX_LEFT_OFF`] characters; `None` when nothing is left.
    pub fn new(text: &str, now: &str) -> Option<Self> {
        let text: String = text.trim().chars().take(MAX_LEFT_OFF).collect();
        let text = text.trim_end().to_string();
        (!text.is_empty()).then(|| Self {
            text,
            updated: now.to_string(),
        })
    }
}

/// A fresh id for a new note.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The order of a day's notes: the whole-day note first, then by time, then
/// by when they were written.
fn order(a: &JournalEntry, b: &JournalEntry) -> std::cmp::Ordering {
    a.time
        .cmp(&b.time)
        .then_with(|| a.updated.cmp(&b.updated))
        .then_with(|| a.id.cmp(&b.id))
}

/// Take a note out of every day of `days`; returns the day it was on.
fn take_out(days: &mut BTreeMap<NaiveDate, Vec<JournalEntry>>, id: &str) -> Option<NaiveDate> {
    let mut found = None;
    for (day, notes) in days.iter_mut() {
        let before = notes.len();
        notes.retain(|n| n.id != id);
        if notes.len() != before {
            found = Some(*day);
        }
    }
    days.retain(|_, notes| !notes.is_empty());
    found
}

/// Write `entry` among the notes of `key` on `day`, dated `now`: it
/// replaces the note with its id (on any day), and gets a new id when it
/// has none. A note with nothing in it is removed instead. Returns the
/// note's id.
pub(crate) fn put(
    journal: &mut Journal,
    key: &str,
    day: NaiveDate,
    entry: JournalEntry,
    now: &str,
) -> String {
    let mut entry = entry.normalized(now);
    if entry.id.is_empty() {
        entry.id = new_id();
    }
    let id = entry.id.clone();
    let days = journal.entry(key.to_string()).or_default();
    take_out(days, &id);
    if !entry.is_empty() {
        let notes = days.entry(day).or_default();
        notes.push(entry);
        notes.sort_by(order);
    }
    if days.is_empty() {
        journal.remove(key);
    }
    id
}

/// Remove the note `id` of `key`; returns it with the day it was on.
pub(crate) fn remove(
    journal: &mut Journal,
    key: &str,
    id: &str,
) -> Option<(NaiveDate, JournalEntry)> {
    let days = journal.get_mut(key)?;
    let found = days
        .iter()
        .find_map(|(day, notes)| notes.iter().find(|n| n.id == id).map(|n| (*day, n.clone())));
    take_out(days, id);
    if days.is_empty() {
        journal.remove(key);
    }
    found
}

/// Bring one-note-a-day notes into `journal` as whole-day notes. A note
/// already there (by its id, from an earlier migration or another
/// computer) is kept.
pub(crate) fn migrate(journal: &mut Journal, legacy: LegacyJournal) {
    for (key, days) in legacy {
        for (day, mut entry) in days {
            if entry.is_empty() {
                continue;
            }
            entry.id = format!("{DAY_NOTE_ID}{day}");
            entry.time = None;
            let days = journal.entry(key.clone()).or_default();
            let known = days.values().flatten().any(|n| n.id == entry.id);
            if !known {
                let notes = days.entry(day).or_default();
                notes.push(entry);
                notes.sort_by(order);
            }
        }
    }
}

/// Add a backup's notes: a note from the backup is taken when the current
/// one of its id is missing or was written earlier.
pub(crate) fn merge(journal: &mut Journal, backup: &Journal) {
    for (key, days) in backup {
        for (day, notes) in days {
            for entry in notes {
                if entry.is_empty() || entry.id.is_empty() {
                    continue;
                }
                let current = journal
                    .get(key)
                    .and_then(|days| days.values().flatten().find(|n| n.id == entry.id));
                let newer =
                    current.is_none_or(|c| crate::backup::later(&entry.updated, &c.updated));
                if newer {
                    let days = journal.entry(key.clone()).or_default();
                    take_out(days, &entry.id);
                    let list = days.entry(*day).or_default();
                    list.push(entry.clone());
                    list.sort_by(order);
                }
            }
        }
    }
    journal.retain(|_, days| !days.is_empty());
}

/// Move the notes of `from` to `to` (one game that got another library
/// entry); notes `to` already has (by id) stay as they are.
pub(crate) fn carry_over(journal: &mut Journal, from: &str, to: &str) {
    let Some(days) = journal.remove(from) else {
        return;
    };
    let mine = journal.entry(to.to_string()).or_default();
    for (day, notes) in days {
        for note in notes {
            if !mine.values().flatten().any(|n| n.id == note.id) {
                let list = mine.entry(day).or_default();
                list.push(note);
                list.sort_by(order);
            }
        }
    }
}

/// A journal holding `notes` (game key, day, note), each day in its order.
pub(crate) fn from_notes(
    notes: impl IntoIterator<Item = (String, NaiveDate, JournalEntry)>,
) -> Journal {
    let mut journal = Journal::new();
    for (key, day, note) in notes {
        journal
            .entry(key)
            .or_default()
            .entry(day)
            .or_default()
            .push(note);
    }
    for notes in journal.values_mut().flat_map(|days| days.values_mut()) {
        notes.sort_by(order);
    }
    journal
}

/// The day's whole-day note to link pictures in (a clip just saved), or a
/// new one.
pub fn day_note(notes: &[JournalEntry]) -> JournalEntry {
    notes
        .iter()
        .find(|n| n.time.is_none() && n.capture.is_none() && !n.marked)
        .cloned()
        .unwrap_or_default()
}

/// Every note, newest day first, then by game key, then in the day's order.
pub fn all(journal: &Journal) -> Vec<(&str, NaiveDate, &JournalEntry)> {
    let mut list: Vec<_> = journal
        .iter()
        .flat_map(|(key, days)| {
            days.iter()
                .flat_map(move |(d, notes)| notes.iter().map(move |n| (key.as_str(), *d, n)))
        })
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)).then(order(a.2, b.2)));
    list
}

/// The notes of one game whose words hold every word of `query` (accents
/// and case ignored), newest day first. An empty query finds them all.
pub fn search<'a>(
    days: &'a BTreeMap<NaiveDate, Vec<JournalEntry>>,
    query: &str,
) -> Vec<(NaiveDate, &'a JournalEntry)> {
    let folded = crate::search::fold(query);
    days.iter()
        .rev()
        .flat_map(|(d, notes)| notes.iter().map(move |n| (*d, n)))
        .filter(|(_, n)| n.matches(&folded))
        .collect()
}

/// The newest note of one game, with its day.
pub fn latest(days: &BTreeMap<NaiveDate, Vec<JournalEntry>>) -> Option<(NaiveDate, &JournalEntry)> {
    days.iter()
        .next_back()
        .and_then(|(d, notes)| notes.last().map(|n| (*d, n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn at(s: &str) -> Option<NaiveTime> {
        Some(NaiveTime::parse_from_str(s, "%H:%M").unwrap())
    }

    fn note(text: &str, updated: &str) -> JournalEntry {
        JournalEntry {
            text: text.into(),
            updated: updated.into(),
            ..Default::default()
        }
    }

    fn texts(notes: &[JournalEntry]) -> Vec<&str> {
        notes.iter().map(|n| n.text.as_str()).collect()
    }

    #[test]
    fn writing_trims_dedupes_and_dates_and_empty_notes_are_removed() {
        let mut j = Journal::new();
        let entry = JournalEntry {
            title: "  Boss down ".into(),
            text: "\nFinally beat the knight.  ".into(),
            screenshots: vec![
                "/a.png".into(),
                "/a.png".into(),
                String::new(),
                "/c.png".into(),
            ],
            capture: Some("/c.png".into()),
            ..Default::default()
        };
        let id = put(&mut j, "gog:1", day("2026-11-02"), entry, "now");
        assert!(!id.is_empty());
        let kept = &j["gog:1"][&day("2026-11-02")][0];
        assert_eq!(kept.id, id);
        assert_eq!(kept.title, "Boss down");
        assert_eq!(kept.text, "Finally beat the knight.");
        // The capture is not linked twice.
        assert_eq!(kept.screenshots, ["/a.png"]);
        assert_eq!(kept.updated, "now");

        // Clearing everything removes the note and the game's empty map.
        let cleared = JournalEntry {
            id: id.clone(),
            text: "   ".into(),
            ..Default::default()
        };
        put(&mut j, "gog:1", day("2026-11-02"), cleared, "later");
        assert!(j.is_empty());
        assert!(remove(&mut j, "gog:1", &id).is_none());
    }

    #[test]
    fn a_day_holds_several_notes_in_time_order_and_edits_keep_their_place() {
        let mut j = Journal::new();
        let d = day("2026-11-02");
        let evening = put(
            &mut j,
            "gog:1",
            d,
            JournalEntry {
                time: at("21:10"),
                ..note("evening", "")
            },
            "t1",
        );
        put(&mut j, "gog:1", d, note("whole day", ""), "t2");
        let morning = put(
            &mut j,
            "gog:1",
            d,
            JournalEntry {
                time: at("09:30"),
                ..note("morning", "")
            },
            "t3",
        );
        assert_eq!(texts(&j["gog:1"][&d]), ["whole day", "morning", "evening"]);
        // An edit replaces the note of its id, wherever it was.
        put(
            &mut j,
            "gog:1",
            d,
            JournalEntry {
                id: evening.clone(),
                time: at("21:10"),
                ..note("evening, edited", "")
            },
            "t4",
        );
        assert_eq!(j["gog:1"][&d].len(), 3);
        assert_eq!(j["gog:1"][&d][2].text, "evening, edited");
        let (on, removed) = remove(&mut j, "gog:1", &morning).unwrap();
        assert_eq!((on, removed.text.as_str()), (d, "morning"));
        assert_eq!(texts(&j["gog:1"][&d]), ["whole day", "evening, edited"]);
    }

    #[test]
    fn a_marked_moment_is_kept_without_words_and_waits_for_them() {
        let mut j = Journal::new();
        let moment = JournalEntry {
            time: at("20:00"),
            marked: true,
            ..Default::default()
        };
        assert!(!moment.is_empty() && moment.is_waiting());
        let id = put(&mut j, "gog:1", day("2026-11-02"), moment, "t");
        let kept = &j["gog:1"][&day("2026-11-02")][0];
        assert!(kept.is_waiting());
        let written = JournalEntry {
            text: "The bridge".into(),
            ..kept.clone()
        };
        put(&mut j, "gog:1", day("2026-11-02"), written, "t2");
        let kept = &j["gog:1"][&day("2026-11-02")][0];
        assert_eq!(kept.id, id);
        assert!(kept.marked && !kept.is_waiting());
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
    fn one_note_a_day_journals_become_whole_day_notes_once() {
        let legacy: LegacyJournal = serde_json::from_str(
            r#"{"gog:1":{"2026-11-01":{"title":"Start","text":"Found the map","screenshots":["/a.png"],"updated":"2026-11-01T20:00:00Z"},"2026-11-02":{"text":"  "}}}"#,
        )
        .unwrap();
        let mut j = Journal::new();
        migrate(&mut j, legacy.clone());
        let notes = &j["gog:1"][&day("2026-11-01")];
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "day-2026-11-01");
        assert_eq!(notes[0].time, None);
        assert_eq!(notes[0].title, "Start");
        assert_eq!(notes[0].text, "Found the map");
        assert_eq!(notes[0].screenshots, ["/a.png"]);
        assert_eq!(notes[0].updated, "2026-11-01T20:00:00Z");
        // Empty old notes are dropped.
        assert!(!j["gog:1"].contains_key(&day("2026-11-02")));
        // Reading the old notes again (an old backup) adds nothing twice,
        // and keeps the note as edited since.
        j.get_mut("gog:1")
            .unwrap()
            .get_mut(&day("2026-11-01"))
            .unwrap()[0]
            .text = "Edited".into();
        migrate(&mut j, legacy);
        assert_eq!(texts(&j["gog:1"][&day("2026-11-01")]), ["Edited"]);
    }

    #[test]
    fn restore_keeps_the_later_version_of_each_note() {
        let mut j = Journal::new();
        let a = put(
            &mut j,
            "gog:1",
            day("2026-11-01"),
            note("current", ""),
            "2026-11-03T10:00:00Z",
        );
        let b = put(
            &mut j,
            "gog:1",
            day("2026-11-02"),
            note("current", ""),
            "2026-11-03T10:00:00Z",
        );
        let mut backup = Journal::new();
        backup.entry("gog:1".into()).or_default().extend([
            (
                day("2026-11-01"),
                vec![JournalEntry {
                    id: a,
                    ..note("older", "2026-11-01T10:00:00Z")
                }],
            ),
            (
                day("2026-11-02"),
                vec![JournalEntry {
                    id: b,
                    ..note("newer", "2026-11-04T10:00:00Z")
                }],
            ),
            (
                day("2026-10-30"),
                vec![JournalEntry {
                    id: "x".into(),
                    ..note("only here", "2026-10-30T10:00:00Z")
                }],
            ),
        ]);
        backup.entry("gog:2".into()).or_default().insert(
            day("2026-10-30"),
            vec![JournalEntry {
                id: "y".into(),
                ..Default::default()
            }],
        );
        merge(&mut j, &backup);
        let days = &j["gog:1"];
        assert_eq!(texts(&days[&day("2026-11-01")]), ["current"]);
        assert_eq!(texts(&days[&day("2026-11-02")]), ["newer"]);
        assert_eq!(texts(&days[&day("2026-10-30")]), ["only here"]);
        // Empty notes in a backup are not restored.
        assert!(!j.contains_key("gog:2"));
    }

    #[test]
    fn search_ignores_case_and_accents_and_needs_every_word() {
        let mut j = Journal::new();
        put(
            &mut j,
            "gog:1",
            day("2026-11-01"),
            note("Le Château", ""),
            "t",
        );
        put(
            &mut j,
            "gog:1",
            day("2026-11-02"),
            JournalEntry {
                title: "Boss".into(),
                ..note("the bridge troll", "")
            },
            "t",
        );
        let days = &j["gog:1"];
        let found: Vec<_> = search(days, "chateau")
            .iter()
            .map(|(_, n)| n.text.clone())
            .collect();
        assert_eq!(found, ["Le Château"]);
        assert_eq!(search(days, "boss TROLL").len(), 1);
        assert!(search(days, "boss castle").is_empty());
        // Everything, newest day first.
        let all: Vec<_> = search(days, "").iter().map(|(d, _)| *d).collect();
        assert_eq!(all, [day("2026-11-02"), day("2026-11-01")]);
        assert_eq!(latest(days).unwrap().1.text, "the bridge troll");
    }

    #[test]
    fn notes_list_newest_first_and_read_back_with_unknown_fields() {
        let mut j = Journal::new();
        put(&mut j, "gog:2", day("2026-10-01"), note("a", ""), "t");
        put(&mut j, "gog:1", day("2026-11-01"), note("b", ""), "t");
        put(&mut j, "gog:1", day("2026-10-01"), note("c", ""), "t");
        let order: Vec<_> = all(&j)
            .into_iter()
            .map(|(_, _, e)| e.text.as_str())
            .collect();
        assert_eq!(order, ["b", "c", "a"]);

        let text = serde_json::to_string(&j).unwrap();
        let back: Journal = serde_json::from_str(&text).unwrap();
        assert_eq!(back, j);
        // A note written by a later version with fields this one lacks.
        let newer: Journal =
            serde_json::from_str(r#"{"gog:1":{"2026-11-01":[{"text":"x","mood":3}]}}"#).unwrap();
        assert_eq!(newer["gog:1"][&day("2026-11-01")][0].text, "x");
    }

    #[test]
    fn the_days_note_for_a_clip_is_the_whole_day_note() {
        let notes = vec![
            JournalEntry {
                id: "m".into(),
                time: at("10:00"),
                marked: true,
                ..Default::default()
            },
            JournalEntry {
                id: "d".into(),
                ..note("day", "")
            },
        ];
        assert_eq!(day_note(&notes).id, "d");
        assert_eq!(day_note(&notes[..1]), JournalEntry::default());
    }

    #[test]
    fn where_i_left_off_is_trimmed_and_cut() {
        assert_eq!(LeftOff::new("   ", "t"), None);
        let kept = LeftOff::new("  Before the bridge  ", "t").unwrap();
        assert_eq!(
            (kept.text.as_str(), kept.updated.as_str()),
            ("Before the bridge", "t")
        );
        let long = "é".repeat(400);
        assert_eq!(
            LeftOff::new(&long, "t").unwrap().text.chars().count(),
            MAX_LEFT_OFF
        );
    }

    #[test]
    fn carrying_over_keeps_notes_of_both() {
        let mut j = Journal::new();
        put(
            &mut j,
            "emulation:a",
            day("2026-11-01"),
            note("rom", ""),
            "t",
        );
        put(&mut j, "sideload:b", day("2026-11-01"), note("pc", ""), "t");
        carry_over(&mut j, "emulation:a", "sideload:b");
        assert!(!j.contains_key("emulation:a"));
        assert_eq!(j["sideload:b"][&day("2026-11-01")].len(), 2);
    }
}
