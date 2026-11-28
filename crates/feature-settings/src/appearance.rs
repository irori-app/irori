//! The Appearance section's own controls: the theme family gallery, the
//! circular Hue/Saturation accent colour picker, the interface font picker,
//! the Library → Look shortcut card, and the pinned live library preview.
//!
//! Each control writes `iroriUi.*` and applies the theme immediately, matching
//! the rest of the settings form.

mod accent;
mod look_shortcut;
mod preview;
mod theme_gallery;

use std::rc::Rc;

use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::settings::UiPrefs;
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use crate::i18n::LanguageStamp;

pub(crate) use accent::AccentPicker;
pub(crate) use look_shortcut::LibraryLookShortcut;
pub(crate) use preview::AppearancePreview;
pub(crate) use theme_gallery::ThemeGallery;

/// Accents offered at a glance, both inline and inside the colour wheel: a
/// spread of hues that each read on Irori's light and dark surfaces once held
/// legible.
pub(super) const FEATURED_ACCENTS: [u32; 10] = [
    0x3b82f6, 0x6366f1, 0x8b5cf6, 0xd946ef, 0xec4899, 0xef4444, 0xf97316, 0xeab308, 0x22c55e,
    0x14b8a6,
];

/// Persist a change to the interface preferences and show it right away.
pub(super) fn set_ui(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut UiPrefs)) {
    let b = backend(cx);
    b.settings.update_app(|s| f(&mut s.ui));
    irori_ui::theme::apply(&b.settings.app().ui, Some(window), cx);
}

pub(super) fn ui_prefs(cx: &App) -> UiPrefs {
    models(cx).settings.read(cx).app.ui.clone()
}

/// `#rrggbb` of a colour.
pub(crate) fn hex(color: Hsla) -> String {
    let rgb = color.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(rgb.r), byte(rgb.g), byte(rgb.b))
}

/// A font family in the picker, drawn in itself.
#[derive(Clone)]
struct FontItem {
    /// The family; empty for Irori's own.
    family: SharedString,
    label: SharedString,
}

impl SelectItem for FontItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let family = if self.family.is_empty() {
            SharedString::new_static(irori_ui::theme::FONT_FAMILY)
        } else {
            self.family.clone()
        };
        div()
            .min_w_0()
            .truncate()
            .font_family(family)
            .child(self.label.clone())
    }

    fn value(&self) -> &SharedString {
        &self.family
    }
}

fn font_items(names: &[SharedString]) -> Vec<FontItem> {
    let mut items = vec![FontItem {
        family: SharedString::default(),
        label: t("settings.font.default"),
    }];
    items.extend(
        names
            .iter()
            .filter(|name| name.as_ref() != irori_ui::theme::FONT_FAMILY)
            .map(|name| FontItem {
                family: name.clone(),
                label: name.clone(),
            }),
    );
    items
}

/// The interface font picker: Manrope first, then every installed family,
/// each drawn in its own face.
pub(crate) struct FontPicker {
    state: Entity<SelectState<Vec<FontItem>>>,
    names: Rc<[SharedString]>,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl FontPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let names = irori_ui::theme::font_names(cx);
        let settings = models(cx).settings.clone();
        let current = SharedString::from(ui_prefs(cx).font_family);
        let state = cx.new(|cx| {
            let mut state = SelectState::new(font_items(&names), None, window, cx).searchable(true);
            state.set_selected_value(&current, window, cx);
            state
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &state,
                window,
                |_, _, event: &SelectEvent<Vec<FontItem>>, window, cx| {
                    if let SelectEvent::Confirm(Some(family)) = event {
                        let family = family.to_string();
                        set_ui(window, cx, |ui| ui.font_family = family);
                    }
                },
            ),
            cx.observe_in(&settings, window, |this, _, window, cx| {
                let current = SharedString::from(ui_prefs(cx).font_family);
                this.state
                    .update(cx, |s, cx| s.set_selected_value(&current, window, cx));
            }),
        ];
        Self {
            state,
            names,
            language: LanguageStamp::new(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for FontPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            let items = font_items(&self.names);
            let current = SharedString::from(ui_prefs(cx).font_family);
            cx.defer_in(window, move |this, window, cx| {
                this.state.update(cx, |s, cx| {
                    s.set_items(items, window, cx);
                    s.set_selected_value(&current, window, cx);
                });
            });
        }
        Select::new(&self.state)
            .id("field-fontFamily")
            .accessibility_label(t("settings.field.ui_font_family.label"))
            .search_placeholder(t("settings.font.search"))
            .w_64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_through_the_settings_value() {
        for rgb in FEATURED_ACCENTS {
            let color: Hsla = gpui_kit::rgb(rgb).into();
            let prefs = UiPrefs {
                accent_color: hex(color),
                ..UiPrefs::default()
            };
            assert_eq!(prefs.accent_rgb(), Some(rgb), "{rgb:06x}");
        }
    }

    #[test]
    fn manrope_leads_the_font_list_once() {
        let names: Vec<SharedString> = ["Cantarell", "Manrope", "Noto Sans"]
            .map(SharedString::from)
            .to_vec();
        let items = font_items(&names);
        assert_eq!(items[0].family, "");
        let families: Vec<&str> = items[1..].iter().map(|i| i.family.as_ref()).collect();
        assert_eq!(families, ["Cantarell", "Noto Sans"]);
    }
}
