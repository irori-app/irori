//! Global OptiScaler defaults in `Settings -> Game defaults -> Performance`.
//!
//! Every game inherits these defaults when opening or installing OptiScaler,
//! and changing the global menu shortcut key automatically updates
//! `[Menu] ShortcutKey` in `OptiScaler.ini` for installed games that do not
//! set a per-game shortcut override (including Steam games).

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::OptiscalerDefaults;
use irori_reshade::optiscaler::{
    GpuDriverMode, OptiscalerChannel, OptiscalerConfig, ProxyDll, default_shortcut_from,
};
use irori_ui::i18n::t;
use irori_ui::{backend, models};

use crate::form::ChoiceItem;
use crate::i18n::LanguageStamp;
use crate::optiscaler_key::{KeyCaptured, OptiscalerKeyCapture};
use crate::widgets::{RowState, field_row, group_panel};

const CHANNEL_STABLE: &str = "stable";
const CHANNEL_NIGHTLY: &str = "nightly";
const DRIVER_MESA_AUTO: &str = "mesaAuto";
const DRIVER_NVIDIA: &str = "nvidia";

struct Labels {
    channel_label: SharedString,
    channel_help: SharedString,
    channel_stable: SharedString,
    channel_nightly: SharedString,
    driver_label: SharedString,
    driver_help: SharedString,
    driver_mesa_auto: SharedString,
    driver_nvidia: SharedString,
    proxy_label: SharedString,
    proxy_help: SharedString,
    optipatcher_label: SharedString,
    optipatcher_help: SharedString,
    shortcut_label: SharedString,
    shortcut_help: SharedString,
}

impl Labels {
    fn load() -> Self {
        Self {
            channel_label: t("settings.optiscaler.channel.label"),
            channel_help: t("settings.optiscaler.channel.help"),
            channel_stable: t("settings.optiscaler.channel.stable"),
            channel_nightly: t("settings.optiscaler.channel.nightly"),
            driver_label: t("settings.optiscaler.gpu_driver.label"),
            driver_help: t("settings.optiscaler.gpu_driver.help"),
            driver_mesa_auto: t("settings.optiscaler.gpu_driver.mesa_auto"),
            driver_nvidia: t("settings.optiscaler.gpu_driver.nvidia"),
            proxy_label: t("settings.optiscaler.proxy_dll.label"),
            proxy_help: t("settings.optiscaler.proxy_dll.help"),
            optipatcher_label: t("settings.optiscaler.optipatcher.label"),
            optipatcher_help: t("settings.optiscaler.optipatcher.help"),
            shortcut_label: t("settings.optiscaler.shortcut.label"),
            shortcut_help: t("settings.optiscaler.defaults.shortcut_help"),
        }
    }
}

fn channel_key(channel: OptiscalerChannel) -> &'static str {
    match channel {
        OptiscalerChannel::Stable => CHANNEL_STABLE,
        OptiscalerChannel::Nightly => CHANNEL_NIGHTLY,
    }
}

fn driver_key(driver: GpuDriverMode) -> &'static str {
    match driver {
        GpuDriverMode::MesaAuto => DRIVER_MESA_AUTO,
        GpuDriverMode::Nvidia => DRIVER_NVIDIA,
    }
}

fn channel_items(labels: &Labels) -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::new(
            SharedString::new_static(CHANNEL_STABLE),
            labels.channel_stable.clone(),
        ),
        ChoiceItem::new(
            SharedString::new_static(CHANNEL_NIGHTLY),
            labels.channel_nightly.clone(),
        ),
    ]
}

fn driver_items(labels: &Labels) -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::new(
            SharedString::new_static(DRIVER_MESA_AUTO),
            labels.driver_mesa_auto.clone(),
        ),
        ChoiceItem::new(
            SharedString::new_static(DRIVER_NVIDIA),
            labels.driver_nvidia.clone(),
        ),
    ]
}

fn proxy_items() -> Vec<ChoiceItem> {
    ProxyDll::ALL
        .iter()
        .map(|dll| {
            let name = SharedString::new_static(dll.file_name());
            ChoiceItem::new(name.clone(), name)
        })
        .collect()
}

pub(crate) struct OptiscalerDefaultsPanel {
    defaults: OptiscalerDefaults,
    channel_select: Entity<SelectState<Vec<ChoiceItem>>>,
    driver_select: Entity<SelectState<Vec<ChoiceItem>>>,
    proxy_select: Entity<SelectState<Vec<ChoiceItem>>>,
    key_capture: Entity<OptiscalerKeyCapture>,
    labels: Labels,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl OptiscalerDefaultsPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let labels = Labels::load();
        let defaults = backend(cx).settings.app().game.optiscaler.clone();
        let cfg = OptiscalerConfig::from_defaults(&defaults);
        let shortcut = default_shortcut_from(&defaults);

        let channel_select = cx.new(|cx| {
            let mut s = SelectState::new(channel_items(&labels), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(channel_key(cfg.channel)),
                window,
                cx,
            );
            s
        });
        let driver_select = cx.new(|cx| {
            let mut s = SelectState::new(driver_items(&labels), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(driver_key(cfg.gpu_driver)),
                window,
                cx,
            );
            s
        });
        let proxy_select = cx.new(|cx| {
            let mut s = SelectState::new(proxy_items(), None, window, cx);
            s.set_selected_value(
                &SharedString::new_static(cfg.proxy_dll.file_name()),
                window,
                cx,
            );
            s
        });
        let key_capture = cx
            .new(|cx| OptiscalerKeyCapture::new("optiscaler-global-shortcut", shortcut, None, cx));

        let sub_channel = cx.subscribe_in(
            &channel_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(Some(key)) = event else {
                    return;
                };
                this.update_defaults(cx, |d| d.channel = key.to_string());
            },
        );
        let sub_driver = cx.subscribe_in(
            &driver_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(Some(key)) = event else {
                    return;
                };
                this.update_defaults(cx, |d| d.gpu_driver = key.to_string());
            },
        );
        let sub_proxy = cx.subscribe_in(
            &proxy_select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, _, cx| {
                let SelectEvent::Confirm(Some(key)) = event else {
                    return;
                };
                this.update_defaults(cx, |d| d.proxy_dll = key.to_string());
            },
        );
        let sub_key = cx.subscribe_in(
            &key_capture,
            window,
            |this, _, event: &KeyCaptured, _, cx| {
                let next_str = event.0.clone().unwrap_or_default();
                let next_opt = event.0.clone();
                if this.defaults.shortcut_key != next_str {
                    this.update_defaults(cx, |d| d.shortcut_key = next_str);
                    let b = backend(cx);
                    irori_ui::run(cx, async move {
                        if let Err(e) = b.sync_global_optiscaler_shortcut(next_opt).await {
                            tracing::warn!("syncing global OptiScaler shortcut: {e:#}");
                        }
                        Ok(())
                    })
                    .detach();
                }
            },
        );
        let settings_model = models(cx).settings.clone();
        let sub_settings = cx.observe_in(&settings_model, window, |this, _, window, cx| {
            this.reload_from_settings(window, cx);
        });

        Self {
            defaults,
            channel_select,
            driver_select,
            proxy_select,
            key_capture,
            labels,
            language: LanguageStamp::new(),
            _subscriptions: vec![sub_channel, sub_driver, sub_proxy, sub_key, sub_settings],
        }
    }

    fn update_defaults(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut OptiscalerDefaults)) {
        f(&mut self.defaults);
        let next = self.defaults.clone();
        backend(cx)
            .settings
            .update_app(|app| app.game.optiscaler = next);
        cx.notify();
    }

    fn reload_from_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = backend(cx).settings.app().game.optiscaler.clone();
        if self.defaults == next {
            return;
        }
        self.defaults = next;
        self.sync_controls(window, cx);
        cx.notify();
    }

    fn sync_controls(&self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = OptiscalerConfig::from_defaults(&self.defaults);
        let channel = SharedString::new_static(channel_key(cfg.channel));
        let driver = SharedString::new_static(driver_key(cfg.gpu_driver));
        let proxy = SharedString::new_static(cfg.proxy_dll.file_name());
        self.channel_select
            .update(cx, |s, cx| s.set_selected_value(&channel, window, cx));
        self.driver_select
            .update(cx, |s, cx| s.set_selected_value(&driver, window, cx));
        self.proxy_select
            .update(cx, |s, cx| s.set_selected_value(&proxy, window, cx));
        let shortcut = default_shortcut_from(&self.defaults);
        self.key_capture
            .update(cx, |k, cx| k.set_value(shortcut, cx));
    }

    fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.labels = Labels::load();
        let cfg = OptiscalerConfig::from_defaults(&self.defaults);
        let ch_items = channel_items(&self.labels);
        let dr_items = driver_items(&self.labels);
        let channel = SharedString::new_static(channel_key(cfg.channel));
        let driver = SharedString::new_static(driver_key(cfg.gpu_driver));
        self.channel_select.update(cx, |s, cx| {
            s.set_items(ch_items, window, cx);
            s.set_selected_value(&channel, window, cx);
        });
        self.driver_select.update(cx, |s, cx| {
            s.set_items(dr_items, window, cx);
            s.set_selected_value(&driver, window, cx);
        });
        cx.notify();
    }
}

impl Render for OptiscalerDefaultsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| {
                this.refresh_language(window, cx);
            });
        }
        let border = cx.theme().border;
        let divider = |el: AnyElement| div().border_t_1().border_color(border).child(el);

        let shortcut_row = field_row(
            RowState {
                label: self.labels.shortcut_label.clone(),
                help: self.labels.shortcut_help.clone(),
                ..Default::default()
            },
            self.key_capture.clone().into_any_element(),
            cx,
        )
        .into_any_element();

        let channel_row = field_row(
            RowState {
                label: self.labels.channel_label.clone(),
                help: self.labels.channel_help.clone(),
                ..Default::default()
            },
            Select::new(&self.channel_select)
                .id("optiscaler-global-channel")
                .accessibility_label(self.labels.channel_label.clone())
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let driver_row = field_row(
            RowState {
                label: self.labels.driver_label.clone(),
                help: self.labels.driver_help.clone(),
                ..Default::default()
            },
            Select::new(&self.driver_select)
                .id("optiscaler-global-gpu-driver")
                .accessibility_label(self.labels.driver_label.clone())
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let proxy_row = field_row(
            RowState {
                label: self.labels.proxy_label.clone(),
                help: self.labels.proxy_help.clone(),
                ..Default::default()
            },
            Select::new(&self.proxy_select)
                .id("optiscaler-global-proxy-dll")
                .accessibility_label(self.labels.proxy_label.clone())
                .w_48()
                .into_any_element(),
            cx,
        )
        .into_any_element();

        let optipatcher_row = field_row(
            RowState {
                label: self.labels.optipatcher_label.clone(),
                help: self.labels.optipatcher_help.clone(),
                ..Default::default()
            },
            Switch::new("optiscaler-global-optipatcher")
                .checked(self.defaults.enable_optipatcher)
                .accessibility_label(self.labels.optipatcher_label.clone())
                .on_change(cx.listener(|this, on: &bool, _, cx| {
                    this.update_defaults(cx, |d| d.enable_optipatcher = *on);
                }))
                .into_any_element(),
            cx,
        )
        .into_any_element();

        v_flex().w_full().pt_1().child(
            group_panel(cx)
                .child(shortcut_row)
                .child(divider(channel_row))
                .child(divider(driver_row))
                .child(divider(proxy_row))
                .child(divider(optipatcher_row)),
        )
    }
}
