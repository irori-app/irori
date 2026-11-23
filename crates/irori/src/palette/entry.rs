//! A palette row before it becomes a `CommandItem`: its label, the words it
//! is also found by and its icon. The palette filters rows itself (game
//! rows are ranked by the library's search), so it keeps what it matches on.

use gpui_kit::assets::IconName;
use gpui_kit::component::command::CommandItem;
use gpui_kit::{IntoElement, SharedString};

/// One palette row's text and icon.
#[derive(Clone, Default)]
pub(super) struct Entry {
    pub(super) label: SharedString,
    pub(super) keywords: Vec<SharedString>,
    pub(super) icon: Option<IconName>,
}

impl Entry {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
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

    pub(super) fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Whether the row is found by `query`, already in lower case: its label
    /// or one of its words holds it, as the palette always matched.
    pub(super) fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.label.to_lowercase().contains(query)
            || self
                .keywords
                .iter()
                .any(|k| k.to_lowercase().contains(query))
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
    fn matches_its_label_and_words_without_case() {
        let entry = Entry::new()
            .label("Go to Downloads")
            .keywords(["Queue", "install"]);
        assert!(entry.matches(""));
        assert!(entry.matches("downl"));
        assert!(entry.matches("queue"));
        assert!(!entry.matches("library"));
    }
}
