//! The game page's Setup card: what the game's own settings change from the
//! game defaults, by section ("Wine: Wine version, Esync"), each opening its
//! section, and the way to every setting and to the "Changed" list. It reads
//! the same registry and overrides as the settings page, so both always
//! agree on what changed.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div,
};
use irori_core::settings::{GameSettings, GameSettingsOverrides};
use irori_core::{GameId, Runner};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;
use irori_ui::{Route, backend, models, navigate};

use crate::registry::{FIELDS, FieldKind, FieldSection, FieldSpec};
use crate::value::{effective_overrides, get_path};
use crate::widgets::{muted, panel};

/// Fields named in a section's line before "and N more".
const NAMED: usize = 3;

/// The game's changed settings by section, in the settings page's order:
/// fields a game can set whose override differs from the defaults. Panels
/// and actions hold no value of their own and are left out; so are fields
/// that do nothing for a Steam game when `steam`.
pub fn changed_sections(
    defaults: &GameSettings,
    overrides: &GameSettingsOverrides,
    steam: bool,
) -> Vec<(FieldSection, Vec<&'static FieldSpec>)> {
    let changed = effective_overrides(defaults, overrides);
    let defaults_json = serde_json::to_value(defaults).unwrap_or_default();
    let effective = serde_json::to_value(overrides.apply(defaults)).unwrap_or_default();
    let mut out: Vec<(FieldSection, Vec<&'static FieldSpec>)> = Vec::new();
    for spec in FIELDS.iter().filter(|f| {
        f.applies_to_game(true)
            && !matches!(f.kind, FieldKind::Panel(_) | FieldKind::Action(_))
            && (!steam || f.applies_to_steam())
            && changed.iter().any(|k| k == f.override_key())
            && get_path(&defaults_json, f.path) != get_path(&effective, f.path)
    }) {
        match out.iter_mut().find(|(s, _)| *s == spec.section) {
            Some((_, fields)) => fields.push(spec),
            None => out.push((spec.section, vec![spec])),
        }
    }
    out
}

/// The game settings section (route id) a field section opens.
pub fn section_route(section: FieldSection) -> &'static str {
    match section {
        FieldSection::Wine => "wine",
        FieldSection::Performance => "performance",
        FieldSection::Gpu => "gpu",
        FieldSection::Launch => "launch",
        FieldSection::CloudSaves => "saves",
        FieldSection::Artwork => "art",
        _ => "",
    }
}

/// "Wine version, Esync and 2 more" for one section's line.
fn named(fields: &[&FieldSpec]) -> SharedString {
    let labels: Vec<String> = fields
        .iter()
        .take(NAMED)
        .map(|f| t(f.label).to_string())
        .collect();
    let list = labels.join(&t("settings.setup.separator"));
    match fields.len().saturating_sub(NAMED) {
        0 => list.into(),
        more => t_args(
            "settings.setup.and_more",
            &[("list", &list), ("count", &more.to_string())],
        ),
    }
}

/// The Setup card of one game.
pub struct SetupCard {
    id: GameId,
    _subscriptions: Vec<Subscription>,
}

impl SetupCard {
    pub fn new(id: GameId, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        Self {
            id,
            _subscriptions: vec![cx.observe(&settings, |_, _, cx| cx.notify())],
        }
    }
}

/// The Setup card of `id`, for the game page (through
/// `irori_ui::hooks::game_setup`).
pub fn setup_card(id: GameId, _window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| SetupCard::new(id, cx)).into()
}

impl Render for SetupCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let b = backend(cx);
        let defaults = b.settings.app().game.clone();
        let overrides = b.settings.overrides(&self.id);
        let sections = changed_sections(&defaults, &overrides, self.id.runner == Runner::Steam);
        let count: usize = sections.iter().map(|(_, f)| f.len()).sum();
        let muted_fg = cx.theme().muted_foreground;
        let rows = sections.iter().enumerate().map(|(ix, (section, fields))| {
            let route = section_route(*section);
            let id = self.id.clone();
            h_flex()
                .id(ElementId::Name(format!("setup-section-{ix}").into()))
                .keyboard_focusable(cx)
                .cursor_pointer()
                .gap_2()
                .px_3()
                .py_1p5()
                .items_start()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_medium().child(section.label()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .min_w_0()
                                .child(named(fields)),
                        ),
                )
                .child(
                    Icon::new(IconName::ChevronRight)
                        .small()
                        .text_color(muted_fg),
                )
                .on_click(move |_, _, cx| {
                    navigate(Route::GameSettings(id.clone(), route.into()), cx)
                })
        });
        let all = {
            let id = self.id.clone();
            Button::new("setup-all")
                .ghost()
                .xsmall()
                .icon(IconName::Settings)
                .label(t("settings.setup.all"))
                .on_click(move |_, _, cx| navigate(Route::GameSettings(id.clone(), "".into()), cx))
        };
        let review = (count > 0).then(|| {
            let id = self.id.clone();
            Button::new("setup-changed")
                .ghost()
                .xsmall()
                .icon(IconName::RotateCcw)
                .label(t("settings.setup.review"))
                .on_click(move |_, _, cx| {
                    navigate(Route::GameSettings(id.clone(), "changed".into()), cx)
                })
        });
        let summary = match count {
            0 => t("settings.setup.defaults"),
            1 => t("settings.setup.changed_one"),
            n => t_args("settings.setup.changed_other", &[("count", &n.to_string())]),
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child(t("settings.setup.title")),
            )
            .child(muted(summary, cx).text_xs())
            .when(count > 0, |v| v.child(panel(cx).children(rows)))
            .child(h_flex().flex_wrap().gap_1().child(all).children(review))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn changes_are_grouped_by_section_and_only_real_changes_count() {
        let defaults = GameSettings::default();
        let overrides: GameSettingsOverrides = serde_json::from_value(json!({
            "enableEsync": !defaults.enable_esync,
            // The same as the defaults: no change.
            "showFps": defaults.show_fps,
            "launcherArgs": "-dx11",
        }))
        .unwrap();
        let sections = changed_sections(&defaults, &overrides, false);
        let paths: Vec<(FieldSection, Vec<&str>)> = sections
            .iter()
            .map(|(s, f)| (*s, f.iter().map(|f| f.path).collect()))
            .collect();
        assert!(
            paths.contains(&(FieldSection::Wine, vec!["enableEsync"])),
            "{paths:?}"
        );
        assert!(
            paths.contains(&(FieldSection::Launch, vec!["launcherArgs"])),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|(_, f)| f.contains(&"showFps")));
        // Steam starts its games with its own Wine: only Irori's own count.
        let steam = changed_sections(&defaults, &overrides, true);
        assert!(steam.iter().all(|(s, _)| *s != FieldSection::Wine));
        assert!(changed_sections(&defaults, &GameSettingsOverrides::default(), false).is_empty());
    }

    #[test]
    fn every_game_section_has_a_route() {
        for section in FieldSection::GAME {
            assert!(!section_route(section).is_empty(), "{section:?}");
        }
    }
}
