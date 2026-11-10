//! The game's language. The runtime picks a package's language resources
//! from `XODUS_PACKAGE_LOCALE`, and games that ask Windows for the user's
//! locale read `WINEUSERLOCALE`; the Unix locale variables follow so the
//! Wine side agrees. The per-game "Game language" setting chooses it; an
//! empty one picks the package resources of the interface language and
//! leaves the system locale alone.

use std::collections::BTreeMap;

/// The region a language most often means, for codes given without one.
const DEFAULT_REGIONS: &[(&str, &str)] = &[
    ("ar", "SA"),
    ("bg", "BG"),
    ("cs", "CZ"),
    ("da", "DK"),
    ("de", "DE"),
    ("el", "GR"),
    ("en", "US"),
    ("es", "ES"),
    ("fi", "FI"),
    ("fr", "FR"),
    ("he", "IL"),
    ("hu", "HU"),
    ("id", "ID"),
    ("it", "IT"),
    ("ja", "JP"),
    ("ko", "KR"),
    ("nb", "NO"),
    ("nl", "NL"),
    ("pl", "PL"),
    ("pt", "PT"),
    ("ro", "RO"),
    ("ru", "RU"),
    ("sk", "SK"),
    ("sv", "SE"),
    ("th", "TH"),
    ("tr", "TR"),
    ("uk", "UA"),
    ("vi", "VN"),
    ("zh", "CN"),
];

/// A language tag as Windows names locales (`de-DE`, `zh-TW`), from what a
/// user typed (`de`, `pt_BR`, `zh-Hant`). `None` for anything else.
pub fn windows_locale(value: &str) -> Option<String> {
    let value = value.trim().replace('_', "-");
    if value.is_empty() || value.len() > 35 {
        return None;
    }
    let mut parts = value.split('-');
    let language = parts.next()?.to_ascii_lowercase();
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let mut script = None;
    let mut region = None;
    for part in parts {
        if script.is_none() && region.is_none() && part.len() == 4 {
            if !part.bytes().all(|b| b.is_ascii_alphabetic()) {
                return None;
            }
            let mut s = part.to_ascii_lowercase();
            s[..1].make_ascii_uppercase();
            script = Some(s);
        } else if region.is_none()
            && ((part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
                || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit())))
        {
            region = Some(part.to_ascii_uppercase());
        } else {
            return None;
        }
    }
    if language == "zh" {
        // Windows names Chinese by region; the script follows from it.
        let region = match (script.as_deref(), region.as_deref()) {
            (Some("Hant"), None) => "TW".to_string(),
            (Some("Hans"), None) | (None, None) => "CN".to_string(),
            (Some("Hant"), Some(r @ ("TW" | "HK" | "MO"))) => r.to_string(),
            (Some("Hans"), Some(r @ ("CN" | "SG"))) => r.to_string(),
            (None, Some(r)) => r.to_string(),
            _ => return None,
        };
        return Some(format!("zh-{region}"));
    }
    let region = match region {
        Some(r) => r,
        None => DEFAULT_REGIONS
            .iter()
            .find(|(l, _)| *l == language)
            .map(|(_, r)| (*r).to_string())?,
    };
    Some(match script {
        Some(script) => format!("{language}-{script}-{region}"),
        None => format!("{language}-{region}"),
    })
}

/// The variables a launch sets for the game's language: `chosen` (the
/// per-game setting) changes the locale too; without it only the package
/// resources follow `interface` (Irori's language).
pub fn environment(chosen: &str, interface: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    if let Some(tag) = windows_locale(chosen) {
        let mut parts = tag.split('-');
        let language = parts.next().unwrap_or_default();
        let region = tag.rsplit('-').next().unwrap_or_default();
        let script = match parts.next().filter(|p| p.len() == 4) {
            Some("Latn") => "@latin".to_string(),
            Some("Cyrl") => "@cyrillic".to_string(),
            Some(other) => format!("@{}", other.to_ascii_lowercase()),
            None => String::new(),
        };
        let unix = format!("{language}_{region}.UTF-8{script}");
        for key in [
            "LANG",
            "LC_ALL",
            "LC_MESSAGES",
            "LC_CTYPE",
            "LC_TIME",
            "LC_COLLATE",
            "LC_MONETARY",
            "LC_NUMERIC",
            "LC_PAPER",
        ] {
            vars.insert(key.to_string(), unix.clone());
        }
        vars.insert(
            "LANGUAGE".into(),
            format!("{language}_{region}{script}:{language}"),
        );
        vars.insert("WINEUSERLOCALE".into(), tag.clone());
        vars.insert("XODUS_PACKAGE_LOCALE".into(), tag);
    } else if let Some(tag) = windows_locale(interface) {
        vars.insert("XODUS_PACKAGE_LOCALE".into(), tag);
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_become_windows_locales() {
        assert_eq!(windows_locale("de").as_deref(), Some("de-DE"));
        assert_eq!(windows_locale("pt_BR").as_deref(), Some("pt-BR"));
        assert_eq!(windows_locale(" pl-pl ").as_deref(), Some("pl-PL"));
        assert_eq!(windows_locale("zh-Hant").as_deref(), Some("zh-TW"));
        assert_eq!(windows_locale("zh_Hans").as_deref(), Some("zh-CN"));
        assert_eq!(windows_locale("sr-Latn-RS").as_deref(), Some("sr-Latn-RS"));
        assert_eq!(windows_locale("es-419").as_deref(), Some("es-419"));
        assert_eq!(windows_locale("zh-Hant-CN"), None);
        // A language without a known region needs one.
        assert_eq!(windows_locale("sr"), None);
        assert_eq!(windows_locale("english"), None);
        assert_eq!(windows_locale("de;rm -rf"), None);
        assert_eq!(windows_locale(""), None);
    }

    #[test]
    fn a_chosen_language_sets_the_locale() {
        let vars = environment("fr", "en");
        assert_eq!(vars["XODUS_PACKAGE_LOCALE"], "fr-FR");
        assert_eq!(vars["WINEUSERLOCALE"], "fr-FR");
        assert_eq!(vars["LANG"], "fr_FR.UTF-8");
        assert_eq!(vars["LANGUAGE"], "fr_FR:fr");
        let vars = environment("sr-Latn-RS", "en");
        assert_eq!(vars["LC_ALL"], "sr_RS.UTF-8@latin");
    }

    #[test]
    fn without_a_choice_only_the_resources_follow_irori() {
        let vars = environment("", "pt_BR");
        assert_eq!(vars.len(), 1);
        assert_eq!(vars["XODUS_PACKAGE_LOCALE"], "pt-BR");
        assert!(environment("", "nonsense").is_empty());
    }
}
