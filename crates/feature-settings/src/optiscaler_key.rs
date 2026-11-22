//! Interactive GPUI key-capture control for OptiScaler's `[Menu] ShortcutKey`.
//!
//! Zero background hooks or polling: only attaches `.capture_key_down` while
//! focused in listening mode, mapping the pressed GPUI key directly through
//! `irori_reshade::optiscaler_keys::gpui_key_to_vk`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use irori_reshade::optiscaler_keys::{
    SHORTCUT_AUTO, gpui_key_to_vk, is_modifier_key, vk_display_badge,
};
use irori_ui::i18n::t;

/// Emitted when the user captures a new key or resets the shortcut.
pub(crate) struct KeyCaptured(pub(crate) Option<String>);

/// Interactive button that listens for one keypress and converts it to a
/// Windows Virtual-Key hex code (`0x2E` for Delete, `0x2D` for Insert, etc.).
pub(crate) struct OptiscalerKeyCapture {
    id: SharedString,
    focus: FocusHandle,
    listening: bool,
    disabled: bool,
    /// `None` leaves `[Menu] ShortcutKey` at its default (`auto` / Insert).
    value: Option<String>,
}

impl EventEmitter<KeyCaptured> for OptiscalerKeyCapture {}

impl Focusable for OptiscalerKeyCapture {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl OptiscalerKeyCapture {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        value: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            focus: cx.focus_handle(),
            listening: false,
            disabled: false,
            value: clean_value(value),
        }
    }

    pub(crate) fn set_value(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        let next = clean_value(value);
        if self.value != next {
            self.value = next;
            cx.notify();
        }
    }

    pub(crate) fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            if disabled {
                self.listening = false;
            }
            cx.notify();
        }
    }

    fn start_listening(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.listening = true;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_listening(&mut self, cx: &mut Context<Self>) {
        if self.listening {
            self.listening = false;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.listening {
            return;
        }
        cx.stop_propagation();
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("escape") {
            self.cancel_listening(cx);
            return;
        }
        if is_modifier_key(key) {
            return;
        }
        if let Some(vk) = gpui_key_to_vk(key) {
            let next = Some(vk.to_string());
            self.value = next.clone();
            self.listening = false;
            cx.emit(KeyCaptured(next));
            cx.notify();
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.listening = false;
        self.value = None;
        cx.emit(KeyCaptured(None));
        cx.notify();
    }

    fn button_label(&self) -> SharedString {
        if self.listening {
            return t("settings.optiscaler.shortcut.listening");
        }
        match &self.value {
            Some(v) => vk_display_badge(v).into_owned().into(),
            None => t("settings.optiscaler.shortcut.default_insert"),
        }
    }

    fn can_reset(&self) -> bool {
        match &self.value {
            Some(v) => !v.is_empty() && !v.eq_ignore_ascii_case(SHORTCUT_AUTO),
            None => false,
        }
    }
}

fn clean_value(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Render for OptiscalerKeyCapture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.listening && !self.focus.is_focused(window) {
            self.listening = false;
        }
        let listening = self.listening;
        let disabled = self.disabled;
        let can_reset = self.can_reset();
        let label = self.button_label();
        let btn_id = ElementId::Name(format!("{}-btn", self.id).into());
        let reset_id = ElementId::Name(format!("{}-reset", self.id).into());

        let mut capture_btn = Button::new(btn_id)
            .small()
            .icon(IconName::Keyboard)
            .label(label)
            .disabled(disabled)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.listening {
                    this.cancel_listening(cx);
                } else {
                    this.start_listening(window, cx);
                }
            }));
        capture_btn = if listening {
            capture_btn.primary()
        } else {
            capture_btn.outline()
        };

        div()
            .id(ElementId::Name(self.id.clone()))
            .track_focus(&self.focus)
            .when(listening, |el| {
                el.capture_key_down(cx.listener(Self::on_key_down))
            })
            .child(h_flex().gap_2().items_center().child(capture_btn).when(
                can_reset && !listening,
                |row| {
                    row.child(
                        Button::new(reset_id)
                            .ghost()
                            .small()
                            .label(t("settings.optiscaler.shortcut.reset"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
                    )
                },
            ))
    }
}
