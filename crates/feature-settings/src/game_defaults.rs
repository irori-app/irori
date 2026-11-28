//! Contextual summary strip, tab helper banners, related-page links, and reset
//! action for `Settings -> Game defaults`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use irori_core::hud::{GlobalHudMode, HudBackend};
use irori_core::settings::GameSettings;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::view::{SettingsSection, SettingsView};
use crate::widgets::{group_panel, muted};

/// Cached Linux MangoHud installation status without blocking the UI thread.
///
/// Returns `Some(true)` if the Vulkan layer or `mangohud` binary was detected,
/// `Some(false)` if the probe ran and found neither, or `None` on non-Linux
/// platforms or before the one-shot probe finishes.
pub(crate) fn mangohud_installed() -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    irori_runtime::hud::hardware_linux::cached_linux_hud_env()
        .map(|env| env.has_vulkan_layer || env.mangohud_bin_path.is_some())
}

struct SummaryPill {
    id: &'static str,
    tab: &'static str,
    label: SharedString,
    active: bool,
}

impl SettingsView {
    /// Populate the process-wide Linux HUD probe once off the UI thread so the
    /// Performance card and form know whether MangoHud is installed.
    pub(crate) fn probe_linux_hud_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cfg!(target_os = "linux")
            || irori_runtime::hud::hardware_linux::cached_linux_hud_env().is_some()
        {
            return;
        }
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let _ = b
                .spawn_blocking(irori_runtime::hud::hardware_linux::detect_linux_hud_env)
                .await;
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let _ = task.await;
            this.update(cx, |this, cx| {
                this.defaults_form().update(cx, |_, cx| cx.notify());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn render_game_defaults_header_action(&self, cx: &mut Context<Self>) -> AnyElement {
        let customized = backend(cx).settings.app().game != GameSettings::default();
        Button::new("game-defaults-reset")
            .outline()
            .icon(IconName::RotateCcw)
            .label(t("settings.game_defaults.reset"))
            .disabled(!customized)
            .on_click(
                cx.listener(|this, _, window, cx| this.confirm_reset_game_defaults(window, cx)),
            )
            .into_any_element()
    }

    fn confirm_reset_game_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_alert_dialog(cx, |alert, _, _| {
            alert
                .title(t("settings.game_defaults.reset.title"))
                .description(t("settings.game_defaults.reset.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.game_defaults.reset.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(|_, window, cx| {
                    backend(cx)
                        .settings
                        .update_app(|app| app.game = GameSettings::default());
                    window.push_notification(
                        Notification::success(t("settings.game_defaults.reset.done")),
                        cx,
                    );
                    true
                })
        });
    }

    pub(crate) fn render_game_defaults_summary(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tabs = crate::layout::supported_defaults_tabs();
        let has_tab = |id: &str| tabs.iter().any(|tab| tab.id == id);
        let app = backend(cx).settings.app();
        let game = &app.game;
        let mut pills: Vec<SummaryPill> = Vec::with_capacity(6);

        if has_tab("wine") {
            let build = game.wine_version.name.trim();
            let (label, active) = if build.is_empty() {
                (t("settings.game_defaults.pill.build_auto"), false)
            } else {
                (
                    t_args("settings.game_defaults.pill.build", &[("name", build)]),
                    true,
                )
            };
            pills.push(SummaryPill {
                id: "defaults-pill-build",
                tab: "wine",
                label,
                active,
            });
            if cfg!(target_os = "linux") {
                let (key, active) = match (game.eac_runtime, game.battl_eye_runtime) {
                    (true, true) => ("settings.game_defaults.pill.anticheat_both", true),
                    (true, false) => ("settings.game_defaults.pill.anticheat_eac", true),
                    (false, true) => ("settings.game_defaults.pill.anticheat_battleye", true),
                    (false, false) => ("settings.game_defaults.pill.anticheat_off", false),
                };
                pills.push(SummaryPill {
                    id: "defaults-pill-anticheat",
                    tab: "wine",
                    label: t(key),
                    active,
                });
            }
        }

        if has_tab("performance") {
            if cfg!(target_os = "linux") {
                pills.push(SummaryPill {
                    id: "defaults-pill-gamemode",
                    tab: "performance",
                    label: t(if game.use_game_mode {
                        "settings.game_defaults.pill.gamemode_on"
                    } else {
                        "settings.game_defaults.pill.gamemode_off"
                    }),
                    active: game.use_game_mode,
                });
            }
            if HudBackend::current().is_supported() {
                let (label, active) = stats_pill_summary(&app);
                pills.push(SummaryPill {
                    id: "defaults-pill-stats",
                    tab: "performance",
                    label,
                    active,
                });
            }
        }

        if has_tab("gpu") && cfg!(target_os = "linux") {
            let (key, active) = match (game.enable_wine_wayland, game.enable_hdr) {
                (true, true) => ("settings.game_defaults.pill.display_wayland_hdr", true),
                (true, false) => ("settings.game_defaults.pill.display_wayland", true),
                (false, _) => ("settings.game_defaults.pill.display_xwayland", false),
            };
            pills.push(SummaryPill {
                id: "defaults-pill-display",
                tab: "gpu",
                label: t(key),
                active,
            });
        }

        if has_tab("replay") {
            let (label, active) = if game.replay.enabled {
                (
                    t_args(
                        "settings.game_defaults.pill.replay_on",
                        &[("seconds", &game.replay.seconds.to_string())],
                    ),
                    true,
                )
            } else {
                (t("settings.game_defaults.pill.replay_off"), false)
            };
            pills.push(SummaryPill {
                id: "defaults-pill-replay",
                tab: "replay",
                label,
                active,
            });
        }

        if pills.is_empty() {
            return None;
        }

        let current_tab = self.defaults_tab();
        let theme = cx.theme();
        let active_dot = theme.primary;
        let muted_dot = theme.muted_foreground;

        Some(
            h_flex()
                .id("game-defaults-summary")
                .debug_selector(|| "game-defaults-summary".into())
                .w_full()
                .flex_wrap()
                .gap_1p5()
                .pb_1()
                .children(pills.into_iter().map(|pill| {
                    let tab = pill.tab;
                    let selected = current_tab == tab;
                    let dot = if pill.active { active_dot } else { muted_dot };
                    Button::new(ElementId::Name(pill.id.into()))
                        .outline()
                        .xsmall()
                        .selected(selected)
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(div().flex_none().size_1p5().rounded_full().bg(dot))
                                .child(div().text_xs().child(pill.label)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_defaults_tab(tab, window, cx)
                        }))
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn render_game_defaults_body(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_3()
            .children(self.render_defaults_tab_banner(cx))
            .child(self.defaults_form().clone())
            .child(self.render_defaults_related_footer(cx))
            .into_any_element()
    }

    fn render_defaults_tab_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.defaults_tab() {
            "wine" if !cfg!(windows) => Some(self.render_wine_prefix_banner(cx)),
            "performance" if HudBackend::current().is_supported() => {
                Some(self.render_performance_hud_banner(cx))
            }
            _ => None,
        }
    }

    fn render_wine_prefix_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let app = backend(cx).settings.app();
        let shared_prefix = app.game.wine_prefix.trim();
        let warning = cx.theme().warning;

        if !shared_prefix.is_empty() {
            return group_panel(cx)
                .id("defaults-wine-shared-prefix")
                .debug_selector(|| "defaults-wine-shared-prefix".into())
                .px_4()
                .py_3()
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2p5()
                                .items_start()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .small()
                                        .text_color(warning)
                                        .mt_0p5(),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_0p5()
                                        .child(
                                            div().text_sm().font_medium().child(t(
                                                "settings.game_defaults.wine.shared_title",
                                            )),
                                        )
                                        .child(muted(
                                            t_args(
                                                "settings.game_defaults.wine.shared_help",
                                                &[("folder", shared_prefix)],
                                            ),
                                            cx,
                                        )),
                                ),
                        )
                        .child(
                            Button::new("defaults-wine-reset-prefix")
                                .outline()
                                .small()
                                .label(t("settings.game_defaults.wine.reset_prefix"))
                                .on_click(|_, window, cx| {
                                    backend(cx)
                                        .settings
                                        .update_app(|s| s.game.wine_prefix.clear());
                                    window.push_notification(
                                        Notification::success(t(
                                            "settings.game_defaults.wine.reset_prefix_done",
                                        )),
                                        cx,
                                    );
                                }),
                        ),
                )
                .into_any_element();
        }

        let root = irori_runtime::prefixes_root(&app)
            .to_string_lossy()
            .into_owned();
        group_panel(cx)
            .id("defaults-wine-prefixes")
            .debug_selector(|| "defaults-wine-prefixes".into())
            .px_4()
            .py_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(t("settings.game_defaults.wine.isolated_title")),
                            )
                            .child(muted(
                                t_args(
                                    "settings.game_defaults.wine.isolated_help",
                                    &[("folder", &root)],
                                ),
                                cx,
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_shrink_0()
                            .child(
                                Button::new("defaults-wine-change-folder")
                                    .outline()
                                    .small()
                                    .label(t("settings.game_defaults.wine.change_folder"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_general_basics(window, cx)
                                    })),
                            )
                            .when(SettingsSection::WineManager.supported_on_host(), |row| {
                                row.child(
                                    Button::new("defaults-wine-open-manager")
                                        .ghost()
                                        .small()
                                        .label(t("settings.page.wine_manager"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_section(
                                                SettingsSection::WineManager,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_performance_hud_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let app = backend(cx).settings.app();
        let hud_on = app.hud.mode == GlobalHudMode::IroriProfile;
        let is_linux = cfg!(target_os = "linux");
        let mangohud_missing = is_linux && mangohud_installed() == Some(false);

        let title = if is_linux {
            t("settings.game_defaults.performance.hud_title_linux")
        } else {
            t("settings.game_defaults.performance.hud_title")
        };

        let badge = if mangohud_missing {
            Tag::warning()
                .xsmall()
                .child(t("settings.game_defaults.performance.hud_missing_badge"))
        } else if hud_on {
            Tag::success()
                .xsmall()
                .child(t("settings.game_defaults.performance.hud_active_badge"))
        } else {
            Tag::secondary()
                .xsmall()
                .child(t("settings.game_defaults.performance.hud_off_badge"))
        };

        let mut summary = if mangohud_missing {
            t("settings.game_defaults.performance.hud_missing_help").to_string()
        } else if is_linux {
            if hud_on {
                let profile = &app.hud.mangohud;
                match (profile.fps_limit.filter(|&fps| fps > 0), profile.no_display) {
                    (Some(fps), true) => t_args(
                        "settings.game_defaults.performance.hud_active_hidden_cap",
                        &[("fps", &fps.to_string())],
                    )
                    .to_string(),
                    (Some(fps), false) => t_args(
                        "settings.game_defaults.performance.hud_active_cap",
                        &[("fps", &fps.to_string())],
                    )
                    .to_string(),
                    (None, true) => {
                        t("settings.game_defaults.performance.hud_active_hidden").to_string()
                    }
                    (None, false) => t("settings.game_defaults.performance.hud_active").to_string(),
                }
            } else {
                t("settings.game_defaults.performance.hud_off_help_linux").to_string()
            }
        } else if hud_on {
            t("settings.game_defaults.performance.hud_active_metal").to_string()
        } else {
            t("settings.game_defaults.performance.hud_off_help_metal").to_string()
        };

        if is_linux && app.game.enable_dxvk_fps_limit {
            let cap = app.game.dxvk_fps_cap.trim();
            let note = if cap.is_empty() {
                t("settings.game_defaults.performance.legacy_cap_note_unset")
            } else {
                t_args(
                    "settings.game_defaults.performance.legacy_cap_note",
                    &[("fps", cap)],
                )
            };
            summary.push(' ');
            summary.push_str(&note);
        }

        group_panel(cx)
            .id("defaults-performance-hud")
            .debug_selector(|| "defaults-performance-hud".into())
            .px_4()
            .py_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .flex_wrap()
                                    .child(div().text_sm().font_medium().child(title))
                                    .child(badge),
                            )
                            .child(muted(summary, cx)),
                    )
                    .child(
                        Button::new("defaults-open-hud")
                            .outline()
                            .small()
                            .icon(IconName::Gauge)
                            .label(t("settings.game_defaults.performance.open_hud"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_section(SettingsSection::PerformanceHud, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_defaults_related_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted_fg = cx.theme().muted_foreground;
        div()
            .pt_2()
            .child(
                group_panel(cx)
                    .id("game-defaults-related")
                    .debug_selector(|| "game-defaults-related".into())
                    .px_4()
                    .py_3()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(t("settings.game_defaults.related.prompt")),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("defaults-related-saves")
                                    .outline()
                                    .xsmall()
                                    .icon(IconName::DatabaseBackup)
                                    .label(SettingsSection::Saves.label())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_section(SettingsSection::Saves, window, cx)
                                    })),
                            )
                            .when(SettingsSection::PerformanceHud.supported_on_host(), |row| {
                                row.child(
                                    Button::new("defaults-related-hud")
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::Gauge)
                                        .label(SettingsSection::PerformanceHud.label())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_section(
                                                SettingsSection::PerformanceHud,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                            })
                            .when(SettingsSection::WineManager.supported_on_host(), |row| {
                                row.child(
                                    Button::new("defaults-related-wine")
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::Wine)
                                        .label(SettingsSection::WineManager.label())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_section(
                                                SettingsSection::WineManager,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(t("settings.game_defaults.related.optiscaler_note")),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn stats_pill_summary(app: &irori_core::settings::AppSettings) -> (SharedString, bool) {
    let hud_on = app.hud.mode == GlobalHudMode::IroriProfile;
    if cfg!(target_os = "linux") {
        if hud_on {
            let profile = &app.hud.mangohud;
            if let Some(fps) = profile.fps_limit.filter(|&fps| fps > 0) {
                let key = if profile.no_display {
                    "settings.game_defaults.pill.fps_cap"
                } else {
                    "settings.game_defaults.pill.stats_fps"
                };
                return (t_args(key, &[("fps", &fps.to_string())]), true);
            }
            return (t("settings.game_defaults.pill.stats_on"), true);
        }
    } else if hud_on {
        return (t("settings.game_defaults.pill.stats_on"), true);
    }

    if app.game.enable_dxvk_fps_limit {
        let cap = app.game.dxvk_fps_cap.trim();
        if !cap.is_empty() {
            return (
                t_args("settings.game_defaults.pill.fps_cap", &[("fps", cap)]),
                true,
            );
        }
    }
    (t("settings.game_defaults.pill.stats_off"), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::settings::AppSettings;

    #[test]
    fn stats_pill_reflects_hud_and_fps_limit_modes() {
        crate::i18n::register();
        let mut app = AppSettings::default();
        let (off, active) = stats_pill_summary(&app);
        assert!(!active);
        assert!(off.contains("Off"), "{off}");

        app.hud.mode = GlobalHudMode::IroriProfile;
        app.hud.mangohud.fps_limit = Some(120);
        app.hud.mangohud.no_display = false;
        let (on_cap, active) = stats_pill_summary(&app);
        assert!(active);
        if cfg!(target_os = "linux") {
            assert!(on_cap.contains("120"), "{on_cap}");
        }

        app.hud.mangohud.no_display = true;
        let (hidden_cap, active) = stats_pill_summary(&app);
        assert!(active);
        if cfg!(target_os = "linux") {
            assert!(hidden_cap.contains("120"), "{hidden_cap}");
        }

        app.hud.mode = GlobalHudMode::Off;
        app.game.enable_dxvk_fps_limit = true;
        app.game.dxvk_fps_cap = "90".into();
        let (legacy_cap, active) = stats_pill_summary(&app);
        assert!(active);
        assert!(legacy_cap.contains("90"), "{legacy_cap}");
    }
}
