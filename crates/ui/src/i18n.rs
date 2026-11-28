//! Interface translations.
//!
//! Each crate with user-facing text owns its catalogs: flat JSON objects
//! (`"library.search.placeholder": "Search library"`) in
//! `crates/<crate>/locales/<code>.json`, embedded with `include_str!` and
//! handed to [`register`] from the crate's `init`. Keys are namespaced by
//! crate (`library.`, `game.`, `settings.`, `manage.`, `bigpicture.`, `shell.`,
//! `ui.`), so catalogs never collide.
//!
//! English is the source and the fallback for keys a locale lacks.
//! Placeholders use `{name}`.
//!
//! ```ignore
//! use irori_ui::i18n::{t, t_args};
//! Button::new("play").label(t("game.play"));
//! t_args("downloads.installed", &[("title", &game.title)]);
//! ```
//!
//! Keys describe intent (`game.uninstall.title`), never the English text;
//! sentences are never assembled from fragments.

use std::collections::HashMap;

use gpui_kit::SharedString;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// A selectable interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// Code stored in settings (`ui_language`), e.g. `pt_BR`.
    pub code: &'static str,
    /// Name in the language itself.
    pub native_name: &'static str,
}

/// Languages Irori ships translations for, English first.
pub const LANGUAGES: &[Language] = &[
    Language {
        code: "en",
        native_name: "English",
    },
    Language {
        code: "cs",
        native_name: "Čeština",
    },
    Language {
        code: "de",
        native_name: "Deutsch",
    },
    Language {
        code: "es",
        native_name: "Español",
    },
    Language {
        code: "fr",
        native_name: "Français",
    },
    Language {
        code: "it",
        native_name: "Italiano",
    },
    Language {
        code: "nl",
        native_name: "Nederlands",
    },
    Language {
        code: "pl",
        native_name: "Polski",
    },
    Language {
        code: "pt",
        native_name: "Português",
    },
    Language {
        code: "pt_BR",
        native_name: "Português (Brasil)",
    },
    Language {
        code: "ro",
        native_name: "Română",
    },
    Language {
        code: "sv",
        native_name: "Svenska",
    },
    Language {
        code: "tr",
        native_name: "Türkçe",
    },
    Language {
        code: "ru",
        native_name: "Русский",
    },
    Language {
        code: "uk",
        native_name: "Українська",
    },
    Language {
        code: "ja",
        native_name: "日本語",
    },
    Language {
        code: "ko",
        native_name: "한국어",
    },
    Language {
        code: "zh_Hans",
        native_name: "简体中文",
    },
    Language {
        code: "zh_Hant",
        native_name: "繁體中文",
    },
];

/// Catalogs of this crate (`ui.` keys: statuses, progress, shared components).
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

/// Translated label of a game status.
pub fn status_label(kind: irori_core::StatusKind) -> SharedString {
    use irori_core::StatusKind as K;
    t(match kind {
        K::Installing => "ui.status.installing",
        K::Importing => "ui.status.importing",
        K::Updating => "ui.status.updating",
        K::Launching => "ui.status.launching",
        K::Playing => "ui.status.playing",
        K::Uninstalling => "ui.status.uninstalling",
        K::Repairing => "ui.status.repairing",
        K::Moving => "ui.status.moving",
        K::Queued => "ui.status.queued",
        K::SyncingSaves => "ui.status.syncing_saves",
        K::Redist => "ui.status.redist",
        K::Extracting => "ui.status.extracting",
        K::Winetricks => "ui.status.winetricks",
        K::Done => "ui.status.done",
        K::Canceled => "ui.status.canceled",
        K::Error => "ui.status.error",
    })
}

/// `(language code, JSON catalog)` pairs of one crate.
pub type Catalogs = &'static [(&'static str, &'static str)];

type Catalog = HashMap<String, String>;

struct State {
    code: String,
    sources: Vec<Catalogs>,
    current: Catalog,
    english: Catalog,
}

impl State {
    fn new() -> Self {
        Self {
            code: "en".into(),
            sources: Vec::new(),
            current: Catalog::new(),
            english: Catalog::new(),
        }
    }

    fn register(&mut self, catalogs: Catalogs) {
        self.sources.push(catalogs);
        self.english = load(&self.sources, "en");
        self.current = if self.code == "en" {
            Catalog::new()
        } else {
            load(&self.sources, &self.code)
        };
    }

    fn set_language(&mut self, code: &str) {
        let resolved = resolve_locale(code);
        self.code = resolved.to_string();
        self.current = if resolved == "en" {
            Catalog::new()
        } else {
            load(&self.sources, resolved)
        };
    }

    fn t(&self, key: &str) -> SharedString {
        self.current
            .get(key)
            .or_else(|| self.english.get(key))
            .map(|s| SharedString::from(s.clone()))
            .unwrap_or_else(|| SharedString::from(key.to_string()))
    }

    fn t_args(&self, key: &str, args: &[(&str, &str)]) -> SharedString {
        fill(&self.t(key), args)
    }
}

static STATE: Lazy<RwLock<State>> = Lazy::new(|| RwLock::new(State::new()));

fn parse(json: &str) -> Catalog {
    match serde_json::from_str::<Catalog>(json) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("invalid translation catalog: {e}");
            Catalog::new()
        }
    }
}

fn load(sources: &[Catalogs], code: &str) -> Catalog {
    let mut out = Catalog::new();
    for catalogs in sources {
        for (c, json) in catalogs.iter() {
            if *c == code {
                out.extend(parse(json));
            }
        }
    }
    out
}

fn fill(text: &str, args: &[(&str, &str)]) -> SharedString {
    let mut text = text.to_string();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    SharedString::from(text)
}

/// Resolve an OS locale or BCP-47 tag (such as `pt-BR`, `pt_BR.UTF-8`,
/// `zh-TW`, `de-AT`) to the closest shipped language code in [`LANGUAGES`],
/// falling back to `"en"`.
pub fn resolve_locale(raw: &str) -> &'static str {
    let stripped = raw.trim().split(['.', '@']).next().unwrap_or("");
    if stripped.is_empty() {
        return "en";
    }
    let norm = stripped.replace('-', "_");
    if let Some(exact) = LANGUAGES
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(&norm))
    {
        return exact.code;
    }
    let lower = norm.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('_').filter(|s| !s.is_empty()).collect();
    match parts.first().copied() {
        Some("zh") => {
            if parts
                .iter()
                .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"))
            {
                return "zh_Hant";
            }
            return "zh_Hans";
        }
        Some("pt") if parts.contains(&"br") => return "pt_BR",
        Some(base) => {
            if let Some(lang) = LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(base)) {
                return lang.code;
            }
        }
        None => {}
    }
    "en"
}

/// On a brand-new installation (before any settings file is saved), pick the
/// interface language from the operating system locale so first-run screens
/// open in the user's language immediately.
pub fn apply_system_language_on_first_run(backend: &irori_services::Backend) {
    if irori_core::Paths::get().settings_file().exists() {
        return;
    }
    if backend.settings.app().ui_language != "en" {
        return;
    }
    let Some(locale) = crate::age_rating::system_locale() else {
        return;
    };
    let resolved = resolve_locale(locale);
    if resolved != "en" {
        backend
            .settings
            .update_app(|s| s.ui_language = resolved.to_string());
    }
}

/// Add a crate's catalogs. Call once from the crate's `init`.
pub fn register(catalogs: Catalogs) {
    STATE.write().register(catalogs);
}

/// Switch the interface language. Unknown codes fall back to English (a
/// region-specific code like `pt_PT` falls back to `pt` first).
pub fn set_language(code: &str) {
    STATE.write().set_language(code);
}

/// Code of the active language.
pub fn language() -> String {
    STATE.read().code.clone()
}

/// Translate `key`. Missing keys fall back to English, then to the key itself
/// (so a missing string is visible rather than blank).
pub fn t(key: &str) -> SharedString {
    STATE.read().t(key)
}

/// Translate `key` and fill `{name}` placeholders.
pub fn t_args(key: &str, args: &[(&str, &str)]) -> SharedString {
    STATE.read().t_args(key, args)
}

/// Keys present in English but missing in `code` (for catalog tests).
pub fn missing_keys(catalogs: Catalogs, code: &str) -> Vec<String> {
    let en = load(&[catalogs], "en");
    let other = load(&[catalogs], code);
    let mut missing: Vec<String> = en
        .keys()
        .filter(|k| !other.contains_key(*k))
        .cloned()
        .collect();
    missing.sort();
    missing
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    static TEST: Catalogs = &[
        (
            "en",
            r#"{"i18n.test.hello": "Hello {name}", "i18n.test.only_en": "Only", "i18n.test.only_en_args": "Play {title}"}"#,
        ),
        ("de", r#"{"i18n.test.hello": "Hallo {name}"}"#),
    ];

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('}') {
                let name = &rest[..end];
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
                    out.insert(name.to_string());
                }
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
        out
    }

    fn validate_catalogs(catalogs: Catalogs, namespace: &str) -> Result<(), String> {
        let (_, en_json) = catalogs
            .iter()
            .find(|(code, _)| *code == "en")
            .ok_or_else(|| "missing en catalog".to_string())?;
        let en: Catalog =
            serde_json::from_str(en_json).map_err(|e| format!("en.json does not parse: {e}"))?;
        if en.is_empty() {
            return Err("en.json is empty".to_string());
        }
        for (key, value) in &en {
            if !key.starts_with(namespace) {
                return Err(format!("{key} is outside {namespace}"));
            }
            if value.trim().is_empty() {
                return Err(format!("en: {key} is empty"));
            }
        }
        for (code, json) in catalogs {
            let catalog: Catalog = serde_json::from_str(json)
                .map_err(|e| format!("{code}.json does not parse: {e}"))?;
            let mut extra: Vec<&String> = catalog.keys().filter(|k| !en.contains_key(*k)).collect();
            extra.sort();
            if !extra.is_empty() {
                return Err(format!("{code} has orphan keys: {extra:?}"));
            }
            for (key, value) in &catalog {
                if value.trim().is_empty() {
                    return Err(format!("{code}: {key} is empty"));
                }
                if placeholders(value) != placeholders(&en[key]) {
                    return Err(format!("{code}: placeholders of {key} differ from English"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn own_catalogs_parse_and_match_english() {
        let codes: Vec<&str> = CATALOGS.iter().map(|(c, _)| *c).collect();
        let expected: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
        assert_eq!(codes, expected);
        assert_eq!(validate_catalogs(CATALOGS, "ui."), Ok(()));
    }

    // Uses its own state: switching the global language would race with
    // every test that compares translated text.
    #[test]
    fn registers_falls_back_and_fills_placeholders() {
        let mut state = State::new();
        state.register(TEST);
        state.set_language("de_AT");
        assert_eq!(state.code, "de");
        assert_eq!(
            state.t_args("i18n.test.hello", &[("name", "Ana")]),
            "Hallo Ana"
        );
        assert_eq!(state.t("i18n.test.only_en"), "Only");
        assert_eq!(
            state.t_args("i18n.test.only_en_args", &[("title", "Celeste")]),
            "Play Celeste"
        );
        assert_eq!(state.t("i18n.test.missing"), "i18n.test.missing");
        assert_eq!(
            missing_keys(TEST, "de"),
            vec![
                "i18n.test.only_en".to_string(),
                "i18n.test.only_en_args".to_string()
            ]
        );
        assert_eq!(validate_catalogs(TEST, "i18n.test."), Ok(()));
        state.set_language("xx");
        assert_eq!(state.code, "en");
    }

    #[test]
    fn validation_allows_missing_non_english_keys_but_rejects_defects() {
        static ORPHAN: Catalogs = &[
            ("en", r#"{"ui.ok": "OK"}"#),
            ("de", r#"{"ui.ok": "OK", "ui.orphan": "Orphan"}"#),
        ];
        static BLANK: Catalogs = &[("en", r#"{"ui.ok": "OK"}"#), ("de", r#"{"ui.ok": "   "}"#)];
        static BAD_PLACEHOLDER: Catalogs = &[
            ("en", r#"{"ui.hi": "Hi {name}"}"#),
            ("de", r#"{"ui.hi": "Hallo {user}"}"#),
        ];
        assert!(validate_catalogs(ORPHAN, "ui.").is_err());
        assert!(validate_catalogs(BLANK, "ui.").is_err());
        assert!(validate_catalogs(BAD_PLACEHOLDER, "ui.").is_err());
    }

    #[test]
    fn resolves_os_locales_to_shipped_languages() {
        assert_eq!(resolve_locale("pt-BR"), "pt_BR");
        assert_eq!(resolve_locale("pt_BR.UTF-8"), "pt_BR");
        assert_eq!(resolve_locale("pt-Latn-BR"), "pt_BR");
        assert_eq!(resolve_locale("pt-PT"), "pt");
        assert_eq!(resolve_locale("zh-CN"), "zh_Hans");
        assert_eq!(resolve_locale("zh-Hans-CN"), "zh_Hans");
        assert_eq!(resolve_locale("zh-TW"), "zh_Hant");
        assert_eq!(resolve_locale("zh-HK"), "zh_Hant");
        assert_eq!(resolve_locale("zh"), "zh_Hans");
        assert_eq!(resolve_locale("de_AT@euro"), "de");
        assert_eq!(resolve_locale("es-419"), "es");
        assert_eq!(resolve_locale("en-US"), "en");
        assert_eq!(resolve_locale("xx"), "en");
        assert_eq!(resolve_locale(""), "en");
    }
}
