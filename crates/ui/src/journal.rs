//! The journal note editor, shared by the game page's timeline, the stats
//! page's Journal tab and the in-game overlay: one note of a day, with its
//! time (empty for a note about the whole day), a title, the note itself,
//! the capture it is about and the screenshots taken that day to link to it.
//! Saving writes the library file, so it runs off the UI thread; deleting a
//! note asks first.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::Path;

use chrono::{Local, NaiveDate, NaiveTime, TimeZone as _, Timelike as _};
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

use crate::components::{art_img, clip_mark, is_clip, physical_width};
use crate::i18n::{t, t_args};
use crate::style::KeyboardFocusable as _;
use crate::{backend, models};

/// Width of a screenshot in the picker, in rem.
const SHOT_REM: f32 = 8.;
/// Width of the capture a note is about, in rem.
const CAPTURE_REM: f32 = 16.;

/// A screenshot as the art store addresses it.
pub fn shot_url(path: &str) -> String {
    format!("file://{path}")
}

/// A note's time as the journal shows it ("21:40"); nothing for a note
/// about the whole day.
pub fn time_label(time: Option<NaiveTime>) -> Option<SharedString> {
    time.map(|t| t.format("%H:%M").to_string().into())
}

/// The time of day typed in the editor: `HH:MM` (or `H:MM`, `HH.MM`); empty
/// is a note about the whole day, and anything else isn't a time.
fn parse_time(text: &str) -> Result<Option<NaiveTime>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let text = text.replace('.', ":");
    NaiveTime::parse_from_str(&text, "%H:%M")
        .map(Some)
        .map_err(|_| ())
}

/// The time a new note of `day` gets: now (to the minute) when it is today,
/// else none: a note written later about another day is about all of it.
fn new_note_time(day: NaiveDate, now: chrono::NaiveDateTime) -> Option<NaiveTime> {
    (day == now.date())
        .then(|| NaiveTime::from_hms_opt(now.hour(), now.minute(), 0).unwrap_or_default())
}

struct JournalEditor {
    id: GameId,
    day: NaiveDate,
    /// The note as it was opened; a new note has no id yet.
    note: JournalEntry,
    /// The game and day, for the delete question.
    label: SharedString,
    existed: bool,
    time: Entity<InputState>,
    /// The time typed can't be read.
    bad_time: bool,
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
        note: Option<String>,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let user = models(cx).library.read(cx).user.clone();
        let existing = note
            .as_deref()
            .and_then(|n| user.journal_note(&id.key(), n))
            .map(|(_, e)| e.clone());
        let existed = existing.is_some();
        let entry = existing.unwrap_or_else(|| JournalEntry {
            time: new_note_time(day, Local::now().naive_local()),
            ..Default::default()
        });
        let time = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("ui.journal.whole_day"))
                .default_value(time_label(entry.time).unwrap_or_default())
        });
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
        let linked = entry.screenshots.clone();
        let mut editor = Self {
            id,
            day,
            note: entry,
            label,
            existed,
            time,
            bad_time: false,
            title,
            text,
            shots: None,
            linked,
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
                let capture = this.note.capture.clone();
                let mut list = candidates(found, &this.linked);
                list.retain(|p| Some(p) != capture.as_ref());
                this.shots = Some(list);
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

    fn entry(&self, time: Option<NaiveTime>, cx: &App) -> JournalEntry {
        JournalEntry {
            time,
            title: self.title.read(cx).value().to_string(),
            text: self.text.read(cx).value().to_string(),
            screenshots: self.linked.clone(),
            updated: String::new(),
            ..self.note.clone()
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(time) = parse_time(&self.time.read(cx).value()) else {
            self.bad_time = true;
            cx.notify();
            return;
        };
        write(self.id.clone(), self.day, self.entry(time, cx), cx);
        window.close_dialog(cx);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (id, note) = (self.id.clone(), self.note.id.clone());
        let question = t_args("ui.journal.delete_title", &[("day", &self.label)]);
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| {
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (id, note) = (id.clone(), note.clone());
                alert
                    .title(question.clone())
                    .description(t("ui.journal.delete_description"))
                    .confirm()
                    .ok_text(t("ui.journal.delete"))
                    .ok_variant(ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        let (id, note) = (id.clone(), note.clone());
                        let b = backend(cx);
                        crate::state::run_infallible(cx, async move {
                            b.userdata.remove_journal_entry(&id, &note)
                        })
                        .detach();
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

/// Write a note, off the UI thread.
fn write(id: GameId, day: NaiveDate, entry: JournalEntry, cx: &mut App) {
    let b = backend(cx);
    crate::state::run_infallible(cx, async move {
        b.userdata.set_journal_entry(&id, day, entry);
    })
    .detach();
}

/// A picture of a note, cut to 16:9.
fn picture(path: &str, width_rem: f32, window: &Window, cx: &mut App) -> gpui_kit::Div {
    let width = physical_width(rems(width_rem), window);
    let image = art_img(&shot_url(path), width, false, cx);
    let theme = cx.theme();
    div()
        .relative()
        .w(rems(width_rem))
        .h(rems(width_rem * 9. / 16.))
        .flex_shrink_0()
        .rounded(theme.radius)
        .overflow_hidden()
        .bg(theme.muted)
        .map(|d| match image {
            Some(img) => d.child(img.size_full().object_fit(ObjectFit::Cover)),
            None => d.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Image)),
            ),
        })
        .when(is_clip(Path::new(path)), |d| {
            d.child(clip_mark(Path::new(path), cx))
        })
}

impl Render for JournalEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, primary, primary_fg, danger) = (
            theme.muted_foreground,
            theme.border,
            theme.primary,
            theme.primary_foreground,
            theme.danger,
        );
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
                    let p = path.clone();
                    picture(path, SHOT_REM, window, cx)
                        .id(ElementId::Name(format!("journal-shot-{ix}").into()))
                        .keyboard_focusable(cx)
                        .cursor_pointer()
                        .border_2()
                        .border_color(if selected { primary } else { border })
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
        let capture = self
            .note
            .capture
            .clone()
            .map(|path| picture(&path, CAPTURE_REM, window, cx));
        v_flex()
            .id("journal-editor")
            .gap_4()
            .when_some(capture, |c, capture| {
                c.child(
                    v_flex().gap_1().child(capture).child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("ui.journal.about_capture")),
                    ),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        v_flex()
                            .w(rems(7.))
                            .flex_shrink_0()
                            .gap_1()
                            .child(Input::new(&self.time).aria_label(t("ui.journal.time")))
                            .when(self.bad_time, |c| {
                                c.child(
                                    div()
                                        .text_xs()
                                        .text_color(danger)
                                        .child(t("ui.journal.bad_time")),
                                )
                            }),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&self.title))),
            )
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

/// Open one note of `id` on `day` to read, write or delete it, or, with
/// `note` `None`, a new note of that day. `title` is the game's title and
/// `date` the day as the caller shows it.
pub fn open_editor(
    id: GameId,
    day: NaiveDate,
    note: Option<String>,
    title: &str,
    date: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let label = t_args("ui.journal.label", &[("title", title), ("date", date)]);
    let view = cx.new(|cx| JournalEditor::new(id, day, note, label.clone(), window, cx));
    let width = window.rem_size() * 40.;
    let focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            // A note about a capture starts at its words.
            let editor = view.read(cx);
            if editor.note.capture.is_some() || editor.existed {
                editor.text.focus_handle(cx).focus(window, cx);
            } else {
                editor.title.focus_handle(cx).focus(window, cx);
            }
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

    #[test]
    fn times_are_read_as_typed_and_new_notes_of_today_get_now() {
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(parse_time(""), Ok(None));
        assert_eq!(parse_time(" 21:40 "), Ok(at(21, 40)));
        assert_eq!(parse_time("9:05"), Ok(at(9, 5)));
        assert_eq!(parse_time("21.40"), Ok(at(21, 40)));
        assert_eq!(parse_time("late"), Err(()));
        assert_eq!(parse_time("25:00"), Err(()));

        let now = NaiveDate::from_ymd_opt(2026, 11, 2)
            .unwrap()
            .and_hms_opt(21, 40, 31)
            .unwrap();
        assert_eq!(new_note_time(now.date(), now), at(21, 40));
        let yesterday = now.date().pred_opt().unwrap();
        assert_eq!(new_note_time(yesterday, now), None);
        assert_eq!(time_label(at(9, 5)).as_deref(), Some("09:05"));
    }
}
