//! Library search that forgives typos.
//!
//! A [`SearchIndex`] folds every game's title, alternative titles (the store
//! title behind a renamed game, the folder name), developer and publisher
//! once per library revision: lower case, accents and punctuation dropped,
//! Roman numerals also read as digits ("III" is "3"), and the initials of
//! each title ("gta" finds "Grand Theft Auto"). A query is folded the same
//! way and each game gets a score; matches rank exact, then prefix, then
//! word, then fuzzy (typos and abbreviations), titles before people.
//!
//! A query may also hold `mode:` terms ([`ParsedQuery`]): `mode:coop`
//! keeps games played in co-op, `mode:local,online` games with either, and
//! several terms must all hold. `tag:` terms work the same way on Steam's
//! user tags (`tag:puzzle`, `tag:open-world,roguelike`; spaces, hyphens and
//! case don't matter). The rest of the query is searched as text.

use std::sync::Arc;

use fuzzy_matcher::FuzzyMatcher as _;
use fuzzy_matcher::skim::SkimMatcherV2;
use irori_core::{GameInfo, GameMode};

/// How well a query matched one field, best last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    /// Every query word is within a typo or two of a word, or the query's
    /// letters appear in order.
    Fuzzy,
    /// Every query word starts a word of the field, in any order (or the
    /// query is a prefix of the initials).
    Word,
    /// The field starts with the query.
    Prefix,
    /// The field is the query.
    Exact,
}

/// One searchable text, folded.
#[derive(Debug, Clone, Default)]
struct Field {
    /// Folded text, words separated by one space.
    text: String,
    /// The text without a leading English article.
    bare: String,
    /// The text with Roman numerals read as digits.
    digits: String,
    /// Each word, with its other spelling (Roman numeral ↔ digits).
    words: Vec<(String, Option<String>)>,
    /// First letter of each word, numbers kept whole ("rdr2").
    initials: String,
}

impl Field {
    fn new(raw: &str) -> Option<Self> {
        let text = fold(raw);
        if text.is_empty() {
            return None;
        }
        let bare = ["the ", "a ", "an "]
            .iter()
            .find_map(|a| text.strip_prefix(a))
            .unwrap_or(&text)
            .to_string();
        let words: Vec<(String, Option<String>)> = text
            .split(' ')
            .enumerate()
            .map(|(ix, w)| (w.to_string(), other_spelling(w, ix)))
            .collect();
        let initials = words
            .iter()
            .filter(|(w, _)| !matches!(w.as_str(), "the" | "a" | "an" | "of" | "and"))
            .map(|(w, alt)| {
                if w.chars().all(|c| c.is_ascii_digit()) {
                    w.clone()
                } else if let Some(digits) = alt.as_ref() {
                    digits.clone()
                } else {
                    w.chars().next().map(String::from).unwrap_or_default()
                }
            })
            .collect();
        let digits = words
            .iter()
            .map(|(w, alt)| match alt {
                Some(a) if a.chars().all(|c| c.is_ascii_digit()) => a.as_str(),
                _ => w.as_str(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        Some(Self {
            text,
            bare,
            digits,
            words,
            initials,
        })
    }

    /// The best tier `query` reaches here, with the typos it took.
    fn matches(&self, query: &Query) -> Option<(MatchTier, u32)> {
        if self.text == query.text || self.bare == query.text || self.digits == query.digits {
            return Some((MatchTier::Exact, 0));
        }
        if self.text.starts_with(&query.text)
            || self.bare.starts_with(&query.text)
            || self.digits.starts_with(&query.digits)
        {
            return Some((MatchTier::Prefix, 0));
        }
        let word_start = |q: &str| {
            self.words
                .iter()
                .any(|(w, alt)| w.starts_with(q) || alt.as_ref().is_some_and(|a| a == q))
        };
        if query.words.iter().all(|q| word_start(q))
            || (query.words.len() == 1
                && query.text.len() >= 2
                && self.words.len() > 1
                && self.initials.starts_with(&query.text))
        {
            return Some((MatchTier::Word, 0));
        }
        let mut typos = 0;
        for q in &query.words {
            let allowed = allowed_typos(q);
            if allowed == 0 {
                return None;
            }
            let best = self
                .words
                .iter()
                .filter_map(|(w, _)| word_distance(q, w, allowed))
                .min()?;
            typos += best;
        }
        Some((MatchTier::Fuzzy, typos))
    }
}

/// One game's folded fields.
#[derive(Debug, Clone, Default)]
struct Entry {
    title: Option<Field>,
    /// The original title, alternative titles and folder name.
    alt: Vec<Field>,
    /// Developer and publisher.
    people: Vec<Field>,
}

/// A folded query, ready to score many games.
#[derive(Debug, Clone)]
pub struct Query {
    text: String,
    /// The text with Roman numerals read as digits.
    digits: String,
    words: Vec<String>,
}

impl Query {
    /// Fold `raw`; `None` when nothing searchable is left.
    pub fn new(raw: &str) -> Option<Self> {
        let text = fold(raw);
        if text.is_empty() {
            return None;
        }
        let words: Vec<String> = text.split(' ').map(str::to_string).collect();
        let digits = words
            .iter()
            .enumerate()
            .map(|(ix, w)| match other_spelling(w, ix) {
                Some(d) if d.chars().all(|c| c.is_ascii_digit()) => d,
                _ => w.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        Some(Self {
            text,
            digits,
            words,
        })
    }
}

/// A query split into its text and its `mode:` terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedQuery {
    /// What is left to search as text.
    pub text: String,
    /// One entry per `mode:` term: the modes it names, any of which a game
    /// must have.
    pub modes: Vec<Vec<GameMode>>,
    /// One entry per `tag:` term: the tags it names ([`tag_key`]), any of
    /// which a game must have.
    pub tags: Vec<Vec<String>>,
}

/// How store tags compare: letters and digits only, lower case, so
/// `open-world` names "Open World".
pub fn tag_key(tag: &str) -> String {
    tag.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl ParsedQuery {
    /// Split `raw`. A `mode:` term (any case) names modes by their ids or
    /// usual spellings ([`GameMode::from_search`]), several separated by
    /// commas; a term naming something that isn't a mode stays text.
    pub fn new(raw: &str) -> Self {
        let mut text: Vec<&str> = Vec::new();
        let mut modes = Vec::new();
        let mut tags = Vec::new();
        for word in raw.split_whitespace() {
            if let Some(rest) = word
                .get(..4)
                .filter(|p| p.eq_ignore_ascii_case("tag:"))
                .map(|_| &word[4..])
            {
                let any: Vec<String> = rest
                    .split(',')
                    .map(tag_key)
                    .filter(|k| !k.is_empty())
                    .collect();
                if !any.is_empty() {
                    tags.push(any);
                    continue;
                }
            }
            let named = word
                .get(..5)
                .filter(|p| p.eq_ignore_ascii_case("mode:"))
                .and_then(|_| {
                    let mut any = Vec::new();
                    for part in word[5..].split(',').filter(|p| !p.is_empty()) {
                        any.extend(GameMode::from_search(part)?);
                    }
                    (!any.is_empty()).then(|| irori_core::game_mode::normalized(any))
                });
            match named {
                Some(any) => modes.push(any),
                None => text.push(word),
            }
        }
        Self {
            text: text.join(" "),
            modes,
            tags,
        }
    }

    /// Whether a game with `store_tags` passes every `tag:` term.
    pub fn tags_match(&self, store_tags: &[String]) -> bool {
        self.tags.is_empty() || {
            let keys: Vec<String> = store_tags.iter().map(|t| tag_key(t)).collect();
            self.tags
                .iter()
                .all(|any| any.iter().any(|k| keys.contains(k)))
        }
    }

    /// Whether a game with `game_modes` passes every `mode:` term.
    pub fn modes_match(&self, game_modes: &[GameMode]) -> bool {
        self.modes
            .iter()
            .all(|any| any.iter().any(|m| game_modes.contains(m)))
    }
}

/// Folded search fields of every game of one library snapshot, in the
/// snapshot's order. Built once per revision ([`crate::LibrarySnapshot::search`]).
#[derive(Debug, Default)]
pub struct SearchIndex {
    entries: Vec<Entry>,
}

impl SearchIndex {
    pub fn build(games: &[Arc<GameInfo>]) -> Self {
        let entries = games
            .iter()
            .map(|g| {
                let mut alt: Vec<Field> =
                    g.alt_titles.iter().filter_map(|t| Field::new(t)).collect();
                if let Some(folder) = &g.folder_name {
                    alt.extend(Field::new(&split_camel(folder)));
                }
                Entry {
                    title: Field::new(&g.title),
                    alt,
                    people: [&g.developer, &g.publisher]
                        .into_iter()
                        .flatten()
                        .filter_map(|p| Field::new(p))
                        .collect(),
                }
            })
            .collect();
        Self { entries }
    }

    /// Score the game at `ix` of the snapshot; `None` when it doesn't match.
    /// Higher is better.
    pub fn score(&self, ix: usize, query: &Query, game: &GameInfo) -> Option<i64> {
        let entry = self.entries.get(ix)?;
        let tier_score = |tier: MatchTier, typos: u32, person: bool| -> i64 {
            let base = match (tier, person) {
                (MatchTier::Exact, false) => 6000,
                (MatchTier::Prefix, false) => 5000,
                (MatchTier::Word, false) => 4000,
                (MatchTier::Fuzzy, false) => 2000,
                (MatchTier::Fuzzy, true) => 1000,
                (_, true) => 3000,
            };
            base - 100 * i64::from(typos)
        };
        let mut best: Option<i64> = None;
        if let Some((tier, typos)) = entry.title.as_ref().and_then(|f| f.matches(query)) {
            take(&mut best, tier_score(tier, typos, false));
        }
        for f in &entry.alt {
            if let Some((tier, typos)) = f.matches(query) {
                take(&mut best, tier_score(tier, typos, false) - 200);
            }
        }
        for f in &entry.people {
            if let Some((tier, typos)) = f.matches(query) {
                take(&mut best, tier_score(tier, typos, true));
            }
        }
        // Letters in order ("wtchr"), for the title only and below every
        // word match.
        if best.is_none_or(|b| b < 1500)
            && query.text.len() >= 3
            && let Some(s) = SKIM.with(|m| m.fuzzy_match(&game.title, &query.text))
            && s >= 2 * query.text.len() as i64
        {
            take(&mut best, 1000 + s.min(400));
        }
        best
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Keep the higher of two scores.
fn take(best: &mut Option<i64>, score: i64) {
    *best = Some(best.map_or(score, |b| b.max(score)));
}

thread_local! {
    static SKIM: SkimMatcherV2 = SkimMatcherV2::default();
}

/// Typos a query word may hold: none for short words (they are prefixes
/// while typing), one from four letters, two from eight.
fn allowed_typos(word: &str) -> u32 {
    match word.chars().count() {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Typos between query word `q` and field word `w`, comparing against the
/// whole word and against its start (a word still being typed), when within
/// `allowed`.
fn word_distance(q: &str, w: &str, allowed: u32) -> Option<u32> {
    let q: Vec<char> = q.chars().collect();
    let w: Vec<char> = w.chars().collect();
    let mut best = None;
    for len in [w.len(), q.len(), q.len() + 1, q.len().saturating_sub(1)] {
        if len == 0 || len > w.len() || (len != w.len() && len < 4) {
            continue;
        }
        if q.len().abs_diff(len) as u32 > allowed {
            continue;
        }
        let d = osa_distance(&q, &w[..len]);
        if d <= allowed && best.is_none_or(|b| d < b) {
            best = Some(d);
        }
    }
    best
}

/// Edit distance counting a swap of two neighbouring letters as one edit
/// (optimal string alignment).
fn osa_distance(a: &[char], b: &[char]) -> u32 {
    let (n, m) = (a.len(), b.len());
    let mut prev2 = vec![0u32; m + 1];
    let mut prev: Vec<u32> = (0..=m as u32).collect();
    let mut cur = vec![0u32; m + 1];
    for i in 1..=n {
        cur[0] = i as u32;
        for j in 1..=m {
            let cost = u32::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// Lower case, accents dropped, `&` read as "and", everything else that is
/// not a letter or digit a word break; words separated by one space.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    let mut push = |c: char, out: &mut String| {
        if c == ' ' {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    };
    for c in s.chars() {
        if c == '&' {
            for c in " and ".chars() {
                push(c, &mut out);
            }
            continue;
        }
        // Apostrophes join a word ("Assassin's" is "assassins").
        if matches!(c, '\'' | '’' | '`') {
            continue;
        }
        for lower in c.to_lowercase() {
            match unaccent(lower) {
                Some(plain) => plain.chars().for_each(|p| push(p, &mut out)),
                None if lower.is_alphanumeric() => push(lower, &mut out),
                None => push(' ', &mut out),
            }
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The plain letters of an accented Latin letter.
fn unaccent(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ı' => "i",
        'ł' | 'ľ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// The other spelling of a sequel number: digits for a Roman numeral and
/// the numeral for digits, from 2 to 20. "I", and a lone "V" or "X" as the
/// first word, are words rather than numbers.
fn other_spelling(word: &str, position: usize) -> Option<String> {
    const ROMAN: [&str; 21] = [
        "", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii",
        "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx",
    ];
    if let Ok(n) = word.parse::<usize>() {
        return (2..ROMAN.len()).contains(&n).then(|| ROMAN[n].to_string());
    }
    let n = ROMAN.iter().position(|r| *r == word)?;
    if n < 2 || (word.len() == 1 && position == 0) {
        return None;
    }
    Some(n.to_string())
}

/// "TheWitcher3" → "The Witcher 3", for folder names.
fn split_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if let Some(p) = prev
            && ((p.is_lowercase() && c.is_uppercase())
                || (p.is_alphabetic() && c.is_ascii_digit())
                || (p.is_ascii_digit() && c.is_alphabetic()))
        {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_terms_match_steam_tags_loosely() {
        let q = ParsedQuery::new("hades TAG:rogue-like tag:co_op,Open-World");
        assert_eq!(q.text, "hades");
        assert_eq!(
            q.tags,
            [
                vec!["roguelike".to_owned()],
                vec!["coop".into(), "openworld".into()]
            ]
        );
        let tags = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(q.tags_match(&tags(&["Roguelike", "Open World"])));
        assert!(!q.tags_match(&tags(&["Roguelike"])));
        assert!(ParsedQuery::new("portal").tags_match(&[]));
        // An empty term stays text.
        assert_eq!(ParsedQuery::new("tag: x").text, "tag: x");
    }

    #[test]
    fn mode_terms_split_from_the_text() {
        let q = ParsedQuery::new("  portal  MODE:co-op mode:local,online ");
        assert_eq!(q.text, "portal");
        assert_eq!(
            q.modes,
            [
                vec![GameMode::Coop],
                vec![GameMode::Local, GameMode::Online]
            ]
        );
        let both = [GameMode::Coop, GameMode::Online];
        assert!(q.modes_match(&both));
        assert!(!q.modes_match(&[GameMode::Coop]));
        assert!(!q.modes_match(&[]));
        // Something that isn't a mode is searched as text.
        let q = ParsedQuery::new("mode:racing mode:");
        assert_eq!(q.text, "mode:racing mode:");
        assert!(q.modes.is_empty());
        assert!(q.modes_match(&[]));
        let q = ParsedQuery::new("mode:multiplayer");
        assert!(q.text.is_empty());
        assert!(q.modes_match(&[GameMode::Mmo]));
        assert!(!q.modes_match(&[GameMode::Single]));
    }

    fn game(title: &str) -> Arc<GameInfo> {
        Arc::new(
            GameInfo {
                app_name: title.into(),
                title: title.into(),
                ..Default::default()
            }
            .finalize(),
        )
    }

    fn rank(games: &[Arc<GameInfo>], q: &str) -> Vec<String> {
        let index = SearchIndex::build(games);
        let query = Query::new(q).unwrap();
        let mut hits: Vec<(i64, &Arc<GameInfo>)> = games
            .iter()
            .enumerate()
            .filter_map(|(ix, g)| Some((index.score(ix, &query, g)?, g)))
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.sort_title.cmp(&b.1.sort_title)));
        hits.into_iter().map(|(_, g)| g.title.clone()).collect()
    }

    #[test]
    fn folds_case_accents_and_punctuation() {
        assert_eq!(
            fold("Pokémon: Légendes — Arceus!"),
            "pokemon legendes arceus"
        );
        assert_eq!(fold("Assassin’s Creed"), "assassins creed");
        assert_eq!(fold("Ratchet & Clank"), "ratchet and clank");
        assert_eq!(fold("  ..  "), "");
    }

    #[test]
    fn ranks_exact_then_prefix_then_word_then_fuzzy() {
        let games = vec![
            game("Portal Knights"),
            game("Portal"),
            game("Portal 2"),
            game("Lost Portal Saga"),
            game("Pirtal Wars"),
            game("Hades"),
        ];
        assert_eq!(
            rank(&games, "portal"),
            [
                "Portal",
                "Portal 2",
                "Portal Knights",
                "Lost Portal Saga",
                "Pirtal Wars"
            ]
        );
    }

    #[test]
    fn forgives_typos_swaps_and_partial_words() {
        let games = vec![
            game("The Witcher 3: Wild Hunt"),
            game("Hollow Knight"),
            game("Cyberpunk 2077"),
        ];
        assert_eq!(rank(&games, "wticher"), ["The Witcher 3: Wild Hunt"]);
        assert_eq!(rank(&games, "holow knight"), ["Hollow Knight"]);
        assert_eq!(rank(&games, "cyberpnuk"), ["Cyberpunk 2077"]);
        assert_eq!(rank(&games, "wtch"), ["The Witcher 3: Wild Hunt"]);
        assert!(rank(&games, "zzzz").is_empty());
        // Short words must be typed right: they are prefixes while typing.
        assert!(rank(&games, "hpl").is_empty());
    }

    #[test]
    fn reads_numerals_and_initials() {
        let games = vec![
            game("Final Fantasy VII"),
            game("Grand Theft Auto V"),
            game("Red Dead Redemption 2"),
            game("Gravity Rush"),
        ];
        assert_eq!(rank(&games, "final fantasy 7"), ["Final Fantasy VII"]);
        assert_eq!(rank(&games, "gta"), ["Grand Theft Auto V"]);
        assert_eq!(rank(&games, "gta5"), ["Grand Theft Auto V"]);
        assert_eq!(rank(&games, "rdr2"), ["Red Dead Redemption 2"]);
        assert_eq!(
            rank(&games, "red dead redemption ii"),
            ["Red Dead Redemption 2"]
        );
    }

    #[test]
    fn searches_alternative_titles_folders_and_people_below_titles() {
        let renamed = Arc::new(
            GameInfo {
                app_name: "a".into(),
                title: "My favourite".into(),
                alt_titles: vec!["Disco Elysium".into()],
                ..Default::default()
            }
            .finalize(),
        );
        let folder = Arc::new(
            GameInfo {
                app_name: "b".into(),
                title: "Unknown app".into(),
                folder_name: Some("StardewValley".into()),
                ..Default::default()
            }
            .finalize(),
        );
        let by_studio = Arc::new(
            GameInfo {
                app_name: "c".into(),
                title: "Hades".into(),
                developer: Some("Supergiant Games".into()),
                ..Default::default()
            }
            .finalize(),
        );
        let games = vec![renamed, folder, by_studio, game("Supergiant Story")];
        assert_eq!(rank(&games, "disco"), ["My favourite"]);
        assert_eq!(rank(&games, "stardew"), ["Unknown app"]);
        assert_eq!(rank(&games, "supergiant"), ["Supergiant Story", "Hades"]);
        assert_eq!(rank(&games, "supregiant"), ["Supergiant Story", "Hades"]);
    }

    #[test]
    fn scores_ten_thousand_games_quickly() {
        let words = [
            "dark", "star", "legend", "knight", "dragon", "city", "racing", "space", "quest",
            "tales", "battle", "shadow",
        ];
        let games: Vec<Arc<GameInfo>> = (0..10_000)
            .map(|i| {
                let title = format!(
                    "{} {} {}",
                    words[i % words.len()],
                    words[(i / 12) % words.len()],
                    i
                );
                Arc::new(
                    GameInfo {
                        app_name: i.to_string(),
                        title,
                        developer: Some(format!("Studio {}", i % 97)),
                        ..Default::default()
                    }
                    .finalize(),
                )
            })
            .collect();
        let built = std::time::Instant::now();
        let index = SearchIndex::build(&games);
        let build_time = built.elapsed();
        let query = Query::new("dragn quets").unwrap();
        let started = std::time::Instant::now();
        let hits = games
            .iter()
            .enumerate()
            .filter(|(ix, g)| index.score(*ix, &query, g).is_some())
            .count();
        let elapsed = started.elapsed();
        assert!(hits > 0);
        // Generous for unoptimized CI builds; release builds take a few
        // milliseconds.
        assert!(elapsed.as_millis() < 1500, "search took {elapsed:?}");
        assert!(build_time.as_millis() < 3000, "index took {build_time:?}");
    }
}
