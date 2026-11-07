//! The Appearance section's own controls: the accent colour picker, the
//! interface font picker and the live preview of the library at the top of
//! the section.
//!
//! The pickers write `iroriUi.accentColor` and `iroriUi.fontFamily` and apply
//! the theme at once, as the settings form does for every `iroriUi.` key. The
//! preview draws from the same settings, theme and art the library uses, so
//! what it shows is what the library will look like.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::GameInfo;
use irori_core::settings::{LibraryBackground, UiPrefs};
use irori_ui::components::{BackdropInk, GameCover, library_backdrop, library_backdrop_url};
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use crate::i18n::LanguageStamp;

/// Accents offered at a glance, beside the full picker: a spread of hues
/// that each read on Irori's light and dark surfaces once held legible.
const FEATURED_ACCENTS: [u32; 10] = [
    0x3b82f6, 0x6366f1, 0x8b5cf6, 0xd946ef, 0xec4899, 0xef4444, 0xf97316, 0xeab308, 0x22c55e,
    0x14b8a6,
];

/// Covers the preview shows.
const PREVIEW_COVERS: usize = 5;
/// Width of a preview cover at comfortable density, in rem.
const PREVIEW_COVER_REM: f32 = 4.5;
/// Space between preview covers at comfortable density, in rem.
const PREVIEW_GAP_REM: f32 = 0.75;

/// Persist a change to the interface preferences and show it right away.
fn set_ui(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut UiPrefs)) {
    let b = backend(cx);
    b.settings.update_app(|s| f(&mut s.ui));
    irori_ui::theme::apply(&b.settings.app().ui, Some(window), cx);
}

fn ui_prefs(cx: &App) -> UiPrefs {
    models(cx).settings.read(cx).app.ui.clone()
}

/// `#rrggbb` of a colour.
pub(crate) fn hex(color: Hsla) -> String {
    let rgb = color.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(rgb.r), byte(rgb.g), byte(rgb.b))
}

/// The accent colour picker. Empty means the theme's own accent, or the
/// system's where Irori follows it.
pub(crate) struct AccentPicker {
    state: Entity<ColorPickerState>,
    _subscriptions: Vec<Subscription>,
}

impl AccentPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picked = ui_prefs(cx).accent_rgb();
        let settings = models(cx).settings.clone();
        let state = cx.new(|cx| {
            let state = ColorPickerState::new(window, cx);
            match picked {
                Some(rgb) => state.default_value(gpui_kit::rgb(rgb)),
                None => state,
            }
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &state,
                window,
                |_, _, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    let value = color.map(hex).unwrap_or_default();
                    set_ui(window, cx, |ui| ui.accent_color = value);
                },
            ),
            cx.observe_in(&settings, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
        ];
        Self {
            state,
            _subscriptions: subscriptions,
        }
    }

    /// Follow a change made elsewhere (a restored backup, a reset).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = ui_prefs(cx).accent_rgb();
        let shown = self.state.read(cx).value().map(hex);
        let wanted = picked.map(|rgb| hex(gpui_kit::rgb(rgb).into()));
        if shown == wanted {
            return;
        }
        self.state.update(cx, |s, cx| match picked {
            Some(rgb) => s.set_value(gpui_kit::rgb(rgb), window, cx),
            None => s.clear_value(window, cx),
        });
        cx.notify();
    }
}

impl Render for AccentPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picked = ui_prefs(cx).accent_rgb().is_some();
        // The wallpaper's colours are a theme like any other, not a desktop's.
        let desktop = irori_ui::theme::followed_desktop(cx)
            .filter(|d| *d != irori_ui::theme::Desktop::Wallpaper);
        let default_label = if desktop.is_some() {
            t("settings.accent.use_desktop")
        } else {
            t("settings.accent.use_theme")
        };
        h_flex()
            .gap_2()
            .child(
                ColorPicker::new(&self.state)
                    .featured_colors(
                        FEATURED_ACCENTS
                            .iter()
                            .map(|rgb| gpui_kit::rgb(*rgb).into())
                            .collect(),
                    )
                    .accessibility_label(t("settings.field.ui_accent_color.label")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .min_w_0()
                    .truncate()
                    .child(if picked {
                        self.state
                            .read(cx)
                            .value()
                            .map(|c| SharedString::from(hex(c)))
                            .unwrap_or_default()
                    } else {
                        t("settings.accent.default")
                    }),
            )
            .child(
                Button::new("accent-default")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RotateCcw)
                    .tooltip(default_label)
                    .disabled(!picked)
                    .on_click(|_, window, cx| {
                        set_ui(window, cx, |ui| ui.accent_color.clear());
                    }),
            )
    }
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

/// A small library drawn with the current appearance settings: background,
/// accent, density and font, over a few of the user's own covers.
pub(crate) struct AppearancePreview {
    _subscriptions: Vec<Subscription>,
}

impl AppearancePreview {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let art = irori_ui::art::art_store(cx);
        Self {
            _subscriptions: vec![
                cx.observe(&m.settings, |_, _, cx| cx.notify()),
                cx.observe(&m.library, |_, _, cx| cx.notify()),
                cx.observe(&art, |_, _, cx| cx.notify()),
            ],
        }
    }

    /// Covers to show: recently played games first, then any with art.
    fn games(cx: &App) -> Vec<Arc<GameInfo>> {
        let b = backend(cx);
        let mut games = b.recent_games(false);
        let library = models(cx).library.read(cx).snapshot.clone();
        for game in library.games.iter() {
            if games.len() >= PREVIEW_COVERS {
                break;
            }
            if game.cover_art().is_some() && !games.iter().any(|g| g.id() == game.id()) {
                games.push(game.clone());
            }
        }
        games.truncate(PREVIEW_COVERS);
        games
    }
}

impl Render for AppearancePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = ui_prefs(cx);
        let games = Self::games(cx);
        let url = library_backdrop_url(&prefs, games.first().map(|g| g.as_ref()), cx);
        let ink = BackdropInk::of(url.as_deref(), cx);
        let theme = cx.theme();
        let (border, radius, ring) = (theme.border, theme.radius_lg, theme.ring);
        let cover = rems(PREVIEW_COVER_REM * prefs.density.cover_scale());
        let gap = rems(PREVIEW_GAP_REM * prefs.density.spacing_scale());
        let note = match prefs.library_background {
            LibraryBackground::None => None,
            _ if irori_ui::theme::reduce_transparency(cx) => {
                Some(t("settings.preview.background_off_transparency"))
            }
            LibraryBackground::Picture if prefs.library_background_picture.trim().is_empty() => {
                Some(t("settings.preview.background_no_picture"))
            }
            _ => None,
        };
        let covers: Vec<AnyElement> = if games.is_empty() {
            (0..PREVIEW_COVERS)
                .map(|ix| {
                    div()
                        .w(cover)
                        .h(rems(cover.0 * 1.5))
                        .flex_shrink_0()
                        .rounded(radius)
                        .bg(ink.panel)
                        .border_1()
                        .border_color(border)
                        .when(ix == 0, |d| d.border_2().border_color(ring))
                        .into_any_element()
                })
                .collect()
        } else {
            games
                .iter()
                .enumerate()
                .map(|(ix, game)| {
                    v_flex()
                        .w(cover)
                        .flex_shrink_0()
                        .gap_1()
                        .child(
                            div()
                                .rounded(radius)
                                .when(ix == 0, |d| d.border_2().border_color(ring))
                                .child(GameCover::new(game.clone()).width(cover)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .min_w_0()
                                .truncate()
                                .text_color(ink.text)
                                .child(game.title.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        };
        v_flex()
            .gap_2()
            .mb_4()
            .child(
                div()
                    .relative()
                    .w_full()
                    .overflow_hidden()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .bg(theme.background)
                    .child(library_backdrop(&prefs, url))
                    .child(
                        v_flex()
                            .relative()
                            .p_4()
                            .gap(gap)
                            .child(
                                h_flex()
                                    .gap_3()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_lg()
                                                    .font_semibold()
                                                    .text_color(ink.text)
                                                    .truncate()
                                                    .child(t("settings.preview.library")),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ink.muted)
                                                    .truncate()
                                                    .child(t("settings.preview.sample")),
                                            ),
                                    )
                                    .child(
                                        Button::new("preview-play")
                                            .primary()
                                            .small()
                                            .icon(IconName::Play)
                                            .label(t("settings.preview.play")),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap(gap)
                                    .items_start()
                                    .overflow_hidden()
                                    .children(covers),
                            ),
                    ),
            )
            .when_some(note, |v, note| {
                v.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                )
            })
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
