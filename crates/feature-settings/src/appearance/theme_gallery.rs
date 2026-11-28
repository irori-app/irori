//! Visual theme family gallery: one card per theme family with live palette
//! dots, plus a compact sub-variant selector whenever the chosen family offers
//! multiple flavours for the current colour mode (such as Catppuccin Frappe,
//! Macchiato, and Mocha or Tokyo Night, Storm, and Moon).

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    px, rems, transparent_black,
};
use irori_core::settings::ThemePreference;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::theme::{Desktop, ThemeFamilyPreview, ThemeVariantPreview};

use super::{set_ui, ui_prefs};

fn default_family_title(cx: &App) -> SharedString {
    match irori_ui::theme::followed_desktop(cx) {
        Some(Desktop::Omarchy) => t("settings.theme.omarchy"),
        Some(Desktop::Cosmic) => t("settings.theme.cosmic"),
        Some(Desktop::Kde) => t("settings.theme.kde"),
        Some(Desktop::Wallpaper) => t("settings.theme.wallpaper"),
        None => t("settings.theme.default"),
    }
}

fn pick_theme_name(name: String, window: &mut Window, cx: &mut App) {
    let followed = irori_ui::theme::followed_desktop(cx).is_some();
    let dark_mode = cx.theme().mode.is_dark();
    let target_dark = irori_ui::theme::is_dark_theme(&name, cx);
    set_ui(window, cx, |ui| {
        if name.is_empty() && followed {
            ui.theme = ThemePreference::System;
        } else if let Some(dark) = target_dark
            && dark != dark_mode
        {
            ui.theme = if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            };
        }
        ui.theme_name = name;
    });
}

fn family_subtitle(fam: &ThemeFamilyPreview) -> SharedString {
    let dark_count = fam.variants.iter().filter(|v| v.dark).count();
    let light_count = fam.variants.len().saturating_sub(dark_count);
    match (dark_count, light_count) {
        (0, 1) => t("settings.option.theme_mode.light"),
        (1, 0) => t("settings.option.theme_mode.dark"),
        (1, 1) => t("settings.theme.pair_label"),
        (d, 0) => t_args(
            "settings.theme.dark_only_count",
            &[("count", &d.to_string())],
        ),
        (d, l) => t_args(
            "settings.theme.multi_count",
            &[("dark", &d.to_string()), ("light", &l.to_string())],
        ),
    }
}

/// Representative variant shown on a family card for `dark` mode.
fn card_swatch<'a>(
    fam: &'a ThemeFamilyPreview,
    active_name: &str,
    dark: bool,
) -> Option<&'a ThemeVariantPreview> {
    if let Some(exact) = fam.variants.iter().find(|v| v.name.as_ref() == active_name) {
        return Some(exact);
    }
    fam.variants
        .iter()
        .rev()
        .find(|v| v.dark == dark)
        .or_else(|| fam.variants.first())
}

fn palette_dots(swatch: &ThemeVariantPreview) -> impl IntoElement {
    let dot = |fill: Hsla, ring: Option<Hsla>| {
        div()
            .w(px(13.))
            .h(px(13.))
            .flex_shrink_0()
            .rounded_full()
            .bg(fill)
            .when_some(ring, |d, color| d.border_1().border_color(color))
    };
    h_flex()
        .items_center()
        .gap_1()
        .child(dot(swatch.background, Some(swatch.border)))
        .child(dot(swatch.secondary, Some(swatch.border)))
        .child(dot(swatch.primary, None))
}

/// Theme family card gallery with automatic sub-variant strip.
pub(crate) struct ThemeGallery {
    _subscriptions: Vec<Subscription>,
}

impl ThemeGallery {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        Self {
            _subscriptions: vec![cx.observe(&settings, |_, _, cx| cx.notify())],
        }
    }

    #[allow(clippy::too_many_arguments)] // Presentational card builder for family swatches.
    fn render_card(
        id: ElementId,
        debug_id: String,
        title: SharedString,
        subtitle: SharedString,
        swatch: &ThemeVariantPreview,
        selected: bool,
        target_name: String,
        cx: &App,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, primary, muted, hover_border) = (
            theme.background,
            theme.border,
            theme.primary,
            theme.muted_foreground,
            theme.foreground.opacity(0.35),
        );
        v_flex()
            .id(id)
            .debug_selector(move || debug_id.clone())
            .w(rems(9.6))
            .min_w_0()
            .px_2p5()
            .py_2()
            .gap_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .keyboard_focusable(cx)
            .when(selected, |d| {
                d.bg(primary.opacity(0.12)).border_2().border_color(primary)
            })
            .when(!selected, |d| {
                d.bg(bg)
                    .border_1()
                    .border_color(border)
                    .hover(move |s| s.border_color(hover_border))
            })
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .child(palette_dots(swatch))
                    .when(selected, |row| {
                        row.child(div().size_2().flex_shrink_0().rounded_full().bg(primary))
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .min_w_0()
                    .truncate()
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .min_w_0()
                    .truncate()
                    .child(subtitle),
            )
            .on_click(move |_, window, cx| pick_theme_name(target_name.clone(), window, cx))
    }
}

impl Render for ThemeGallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = ui_prefs(cx);
        let dark = cx.theme().mode.is_dark();
        let active_name = prefs.theme_name.trim().to_string();
        let default_swatch = irori_ui::theme::default_theme_swatch(dark, cx);
        let families = irori_ui::theme::theme_families(cx);

        let mut cards = Vec::with_capacity(families.len() + 1);
        cards.push(Self::render_card(
            ElementId::Name("theme-family-default".into()),
            "theme-family-default".into(),
            default_family_title(cx),
            t("settings.theme.default_subtitle"),
            &default_swatch,
            active_name.is_empty(),
            String::new(),
            cx,
        ));

        let mut active_family_variants: Vec<ThemeVariantPreview> = Vec::new();
        for fam in &families {
            let selected = !active_name.is_empty()
                && fam
                    .variants
                    .iter()
                    .any(|v| v.name.as_ref() == active_name.as_str());
            if selected {
                let mode_variants: Vec<ThemeVariantPreview> = fam
                    .variants
                    .iter()
                    .filter(|v| v.dark == dark)
                    .cloned()
                    .collect();
                if mode_variants.len() > 1 {
                    active_family_variants = mode_variants;
                }
            }
            let Some(swatch) = card_swatch(fam, &active_name, dark) else {
                continue;
            };
            let slug = fam.name.to_ascii_lowercase().replace(' ', "-");
            let id_str = format!("theme-family-{slug}");
            cards.push(Self::render_card(
                ElementId::Name(id_str.clone().into()),
                id_str,
                fam.name.clone(),
                family_subtitle(fam),
                swatch,
                selected,
                swatch.name.to_string(),
                cx,
            ));
        }

        let theme = cx.theme();
        let (border, bg, primary, fg, muted, hover_bg) = (
            theme.border,
            theme.background,
            theme.primary,
            theme.foreground,
            theme.muted_foreground,
            theme.secondary_hover,
        );
        let active_fg = legible(primary, &[bg, theme.secondary], TEXT_CONTRAST, cx);
        let radius = theme.radius;

        v_flex()
            .w_full()
            .gap_2p5()
            .child(h_flex().w_full().flex_wrap().gap_2().children(cards))
            .when(!active_family_variants.is_empty(), |col| {
                col.child(
                    h_flex()
                        .w_full()
                        .pt_2()
                        .border_t_1()
                        .border_color(border)
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("settings.theme.family_variant")),
                        )
                        .child(
                            h_flex()
                                .p_0p5()
                                .gap_1()
                                .rounded(radius)
                                .bg(bg)
                                .border_1()
                                .border_color(border)
                                .flex_wrap()
                                .children(active_family_variants.into_iter().map(|v| {
                                    let is_active = v.name.as_ref() == active_name.as_str();
                                    let target = v.name.to_string();
                                    let slug = v.short_label.to_ascii_lowercase().replace(' ', "-");
                                    let id =
                                        ElementId::Name(format!("theme-variant-{slug}").into());
                                    h_flex()
                                        .id(id)
                                        .px_2p5()
                                        .py_1()
                                        .rounded_md()
                                        .border_1()
                                        .cursor_pointer()
                                        .keyboard_focusable(cx)
                                        .text_xs()
                                        .when(is_active, |d| {
                                            d.bg(primary.opacity(0.18))
                                                .border_color(primary)
                                                .text_color(active_fg)
                                                .font_semibold()
                                        })
                                        .when(!is_active, |d| {
                                            d.border_color(transparent_black())
                                                .text_color(muted)
                                                .hover(move |s| s.bg(hover_bg).text_color(fg))
                                        })
                                        .child(v.short_label)
                                        .on_click(move |_, window, cx| {
                                            pick_theme_name(target.clone(), window, cx)
                                        })
                                })),
                        ),
                )
            })
    }
}
