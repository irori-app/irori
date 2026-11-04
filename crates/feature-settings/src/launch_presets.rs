//! "Launch presets" panel of the Launch section: named sets of launch
//! settings (Irori's own and the user's), applied to the game being edited,
//! to the game defaults or to several games at once, and saved from them.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString, Size,
    Styled as _, Subscription, Window, div, px, rems, size,
};
use irori_core::{GameId, LaunchPreset};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::form::FormTarget;
use crate::i18n::LanguageStamp;
use crate::widgets::{muted, panel};

/// Translation key stem of a built-in preset (`builtin:steam-deck-handheld`
/// → `settings.presets.builtin.steam_deck_handheld`).
pub(crate) fn builtin_key(id: &str) -> Option<String> {
    let slug = id.strip_prefix("builtin:")?;
    Some(format!(
        "settings.presets.builtin.{}",
        slug.replace('-', "_")
    ))
}

/// Translation key naming one settings key a preset holds.
pub(crate) fn setting_label_key(key: &str) -> &'static str {
    match key {
        "gamescope" => "settings.group.gamescope",
        crate::launch_programs::KEY => "settings.field.panel_launch_programs.label",
        _ => crate::registry::find(key).map_or("settings.presets.other_setting", |f| f.label),
    }
}

/// The preset's name in the interface language.
pub(crate) fn preset_name(preset: &LaunchPreset) -> SharedString {
    match builtin_key(&preset.id) {
        Some(stem) => t(&format!("{stem}.name")),
        None => preset.name.clone().into(),
    }
}

/// What the preset does: Irori's own description, or the settings it holds.
fn preset_description(preset: &LaunchPreset) -> SharedString {
    if let Some(stem) = builtin_key(&preset.id) {
        return t(&format!("{stem}.description"));
    }
    let names: Vec<String> = preset
        .keys()
        .into_iter()
        .map(|k| t(setting_label_key(k)).to_string())
        .collect();
    t_args("settings.presets.sets", &[("settings", &names.join(", "))])
}

pub(crate) struct LaunchPresetsPanel {
    target: FormTarget,
    presets: Vec<LaunchPreset>,
    /// The "Save as preset" name field while it is open.
    naming: Option<Entity<InputState>>,
    error: Option<SharedString>,
    language: LanguageStamp,
    _naming_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl LaunchPresetsPanel {
    pub(crate) fn new(target: FormTarget, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        Self {
            target,
            presets: backend(cx).settings.launch_presets(),
            naming: None,
            error: None,
            language: LanguageStamp::new(),
            _naming_subscription: None,
            _subscriptions: vec![cx.observe(&settings, |this, _, cx| {
                this.presets = backend(cx).settings.launch_presets();
                cx.notify();
            })],
        }
    }

    fn game(&self) -> Option<&GameId> {
        match &self.target {
            FormTarget::Game(id) => Some(id),
            _ => None,
        }
    }

    fn apply(&mut self, preset: LaunchPreset, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let name = preset_name(&preset);
        let message = match self.game() {
            Some(id) => {
                let title = b
                    .game(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| id.app_name.to_string());
                b.settings
                    .apply_launch_preset(&preset, std::slice::from_ref(id));
                t_args(
                    "settings.presets.applied_game",
                    &[("preset", &name), ("title", &title)],
                )
            }
            None => {
                b.settings.apply_launch_preset_to_defaults(&preset);
                t_args("settings.presets.applied_defaults", &[("preset", &name)])
            }
        };
        window.push_notification(Notification::success(message), cx);
    }

    fn open_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let suggestion = self
            .game()
            .and_then(|id| b.game(id))
            .map(|g| g.title.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("settings.presets.name_placeholder"))
                .default_value(suggestion)
        });
        self._naming_subscription = Some(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save(window, cx),
                InputEvent::Change if this.error.take().is_some() => cx.notify(),
                _ => {}
            },
        ));
        input.focus_handle(cx).focus(window, cx);
        self.naming = Some(input);
        self.error = None;
        cx.notify();
    }

    fn close_naming(&mut self, cx: &mut Context<Self>) {
        self.naming = None;
        self._naming_subscription = None;
        self.error = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.naming else { return };
        let name = input.read(cx).value().to_string();
        if name.trim().is_empty() {
            self.error = Some(t("settings.presets.error.name"));
            cx.notify();
            return;
        }
        let from = self.game().cloned();
        match backend(cx)
            .settings
            .save_launch_preset(&name, from.as_ref())
        {
            Ok(preset) => {
                self.close_naming(cx);
                window.push_notification(
                    Notification::success(t_args(
                        "settings.presets.saved",
                        &[("preset", &preset.name)],
                    )),
                    cx,
                );
            }
            Err(_) => {
                self.error = Some(t("settings.presets.error.empty"));
                cx.notify();
            }
        }
    }

    fn confirm_remove(
        &mut self,
        preset: LaunchPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = preset.name.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let id = preset.id.clone();
            alert
                .title(t_args(
                    "settings.presets.remove.title",
                    &[("preset", &name)],
                ))
                .description(t("settings.presets.remove.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.common.remove"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    backend(cx).settings.remove_launch_preset(&id);
                    true
                })
        });
    }

    fn render_row(&self, ix: usize, preset: &LaunchPreset, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let name = preset_name(preset);
        let apply_label = if self.game().is_some() {
            t("settings.presets.apply")
        } else {
            t("settings.presets.apply_defaults")
        };
        let (apply, to_games, remove) = (preset.clone(), preset.clone(), preset.clone());
        h_flex()
            .id(ElementId::Name(format!("preset-row-{ix}").into()))
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .when(ix > 0, |r| r.border_t_1().border_color(border))
            .child(
                Icon::new(if preset.is_builtin() {
                    IconName::Sparkles
                } else {
                    IconName::Bookmark
                })
                .small()
                .text_color(muted_fg),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(preset_description(preset)),
                    ),
            )
            .child(
                Button::new(ElementId::Name(format!("preset-apply-{ix}").into()))
                    .outline()
                    .xsmall()
                    .label(apply_label)
                    .accessibility_label(t_args(
                        "settings.presets.apply_named",
                        &[("preset", &name)],
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply(apply.clone(), window, cx)
                    })),
            )
            .child(
                Button::new(ElementId::Name(format!("preset-games-{ix}").into()))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Layers)
                    .tooltip(t("settings.presets.apply_to_games"))
                    .accessibility_label(t_args(
                        "settings.presets.apply_to_games_named",
                        &[("preset", &name)],
                    ))
                    .on_click(move |_, window, cx| open_apply_dialog(to_games.clone(), window, cx)),
            )
            .when(!preset.is_builtin(), |row| {
                row.child(
                    Button::new(ElementId::Name(format!("preset-remove-{ix}").into()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Trash)
                        .tooltip(t("settings.common.remove"))
                        .accessibility_label(t_args(
                            "settings.presets.remove_named",
                            &[("preset", &name)],
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.confirm_remove(remove.clone(), window, cx)
                        })),
                )
            })
            .into_any_element()
    }
}

impl Render for LaunchPresetsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() && self.naming.is_some() {
            cx.defer_in(window, |this, window, cx| this.open_naming(window, cx));
        }
        let muted_fg = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let presets = self.presets.clone();
        let list = panel(cx).when(presets.is_empty(), |p| {
            p.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("settings.presets.empty")),
            )
        });
        let list = presets
            .iter()
            .enumerate()
            .fold(list, |list, (ix, p)| list.child(self.render_row(ix, p, cx)));
        let save_help = if self.game().is_some() {
            t("settings.presets.save_help_game")
        } else {
            t("settings.presets.save_help_defaults")
        };
        let footer = match &self.naming {
            Some(input) => h_flex()
                .gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(input)))
                .child(
                    Button::new("preset-save")
                        .primary()
                        .icon(IconName::Check)
                        .label(t("settings.common.save"))
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                )
                .child(
                    Button::new("preset-save-cancel")
                        .ghost()
                        .label(t("settings.common.cancel"))
                        .on_click(cx.listener(|this, _, _, cx| this.close_naming(cx))),
                )
                .into_any_element(),
            None => h_flex()
                .gap_2()
                .child(
                    Button::new("preset-new")
                        .outline()
                        .icon(IconName::Plus)
                        .label(t("settings.presets.save_as"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_naming(window, cx))),
                )
                .into_any_element(),
        };
        v_flex()
            .id("launch-presets")
            .w_full()
            .gap_2()
            .child(list)
            .child(footer)
            .when_some(self.error.clone(), |col, e| {
                col.child(div().text_xs().text_color(danger).child(e))
            })
            .child(muted(save_help, cx).text_xs())
    }
}

/// Height of one game row in the apply dialog, in rems.
const ROW_REMS: f32 = 2.5;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: usize = 9;

/// Installed games whose title contains `query` (case-insensitive), in
/// library order.
pub(crate) fn filter_games(games: &[(GameId, String)], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    games
        .iter()
        .enumerate()
        .filter(|(_, (_, title))| query.is_empty() || title.to_lowercase().contains(&query))
        .map(|(ix, _)| ix)
        .collect()
}

/// Open the dialog that applies `preset` to the games the user picks.
pub(crate) fn open_apply_dialog(preset: LaunchPreset, window: &mut Window, cx: &mut App) {
    let title = t_args(
        "settings.presets.dialog.title",
        &[("preset", &preset_name(&preset))],
    );
    let view = cx.new(|cx| ApplyPresetDialog::new(preset, window, cx));
    let search = view.read(cx).search.clone();
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            search.focus_handle(cx).focus(window, cx);
        }
        dialog
            .title(title.clone())
            .w(window.rem_size() * 34.)
            .child(view.clone())
    });
}

struct ApplyPresetDialog {
    preset: LaunchPreset,
    /// Installed games in library order.
    games: Vec<(GameId, String)>,
    /// Indices into `games` matching the search.
    shown: Vec<usize>,
    selected: HashSet<GameId>,
    search: Entity<InputState>,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
    applying: bool,
    _subscriptions: Vec<Subscription>,
}

impl ApplyPresetDialog {
    fn new(preset: LaunchPreset, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let games: Vec<(GameId, String)> = backend(cx)
            .library()
            .games
            .iter()
            .filter(|g| g.is_installed)
            .map(|g| (g.id(), g.title.clone()))
            .collect();
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("settings.presets.dialog.search")));
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.shown = filter_games(&this.games, &state.read(cx).value());
                    cx.notify();
                }
            },
        );
        Self {
            shown: filter_games(&games, ""),
            games,
            preset,
            selected: HashSet::new(),
            search,
            sizes: Rc::new(Vec::new()),
            sized_for: (usize::MAX, px(0.)),
            applying: false,
            _subscriptions: vec![subscription],
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<GameId> = self
            .games
            .iter()
            .filter(|(id, _)| self.selected.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        self.applying = true;
        cx.notify();
        let b = backend(cx);
        let preset = self.preset.clone();
        let count = ids.len();
        // Reads each game's settings file, so it runs off the UI thread.
        let task = irori_ui::run(cx, async move {
            let worker = b.clone();
            b.spawn_blocking(move || worker.settings.apply_launch_preset(&preset, &ids))
                .await
                .map_err(anyhow::Error::from)
        });
        let name = preset_name(&self.preset);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        window.push_notification(
                            Notification::success(t_args(
                                if count == 1 {
                                    "settings.presets.applied_games_one"
                                } else {
                                    "settings.presets.applied_games_other"
                                },
                                &[("preset", &name), ("count", &count.to_string())],
                            )),
                            cx,
                        );
                    }
                    Err(e) => {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.presets.apply_failed")),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range
            .filter_map(|row| {
                let ix = *self.shown.get(row)?;
                let (id, title) = self.games.get(ix)?;
                let checked = self.selected.contains(id);
                let id = id.clone();
                Some(
                    h_flex()
                        .w_full()
                        .h(rems(ROW_REMS))
                        .px_3()
                        .child(
                            Checkbox::new(ElementId::Name(format!("preset-game-{ix}").into()))
                                .checked(checked)
                                .label(title.clone())
                                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                    if *on {
                                        this.selected.insert(id.clone());
                                    } else {
                                        this.selected.remove(&id);
                                    }
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for ApplyPresetDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted_fg, border, radius) = (theme.muted_foreground, theme.border, theme.radius_lg);
        let row_h = window.rem_size() * ROW_REMS;
        let count = self.shown.len();
        if self.sized_for != (count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (count, row_h);
        }
        let selected = self.selected.len();
        let all_shown = count > 0
            && self
                .shown
                .iter()
                .all(|ix| self.selected.contains(&self.games[*ix].0));
        let list: AnyElement = if self.games.is_empty() {
            muted(t("settings.presets.dialog.no_games"), cx).into_any_element()
        } else if count == 0 {
            muted(t("settings.common.no_match"), cx).into_any_element()
        } else {
            div()
                .id("preset-games")
                .w_full()
                .h(rems(ROW_REMS * count.min(VISIBLE_ROWS) as f32))
                .rounded(radius)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "preset-game-rows",
                        self.sizes.clone(),
                        |this, range, _, cx| this.render_rows(range, cx),
                    )
                    .size_full(),
                )
                .into_any_element()
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(preset_description(&self.preset)),
            )
            .child(Input::new(&self.search))
            .when(count > 0, |col| {
                col.child(
                    Checkbox::new("preset-games-all")
                        .checked(all_shown)
                        .label(t("settings.presets.dialog.select_shown"))
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            for ix in this.shown.clone() {
                                let id = this.games[ix].0.clone();
                                if *on {
                                    this.selected.insert(id);
                                } else {
                                    this.selected.remove(&id);
                                }
                            }
                            cx.notify();
                        })),
                )
            })
            .child(list)
            .child(muted(t("settings.presets.dialog.help"), cx).text_xs())
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("preset-dialog-cancel")
                            .ghost()
                            .label(t("settings.common.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("preset-dialog-apply")
                            .primary()
                            .label(if selected == 1 {
                                t("settings.presets.dialog.apply_one")
                            } else {
                                t_args(
                                    "settings.presets.dialog.apply_other",
                                    &[("count", &selected.to_string())],
                                )
                            })
                            .loading(self.applying)
                            .disabled(selected == 0 || self.applying)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    #[test]
    fn names_builtin_presets_by_id() {
        // Joined, so the catalog test doesn't take the stem for a key.
        let stem = ["settings", "presets", "builtin", "steam_deck_handheld"].join(".");
        assert_eq!(builtin_key("builtin:steam-deck-handheld"), Some(stem));
        assert_eq!(builtin_key("user:12ab"), None);
        assert_eq!(setting_label_key("gamescope"), "settings.group.gamescope");
        assert_eq!(
            setting_label_key("showMangohud"),
            "settings.field.show_mangohud.label"
        );
    }

    #[test]
    fn every_preset_key_and_builtin_has_text() {
        let en: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(include_str!("../locales/en.json")).unwrap();
        for key in irori_core::launch_extras::PRESET_KEYS {
            let label = setting_label_key(key);
            assert_ne!(label, "settings.presets.other_setting", "{key}");
            assert!(en.contains_key(label), "{label}");
        }
        for host in [
            irori_core::HostPlatform::Linux,
            irori_core::HostPlatform::Mac,
            irori_core::HostPlatform::Windows,
        ] {
            for preset in irori_core::launch_extras::builtin_presets(host) {
                let stem = builtin_key(&preset.id).unwrap();
                for part in ["name", "description"] {
                    let key = format!("{stem}.{part}");
                    assert!(en.contains_key(&key), "{key}");
                }
            }
        }
    }

    #[test]
    fn filters_games_by_title() {
        let games = vec![
            (GameId::new(Runner::Gog, "1"), "Hades".to_string()),
            (GameId::new(Runner::Gog, "2"), "Hades II".to_string()),
            (GameId::new(Runner::Gog, "3"), "Celeste".to_string()),
        ];
        assert_eq!(filter_games(&games, ""), vec![0, 1, 2]);
        assert_eq!(filter_games(&games, " hades "), vec![0, 1]);
        assert!(filter_games(&games, "zelda").is_empty());
    }
}
