//! A palette row before it becomes a `CommandItem`: its label, optional
//! English fallback, keywords, optional help text, category, empty-query
//! visibility, and pre-folded search representation.

use gpui_kit::assets::IconName;
use gpui_kit::component::command::CommandItem;
use gpui_kit::{IntoElement, SharedString};
use irori_services::search::Query;

use super::score::{EntryKind, FoldedEntry, score_entry};

/// One palette row's text, icon, category, and pre-folded search index.
#[derive(Clone)]
pub(super) struct Entry {
    pub(super) label: SharedString,
    pub(super) english_label: Option<&'static str>,
    pub(super) keywords: Vec<SharedString>,
    pub(super) help: Option<SharedString>,
    pub(super) english_help: Option<&'static str>,
    pub(super) icon: Option<IconName>,
    pub(super) kind: EntryKind,
    /// Whether this entry appears in the curated empty-query home list.
    pub(super) show_when_empty: bool,
    /// Whether this entry should be offered in the standalone Quick Launcher
    /// window (`Host::Window`). Pure in-window chrome commands opt out.
    pub(super) quick_launch: bool,
    folded: Option<FoldedEntry>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            label: SharedString::default(),
            english_label: None,
            keywords: Vec::new(),
            help: None,
            english_help: None,
            icon: None,
            kind: EntryKind::Command,
            show_when_empty: false,
            quick_launch: true,
            folded: None,
        }
    }
}

impl Entry {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub(super) fn english_label(mut self, english: &'static str) -> Self {
        if !english.is_empty() {
            self.english_label = Some(english);
        }
        self
    }

    pub(super) fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<SharedString>,
    {
        self.keywords.extend(keywords.into_iter().map(Into::into));
        self
    }

    pub(super) fn help(mut self, help: impl Into<SharedString>) -> Self {
        let help = help.into();
        if !help.is_empty() {
            self.help = Some(help);
        }
        self
    }

    pub(super) fn english_help(mut self, help: &'static str) -> Self {
        if !help.is_empty() {
            self.english_help = Some(help);
        }
        self
    }

    pub(super) fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub(super) fn kind(mut self, kind: EntryKind) -> Self {
        self.kind = kind;
        self
    }

    pub(super) fn show_when_empty(mut self, show: bool) -> Self {
        self.show_when_empty = show;
        self
    }

    pub(super) fn window_only(mut self) -> Self {
        self.quick_launch = false;
        self
    }

    /// Pre-fold labels, keywords, and help text once when the palette opens.
    pub(super) fn finish(mut self) -> Self {
        self.folded = Some(FoldedEntry::build(
            self.label.as_ref(),
            self.english_label,
            &self.keywords,
            self.help.as_deref(),
            self.english_help,
        ));
        self
    }

    /// Relevance score against `query` (`None` when it does not match).
    pub(super) fn score(&self, query: &Query) -> Option<i64> {
        if let Some(folded) = &self.folded {
            score_entry(self.kind, folded, query)
        } else {
            let folded = FoldedEntry::build(
                self.label.as_ref(),
                self.english_label,
                &self.keywords,
                self.help.as_deref(),
                self.english_help,
            );
            score_entry(self.kind, &folded, query)
        }
    }

    /// Whether the row is found by `raw` (empty matches everything). Used by
    /// secondary levels (`Game`, `Random`, `Picked`).
    pub(super) fn matches_text(&self, raw: &str) -> bool {
        let Some(query) = Query::new(raw) else {
            return true;
        };
        self.score(&query).is_some()
    }

    /// The row as the palette draws it.
    pub(super) fn item(&self) -> CommandItem {
        let item = CommandItem::new()
            .label(self.label.clone())
            .keywords(self.keywords.iter().cloned());
        match self.icon {
            Some(icon) => item.icon(icon),
            None => item,
        }
    }

    /// The row drawn by `content` instead of its icon and label (still found
    /// by them).
    pub(super) fn item_with<E: IntoElement>(
        &self,
        content: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App) -> E + 'static,
    ) -> CommandItem {
        self.item().child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_accents_case_english_and_keywords() {
        let entry = Entry::new()
            .label("Ir para Configurações")
            .english_label("Go to Settings")
            .keywords(["preferencias", "options"])
            .kind(EntryKind::Page)
            .finish();
        assert!(entry.matches_text(""));
        assert!(entry.matches_text("configuracoes"));
        assert!(entry.matches_text("SETTINGS"));
        assert!(entry.matches_text("preferências"));
        assert!(!entry.matches_text("library"));
    }
}
