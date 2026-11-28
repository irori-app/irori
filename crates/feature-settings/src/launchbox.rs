//! The LaunchBox panel of Advanced -> Artwork & metadata, directly under the
//! `iroriLaunchBox` toggle: displays the stored game list size as a compact
//! status strip and, while the switch is off with a list still on disk, offers
//! a confirmed 1-click remove button. When the switch is off and nothing is
//! stored, the parent form hides this row so no empty box wastes space.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Context, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Window, div,
};
use irori_core::util::format_bytes;
use irori_emulation::EmulationStore;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use crate::widgets::muted;

/// Whether the LaunchBox storage row should be shown in compact App Settings.
/// Turning the switch off with no stored list leaves nothing to manage.
pub(crate) fn should_show_row(enabled: bool, stored: Option<Option<u64>>) -> bool {
    enabled || matches!(stored, Some(Some(_)))
}

pub(crate) struct LaunchBoxPanel {
    store: Arc<EmulationStore>,
    /// `None` until measured off the UI thread; `Some(None)` when nothing is
    /// stored.
    stored: Option<Option<u64>>,
    removing: bool,
    _subscriptions: Vec<Subscription>,
}

impl LaunchBoxPanel {
    pub(crate) fn new(store: Arc<EmulationStore>, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let mut this = Self {
            store,
            stored: None,
            removing: false,
            // The switch above the panel reaches the settings model.
            _subscriptions: vec![cx.observe(&settings, |this, _, cx| this.measure(cx))],
        };
        this.measure(cx);
        this
    }

    /// Whether this panel currently has actionable or active status to display.
    pub(crate) fn has_visible_content(&self, cx: &App) -> bool {
        let on = models(cx).settings.read(cx).app.launchbox;
        should_show_row(on, self.stored)
    }

    fn measure(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let task =
            irori_ui::state::run_infallible(cx, async move { store.launchbox_stored().await });
        cx.spawn(async move |this, cx| {
            let stored = task.await;
            let _ = this.update(cx, |this, cx| {
                this.stored = Some(stored.flatten());
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_remove(&mut self, size: u64, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let size = format_bytes(size);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t("settings.launchbox.remove_title"))
                .description(t_args("settings.launchbox.remove_body", &[("size", &size)]))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.launchbox.remove"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |this, cx| this.remove(window, cx)).ok();
                    true
                })
        });
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.removing {
            return;
        }
        self.removing = true;
        cx.notify();
        let store = self.store.clone();
        let task = irori_ui::run(cx, async move { store.remove_launchbox().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.removing = false;
                let notice = match result {
                    Ok(()) => Notification::success(t("settings.launchbox.removed")),
                    Err(e) => Notification::error(format!("{e:#}"))
                        .title(t("settings.launchbox.remove_failed")),
                };
                window.push_notification(notice, cx);
                this.measure(cx);
            });
        })
        .detach();
    }
}

impl Render for LaunchBoxPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on = models(cx).settings.read(cx).app.launchbox;
        let theme = cx.theme();
        let subpanel = || {
            v_flex()
                .w_full()
                .px_3()
                .py_2p5()
                .gap_2()
                .rounded(theme.radius)
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
        };
        let Some(stored) = self.stored else {
            return subpanel().child(muted(t("settings.launchbox.looking"), cx));
        };
        let badge = match stored {
            Some(size) => Tag::info()
                .xsmall()
                .child(t_args(
                    "settings.launchbox.stored_badge",
                    &[("size", &format_bytes(size))],
                ))
                .into_any_element(),
            None => Tag::secondary()
                .xsmall()
                .outline()
                .child(t("settings.launchbox.not_stored_badge"))
                .into_any_element(),
        };
        let text = match stored {
            Some(size) => t_args(
                "settings.launchbox.stored",
                &[("size", &format_bytes(size))],
            ),
            None => t("settings.launchbox.none"),
        };
        subpanel()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .child(badge)
                    .child(div().flex_1().min_w_0().text_xs().child(text))
                    .when_some(stored.filter(|_| !on), |row, size| {
                        row.child(
                            Button::new("launchbox-remove")
                                .ghost()
                                .danger()
                                .small()
                                .icon(IconName::Trash)
                                .label(t("settings.launchbox.remove"))
                                .loading(self.removing)
                                .disabled(self.removing)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_remove(size, window, cx)
                                })),
                        )
                    }),
            )
            .when(!on && stored.is_some(), |body| {
                body.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t("settings.launchbox.off_hint")),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_row_when_off_and_nothing_stored() {
        assert!(!should_show_row(false, None));
        assert!(!should_show_row(false, Some(None)));
        assert!(should_show_row(false, Some(Some(112_000_000))));
        assert!(should_show_row(true, None));
        assert!(should_show_row(true, Some(None)));
        assert!(should_show_row(true, Some(Some(112_000_000))));
    }
}
