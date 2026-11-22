//! Interactive keyboard shortcut recorder for MangoHud's `toggle_hud` binding.
//!
//! Captures GPUI keystrokes and translates them to case-sensitive X11 keysyms
//! via [`irori_runtime::hud::keysym`], defaulting modifier tokens to the left
//! side (`Shift_L`, `Control_L`, `Alt_L`) while offering a compact `L / R`
//! side switch whenever the recorded combo includes modifiers.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::ModifierSide;
use irori_runtime::hud::keysym::{
    DEFAULT_TOGGLE_HUD, KeyModifiers, combo_has_modifiers, combo_modifier_side, format_toggle_hud,
    set_modifier_side,
};
use irori_ui::i18n::t;

use crate::i18n::LanguageStamp;

struct ShortcutLabels {
    listening: SharedString,
    click_to_record: SharedString,
    side_left: SharedString,
    side_right: SharedString,
    side_left_tooltip: SharedString,
    side_right_tooltip: SharedString,
    wayland_note: SharedString,
}

impl ShortcutLabels {
    fn load() -> Self {
        Self {
            listening: t("settings.hud.shortcut.listening"),
            click_to_record: t("settings.hud.shortcut.click_to_record"),
            side_left: t("settings.hud.shortcut.side_left"),
            side_right: t("settings.hud.shortcut.side_right"),
            side_left_tooltip: t("settings.hud.shortcut.side_left_tooltip"),
            side_right_tooltip: t("settings.hud.shortcut.side_right_tooltip"),
            wayland_note: t("settings.hud.shortcut.wayland_note"),
        }
    }
}

type ShortcutChangeHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// Interactive MangoHud toggle-shortcut recorder.
pub struct ShortcutRecorder {
    combo: SharedString,
    listening: bool,
    has_modifiers: bool,
    modifier_side: Option<ModifierSide>,
    focus_handle: FocusHandle,
    on_change: ShortcutChangeHandler,
    labels: ShortcutLabels,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl ShortcutRecorder {
    pub fn new(
        initial_combo: impl Into<SharedString>,
        on_change: impl Fn(SharedString, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let raw = initial_combo.into();
        let combo = if raw.trim().is_empty() {
            SharedString::new_static(DEFAULT_TOGGLE_HUD)
        } else {
            raw
        };
        let has_modifiers = combo_has_modifiers(&combo);
        let modifier_side = combo_modifier_side(&combo);
        let focus_handle = cx.focus_handle();
        let blur_sub = cx.on_blur(&focus_handle, window, |this, _, cx| {
            if this.listening {
                this.listening = false;
                cx.notify();
            }
        });
        Self {
            combo,
            listening: false,
            has_modifiers,
            modifier_side,
            focus_handle,
            on_change: Rc::new(on_change),
            labels: ShortcutLabels::load(),
            language: LanguageStamp::new(),
            _subscriptions: vec![blur_sub],
        }
    }

    /// Current recorded `toggle_hud` string.
    pub fn combo(&self) -> &SharedString {
        &self.combo
    }

    /// Whether the recorder is actively listening for a keystroke.
    pub fn is_listening(&self) -> bool {
        self.listening
    }

    /// Sync the displayed combo from external settings updates without firing `on_change`.
    pub fn set_combo(&mut self, combo: impl Into<SharedString>, cx: &mut Context<Self>) {
        let next = combo.into();
        let normalized = if next.trim().is_empty() {
            SharedString::new_static(DEFAULT_TOGGLE_HUD)
        } else {
            next
        };
        if self.combo == normalized {
            return;
        }
        self.combo = normalized;
        self.has_modifiers = combo_has_modifiers(&self.combo);
        self.modifier_side = combo_modifier_side(&self.combo);
        cx.notify();
    }

    fn apply_combo(&mut self, combo: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.combo = combo.clone();
        self.has_modifiers = combo_has_modifiers(&self.combo);
        self.modifier_side = combo_modifier_side(&self.combo);
        let cb = self.on_change.clone();
        window.defer(cx, move |window, cx| {
            cb(combo, window, cx);
        });
        cx.notify();
    }

    fn start_listening(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.listening = true;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.listening {
            return;
        }
        cx.stop_propagation();
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("escape") {
            self.listening = false;
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("backspace") {
            self.listening = false;
            self.apply_combo(SharedString::new_static(DEFAULT_TOGGLE_HUD), window, cx);
            return;
        }
        let mods = KeyModifiers {
            shift: event.keystroke.modifiers.shift,
            control: event.keystroke.modifiers.control,
            alt: event.keystroke.modifiers.alt,
        };
        if let Some(formatted) = format_toggle_hud(mods, key, ModifierSide::Left) {
            self.listening = false;
            self.apply_combo(SharedString::from(formatted), window, cx);
        }
    }

    fn choose_side(&mut self, side: ModifierSide, window: &mut Window, cx: &mut Context<Self>) {
        let updated = set_modifier_side(&self.combo, side);
        self.apply_combo(SharedString::from(updated), window, cx);
    }
}

impl Focusable for ShortcutRecorder {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ShortcutRecorder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            self.labels = ShortcutLabels::load();
        }
        let theme = cx.theme();
        let muted_fg = theme.muted_foreground;
        let display_label = if self.listening {
            self.labels.listening.clone()
        } else {
            self.combo.clone()
        };
        let main_btn = if self.listening {
            Button::new("hud-shortcut-record")
                .primary()
                .small()
                .icon(IconName::Keyboard)
                .label(display_label)
                .tooltip(self.labels.click_to_record.clone())
        } else {
            Button::new("hud-shortcut-record")
                .outline()
                .small()
                .icon(IconName::Keyboard)
                .label(display_label)
                .tooltip(self.labels.click_to_record.clone())
        }
        .on_click(cx.listener(|this, _, window, cx| this.start_listening(window, cx)));

        let left_active = self.modifier_side == Some(ModifierSide::Left);
        let right_active = self.modifier_side == Some(ModifierSide::Right);

        v_flex()
            .id("hud-shortcut-recorder")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .gap_1()
            .items_end()
            .child(h_flex().gap_1p5().items_center().child(main_btn).when(
                self.has_modifiers,
                |row| {
                    let left_btn = if left_active {
                        Button::new("hud-shortcut-side-left").primary()
                    } else {
                        Button::new("hud-shortcut-side-left").outline()
                    }
                    .xsmall()
                    .label(self.labels.side_left.clone())
                    .tooltip(self.labels.side_left_tooltip.clone())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_side(ModifierSide::Left, window, cx);
                    }));

                    let right_btn = if right_active {
                        Button::new("hud-shortcut-side-right").primary()
                    } else {
                        Button::new("hud-shortcut-side-right").outline()
                    }
                    .xsmall()
                    .label(self.labels.side_right.clone())
                    .tooltip(self.labels.side_right_tooltip.clone())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.choose_side(ModifierSide::Right, window, cx);
                    }));

                    row.child(left_btn).child(right_btn)
                },
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .max_w_80()
                    .text_right()
                    .child(self.labels.wayland_note.clone()),
            )
    }
}
