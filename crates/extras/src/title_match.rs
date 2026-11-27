//! Match-confidence rules for automatic game title linking (Steam and IGDB).
//!
//! Two titles describe the same game ([`same_title`]) when:
//! - their folded words are equal, or
//! - their folded words match once trailing edition words are removed on either
//!   side (`"Hades"` and `"Hades - Deluxe Edition"`), or
//! - the longer title starts with the shorter title followed immediately by a
//!   subtitle separator (`:`, `-`, `–`, `—`, `(`) and does not begin with a
//!   sequel marker (digit, Roman numeral, `"zero"`) or contain an add-on word
//!   (`"soundtrack"`, `"dlc"`, `"demo"`, `"artbook"`, `"pack"`, `"season"`, ...).
//!
//! When both sides know a release year, [`same_game`] requires them to be at
//! most [`YEAR_SLACK`] year apart (platforms and regions often release a year
//! apart, while remakes and reboots are further apart).

/// Maximum difference in release year allowed for two records of the same game.
pub const YEAR_SLACK: u32 = 1;

/// Words that may precede `"edition"` in an edition suffix.
pub const EDITION_WORDS: &[&str] = &[
    "game",
    "of",
    "the",
    "year",
    "goty",
    "deluxe",
    "digital",
    "definitive",
    "complete",
    "gold",
    "ultimate",
    "enhanced",
    "special",
    "standard",
    "premium",
    "collectors",
    "anniversary",
    "legendary",
    "extended",
    "platinum",
    "remastered",
    "remake",
    "hd",
];

/// A word after a shared base title that marks a sequel or separate entry.
fn is_sequel_word(word: &str) -> bool {
    (!word.is_empty() && word.chars().all(|c| c.is_ascii_digit()))
        || matches!(
            word,
            "ii" | "iii"
                | "iv"
                | "v"
                | "vi"
                | "vii"
                | "viii"
                | "ix"
                | "x"
                | "xi"
                | "xii"
                | "xiii"
                | "zero"
        )
}

/// Words that mark companion content or add-ons rather than the base game.
fn is_addon_word(word: &str) -> bool {
    matches!(
        word,
        "soundtrack"
            | "ost"
            | "dlc"
            | "pack"
            | "bundle"
            | "demo"
            | "toolkit"
            | "bonus"
            | "skin"
            | "skins"
            | "expansion"
            | "season"
            | "update"
            | "artbook"
            | "costume"
            | "mod"
            | "pass"
            | "beta"
            | "playtest"
    )
}

fn is_subtitle_separator(sep: &str) -> bool {
    sep.contains(':')
        || sep.contains('-')
        || sep.contains('–')
        || sep.contains('—')
        || sep.contains('(')
}

fn fold_roman(word: &str) -> Option<&'static str> {
    match word {
        "ii" => Some("2"),
        "iii" => Some("3"),
        "iv" => Some("4"),
        "v" => Some("5"),
        "vi" => Some("6"),
        "vii" => Some("7"),
        "viii" => Some("8"),
        "ix" => Some("9"),
        "x" => Some("10"),
        "xi" => Some("11"),
        "xii" => Some("12"),
        "xiii" => Some("13"),
        _ => None,
    }
}

/// Fold a title into normalized words: lowercase letters and digits, `&` as
/// `"and"`, apostrophes joined (`"Assassin's"` -> `"assassins"`), trademark
/// marks stripped, and Roman numerals `II` through `XIII` mapped to digits.
pub fn fold_words(title: &str) -> Vec<String> {
    let title = title.replace('&', " and ").replace(['™', '®', '©'], "");
    title
        .replace(['\'', '’', '`'], "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            fold_roman(&lower).map(str::to_owned).unwrap_or(lower)
        })
        .collect()
}

/// Strip a trailing edition label from folded words (`"… Deluxe Edition"`,
/// `"… Game of the Year Edition"`, `"… GOTY"`, `"… Game of the Year"`,
/// `"… Director's Cut"`). Never strips the entire title.
pub fn without_edition(words: &[String]) -> &[String] {
    let ends = |tail: &[&str]| {
        words.len() > tail.len() && words[words.len() - tail.len()..].iter().eq(tail.iter())
    };
    if ends(&["game", "of", "the", "year"]) {
        return &words[..words.len() - 4];
    }
    if ends(&["directors", "cut"]) {
        return &words[..words.len() - 2];
    }
    if ends(&["goty"]) {
        return &words[..words.len() - 1];
    }
    if ends(&["edition"]) {
        let mut end = words.len() - 1;
        while end > 1 && EDITION_WORDS.contains(&words[end - 1].as_str()) {
            end -= 1;
        }
        return &words[..end];
    }
    words
}

/// Check whether `raw` has a subtitle punctuation mark (`:`, `-`, `–`, `—`, `(`)
/// between word `prefix_words - 1` and word `prefix_words`.
fn has_subtitle_mark_after(raw: &str, prefix_words: usize) -> bool {
    if prefix_words == 0 {
        return false;
    }
    let cleaned = raw.replace('&', " and ").replace(['™', '®', '©'], "");
    let cleaned = cleaned.replace(['\'', '’', '`'], "");
    let mut count = 0usize;
    let mut in_word = false;
    let mut boundary_start = None;
    for (idx, ch) in cleaned.char_indices() {
        let alnum = ch.is_alphanumeric();
        if alnum && !in_word {
            if let Some(start) = boundary_start {
                return is_subtitle_separator(&cleaned[start..idx]);
            }
            in_word = true;
            count += 1;
        } else if !alnum && in_word {
            in_word = false;
            if count == prefix_words {
                boundary_start = Some(idx);
            }
        }
    }
    false
}

/// Whether `a` and `b` refer to the same game title (ignoring case,
/// punctuation, Roman numerals, edition suffixes, or a clean subtitle, while
/// rejecting sequels, add-ons, and bare-word extensions).
pub fn same_title(a: &str, b: &str) -> bool {
    let wa = fold_words(a);
    let wb = fold_words(b);
    if wa.is_empty() || wb.is_empty() {
        return false;
    }
    if wa == wb {
        return true;
    }
    if without_edition(&wa) == without_edition(&wb) {
        return true;
    }
    let ((short, _), (long, long_raw)) = if wa.len() <= wb.len() {
        ((&wa, a), (&wb, b))
    } else {
        ((&wb, b), (&wa, a))
    };
    if long.len() <= short.len() || &long[..short.len()] != short.as_slice() {
        return false;
    }
    let rest = &long[short.len()..];
    if rest
        .iter()
        .all(|w| EDITION_WORDS.contains(&w.as_str()) || w == "edition")
    {
        return true;
    }
    if rest.iter().any(|w| is_addon_word(w)) {
        return false;
    }
    if rest.first().is_some_and(|w| is_sequel_word(w)) {
        return false;
    }
    has_subtitle_mark_after(long_raw, short.len())
}

/// Whether two title/year pairs refer to the same game.
pub fn same_game(a_title: &str, a_year: Option<u32>, b_title: &str, b_year: Option<u32>) -> bool {
    let years_agree = match (a_year, b_year) {
        (Some(a), Some(b)) => a.abs_diff(b) <= YEAR_SLACK,
        _ => true,
    };
    years_agree && same_title(a_title, b_title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_punctuation_trademarks_and_roman_numerals() {
        assert_eq!(
            fold_words("DOOM II: Hell on Earth"),
            ["doom", "2", "hell", "on", "earth"]
        );
        assert_eq!(
            fold_words("Assassin’s Creed® & Co.™"),
            ["assassins", "creed", "and", "co"]
        );
        assert_eq!(fold_words("Final Fantasy VII"), ["final", "fantasy", "7"]);
    }

    #[test]
    fn accepts_editions_and_subtitles_and_rejects_sequels_and_addons() {
        assert!(same_title("Portal 2", "portal-2"));
        assert!(same_title("The Witcher 3", "The Witcher 3: Wild Hunt"));
        assert!(same_title(
            "Baldur's Gate 3",
            "Baldur's Gate 3: Deluxe Edition"
        ));
        assert!(same_title("Tomb Raider", "Tomb Raider Game of the Year"));
        assert!(same_title(
            "Castlevania - Symphony of the Night",
            "Castlevania: Symphony of the Night"
        ));
        assert!(same_title("Hades", "Hades - Deluxe Edition"));
        assert!(same_title(
            "The Elder Scrolls V: Skyrim Anniversary Edition",
            "The Elder Scrolls V: Skyrim Special Edition"
        ));

        // Sequels and different entries
        assert!(!same_title("Portal", "Portal 2"));
        assert!(!same_title("Hades", "Hades II"));
        assert!(!same_title("Final Fantasy", "Final Fantasy VII"));
        assert!(!same_title("Resident Evil", "Resident Evil: Zero"));

        // Add-ons and companion entries
        assert!(!same_title("Hades", "Hades Soundtrack"));
        assert!(!same_title("Hades", "Hades - Original Soundtrack"));
        assert!(!same_title(
            "The Witcher 3: Wild Hunt",
            "The Witcher 3: Wild Hunt - Blood and Wine DLC"
        ));
        assert!(!same_title("Hollow Knight", "Hollow Knight: Demo"));
        assert!(!same_title("Celeste", "Celeste: Artbook"));
        assert!(!same_title(
            "eFootball PES 2021",
            "eFootball PES 2021 Season Update"
        ));
        assert!(!same_title(
            "Batman Arkham Knight",
            "Batman: Arkham Knight - Batman Inc. Skin"
        ));
        assert!(!same_title("Control", "Control: Season Pass"));
        assert!(!same_title("Game", "Game - Character Pack"));

        // Unrelated extensions without subtitle mark at boundary
        assert!(!same_title("Portal", "Portal Knights"));
        assert!(!same_title("Hades Battle", "Hades: Battle Out of Hell"));
        assert!(!same_title(
            "NieR:Automata",
            "Untitled NieR:Automata Project"
        ));
        assert!(!same_title("", ""));
    }

    #[test]
    fn year_slack_distinguishes_remakes() {
        assert!(same_game(
            "Tomb Raider",
            Some(2013),
            "Tomb Raider",
            Some(2013)
        ));
        assert!(same_game(
            "Tomb Raider",
            Some(2013),
            "Tomb Raider",
            Some(2013 + YEAR_SLACK)
        ));
        assert!(!same_game(
            "Tomb Raider",
            Some(1996),
            "Tomb Raider",
            Some(2013)
        ));
        assert!(same_game("Tomb Raider", None, "Tomb Raider", Some(2013)));
    }
}
