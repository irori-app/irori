//! "Edit details…": the developer, publisher and genres of one game or of
//! every picked game at once, kept as the user's overrides
//! (`UserDataService::set_metadata`) that win over the stores' and travel
//! with sync. A field the picked games share shows its value; one they
//! differ in starts empty and is only changed when typed into. Emptying a
//! field, or "Use the stores' details", gives it back to the stores.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use irori_core::{GameId, GameInfo, MetadataEdit};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::text;

/// What the picked games share of each detail: `Some` when every game has
/// the same value (empty for none), `None` when they differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Shared {
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genres: Option<String>,
}

/// Genres as the field shows them.
fn genres_text(genres: &[String]) -> String {
    genres.join(", ")
}

/// The genres typed into the field.
fn genres_of(text: &str) -> Vec<String> {
    text.split([',', ';'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
        .collect()
}

/// What `games` share.
pub(crate) fn shared(games: &[Arc<GameInfo>]) -> Shared {
    fn same(values: impl Iterator<Item = String>) -> Option<String> {
        let mut values = values;
        let first = values.next()?;
        values.all(|v| v == first).then_some(first)
    }
    Shared {
        developer: same(
            games
                .iter()
                .map(|g| g.developer.clone().unwrap_or_default()),
        ),
        publisher: same(
            games
                .iter()
                .map(|g| g.publisher.clone().unwrap_or_default()),
        ),
        genres: same(games.iter().map(|g| genres_text(&g.genres))),
    }
}

/// The change the typed fields make: a field changes when its text
/// differs from what the games shared (anything typed into a field they
/// differ in); an emptied field gives the detail back to the stores.
pub(crate) fn edit_from(
    before: &Shared,
    developer: &str,
    publisher: &str,
    genres: &str,
) -> MetadataEdit {
    let changed = |before: &Option<String>, now: &str| match before {
        Some(b) => b.trim() != now.trim(),
        None => !now.trim().is_empty(),
    };
    let text = |now: &str| Some(now.trim().to_string()).filter(|s| !s.is_empty());
    MetadataEdit {
        developer: changed(&before.developer, developer).then(|| text(developer)),
        publisher: changed(&before.publisher, publisher).then(|| text(publisher)),
        genres: changed(&before.genres, genres).then(|| {
            let list = genres_of(genres);
            (!list.is_empty()).then_some(list)
        }),
    }
}

/// Open "Edit details…" for `ids` (one game or the picked ones).
pub fn open_metadata_dialog(ids: Vec<GameId>, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    if ids.is_empty() {
        return;
    }
    let games: Vec<Arc<GameInfo>> = {
        let library = models(cx).library.read(cx);
        ids.iter()
            .filter_map(|id| library.snapshot.get(id).cloned())
            .collect()
    };
    let heading = match games.as_slice() {
        [one] => t_args("library.metadata.title_one", &[("title", &one.title)]),
        _ => t_args(
            "library.metadata.title_other",
            &[("count", &ids.len().to_string())],
        ),
    };
    let dialog = cx.new(|cx| MetadataDialog::new(ids, shared(&games), window, cx));
    let width = window.rem_size() * 30.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

struct MetadataDialog {
    ids: Vec<GameId>,
    before: Shared,
    developer: Entity<InputState>,
    publisher: Entity<InputState>,
    genres: Entity<InputState>,
    saving: bool,
}

impl MetadataDialog {
    fn new(ids: Vec<GameId>, before: Shared, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |value: &Option<String>, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = match value {
                Some(_) => t("library.metadata.none"),
                None => t("library.metadata.mixed"),
            };
            let value = value.clone().unwrap_or_default();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let developer = input(&before.developer, window, cx);
        let publisher = input(&before.publisher, window, cx);
        let genres = input(&before.genres, window, cx);
        Self {
            ids,
            before,
            developer,
            publisher,
            genres,
            saving: false,
        }
    }

    fn save(&mut self, edit: MetadataEdit, window: &mut Window, cx: &mut Context<Self>) {
        if edit.is_empty() {
            window.close_dialog(cx);
            return;
        }
        self.saving = true;
        cx.notify();
        let (b, ids) = (backend(cx), self.ids.clone());
        let task = irori_ui::run(cx, async move {
            let writer = b.clone();
            b.spawn_blocking(move || writer.userdata.set_metadata(&ids, &edit))
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("library.metadata.failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn entered(&self, cx: &App) -> MetadataEdit {
        edit_from(
            &self.before,
            &text(&self.developer, cx),
            &text(&self.publisher, cx),
            &text(&self.genres, cx),
        )
    }
}

impl Render for MetadataDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let field = |label: SharedString, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(Input::new(input).small())
        };
        let reset = MetadataEdit {
            developer: Some(None),
            publisher: Some(None),
            genres: Some(None),
        };
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.metadata.help")),
            )
            .child(field(t("library.metadata.developer"), &self.developer))
            .child(field(t("library.metadata.publisher"), &self.publisher))
            .child(field(t("library.metadata.genres"), &self.genres))
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(
                        Button::new("metadata-reset")
                            .ghost()
                            .small()
                            .label(t("library.metadata.reset"))
                            .disabled(self.saving)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.save(reset.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new("metadata-save")
                            .primary()
                            .small()
                            .label(t("library.metadata.save"))
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let edit = this.entered(cx);
                                this.save(edit, window, cx)
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(developer: Option<&str>, genres: &[&str]) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            developer: developer.map(str::to_string),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn shared_values_show_and_differing_ones_wait_for_typing() {
        let games = [
            game(Some("Supergiant"), &["Action", "Roguelike"]),
            game(Some("Supergiant"), &["Action"]),
        ];
        let before = shared(&games);
        assert_eq!(before.developer.as_deref(), Some("Supergiant"));
        assert_eq!(before.publisher.as_deref(), Some(""));
        assert_eq!(before.genres, None);

        // Nothing typed: nothing changes.
        let edit = edit_from(&before, "Supergiant", "", "");
        assert!(edit.is_empty());
        // Typed into the differing genres, and the developer emptied.
        let edit = edit_from(&before, "", "", "Roguelike; Action ,");
        assert_eq!(edit.developer, Some(None));
        assert_eq!(edit.publisher, None);
        assert_eq!(
            edit.genres,
            Some(Some(vec!["Roguelike".to_string(), "Action".to_string()]))
        );
    }
}
