//! "Edit time played…": set a game's playtime by hand, in hours and
//! minutes, or go back to the time Irori counted. The edit is kept apart
//! from what was counted (`UserData::playtime_adjust`), so sessions played
//! later add to it and sync never counts it twice
//! (`Backend::set_playtime`).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::i18n::format_playtime;

/// The seconds `hours` and `minutes` (as typed) stand for; `None` when
/// either isn't a whole number or minutes reach an hour.
fn parse(hours: &str, minutes: &str) -> Option<u64> {
    let read = |s: &str| match s.trim() {
        "" => Some(0),
        s => s.parse::<u64>().ok(),
    };
    let (h, m) = (read(hours)?, read(minutes)?);
    (m < 60 && h <= 1_000_000).then_some(h * 3600 + m * 60)
}

/// Open "Edit time played…" for `id`.
pub fn open_playtime_dialog(id: GameId, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let library = models(cx).library.read(cx);
    let title = library
        .snapshot
        .get(&id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let key = id.key();
    let total = library
        .user
        .playtime
        .get(&key)
        .map_or(0, irori_services::userdata::record_seconds);
    let counted = library.user.counted_seconds(&key);
    let edited = library.user.playtime_adjust.contains_key(&key);
    let dialog = cx.new(|cx| PlaytimeDialog::new(id, total, counted, edited, window, cx));
    let width = window.rem_size() * 26.;
    let heading = t_args("game.playtime_edit.title", &[("title", &title)]);
    window.open_dialog(cx, move |d, _, _| {
        d.title(heading.clone()).width(width).child(dialog.clone())
    });
}

struct PlaytimeDialog {
    id: GameId,
    counted: u64,
    edited: bool,
    hours: Entity<InputState>,
    minutes: Entity<InputState>,
    invalid: bool,
    saving: bool,
    _subscriptions: Vec<Subscription>,
}

impl PlaytimeDialog {
    fn new(
        id: GameId,
        total: u64,
        counted: u64,
        edited: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let minutes_total = total / 60;
        let hours = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0")
                .default_value((minutes_total / 60).to_string())
        });
        let minutes = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0")
                .default_value((minutes_total % 60).to_string())
        });
        let enter =
            |this: &mut Self, e: &InputEvent, window: &mut Window, cx: &mut Context<Self>| {
                if matches!(e, InputEvent::PressEnter { .. }) {
                    this.save(window, cx);
                }
            };
        let subscriptions = vec![
            cx.subscribe_in(&hours, window, move |this, _, e, window, cx| {
                enter(this, e, window, cx)
            }),
            cx.subscribe_in(&minutes, window, move |this, _, e, window, cx| {
                enter(this, e, window, cx)
            }),
        ];
        hours.focus_handle(cx).focus(window, cx);
        Self {
            id,
            counted,
            edited,
            hours,
            minutes,
            invalid: false,
            saving: false,
            _subscriptions: subscriptions,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hours = self.hours.read(cx).value().to_string();
        let minutes = self.minutes.read(cx).value().to_string();
        match parse(&hours, &minutes) {
            Some(seconds) => self.write(Some(seconds), window, cx),
            None => {
                self.invalid = true;
                cx.notify();
            }
        }
    }

    /// Keep the time (or the counted one, with `None`) off the UI thread,
    /// then close.
    fn write(&mut self, seconds: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.invalid = false;
        cx.notify();
        let (b, id) = (backend(cx), self.id.clone());
        let task = irori_ui::run(cx, async move {
            let writer = b.clone();
            b.spawn_blocking(move || writer.set_playtime(&id, seconds))
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
                        Notification::error(format!("{e:#}")).title(t("game.playtime_edit.failed")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for PlaytimeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let counted = format_playtime(self.counted / 60);
        let field = |input: &Entity<InputState>, unit: &'static str| {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(muted)
                        .child(t(unit)),
                )
        };
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("game.playtime_edit.help")),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(field(&self.hours, "game.playtime_edit.hours"))
                    .child(field(&self.minutes, "game.playtime_edit.minutes")),
            )
            .when(self.invalid, |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(t("game.playtime_edit.invalid")),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t_args("game.playtime_edit.counted", &[("time", &counted)])),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(self.edited, |row| {
                        row.child(
                            Button::new("playtime-reset")
                                .ghost()
                                .small()
                                .label(t("game.playtime_edit.reset"))
                                .disabled(self.saving)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.write(None, window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("playtime-save")
                            .primary()
                            .small()
                            .label(t("game.playtime_edit.save"))
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn hours_and_minutes_read_as_seconds() {
        assert_eq!(parse("12", "30"), Some(12 * 3600 + 30 * 60));
        assert_eq!(parse(" 3 ", ""), Some(3 * 3600));
        assert_eq!(parse("", ""), Some(0));
        assert_eq!(parse("1.5", "0"), None);
        assert_eq!(parse("1", "60"), None);
        assert_eq!(parse("-1", "0"), None);
    }
}
