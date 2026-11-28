//! Interactive keyboard shortcut recorder for MangoHud shortcuts
//! (`toggle_hud` and `toggle_preset`).
//!
//! Captures GPUI keystrokes and translates them to case-sensitive X11 keysyms
//! via [`irori_runtime::hud::keysym`], defaulting modifier tokens to the left
//! side (`Shift_L`, `Control_L`, `Alt_L`) while offering a compact `L / R`
//! side switch whenever the recorded combo includes modifiers.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
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
    reset: SharedString,
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
            reset: t("settings.hud.shortcut.reset"),
            side_left: t("settings.hud.shortcut.side_left"),
            side_right: t("settings.hud.shortcut.side_right"),
            side_left_tooltip: t("settings.hud.shortcut.side_left_tooltip"),
            side_right_tooltip: t("settings.hud.shortcut.side_right_tooltip"),
            wayland_note: t("settings.hud.shortcut.wayland_note"),
        }
    }
}

/// Format an X11 keysym combo (`Shift_R+F12`) into concise display keycaps
/// (`["Shift (R)", "F12"]`) so `render()` allocates zero strings.
fn parse_keycaps(combo: &str) -> Vec<SharedString> {
    combo
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part {
            "Shift_L" => SharedString::new_static("Shift (L)"),
            "Shift_R" => SharedString::new_static("Shift (R)"),
            "Control_L" => SharedString::new_static("Ctrl (L)"),
            "Control_R" => SharedString::new_static("Ctrl (R)"),
            "Alt_L" => SharedString::new_static("Alt (L)"),
            "Alt_R" => SharedString::new_static("Alt (R)"),
            other => SharedString::from(other.to_string()),
        })
        .collect()
}

/// Human-readable shortcut label (`Shift (R) + F12`) for inline help text.
pub(crate) fn format_shortcut_display(combo: &str) -> String {
    let caps = parse_keycaps(combo);
    if caps.is_empty() {
        return format_shortcut_display(DEFAULT_TOGGLE_HUD);
    }
    caps.iter()
        .map(SharedString::as_ref)
        .collect::<Vec<_>>()
        .join(" + ")
}

type ShortcutChangeHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App)>;

/// Interactive MangoHud shortcut recorder.
pub struct ShortcutRecorder {
    combo: SharedString,
    keycaps: Vec<SharedString>,
    default_combo: &'static str,
    show_wayland_note: bool,
    id_recorder: SharedString,
    id_record: SharedString,
    id_side_group: SharedString,
    id_side_left: SharedString,
    id_side_right: SharedString,
    id_reset: SharedString,
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
        Self::with_options(
            "hud-shortcut",
            DEFAULT_TOGGLE_HUD,
            initial_combo,
            true,
            on_change,
            window,
            cx,
        )
    }

    /// Create a shortcut recorder with a unique element ID prefix, custom reset
    /// default combo, and optional Wayland note footer.
    pub fn with_options(
        id_prefix: &'static str,
        default_combo: &'static str,
        initial_combo: impl Into<SharedString>,
        show_wayland_note: bool,
        on_change: impl Fn(SharedString, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let raw = initial_combo.into();
        let combo = if raw.trim().is_empty() {
            SharedString::new_static(default_combo)
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
        let keycaps = parse_keycaps(&combo);
        Self {
            combo,
            keycaps,
            default_combo,
            show_wayland_note,
            id_recorder: SharedString::from(format!("{id_prefix}-recorder")),
            id_record: SharedString::from(format!("{id_prefix}-record")),
            id_side_group: SharedString::from(format!("{id_prefix}-side-group")),
            id_side_left: SharedString::from(format!("{id_prefix}-side-left")),
            id_side_right: SharedString::from(format!("{id_prefix}-side-right")),
            id_reset: SharedString::from(format!("{id_prefix}-reset")),
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

    /// Current recorded shortcut string.
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
            SharedString::new_static(self.default_combo)
        } else {
            next
        };
        if self.combo == normalized {
            return;
        }
        self.keycaps = parse_keycaps(&normalized);
        self.combo = normalized;
        self.has_modifiers = combo_has_modifiers(&self.combo);
        self.modifier_side = combo_modifier_side(&self.combo);
        cx.notify();
    }

    fn apply_combo(&mut self, combo: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.keycaps = parse_keycaps(&combo);
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
            self.apply_combo(SharedString::new_static(self.default_combo), window, cx);
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
        let (muted_fg, cap_bg, mono_font) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.secondary,
                theme.mono_font_family.clone(),
            )
        };
        let main_btn = if self.listening {
            Button::new(self.id_record.clone())
                .primary()
                .small()
                .icon(IconName::Keyboard)
                .label(self.labels.listening.clone())
                .tooltip(self.labels.click_to_record.clone())
        } else {
            let mut caps_row = h_flex().items_center().gap_1();
            for (ix, cap) in self.keycaps.iter().enumerate() {
                if ix > 0 {
                    caps_row = caps_row.child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(SharedString::new_static("+")),
                    );
                }
                caps_row = caps_row.child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(cap_bg)
                        .font_family(mono_font.clone())
                        .text_xs()
                        .child(cap.clone()),
                );
            }
            Button::new(self.id_record.clone())
                .outline()
                .small()
                .icon(IconName::Keyboard)
                .accessibility_label(self.combo.clone())
                .tooltip(self.labels.click_to_record.clone())
                .child(caps_row)
        }
        .on_click(cx.listener(|this, _, window, cx| this.start_listening(window, cx)));

        let left_active = self.modifier_side == Some(ModifierSide::Left);
        let right_active = self.modifier_side == Some(ModifierSide::Right);
        let is_default = self.combo.as_ref() == self.default_combo;
        let id_side_group = self.id_side_group.clone();
        let id_left = self.id_side_left.clone();
        let id_right = self.id_side_right.clone();
        let id_reset = self.id_reset.clone();

        let reset_btn = Button::new(id_reset)
            .ghost()
            .xsmall()
            .icon(IconName::RotateCcw)
            .disabled(is_default)
            .accessibility_label(self.labels.reset.clone())
            .tooltip(self.labels.reset.clone())
            .on_click(cx.listener(|this, _, window, cx| {
                this.listening = false;
                this.apply_combo(SharedString::new_static(this.default_combo), window, cx);
            }));

        v_flex()
            .id(self.id_recorder.clone())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .gap_1()
            .items_end()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .when(self.has_modifiers, |row| {
                        let side_group = ButtonGroup::new(id_side_group)
                            .outline()
                            .xsmall()
                            .child(
                                Button::new(id_left)
                                    .label(self.labels.side_left.clone())
                                    .tooltip(self.labels.side_left_tooltip.clone())
                                    .selected(left_active),
                            )
                            .child(
                                Button::new(id_right)
                                    .label(self.labels.side_right.clone())
                                    .tooltip(self.labels.side_right_tooltip.clone())
                                    .selected(right_active),
                            )
                            .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                                match clicked.first().copied() {
                                    Some(0) => this.choose_side(ModifierSide::Left, window, cx),
                                    Some(1) => this.choose_side(ModifierSide::Right, window, cx),
                                    _ => {}
                                }
                            }));
                        row.child(side_group)
                    })
                    .child(main_btn)
                    .child(reset_btn),
            )
            .when(self.show_wayland_note, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .max_w_80()
                        .text_right()
                        .child(self.labels.wayland_note.clone()),
                )
            })
    }
}
