//! The LaunchBox panel of Advanced → Artwork, under the `iroriLaunchBox`
//! switch: how much the stored game list takes, and, while the switch is
//! off, removing it (confirmed, naming its size). Turning the switch off
//! only stops the weekly downloads; the list stays until removed here.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, Subscription, Window, div,
};
use irori_core::util::format_bytes;
use irori_emulation::EmulationStore;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use crate::widgets::{muted, panel};

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
        let Some(stored) = self.stored else {
            return panel(cx)
                .p_4()
                .child(muted(t("settings.launchbox.looking"), cx));
        };
        let text = match stored {
            Some(size) => t_args(
                "settings.launchbox.stored",
                &[("size", &format_bytes(size))],
            ),
            None => t("settings.launchbox.none"),
        };
        panel(cx)
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_sm().child(text))
                    .when_some(stored.filter(|_| !on), |row, size| {
                        row.child(
                            Button::new("launchbox-remove")
                                .ghost()
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
            .when(!on, |body| {
                body.child(muted(t("settings.launchbox.off_hint"), cx))
            })
    }
}
