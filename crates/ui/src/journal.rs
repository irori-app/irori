//! The journal note editor, shared by the game page's timeline and the
//! stats page's Journal tab: a title, the note itself and the screenshots
//! taken that day to link to it. Saving writes the library file, so it runs
//! off the UI thread; deleting a note asks first.

use std::cell::Cell;
use std::collections::BTreeSet;

use chrono::{Local, NaiveDate, TimeZone as _};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_core::GameId;
use irori_services::JournalEntry;
use irori_services::journal::MAX_LINKED_SCREENSHOTS;

use crate::components::{art_img, physical_width};
use crate::i18n::{t, t_args};
use crate::style::KeyboardFocusable as _;
use crate::{backend, models};

/// Width of a screenshot in the picker, in rem.
const SHOT_REM: f32 = 8.;

/// A screenshot as the art store addresses it.
pub fn shot_url(path: &str) -> String {
    format!("file://{path}")
}

struct JournalEditor {
    id: GameId,
    day: NaiveDate,
    /// The game and day, for the delete question.
    label: SharedString,
    existed: bool,
    title: Entity<InputState>,
    text: Entity<TextareaState>,
    /// The day's screenshots of the game and those the note already links,
    /// oldest first; `None` while they are read.
    shots: Option<Vec<String>>,
    linked: Vec<String>,
}

impl JournalEditor {
    fn new(
        id: GameId,
        day: NaiveDate,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entry = models(cx)
            .library
            .read(cx)
            .user
            .journal_entry(&id.key(), day)
            .cloned();
        let existed = entry.is_some();
        let entry = entry.unwrap_or_default();
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("ui.journal.title_placeholder"))
                .default_value(entry.title.clone())
        });
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(5, 14)
                .placeholder(t("ui.journal.text_placeholder"))
                .default_value(entry.text.clone())
        });
        let mut editor = Self {
            id,
            day,
            label,
            existed,
            title,
            text,
            shots: None,
            linked: entry.screenshots,
        };
        editor.load_shots(cx);
        editor
    }

    /// Find the day's screenshots of the game, off the UI thread.
    fn load_shots(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let (key, day) = (self.id.key(), self.day);
        let task = crate::state::run_infallible(cx, async move {
            let reader = b.clone();
            b.spawn_blocking(move || {
                reader
                    .screenshots()
                    .shots
                    .into_iter()
                    .filter(|s| s.game == key && local_day(s.taken) == Some(day))
                    .filter_map(|s| s.path.to_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .await
            .ok()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await.flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.shots = Some(candidates(found, &this.linked));
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.linked.iter().position(|p| p == path) {
            self.linked.remove(ix);
        } else if self.linked.len() < MAX_LINKED_SCREENSHOTS {
            self.linked.push(path.to_string());
        }
        cx.notify();
    }

    fn entry(&self, cx: &App) -> JournalEntry {
        JournalEntry {
            title: self.title.read(cx).value().to_string(),
            text: self.text.read(cx).value().to_string(),
            screenshots: self.linked.clone(),
            updated: String::new(),
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        write(self.id.clone(), self.day, Some(self.entry(cx)), cx);
        window.close_dialog(cx);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (id, day) = (self.id.clone(), self.day);
        let question = t_args("ui.journal.delete_title", &[("day", &self.label)]);
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let id = id.clone();
                alert
                    .title(question.clone())
                    .description(t("ui.journal.delete_description"))
                    .confirm()
                    .ok_text(t("ui.journal.delete"))
                    .ok_variant(ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        write(id.clone(), day, None, cx);
                        true
                    })
            });
        });
    }
}

/// The screenshots to offer: those found for the day, then any the note
/// links that were not found (a folder no longer looked in), without
/// repeats, in order.
fn candidates(found: Vec<String>, linked: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for path in found.into_iter().rev().chain(linked.iter().cloned()) {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

/// A unix time as a local calendar day.
fn local_day(unix: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(unix, 0)
        .earliest()
        .map(|t| t.date_naive())
}

/// Write (or remove) a note, off the UI thread.
fn write(id: GameId, day: NaiveDate, entry: Option<JournalEntry>, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(
        cx,
        async move { b.userdata.set_journal_entry(&id, day, entry) },
    )
    .detach();
}

impl Render for JournalEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, primary, primary_fg, fill, radius) = (
            theme.muted_foreground,
            theme.border,
            theme.primary,
            theme.primary_foreground,
            theme.muted,
            theme.radius,
        );
        let width = physical_width(rems(SHOT_REM), window);
        let shots = match &self.shots {
            None => div()
                .py_2()
                .child(Spinner::new().small())
                .into_any_element(),
            Some(list) if list.is_empty() => div()
                .text_sm()
                .text_color(muted)
                .child(t("ui.journal.no_screenshots"))
                .into_any_element(),
            Some(list) => h_flex()
                .flex_wrap()
                .gap_2()
                .children(list.iter().enumerate().map(|(ix, path)| {
                    let selected = self.linked.contains(path);
                    let image = art_img(&shot_url(path), width, false, cx);
                    let p = path.clone();
                    div()
                        .id(ElementId::Name(format!("journal-shot-{ix}").into()))
                        .keyboard_focusable(cx)
                        .relative()
                        .w(rems(SHOT_REM))
                        .h(rems(SHOT_REM * 9. / 16.))
                        .rounded(radius)
                        .overflow_hidden()
                        .cursor_pointer()
                        .bg(fill)
                        .border_2()
                        .border_color(if selected { primary } else { border })
                        .map(|d| match image {
                            Some(img) => d.child(img.size_full().object_fit(ObjectFit::Cover)),
                            None => d.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(muted)
                                    .child(Icon::new(IconName::Image)),
                            ),
                        })
                        .when(selected, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top_1()
                                    .right_1()
                                    .rounded_full()
                                    .p_0p5()
                                    .bg(primary)
                                    .text_color(primary_fg)
                                    .child(Icon::new(IconName::Check).xsmall()),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(&p, cx)))
                }))
                .into_any_element(),
        };
        v_flex()
            .id("journal-editor")
            .gap_4()
            .child(Input::new(&self.title))
            .child(Textarea::new(&self.text))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .font_semibold()
                            .child(Icon::new(IconName::Images).small())
                            .child(t("ui.journal.screenshots")),
                    )
                    .child(shots),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(div().when(self.existed, |d| {
                        d.child(
                            Button::new("journal-delete")
                                .ghost()
                                .icon(IconName::Trash)
                                .label(t("ui.journal.delete"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.delete(window, cx)),
                                ),
                        )
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("journal-cancel")
                                    .ghost()
                                    .label(t("ui.journal.cancel"))
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("journal-save")
                                    .primary()
                                    .label(t("ui.journal.save"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                                    ),
                            ),
                    ),
            )
    }
}

/// Open the note of `id` on `day` to read, write or delete it. `title` is
/// the game's title and `date` the day as the caller shows it.
pub fn open_editor(
    id: GameId,
    day: NaiveDate,
    title: &str,
    date: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let label = t_args("ui.journal.label", &[("title", title), ("date", date)]);
    let view = cx.new(|cx| JournalEditor::new(id, day, label.clone(), window, cx));
    let width = window.rem_size() * 40.;
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let input = view.read(cx).title.clone();
            input.focus_handle(cx).focus(window, cx);
        }
        dialog.title(label.clone()).w(width).child(view.clone())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_the_days_screenshots_then_linked_ones_not_found() {
        // Found newest first, as the index lists them.
        let found = vec!["/b.png".to_string(), "/a.png".to_string()];
        let linked = vec!["/a.png".to_string(), "/gone.png".to_string()];
        assert_eq!(
            candidates(found, &linked),
            ["/a.png", "/b.png", "/gone.png"]
        );
        assert!(candidates(Vec::new(), &[]).is_empty());
    }
}
