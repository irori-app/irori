//! Age ratings (ESRB, PEGI, USK, CERO…) as the stores give them: the
//! rating's own label, its content descriptors in the source's words, and
//! the youngest age it allows, so ratings of different systems compare.
//! Also which system fits the user's region, for the one a page shows.
//!
//! Rating names are never translated: "PEGI 16" and "Mature 17+" read the
//! same in every language, and descriptors stay as the source wrote them.

use serde::{Deserialize, Serialize};

/// A rating board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RatingSystem {
    /// North America.
    #[serde(rename = "ESRB")]
    Esrb,
    /// Europe.
    #[serde(rename = "PEGI")]
    Pegi,
    /// Germany.
    #[serde(rename = "USK")]
    Usk,
    /// Japan.
    #[serde(rename = "CERO")]
    Cero,
    /// Brazil (DJCTQ's "Classificação Indicativa").
    #[serde(rename = "ClassInd")]
    ClassInd,
    /// South Korea.
    #[serde(rename = "GRAC")]
    Grac,
    /// Australia.
    #[serde(rename = "ACB")]
    Acb,
    /// Taiwan.
    #[serde(rename = "CSRR")]
    Csrr,
    /// GOG's own age suggestion, region-free.
    #[serde(rename = "GOG")]
    Gog,
    /// A system written by a later Irori; kept so the file still reads.
    #[serde(other)]
    Unknown,
}

/// Every system Irori knows, in the order used when the user's own region
/// has no rating: the widely used boards first.
const FALLBACK_ORDER: [RatingSystem; 9] = [
    RatingSystem::Pegi,
    RatingSystem::Esrb,
    RatingSystem::Usk,
    RatingSystem::Acb,
    RatingSystem::Cero,
    RatingSystem::ClassInd,
    RatingSystem::Grac,
    RatingSystem::Csrr,
    RatingSystem::Gog,
];

impl RatingSystem {
    /// The board's own name, as printed on boxes.
    pub fn name(self) -> &'static str {
        match self {
            RatingSystem::Esrb => "ESRB",
            RatingSystem::Pegi => "PEGI",
            RatingSystem::Usk => "USK",
            RatingSystem::Cero => "CERO",
            RatingSystem::ClassInd => "ClassInd",
            RatingSystem::Grac => "GRAC",
            RatingSystem::Acb => "ACB",
            RatingSystem::Csrr => "CSRR",
            RatingSystem::Gog => "GOG",
            RatingSystem::Unknown => "",
        }
    }

    /// The system a store's key names (`esrb`, Steam's `dejus`, `oflc` and
    /// `kgrb`, GOG's `brRating`…), case and a `Rating` suffix ignored.
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.to_ascii_lowercase();
        let key = key.strip_suffix("rating").unwrap_or(&key);
        Some(match key {
            "esrb" => RatingSystem::Esrb,
            "pegi" => RatingSystem::Pegi,
            "usk" => RatingSystem::Usk,
            "cero" => RatingSystem::Cero,
            "dejus" | "djctq" | "classind" | "br" => RatingSystem::ClassInd,
            "kgrb" | "grac" | "kr" => RatingSystem::Grac,
            "oflc" | "acb" | "au" => RatingSystem::Acb,
            "csrr" | "gsrr" | "tw" => RatingSystem::Csrr,
            "gog" => RatingSystem::Gog,
            _ => return None,
        })
    }

    /// The youngest age a rating of this system allows. `None` for a rating
    /// that says nothing about age (ESRB's "Rating Pending").
    pub fn age_of(self, rating: &str) -> Option<u8> {
        let code = code(self, rating);
        let digits = || -> Option<u8> {
            let d: String = code.chars().filter(char::is_ascii_digit).collect();
            d.parse::<u8>().ok().filter(|a| *a <= 21)
        };
        match self {
            RatingSystem::Esrb => match code.as_str() {
                "EC" => Some(3),
                "E" => Some(6),
                "E10" => Some(10),
                "T" => Some(13),
                "M" => Some(17),
                "AO" => Some(18),
                _ => None,
            },
            RatingSystem::Cero => match code.as_str() {
                "A" => Some(0),
                "B" => Some(12),
                "C" => Some(15),
                "D" => Some(17),
                "Z" => Some(18),
                _ => None,
            },
            RatingSystem::Acb => match code.as_str() {
                "E" | "G" => Some(0),
                "PG" => Some(8),
                "M" | "MA15" => Some(15),
                // Refused classification: not for sale at any age.
                "R18" | "X18" | "RC" => Some(18),
                _ => None,
            },
            RatingSystem::Csrr => match code.as_str() {
                "G" => Some(0),
                "P" => Some(6),
                "PG12" => Some(12),
                "PG15" => Some(15),
                "R" => Some(18),
                _ => digits(),
            },
            RatingSystem::ClassInd => match code.as_str() {
                "L" | "ER" => Some(0),
                _ => digits(),
            },
            RatingSystem::Grac => match code.as_str() {
                "ALL" => Some(0),
                _ => digits(),
            },
            RatingSystem::Pegi | RatingSystem::Usk | RatingSystem::Gog | RatingSystem::Unknown => {
                digits()
            }
        }
    }

    /// The label the board prints ("E10+", "MA 15+", "16"), for a rating
    /// written as a code (Steam's `e10`, `ma15`) or as the board's words
    /// (GOG's "Mature 17+"). A rating this doesn't recognise is kept as
    /// the source wrote it.
    pub fn label(self, rating: &str) -> String {
        let code = code(self, rating);
        let known = match self {
            RatingSystem::Esrb => match code.as_str() {
                "EC" => Some("EC"),
                "E" => Some("E"),
                "E10" => Some("E10+"),
                "T" => Some("T"),
                "M" => Some("M"),
                "AO" => Some("AO"),
                "RP" => Some("RP"),
                _ => None,
            },
            RatingSystem::Acb => match code.as_str() {
                "E" => Some("E"),
                "G" => Some("G"),
                "PG" => Some("PG"),
                "M" => Some("M"),
                "MA15" => Some("MA 15+"),
                "R18" => Some("R 18+"),
                "X18" => Some("X 18+"),
                "RC" => Some("RC"),
                _ => None,
            },
            RatingSystem::Cero => match code.as_str() {
                c @ ("A" | "B" | "C" | "D" | "Z") => Some(c),
                _ => None,
            },
            RatingSystem::Csrr => match code.as_str() {
                "G" => Some("0+"),
                "P" => Some("6+"),
                "PG12" => Some("12+"),
                "PG15" => Some("15+"),
                "R" => Some("18+"),
                _ => None,
            },
            RatingSystem::ClassInd => match code.as_str() {
                "L" | "ER" => Some("L"),
                _ => None,
            },
            RatingSystem::Grac => match code.as_str() {
                "ALL" => Some("ALL"),
                _ => None,
            },
            _ => None,
        };
        if let Some(label) = known {
            return label.to_string();
        }
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()) {
            return code;
        }
        rating.trim().to_string()
    }
}

/// A rating reduced to letters and digits in capitals, without the board's
/// own name or the words around ESRB's and ACB's codes, so `e10`,
/// "Everyone 10+", `EVERYONE_10_PLUS` and "E10+" are one code.
fn code(system: RatingSystem, rating: &str) -> String {
    let mut code: String = rating
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_uppercase();
    for prefix in [system.name().to_ascii_uppercase().as_str(), "AGES", "AGE"] {
        if !prefix.is_empty()
            && let Some(rest) = code.strip_prefix(prefix)
            && !rest.is_empty()
        {
            code = rest.to_string();
        }
    }
    let code = code
        .strip_suffix("PLUS")
        .map(str::to_string)
        .unwrap_or(code);
    match system {
        RatingSystem::Esrb => match code.as_str() {
            "EARLYCHILDHOOD" => "EC",
            "EVERYONE" => "E",
            "EVERYONE10" | "EVERYONETEN" | "E10" => "E10",
            "TEEN" => "T",
            "MATURE" | "MATURE17" | "M17" => "M",
            "ADULTSONLY" | "ADULTSONLY18" | "AO18" => "AO",
            "RATINGPENDING" | "PENDING" => "RP",
            other => other,
        }
        .to_string(),
        RatingSystem::Acb => match code.as_str() {
            "GENERAL" => "G",
            "PARENTALGUIDANCE" => "PG",
            "MATURE" => "M",
            "MATURE15" | "MA15" => "MA15",
            "RESTRICTED18" | "R18" => "R18",
            "REFUSEDCLASSIFICATION" => "RC",
            other => other,
        }
        .to_string(),
        RatingSystem::Grac => match code.as_str() {
            "ALLAGES" | "ALL" => "ALL".to_string(),
            other => other.to_string(),
        },
        // Amazon's enums may spell ages out.
        _ => match code.as_str() {
            "ZERO" => "0",
            "THREE" => "3",
            "SIX" => "6",
            "SEVEN" => "7",
            "TEN" => "10",
            "TWELVE" => "12",
            "FOURTEEN" => "14",
            "SIXTEEN" => "16",
            "EIGHTEEN" => "18",
            other => other,
        }
        .to_string(),
    }
}

/// One board's rating of a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgeRating {
    pub system: RatingSystem,
    /// The board's label ("M", "16", "MA 15+"), never translated.
    pub rating: String,
    /// Content descriptors ("Blood and Gore"), as the source wrote them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub descriptors: Vec<String>,
    /// The youngest age the rating allows; `None` while a rating is pending.
    pub age: Option<u8>,
}

impl Default for AgeRating {
    fn default() -> Self {
        Self {
            system: RatingSystem::Unknown,
            rating: String::new(),
            descriptors: Vec::new(),
            age: None,
        }
    }
}

impl AgeRating {
    /// A rating from a store's text. `age` is the store's own minimum age,
    /// used when the rating's label doesn't tell one. `None` when there is
    /// neither a label nor an age.
    pub fn new(
        system: RatingSystem,
        rating: &str,
        descriptors: impl IntoIterator<Item = String>,
        age: Option<u8>,
    ) -> Option<Self> {
        let label = system.label(rating);
        let age = system.age_of(rating).or(age);
        let label = match (label.is_empty(), age) {
            (false, _) => label,
            (true, Some(age)) => age.to_string(),
            (true, None) => return None,
        };
        let mut seen = Vec::<String>::new();
        for d in descriptors {
            let d = d.trim();
            if !d.is_empty() && !seen.iter().any(|s| s == d) {
                seen.push(d.to_string());
            }
        }
        Some(Self {
            system,
            rating: label,
            descriptors: seen,
            age,
        })
    }

    /// "PEGI 16", "ESRB M".
    pub fn title(&self) -> String {
        format!("{} {}", self.system.name(), self.rating)
    }
}

/// Descriptors given as one text: one per line or, with `commas`, also
/// separated by commas.
pub fn split_descriptors(text: &str, commas: bool) -> Vec<String> {
    text.split(|c| c == '\n' || c == '\r' || (commas && (c == ',' || c == ';')))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Add the ratings of `more` whose system `into` doesn't have yet.
pub fn merge(into: &mut Vec<AgeRating>, more: &[AgeRating]) {
    for r in more {
        if !into.iter().any(|known| known.system == r.system) {
            into.push(r.clone());
        }
    }
}

/// The highest minimum age among a game's ratings: a game passes an age
/// limit only when every board's rating allows that age. `None` when no
/// rating tells an age.
pub fn strictest_age(ratings: &[AgeRating]) -> Option<u8> {
    ratings
        .iter()
        .filter(|r| r.system != RatingSystem::Unknown)
        .filter_map(|r| r.age)
        .max()
}

/// The systems to show first for an interface language (`de`, `pt_BR`)
/// and the system's locale (`en-AU`, `en_GB.UTF-8`), best first: the
/// board of the user's region, then the others. The locale's region counts
/// only when the locale's language is the interface's, so a German
/// interface on an American system still shows USK.
pub fn preferred_systems(ui_language: &str, system_locale: Option<&str>) -> Vec<RatingSystem> {
    let (language, ui_region) = split_locale(ui_language);
    let region = ui_region.or_else(|| {
        let (os_language, os_region) = split_locale(system_locale?);
        (os_language == language).then_some(os_region).flatten()
    });
    let first = match region.as_deref() {
        Some(region) => by_region(region),
        None => match language.as_str() {
            "en" => RatingSystem::Esrb,
            "de" => RatingSystem::Usk,
            "ja" => RatingSystem::Cero,
            "ko" => RatingSystem::Grac,
            _ if ui_language.eq_ignore_ascii_case("zh_Hant") => RatingSystem::Csrr,
            _ => RatingSystem::Pegi,
        },
    };
    std::iter::once(first)
        .chain(FALLBACK_ORDER.into_iter().filter(|s| *s != first))
        .collect()
}

/// The board of a country (ISO 3166 code).
fn by_region(region: &str) -> RatingSystem {
    match region {
        "US" | "CA" | "MX" | "AR" | "CL" | "CO" | "PE" | "VE" | "EC" | "UY" | "PY" | "BO"
        | "CR" | "PA" | "GT" | "DO" | "PR" | "SV" | "HN" | "NI" => RatingSystem::Esrb,
        "BR" => RatingSystem::ClassInd,
        "DE" => RatingSystem::Usk,
        "JP" => RatingSystem::Cero,
        "KR" => RatingSystem::Grac,
        "AU" | "NZ" => RatingSystem::Acb,
        "TW" => RatingSystem::Csrr,
        _ => RatingSystem::Pegi,
    }
}

/// `pt_BR` → (`pt`, `BR`), `en-US.UTF-8` → (`en`, `US`), `zh_Hant` → (`zh`,
/// none: a script, not a region).
fn split_locale(locale: &str) -> (String, Option<String>) {
    let locale = locale.split(['.', '@']).next().unwrap_or("");
    let mut parts = locale.split(['_', '-']);
    let language = parts.next().unwrap_or("").to_ascii_lowercase();
    let region = parts
        .find(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_uppercase);
    (language, region)
}

/// The rating to show: the first of `preferred`'s systems the game has,
/// else any.
pub fn pick<'a>(ratings: &'a [AgeRating], preferred: &[RatingSystem]) -> Option<&'a AgeRating> {
    preferred
        .iter()
        .find_map(|s| ratings.iter().find(|r| r.system == *s))
        .or_else(|| ratings.iter().find(|r| r.system != RatingSystem::Unknown))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rating(system: RatingSystem, label: &str) -> AgeRating {
        AgeRating::new(system, label, Vec::new(), None).unwrap()
    }

    #[test]
    fn reads_every_system_s_ratings_as_ages() {
        use RatingSystem::*;
        let cases: &[(RatingSystem, &str, &str, Option<u8>)] = &[
            (Esrb, "e", "E", Some(6)),
            (Esrb, "e10", "E10+", Some(10)),
            (Esrb, "EVERYONE_10_PLUS", "E10+", Some(10)),
            (Esrb, "Everyone 10+", "E10+", Some(10)),
            (Esrb, "TEEN", "T", Some(13)),
            (Esrb, "Mature 17+", "M", Some(17)),
            (Esrb, "ao", "AO", Some(18)),
            (Esrb, "rp", "RP", None),
            (Pegi, "16", "16", Some(16)),
            (Pegi, "PEGI_18", "18", Some(18)),
            (Usk, "USK_12", "12", Some(12)),
            (Usk, "0", "0", Some(0)),
            (Pegi, "PEGI_SIXTEEN", "16", Some(16)),
            (Cero, "a", "A", Some(0)),
            (Cero, "b", "B", Some(12)),
            (Cero, "c", "C", Some(15)),
            (Cero, "d", "D", Some(17)),
            (Cero, "z", "Z", Some(18)),
            (ClassInd, "l", "L", Some(0)),
            (ClassInd, "14", "14", Some(14)),
            (Acb, "pg", "PG", Some(8)),
            (Acb, "ma15", "MA 15+", Some(15)),
            (Acb, "r18", "R 18+", Some(18)),
            (Grac, "all", "ALL", Some(0)),
            (Grac, "15", "15", Some(15)),
            (Csrr, "pg12", "12+", Some(12)),
            (Csrr, "r", "18+", Some(18)),
            (Gog, "18", "18", Some(18)),
        ];
        for (system, raw, label, age) in cases {
            assert_eq!(system.label(raw), *label, "{system:?} {raw}");
            assert_eq!(system.age_of(raw), *age, "{system:?} {raw}");
        }
        // Unknown words stay as the source wrote them.
        assert_eq!(
            Usk.label("Not approved for young persons"),
            "Not approved for young persons"
        );
    }

    #[test]
    fn names_stores_keys() {
        assert_eq!(
            RatingSystem::from_key("dejus"),
            Some(RatingSystem::ClassInd)
        );
        assert_eq!(
            RatingSystem::from_key("brRating"),
            Some(RatingSystem::ClassInd)
        );
        assert_eq!(RatingSystem::from_key("oflc"), Some(RatingSystem::Acb));
        assert_eq!(RatingSystem::from_key("kgrb"), Some(RatingSystem::Grac));
        assert_eq!(
            RatingSystem::from_key("pegiRating"),
            Some(RatingSystem::Pegi)
        );
        assert_eq!(RatingSystem::from_key("steam_germany"), None);
    }

    #[test]
    fn builds_ratings_with_the_store_s_age_as_fallback() {
        let r = AgeRating::new(
            RatingSystem::Usk,
            "Not approved for young persons aged under 18",
            vec!["Violence".into(), " Violence ".into(), String::new()],
            Some(18),
        )
        .unwrap();
        assert_eq!(r.age, Some(18));
        assert_eq!(r.descriptors, vec!["Violence"]);
        let bare = AgeRating::new(RatingSystem::Gog, "", Vec::new(), Some(16)).unwrap();
        assert_eq!(bare.title(), "GOG 16");
        assert!(AgeRating::new(RatingSystem::Pegi, "", Vec::new(), None).is_none());
        assert_eq!(
            split_descriptors("Blood and Gore\r\nIntense Violence\r\n", false),
            vec!["Blood and Gore", "Intense Violence"]
        );
        assert_eq!(
            split_descriptors("Violence, Language", true),
            vec!["Violence", "Language"]
        );
    }

    #[test]
    fn picks_the_user_s_regional_board() {
        use RatingSystem::*;
        let first = |ui: &str, os: Option<&str>| preferred_systems(ui, os)[0];
        assert_eq!(first("en", Some("en-US")), Esrb);
        assert_eq!(first("en", None), Esrb);
        assert_eq!(first("en", Some("en_GB.UTF-8")), Pegi);
        assert_eq!(first("en", Some("en-AU")), Acb);
        assert_eq!(first("de", Some("en-US")), Usk);
        assert_eq!(first("de", Some("de-AT")), Pegi);
        assert_eq!(first("ja", None), Cero);
        assert_eq!(first("ko", None), Grac);
        assert_eq!(first("pt_BR", None), ClassInd);
        assert_eq!(first("pt", Some("pt-PT")), Pegi);
        assert_eq!(first("es", Some("es-MX")), Esrb);
        assert_eq!(first("fr", None), Pegi);
        assert_eq!(first("zh_Hant", None), Csrr);
        let all = preferred_systems("fr", None);
        assert_eq!(all.len(), FALLBACK_ORDER.len());

        let ratings = vec![rating(Esrb, "m"), rating(Pegi, "18")];
        assert_eq!(
            pick(&ratings, &preferred_systems("en", None))
                .unwrap()
                .system,
            Esrb
        );
        assert_eq!(
            pick(&ratings, &preferred_systems("fr", None))
                .unwrap()
                .system,
            Pegi
        );
        // Without the region's board, the next one the game has.
        assert_eq!(
            pick(&ratings, &preferred_systems("ja", None))
                .unwrap()
                .system,
            Pegi
        );
        assert_eq!(pick(&[rating(Cero, "b")], &[Esrb]).unwrap().system, Cero);
        assert!(pick(&[], &[Esrb]).is_none());
    }

    #[test]
    fn the_strictest_board_decides_the_age() {
        use RatingSystem::*;
        let mut ratings = vec![rating(Pegi, "12")];
        merge(&mut ratings, &[rating(Esrb, "t"), rating(Pegi, "16")]);
        assert_eq!(ratings.len(), 2);
        assert_eq!(strictest_age(&ratings), Some(13));
        assert_eq!(strictest_age(&[rating(Esrb, "rp")]), None);
        assert_eq!(strictest_age(&[]), None);
    }

    #[test]
    fn unknown_systems_still_read() {
        let r: AgeRating =
            serde_json::from_str(r#"{"system": "IARC", "rating": "12", "age": 12}"#).unwrap();
        assert_eq!(r.system, RatingSystem::Unknown);
        assert_eq!(strictest_age(&[r]), None);
        let r = rating(RatingSystem::ClassInd, "16");
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"system":"ClassInd","rating":"16","age":16}"#);
        assert_eq!(serde_json::from_str::<AgeRating>(&json).unwrap(), r);
    }
}
