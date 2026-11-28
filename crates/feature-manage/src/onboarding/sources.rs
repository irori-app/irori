//! Step 2 of first-run onboarding ("Bring in your games"): detected launchers
//! for 1-click import, Steam with local detection status, primary stores,
//! collapsible "More stores" with 1-click enable + sign-in, and detected ROMs.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use irori_core::Runner;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::launchers::{Launcher, import_status};
use super::{ImportState, OnboardingView, import_summary_lines};
use crate::accounts::{StoreCard, set_store_enabled, store_icon};
use crate::common::backend_op;
use crate::login::open_login_dialog;

impl OnboardingView {
    fn render_heroic_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let status: AnyElement = match &self.import {
            ImportState::Idle | ImportState::Running => Button::new("import")
                .small()
                .primary()
                .icon(IconName::Import)
                .label(t("manage.onboarding.import.run"))
                .loading(matches!(self.import, ImportState::Running))
                .on_click(cx.listener(|this, _, window, cx| this.run_import(window, cx)))
                .into_any_element(),
            ImportState::Done(summary) => {
                let lines = import_summary_lines(summary);
                if lines.is_empty() {
                    Alert::info("import-done", t("manage.onboarding.import.nothing"))
                        .into_any_element()
                } else {
                    Alert::success("import-done", lines.join("\n"))
                        .title(t("manage.onboarding.import.done"))
                        .into_any_element()
                }
            }
            ImportState::Failed(message) => v_flex()
                .gap_2()
                .child(
                    Alert::error("import-failed", message.clone())
                        .title(t("manage.onboarding.import.failed")),
                )
                .child(
                    div().child(
                        Button::new("import-retry")
                            .small()
                            .label(t("manage.onboarding.import.retry"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run_import(window, cx)),
                            ),
                    ),
                )
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(t("manage.onboarding.import.title")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.onboarding.import.description")),
                    ),
            )
            .child(div().child(status))
            .into_any_element()
    }

    fn render_launcher_row(&self, launcher: Launcher, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = self.launchers.state(launcher) else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let view = cx.entity().downgrade();
        let status = import_status(launcher, state, move |window, cx| {
            let _ = view.update(cx, |this, cx| {
                this.run_launcher_import(launcher, window, cx)
            });
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2p5()
                    .items_start()
                    .child(launcher.icon().small().text_color(muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_sm().font_semibold().child(t_args(
                                "manage.onboarding.launcher.title",
                                &[("launcher", launcher.name())],
                            )))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(launcher.description()),
                            ),
                    ),
            )
            .child(div().child(status))
            .into_any_element()
    }

    fn render_detected_launchers(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.heroic && self.launchers.entries.is_empty() {
            return None;
        }
        let mut items: Vec<AnyElement> = Vec::new();
        if self.heroic {
            items.push(self.render_heroic_row(cx));
        }
        for (launcher, _) in &self.launchers.entries {
            items.push(self.render_launcher_row(*launcher, cx));
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_3()
                .p_3()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child(t("manage.onboarding.sources.launchers_title")),
                )
                .children(items)
                .into_any_element(),
        )
    }

    fn render_steam_row(&self, card: &StoreCard, online: bool, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let client = card.steam_client.as_ref();
        let steam_found = client.is_some_and(|c| c.steam_found);
        let suggest_client = client.is_some_and(|c| c.suggest());
        let detail = if let Some(user) = &card.user {
            SharedString::from(user.clone())
        } else if steam_found {
            t("manage.onboarding.sources.steam_local")
        } else {
            t("manage.onboarding.sources.steam_not_found")
        };
        let action: AnyElement = if card.user.is_some() {
            Tag::success()
                .xsmall()
                .child(t("manage.onboarding.stores.signed_in"))
                .into_any_element()
        } else {
            h_flex()
                .gap_1p5()
                .flex_shrink_0()
                .when(suggest_client, |row| {
                    row.child(
                        Button::new("onboarding-steam-client")
                            .small()
                            .outline()
                            .label(t("manage.found.steam.read_client"))
                            .on_click(|_, window, cx| crate::open_steam_library_dialog(window, cx)),
                    )
                })
                .child(
                    Button::new("login-steam")
                        .small()
                        .label(t("manage.onboarding.stores.login"))
                        .disabled(!online)
                        .on_click(|_, window, cx| open_login_dialog(Runner::Steam, window, cx)),
                )
                .into_any_element()
        };
        h_flex()
            .gap_3()
            .py_2()
            .items_start()
            .child(store_icon(Runner::Steam).text_color(muted))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().child(Runner::Steam.store_name()))
                    .child(div().text_xs().text_color(muted).child(detail)),
            )
            .child(action)
            .into_any_element()
    }

    fn render_store_row(&self, card: &StoreCard, online: bool, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let runner = card.runner;
        h_flex()
            .gap_3()
            .py_2()
            .items_center()
            .child(store_icon(runner).text_color(muted))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().child(runner.store_name()))
                    .when_some(card.user.clone(), |this, u| {
                        this.child(div().text_xs().text_color(muted).truncate().child(u))
                    }),
            )
            .child(if card.user.is_some() {
                Tag::success()
                    .xsmall()
                    .child(t("manage.onboarding.stores.signed_in"))
                    .into_any_element()
            } else {
                Button::new(SharedString::from(format!("login-{}", runner.id())))
                    .small()
                    .label(t("manage.onboarding.stores.login"))
                    .disabled(!online)
                    .on_click(move |_, window, cx| open_login_dialog(runner, window, cx))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_more_stores(
        &self,
        disabled: &[&StoreCard],
        online: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if disabled.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        let count = disabled.len().to_string();
        let rows: Vec<AnyElement> = if self.more_stores_open {
            disabled
                .iter()
                .map(|s| {
                    let runner = s.runner;
                    h_flex()
                        .gap_3()
                        .py_1p5()
                        .items_center()
                        .child(store_icon(runner).text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_medium()
                                .truncate()
                                .child(runner.store_name()),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "enable-login-{}",
                                runner.id()
                            )))
                            .debug_selector(move || format!("enable-login-{}", runner.id()))
                            .small()
                            .outline()
                            .label(t("manage.onboarding.sources.enable_login"))
                            .disabled(!online)
                            .on_click(
                                move |_, window, cx: &mut App| {
                                    backend_op(cx, move |b| {
                                        b.settings
                                            .update_app(|app| set_store_enabled(app, runner, true));
                                    });
                                    open_login_dialog(runner, window, cx);
                                },
                            ),
                        )
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(
            v_flex()
                .gap_1()
                .pt_1()
                .child(
                    h_flex().child(
                        Button::new("onboarding-more-stores")
                            .debug_selector(|| "onboarding-more-stores".into())
                            .ghost()
                            .small()
                            .icon(if self.more_stores_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label(t_args(
                                "manage.onboarding.sources.more_stores",
                                &[("count", &count)],
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.more_stores_open = !this.more_stores_open;
                                cx.notify();
                            })),
                    ),
                )
                .when(!rows.is_empty(), |d| {
                    d.child(v_flex().pl_3().children(rows))
                })
                .into_any_element(),
        )
    }

    fn render_detected_roms(&self, cx: &App) -> Option<AnyElement> {
        if self.rom_systems == 0 {
            return None;
        }
        let theme = cx.theme();
        let detail = if self.rom_systems == 1 {
            t("manage.rom_sources.found_one")
        } else {
            t_args(
                "manage.rom_sources.found_other",
                &[("count", &self.rom_systems.to_string())],
            )
        };
        Some(
            h_flex()
                .gap_3()
                .p_3()
                .items_center()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(Icon::new(IconName::Joystick).text_color(theme.muted_foreground))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(t("manage.found.emulation.title")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        ),
                )
                .child(
                    Button::new("onboarding-roms")
                        .small()
                        .outline()
                        .label(t("manage.rom_sources.review"))
                        .on_click(|_, window, cx| {
                            crate::emulators::open_rom_sources_dialog(window, cx)
                        }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_sources(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let online = models(cx).accounts.read(cx).online;
        let steam = self
            .stores
            .iter()
            .find(|s| s.runner == Runner::Steam && !s.disabled)
            .map(|s| self.render_steam_row(s, online, cx));
        let enabled_rows: Vec<AnyElement> = self
            .stores
            .iter()
            .filter(|s| s.runner != Runner::Steam && !s.disabled)
            .map(|s| self.render_store_row(s, online, cx))
            .collect();
        let disabled: Vec<&StoreCard> = self.stores.iter().filter(|s| s.disabled).collect();
        let more_stores = self.render_more_stores(&disabled, online, cx);
        let detected_launchers = self.render_detected_launchers(cx);
        let detected_roms = self.render_detected_roms(cx);

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_xl()
                    .font_semibold()
                    .child(t("manage.onboarding.stores.title")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.onboarding.stores.description")),
            )
            .when(!online, |this| {
                this.child(Alert::warning(
                    "onboarding-offline",
                    t("manage.onboarding.stores.offline"),
                ))
            })
            .children(detected_launchers)
            .child(v_flex().children(steam).children(enabled_rows))
            .children(more_stores)
            .children(detected_roms)
            .into_any_element()
    }
}
