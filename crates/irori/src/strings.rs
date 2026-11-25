//! The shell's translation catalogs (`shell.` keys).

use irori_ui::i18n::Catalogs;

pub static CATALOGS: Catalogs = &[
    ("en", include_str!("../locales/en.json")),
    ("cs", include_str!("../locales/cs.json")),
    ("de", include_str!("../locales/de.json")),
    ("es", include_str!("../locales/es.json")),
    ("fr", include_str!("../locales/fr.json")),
    ("it", include_str!("../locales/it.json")),
    ("nl", include_str!("../locales/nl.json")),
    ("pl", include_str!("../locales/pl.json")),
    ("pt", include_str!("../locales/pt.json")),
    ("pt_BR", include_str!("../locales/pt_BR.json")),
    ("ro", include_str!("../locales/ro.json")),
    ("sv", include_str!("../locales/sv.json")),
    ("tr", include_str!("../locales/tr.json")),
    ("ru", include_str!("../locales/ru.json")),
    ("uk", include_str!("../locales/uk.json")),
    ("ja", include_str!("../locales/ja.json")),
    ("ko", include_str!("../locales/ko.json")),
    ("zh_Hans", include_str!("../locales/zh_Hans.json")),
    ("zh_Hant", include_str!("../locales/zh_Hant.json")),
];

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::CATALOGS;
    use irori_ui::i18n::LANGUAGES;

    fn parse(code: &str, json: &str) -> HashMap<String, String> {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{code}.json does not parse: {e}"))
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let re = regex::Regex::new(r"\{([a-z_]+)\}").unwrap();
        re.captures_iter(text).map(|c| c[1].to_string()).collect()
    }

    #[test]
    fn every_language_catalog_parses_and_matches_english() {
        let codes: Vec<&str> = CATALOGS.iter().map(|(c, _)| *c).collect();
        let expected: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
        assert_eq!(codes, expected);

        let en = parse("en", CATALOGS[0].1);
        assert!(!en.is_empty(), "en.json is empty");
        for key in en.keys() {
            assert!(key.starts_with("shell."), "{key} is outside shell.");
        }

        for (code, json) in CATALOGS {
            let catalog = parse(code, json);
            let extra: Vec<_> = catalog.keys().filter(|k| !en.contains_key(*k)).collect();
            assert!(extra.is_empty(), "{code}: extra keys {extra:?}");
            for (key, text) in &catalog {
                assert!(!text.trim().is_empty(), "{code}: {key} is empty");
                assert_eq!(
                    placeholders(text),
                    placeholders(&en[key]),
                    "{code}: placeholders of {key}"
                );
            }
        }
    }

    #[test]
    fn every_used_key_exists() {
        let en: std::collections::HashMap<String, String> =
            serde_json::from_str(CATALOGS[0].1).unwrap();
        let re = regex::Regex::new(r#""(shell\.[a-z0-9_.]+)""#).unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let services = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../services/src");
        let mut dirs = vec![dir, services];
        while let Some(d) = dirs.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                for cap in re.captures_iter(&src) {
                    // Keys passed to `item(...)` and `keys(...)` are literals too.
                    assert!(
                        en.contains_key(&cap[1]),
                        "{} uses unknown key {}",
                        path.display(),
                        &cap[1]
                    );
                }
            }
        }
    }
}
