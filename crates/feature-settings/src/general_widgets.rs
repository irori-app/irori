//! General settings widgets: compact achievement card detail chips and
//! subtle cross-links for options that live on other Settings pages.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};
use irori_core::settings::AchievementToastPrefs;
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use crate::view::SettingsSection;

#[derive(Clone, Copy)]
struct DetailChip {
    id: &'static str,
    label_key: &'static str,
    get: fn(&AchievementToastPrefs) -> bool,
    set: fn(&mut AchievementToastPrefs, bool),
}

const DETAIL_CHIPS: &[DetailChip] = &[
    DetailChip {
        id: "ach-chip-game",
        label_key: "settings.achievement_details.game",
        get: |p| p.show_game,
        set: |p, on| p.show_game = on,
    },
    DetailChip {
        id: "ach-chip-description",
        label_key: "settings.achievement_details.description",
        get: |p| p.show_description,
        set: |p, on| p.show_description = on,
    },
    DetailChip {
        id: "ach-chip-rarity",
        label_key: "settings.achievement_details.rarity",
        get: |p| p.show_rarity,
        set: |p, on| p.show_rarity = on,
    },
    DetailChip {
        id: "ach-chip-progress",
        label_key: "settings.achievement_details.progress",
        get: |p| p.show_progress,
        set: |p, on| p.show_progress = on,
    },
    DetailChip {
        id: "ach-chip-points",
        label_key: "settings.achievement_details.points",
        get: |p| p.show_points,
        set: |p, on| p.show_points = on,
    },
];

/// Compact row of toggle chips controlling which extra lines appear on an
/// achievement unlock card.
pub(crate) struct AchievementDetailsPanel {
    _sub: Subscription,
}

impl AchievementDetailsPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        Self {
            _sub: cx.observe(&settings, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for AchievementDetailsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = models(cx).settings.read(cx).app.achievement_toast.clone();
        h_flex()
            .id("panel-achievement-details")
            .debug_selector(|| "panel-achievement-details".into())
            .flex_wrap()
            .gap_1p5()
            .children(DETAIL_CHIPS.iter().map(|chip| {
                let on = (chip.get)(&prefs);
                let set = chip.set;
                let id = chip.id;
                Button::new(ElementId::Name(id.into()))
                    .debug_selector(move || id.into())
                    .xsmall()
                    .when(on, |b| b.primary().icon(IconName::Check))
                    .when(!on, |b| b.outline())
                    .label(t(chip.label_key))
                    .on_click(move |_, _, cx| {
                        backend(cx)
                            .settings
                            .update_app(|s| set(&mut s.achievement_toast, !on));
                    })
            }))
    }
}

struct FooterLink {
    section: SettingsSection,
    hint_key: &'static str,
}

fn footer_links(tab: &str) -> Vec<FooterLink> {
    match tab {
        "basics" => vec![
            FooterLink {
                section: SettingsSection::Advanced,
                hint_key: "settings.general_footer.advanced_paths",
            },
            FooterLink {
                section: SettingsSection::Storage,
                hint_key: "settings.general_footer.storage",
            },
        ],
        "window" => vec![
            FooterLink {
                section: SettingsSection::Controller,
                hint_key: "settings.general_footer.controller_big_picture",
            },
            FooterLink {
                section: SettingsSection::Logs,
                hint_key: "settings.general_footer.logs",
            },
        ],
        "integrations" => {
            let mut links = vec![
                FooterLink {
                    section: SettingsSection::Accounts,
                    hint_key: "settings.general_footer.accounts",
                },
                FooterLink {
                    section: SettingsSection::Appearance,
                    hint_key: "settings.general_footer.appearance",
                },
            ];
            if cfg!(target_os = "linux") {
                links.push(FooterLink {
                    section: SettingsSection::Advanced,
                    hint_key: "settings.general_footer.advanced_windows_files",
                });
            }
            links
        }
        _ => Vec::new(),
    }
}

/// Subtle footer strip pointing users to options that live on other Settings
/// pages (for example Wine/Steam paths in Advanced or stores in Accounts).
pub(crate) fn render_general_footer(
    tab: &str,
    cx: &mut Context<crate::view::SettingsView>,
) -> Option<AnyElement> {
    let links: Vec<FooterLink> = footer_links(tab)
        .into_iter()
        .filter(|link| link.section.supported_on_host())
        .collect();
    if links.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let (border, muted_fg) = (theme.border, theme.muted_foreground);
    Some(
        h_flex()
            .id(ElementId::Name(format!("general-footer-{tab}").into()))
            .debug_selector(|| "general-footer".into())
            .w_full()
            .flex_wrap()
            .items_center()
            .gap_2()
            .pt_4()
            .mt_3()
            .border_t_1()
            .border_color(border)
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(t("settings.general_footer.looking_for")),
            )
            .children(links.into_iter().enumerate().map(|(ix, link)| {
                let target = link.section;
                let label = format!("{} · {}", target.label(), t(link.hint_key));
                Button::new(ElementId::Name(
                    format!("general-footer-link-{tab}-{ix}").into(),
                ))
                .outline()
                .xsmall()
                .icon(target.icon())
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_section_from_footer(target, window, cx)
                }))
            }))
            .into_any_element(),
    )
}
