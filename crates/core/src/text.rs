//! Folding game titles for comparison: the one place that decides which
//! letters count as the same, so the library's search and every metadata
//! matcher (HowLongToBeat, IGDB, PCGamingWiki, anticheat, SteamGridDB,
//! LaunchBox) agree that "Pokémon" is "Pokemon".
//!
//! Latin and Greek accents are dropped; letters of other scripts (Cyrillic,
//! Japanese, Korean, Chinese…) stay as they are, so those titles still
//! match each other instead of folding to nothing. Trademark signs and
//! emoji variation selectors are dropped without splitting a word.

/// The plain letters of an accented Latin or Greek lower-case letter;
/// `None` for any other character.
pub fn unaccent(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        // Greek: tonos and dialytika, and the final sigma.
        'ά' => "α",
        'έ' => "ε",
        'ή' => "η",
        'ί' | 'ϊ' | 'ΐ' => "ι",
        'ό' => "ο",
        'ύ' | 'ϋ' | 'ΰ' => "υ",
        'ώ' => "ω",
        'ς' => "σ",
        _ => return None,
    })
}

/// Marks that say nothing about which game a title names, dropped without
/// splitting the word around them: ™, ®, ©, emoji variation selectors and
/// zero-width joiners.
pub fn is_ignorable(c: char) -> bool {
    matches!(
        c,
        '™' | '®' | '©' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
    )
}

/// `c` in lower case with Latin and Greek accents dropped, ignorable marks
/// left out, each output character passed to `push`.
pub fn fold_char(c: char, mut push: impl FnMut(char)) {
    if is_ignorable(c) {
        return;
    }
    for lower in c.to_lowercase() {
        match unaccent(lower) {
            Some(plain) => plain.chars().for_each(&mut push),
            None => push(lower),
        }
    }
}

/// A title's letters and digits only, folded with [`fold_char`]: "Pokémon™
/// Legends: Arceus" and "Pokemon Legends Arceus" share one key. Empty when
/// the title has neither; callers must never treat two empty keys as a match.
pub fn title_key(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        fold_char(c, |f| {
            if f.is_alphanumeric() {
                out.push(f);
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_latin_and_greek_accents() {
        assert_eq!(
            title_key("Pokémon™ Légendes: Arceus"),
            "pokemonlegendesarceus"
        );
        assert_eq!(
            title_key("Pokemon Legendes Arceus"),
            "pokemonlegendesarceus"
        );
        assert_eq!(title_key("Ōkami HD"), "okamihd");
        assert_eq!(title_key("Straße"), "strasse");
        assert_eq!(title_key("Οδύσσεια"), "οδυσσεια");
        assert_eq!(title_key("Οδύσσεια"), title_key("ΟΔΥΣΣΕΙΑ"));
        assert_eq!(title_key("Ὀδυσσεύς"), title_key("Ὀδυσσευσ"));
    }

    #[test]
    fn keeps_other_scripts() {
        assert_eq!(
            title_key("ファイナルファンタジー"),
            "ファイナルファンタジー"
        );
        assert_eq!(title_key("Мор (Утопия)"), "морутопия");
        assert_eq!(
            title_key("Ёлка"),
            "ёлка",
            "Cyrillic letters are not Latin accents"
        );
        assert_eq!(title_key("젤다의 전설"), "젤다의전설");
    }

    #[test]
    fn drops_marks_without_splitting_words() {
        assert_eq!(title_key("Star\u{FE0F}Wars®"), "starwars");
        assert_eq!(title_key("™ ®"), "");
        let mut folded = String::new();
        "A™b".chars().for_each(|c| fold_char(c, |f| folded.push(f)));
        assert_eq!(folded, "ab");
    }
}
