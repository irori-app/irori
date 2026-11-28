//! Danger zone footer card inside `Saves & backup -> Library backup`:
//! destructive `Reset all settings` action with confirmation dialog, off-thread
//! reset of global + per-game settings, and live theme re-application.

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _, Window, div,
};
use irori_core::settings::AppSettings;
use irori_ui::backend;
use irori_ui::i18n::t;

use super::BackupPanel;

impl BackupPanel {
    /// Visually isolated Danger zone card at the bottom of `Library backup`.
    pub(super) fn render_danger_zone(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, muted_fg, radius_lg) = (theme.danger, theme.muted_foreground, theme.radius_lg);
        let highlighted = self.highlighted_field == Some("action.reset-settings");

        let header = h_flex()
            .w_full()
            .pt_5()
            .pb_1()
            .gap_1p5()
            .items_center()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .xsmall()
                    .text_color(danger),
            )
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(danger)
                    .child(t("settings.group.danger_zone").to_uppercase()),
            );

        let card = v_flex()
            .id("row-action.reset-settings")
            .debug_selector(|| "row-action.reset-settings".to_string())
            .w_full()
            .rounded(radius_lg)
            .bg(if highlighted {
                danger.opacity(0.14)
            } else {
                danger.opacity(0.06)
            })
            .border_1()
            .border_color(if highlighted {
                danger.opacity(0.75)
            } else {
                danger.opacity(0.35)
            })
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_3p5()
                    .gap_4()
                    .flex_wrap()
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
                                    .font_semibold()
                                    .child(t("settings.backup.reset_all_title")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(t("settings.backup.reset_all_description")),
                            ),
                    )
                    .child(
                        Button::new("backup-reset-settings")
                            .danger()
                            .small()
                            .icon(IconName::RotateCcw)
                            .label(t("settings.backup.reset_all_button"))
                            .loading(self.resetting)
                            .disabled(self.resetting)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_reset_settings(window, cx)
                            })),
                    ),
            );

        v_flex().w_full().gap_1().child(header).child(card)
    }

    fn confirm_reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let weak = weak.clone();
            alert
                .title(t("settings.reset_settings.title"))
                .description(t("settings.reset_settings.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.reset_settings.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let weak = weak.clone();
                    // Dialog callbacks run inside the window update; defer follow-up work.
                    window.defer(cx, move |window, cx| {
                        let _ = weak.update(cx, |this, cx| this.reset_all_settings(window, cx));
                    });
                    true
                })
        });
    }

    fn reset_all_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.resetting {
            return;
        }
        self.resetting = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            // Both calls write files synchronously.
            tokio::task::spawn_blocking(move || {
                b.settings.replace_app(AppSettings::default());
                b.settings.reset_all_games();
            })
            .await
            .context("resetting settings")
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.resetting = false;
                match result {
                    Ok(()) => {
                        irori_ui::theme::apply(&backend(cx).settings.app().ui, Some(window), cx);
                        this.sync_saves_controls(window, cx);
                        this.list(cx);
                        window.push_notification(
                            Notification::success(t("settings.reset_settings.done")),
                            cx,
                        );
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("settings.error.generic")),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
