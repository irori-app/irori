//! Finding a ROM's game in a system's index, and ordering its boxes by the
//! ROM's region.
//!
//! Titles compare folded: lower case, accents dropped, `&` as "and", a
//! leading "The" dropped and everything that isn't a letter or digit
//! removed, so "Legend of Zelda, The - A Link to the Past" (a No-Intro name,
//! tidied) and "The Legend of Zelda: A Link to the Past" are one game. Only
//! when that finds nothing are Roman numerals read as digits ("Doom II" and
//! "Doom 2"), since "Mega Man X" and "Mega Man 10" are different games.

use std::collections::HashMap;

use super::{BoxArt, LaunchBoxGame, SystemIndex};

/// What a title is compared by: plain, and with Roman numerals as digits.
fn folds(title: &str) -> (String, String) {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() {
            words.push(std::mem::take(word));
        }
    };
    for c in title.chars() {
        if c == '&' {
            flush(&mut word, &mut words);
            words.push("and".into());
            continue;
        }
        // Apostrophes join a word ("Assassin's" is "assassins").
        if matches!(c, '\'' | '’' | '`') {
            continue;
        }
        for lower in c.to_lowercase() {
            match unaccent(lower) {
                Some(plain) => word.push_str(plain),
                None if lower.is_alphanumeric() => word.push(lower),
                None => flush(&mut word, &mut words),
            }
        }
    }
    flush(&mut word, &mut words);
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    let plain = words.concat();
    let numbers = words
        .iter()
        .map(|w| roman(w).unwrap_or_else(|| w.clone()))
        .collect::<Vec<_>>()
        .concat();
    (plain, numbers)
}

/// The digits of a Roman numeral from 2 to 20. A lone "V" or "X" stays a
/// letter: "Mega Man X" is not "Mega Man 10".
fn roman(word: &str) -> Option<String> {
    const ROMAN: [&str; 21] = [
        "", "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii",
        "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx",
    ];
    let n = ROMAN.iter().position(|r| *r == word)?;
    (n >= 2 && word.len() > 1).then(|| n.to_string())
}

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

/// A system's games by folded title, built once per system and pass.
pub struct Lookup<'a> {
    index: &'a SystemIndex,
    /// Folded title → games, own names before alternate ones.
    plain: HashMap<String, Vec<(usize, bool)>>,
    numbers: HashMap<String, Vec<(usize, bool)>>,
}

impl<'a> Lookup<'a> {
    pub fn new(index: &'a SystemIndex) -> Self {
        let mut plain: HashMap<String, Vec<(usize, bool)>> = HashMap::new();
        let mut numbers: HashMap<String, Vec<(usize, bool)>> = HashMap::new();
        for (ix, game) in index.games.iter().enumerate() {
            let names = std::iter::once((game.name.as_str(), false))
                .chain(game.alternate.iter().map(|a| (a.as_str(), true)));
            for (name, alternate) in names {
                let (p, n) = folds(name);
                if p.is_empty() {
                    continue;
                }
                plain.entry(p).or_default().push((ix, alternate));
                numbers.entry(n).or_default().push((ix, alternate));
            }
        }
        Self {
            index,
            plain,
            numbers,
        }
    }

    /// The game titled `title`: by its own name before an alternate one,
    /// and of several, the one with the most boxes. `None` rather than a
    /// near miss.
    pub fn find(&self, title: &str) -> Option<&'a LaunchBoxGame> {
        let (p, n) = folds(title);
        if p.is_empty() {
            return None;
        }
        let pick = |found: &Vec<(usize, bool)>| {
            found
                .iter()
                .min_by_key(|(ix, alternate)| {
                    (*alternate, usize::MAX - self.index.games[*ix].boxes.len())
                })
                .map(|(ix, _)| &self.index.games[*ix])
        };
        self.plain
            .get(&p)
            .and_then(pick)
            .or_else(|| self.numbers.get(&n).and_then(pick))
    }
}

impl SystemIndex {
    /// The game titled `title`, as [`Lookup::find`] finds it.
    pub fn find(&self, title: &str) -> Option<&LaunchBoxGame> {
        Lookup::new(self).find(title)
    }

    /// Games whose name holds `query` (folded), names equal to it first,
    /// then those starting with it; at most `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&LaunchBoxGame> {
        let (wanted, _) = folds(query);
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(u8, &LaunchBoxGame)> = self
            .games
            .iter()
            .filter_map(|game| {
                std::iter::once(&game.name)
                    .chain(&game.alternate)
                    .filter_map(|name| {
                        let (folded, _) = folds(name);
                        if folded == wanted {
                            Some(0)
                        } else if folded.starts_with(&wanted) {
                            Some(1)
                        } else if folded.contains(&wanted) {
                            Some(2)
                        } else {
                            None
                        }
                    })
                    .min()
                    .map(|rank| (rank, game))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
        found.truncate(limit);
        found.into_iter().map(|(_, g)| g).collect()
    }
}

/// LaunchBox's regions for a region No-Intro and Redump names use, closest
/// first.
fn launchbox_regions(region: &str) -> &'static [&'static str] {
    match region.to_ascii_lowercase().as_str() {
        "world" => &["World"],
        "usa" => &["North America", "United States"],
        "canada" => &["Canada", "North America"],
        "europe" => &["Europe"],
        "uk" => &["United Kingdom", "Europe"],
        "germany" => &["Germany", "Europe"],
        "france" => &["France", "Europe"],
        "spain" => &["Spain", "Europe"],
        "italy" => &["Italy", "Europe"],
        "netherlands" => &["The Netherlands", "Europe"],
        "sweden" => &["Sweden", "Europe"],
        "scandinavia" => &["Sweden", "Norway", "Finland", "Europe"],
        "russia" => &["Russia", "Europe"],
        "australia" => &["Australia", "Oceania", "Europe"],
        "japan" => &["Japan"],
        "asia" => &["Asia"],
        "korea" => &["Korea", "Asia"],
        "china" => &["China", "Asia"],
        "hong kong" => &["Hong Kong", "China", "Asia"],
        "taiwan" => &["Taiwan", "Asia"],
        "brazil" => &["Brazil", "South America"],
        "latin america" => &["South America", "North America"],
        _ => &[],
    }
}

/// The regions a ROM's badges (`USA, Europe`, `Rev 1`) ask for, best first.
/// Without a region, a box for the whole world comes first, then the
/// commonest releases.
pub fn wanted_regions(tags: &[String]) -> Vec<&'static str> {
    let mut wanted: Vec<&'static str> = Vec::new();
    for part in tags.iter().flat_map(|t| t.split(',')) {
        for region in launchbox_regions(part.trim()) {
            if !wanted.contains(region) {
                wanted.push(region);
            }
        }
    }
    if wanted.is_empty() {
        wanted = vec!["World", "North America", "United States", "Europe", "Japan"];
    }
    wanted
}

/// `boxes` in the order a ROM with `tags` wants them: its own regions'
/// boxes first (scans before reconstructions), then boxes without a region
/// or for the whole world, then the rest as LaunchBox lists them.
pub fn rank<'a>(boxes: &'a [BoxArt], tags: &[String]) -> Vec<&'a BoxArt> {
    let wanted = wanted_regions(tags);
    let mut ranked: Vec<(usize, bool, usize, &BoxArt)> = boxes
        .iter()
        .enumerate()
        .map(|(ix, art)| {
            let place = match art.region.as_deref() {
                Some(r) => match wanted.iter().position(|w| w.eq_ignore_ascii_case(r)) {
                    Some(p) => p,
                    None if r.eq_ignore_ascii_case("World") => wanted.len(),
                    None => wanted.len() + 1,
                },
                None => wanted.len(),
            };
            (place, art.reconstructed, ix, art)
        })
        .collect();
    ranked.sort_by_key(|(place, reconstructed, ix, _)| (*place, *reconstructed, *ix));
    ranked.into_iter().map(|(.., art)| art).collect()
}

/// The box a ROM with `tags` shows as its cover.
pub fn best<'a>(boxes: &'a [BoxArt], tags: &[String]) -> Option<&'a BoxArt> {
    rank(boxes, tags).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: u32, name: &str, alternate: &[&str], boxes: usize) -> LaunchBoxGame {
        LaunchBoxGame {
            id,
            name: name.into(),
            alternate: alternate.iter().map(|a| a.to_string()).collect(),
            boxes: (0..boxes)
                .map(|i| BoxArt {
                    file: format!("{id}-{i}.png"),
                    region: None,
                    reconstructed: false,
                })
                .collect(),
        }
    }

    fn boxed(file: &str, region: Option<&str>, reconstructed: bool) -> BoxArt {
        BoxArt {
            file: file.into(),
            region: region.map(str::to_string),
            reconstructed,
        }
    }

    #[test]
    fn folded_titles_match() {
        let index = SystemIndex {
            games: vec![
                game(1, "The Legend of Zelda: A Link to the Past", &[], 1),
                game(2, "Pokémon Red Version", &[], 1),
                game(3, "Mega Man X", &["Rockman X"], 1),
                game(4, "Doom II: Hell on Earth", &[], 1),
                game(5, "Sonic & Knuckles", &[], 1),
            ],
        };
        let lookup = Lookup::new(&index);
        let id = |t: &str| lookup.find(t).map(|g| g.id);
        assert_eq!(id("The Legend of Zelda - A Link to the Past"), Some(1));
        assert_eq!(id("Legend of Zelda - A Link to the Past"), Some(1));
        assert_eq!(id("Pokemon - Red Version"), Some(2));
        assert_eq!(id("Rockman X"), Some(3));
        assert_eq!(id("Doom 2 - Hell on Earth"), Some(4));
        assert_eq!(id("Sonic and Knuckles"), Some(5));
        assert_eq!(id("Mega Man 10"), None);
        assert_eq!(id("Zelda"), None);
        assert_eq!(id(""), None);
    }

    #[test]
    fn own_names_win_over_alternate_ones_then_more_boxes() {
        let index = SystemIndex {
            games: vec![
                game(1, "Tetris (Homebrew)", &["Tetris"], 9),
                game(2, "Tetris", &[], 1),
                game(3, "Tetris", &[], 3),
            ],
        };
        assert_eq!(index.find("Tetris").map(|g| g.id), Some(3));
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_contains() {
        let index = SystemIndex {
            games: vec![
                game(1, "Super Mario Bros. 3", &[], 1),
                game(2, "Mario Bros.", &[], 1),
                game(3, "Mario Bros. Special", &[], 1),
            ],
        };
        let ids: Vec<u32> = index
            .search("mario bros", 10)
            .iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(ids, [2, 3, 1]);
        assert_eq!(index.search("mario bros", 1).len(), 1);
        assert!(index.search("  ", 10).is_empty());
    }

    #[test]
    fn boxes_follow_the_roms_region() {
        let boxes = vec![
            boxed("jp.png", Some("Japan"), false),
            boxed("eu-rec.png", Some("Europe"), true),
            boxed("none.png", None, false),
            boxed("eu.png", Some("Europe"), false),
            boxed("us.png", Some("North America"), false),
            boxed("de.png", Some("Germany"), false),
        ];
        let order = |tags: &[&str]| {
            let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
            rank(&boxes, &tags)
                .into_iter()
                .map(|b| b.file.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            order(&["Europe", "Rev 1"]),
            [
                "eu.png",
                "eu-rec.png",
                "none.png",
                "jp.png",
                "us.png",
                "de.png"
            ]
        );
        assert_eq!(
            order(&["USA, Europe"]),
            [
                "us.png",
                "eu.png",
                "eu-rec.png",
                "none.png",
                "jp.png",
                "de.png"
            ]
        );
        assert_eq!(order(&["Germany"])[..2], ["de.png", "eu.png"]);
        assert_eq!(order(&["Japan"])[0], "jp.png");
        // No region: the commonest release first.
        assert_eq!(order(&[])[0], "us.png");
        assert_eq!(best(&[], &[]), None);
    }
}
