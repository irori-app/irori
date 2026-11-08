//! The hidden-games PIN as screens show it: whether hidden games are locked
//! now, the dialog that unlocks them, and the one that sets, changes or
//! removes the PIN (`irori_services::hidden_lock`).

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_services::hidden_lock::{MIN_PIN, valid_pin};

use crate::i18n::{t, t_args};
use crate::state::{backend, run};

/// Whether hidden games are behind a PIN that hasn't been entered.
pub fn locked(cx: &App) -> bool {
    crate::models(cx).library.read(cx).snapshot.hidden_locked
}

/// Whether a PIN guards hidden games at all.
pub fn pin_set(cx: &App) -> bool {
    backend(cx).hidden_pin_set()
}

/// Lock hidden games again.
pub fn lock(cx: &mut App) {
    let b = backend(cx);
    run(cx, async move {
        let locker = b.clone();
        b.spawn_blocking(move || locker.lock_hidden()).await?;
        Ok(())
    })
    .detach();
}

/// Runs once hidden games are unlocked.
pub type OnUnlocked = Rc<dyn Fn(&mut Window, &mut App)>;

/// Ask for the PIN; `on_unlocked` runs once it was right. Without a PIN or
/// already unlocked, `on_unlocked` runs at once.
pub fn open_unlock_dialog(on_unlocked: OnUnlocked, window: &mut Window, cx: &mut App) {
    if !locked(cx) {
        on_unlocked(window, cx);
        return;
    }
    let dialog = cx.new(|cx| PinDialog::new(Mode::Unlock(on_unlocked), window, cx));
    let width = window.rem_size() * 24.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("ui.hidden_lock.unlock_title"))
            .width(width)
            .child(dialog.clone())
    });
}

/// Set a PIN, or change or remove the one there is.
pub fn open_pin_dialog(window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| PinDialog::new(Mode::Manage, window, cx));
    let width = window.rem_size() * 26.;
    let title = if pin_set(cx) {
        t("ui.hidden_lock.change_title")
    } else {
        t("ui.hidden_lock.set_title")
    };
    window.open_dialog(cx, move |d, _, _| {
        d.title(title.clone()).width(width).child(dialog.clone())
    });
}

enum Mode {
    Unlock(OnUnlocked),
    Manage,
}

struct PinDialog {
    mode: Mode,
    had_pin: bool,
    current: Entity<InputState>,
    new: Entity<InputState>,
    confirm: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl PinDialog {
    fn new(mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: SharedString, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(placeholder)
            })
        };
        let current = field(t("ui.hidden_lock.current_pin"), window, cx);
        let new = field(t("ui.hidden_lock.new_pin"), window, cx);
        let confirm = field(t("ui.hidden_lock.confirm_pin"), window, cx);
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     e: &InputEvent,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.submit(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&current, window, enter),
            cx.subscribe_in(&new, window, enter),
            cx.subscribe_in(&confirm, window, enter),
        ];
        let first = match mode {
            Mode::Unlock(_) => &current,
            Mode::Manage if pin_set(cx) => &current,
            Mode::Manage => &new,
        };
        let focus = first.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        Self {
            mode,
            had_pin: pin_set(cx),
            current,
            new,
            confirm,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match &self.mode {
            Mode::Unlock(_) => self.unlock(window, cx),
            Mode::Manage => {
                let new = Self::value(&self.new, cx);
                self.save(Some(new), window, cx)
            }
        }
    }

    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pin = Self::value(&self.current, cx);
        let b = backend(cx);
        self.busy = true;
        cx.notify();
        let task = run(cx, async move {
            let checker = b.clone();
            Ok(b.spawn_blocking(move || checker.unlock_hidden(&pin))
                .await?)
        });
        cx.spawn_in(window, async move |this, cx| {
            let unlocked = task.await.unwrap_or(false);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if unlocked {
                    let Mode::Unlock(on_unlocked) = &this.mode else {
                        return;
                    };
                    let on_unlocked = on_unlocked.clone();
                    window.close_dialog(cx);
                    on_unlocked(window, cx);
                } else {
                    this.error = Some(t("ui.hidden_lock.wrong_pin"));
                    this.current
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Set the new PIN, or remove it with `None`.
    fn save(&mut self, new: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pin) = &new {
            if !valid_pin(pin) {
                self.error = Some(t_args(
                    "ui.hidden_lock.pin_rule",
                    &[("count", &MIN_PIN.to_string())],
                ));
                cx.notify();
                return;
            }
            if *pin != Self::value(&self.confirm, cx) {
                self.error = Some(t("ui.hidden_lock.pins_differ"));
                cx.notify();
                return;
            }
        }
        let current = self.had_pin.then(|| Self::value(&self.current, cx));
        let removing = new.is_none();
        let b = backend(cx);
        self.busy = true;
        cx.notify();
        let task = run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || saver.set_hidden_pin(current.as_deref(), new.as_deref()))
                .await?
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        let done = if removing {
                            t("ui.hidden_lock.removed")
                        } else {
                            t("ui.hidden_lock.saved")
                        };
                        window.push_notification(Notification::success(done), cx);
                    }
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for PinDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let unlocking = matches!(self.mode, Mode::Unlock(_));
        let primary = if unlocking {
            t("ui.hidden_lock.unlock")
        } else {
            t("ui.hidden_lock.save")
        };
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child(if unlocking {
                t("ui.hidden_lock.unlock_description")
            } else {
                t("ui.hidden_lock.description")
            }))
            .when(unlocking || self.had_pin, |col| {
                col.child(Input::new(&self.current).mask_toggle())
            })
            .when(!unlocking, |col| {
                col.child(Input::new(&self.new).mask_toggle())
                    .child(Input::new(&self.confirm).mask_toggle())
            })
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_sm().text_color(danger).child(e)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .flex_wrap()
                    .when(!unlocking && self.had_pin, |row| {
                        row.child(
                            Button::new("pin-remove")
                                .ghost()
                                .small()
                                .label(t("ui.hidden_lock.remove"))
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save(None, window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("pin-ok")
                            .primary()
                            .small()
                            .label(primary)
                            .loading(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
