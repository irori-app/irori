//! The "Connected controllers" panel of Settings -> Controller.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_bigpicture::gamepad::{self, Battery, ControllerInfo, ControllerKind};
use irori_ui::i18n::{t, t_args};

use crate::widgets::{muted, panel};

/// Display name of a controller family.
pub(crate) fn kind_label(kind: ControllerKind) -> &'static str {
    match kind {
        ControllerKind::Xbox => "Xbox",
        ControllerKind::PlayStation4 => "PlayStation 4",
        ControllerKind::PlayStation5 => "PlayStation 5",
        ControllerKind::Nintendo => "Nintendo",
        ControllerKind::SteamDeck => "Steam Deck",
        ControllerKind::Generic => "Generic",
    }
}

/// Display name of a controller family in the interface language.
pub(crate) fn kind_text(kind: ControllerKind) -> SharedString {
    match kind {
        ControllerKind::Generic => t("settings.controllers.kind_generic"),
        other => kind_label(other).into(),
    }
}

/// Text of the battery column; unknown shows nothing.
pub(crate) fn battery_text(battery: Battery) -> Option<SharedString> {
    match battery {
        Battery::Wired => Some(t("settings.controllers.battery.wired")),
        Battery::Discharging(level) => Some(format!("{level}%").into()),
        Battery::Charging(level) => Some(t_args(
            "settings.controllers.battery.charging",
            &[("level", &level.to_string())],
        )),
        Battery::Full => Some(t("settings.controllers.battery.full")),
        Battery::Unknown => None,
    }
}

/// Lists the controllers the gamepad reader sees. Follows connections live
/// when controller support is running; the refresh button re-reads the list
/// otherwise.
pub(crate) struct ControllersPanel {
    controllers: Vec<ControllerInfo>,
    _subscriptions: Vec<Subscription>,
}

impl ControllersPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(model) = gamepad::model(cx) {
            subscriptions.push(cx.observe(&model, |this, _, cx| this.refresh(cx)));
        }
        Self {
            controllers: gamepad::controllers(),
            _subscriptions: subscriptions,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.controllers = gamepad::controllers();
        cx.notify();
    }
}

impl Render for ControllersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_fg, danger, primary, bg, success) = (
            theme.border,
            theme.muted_foreground,
            theme.danger,
            theme.primary,
            theme.background,
            theme.success,
        );
        let count = self.controllers.len();
        let status_tag = if count == 0 {
            Tag::secondary()
                .xsmall()
                .outline()
                .child(t("settings.controllers.count_none"))
        } else {
            let key = if count == 1 {
                "settings.controllers.count_one"
            } else {
                "settings.controllers.count_other"
            };
            Tag::success()
                .xsmall()
                .child(t_args(key, &[("count", &count.to_string())]))
        };
        let refresh = Button::new("controllers-refresh")
            .ghost()
            .xsmall()
            .icon(IconName::RefreshCw)
            .label(t("settings.common.refresh"))
            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)));
        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .child(status_tag)
            .child(refresh);
        let icon_badge = |bg_tint, fg_tint| {
            div()
                .size_8()
                .rounded(theme.radius)
                .bg(bg_tint)
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Gamepad2).small().text_color(fg_tint))
        };
        let list = if self.controllers.is_empty() {
            panel(cx).bg(bg.opacity(0.45)).child(
                h_flex()
                    .gap_3()
                    .px_3()
                    .py_2p5()
                    .items_center()
                    .child(icon_badge(theme.muted, muted_fg))
                    .child(
                        muted(t("settings.controllers.empty"), cx)
                            .flex_1()
                            .min_w_0(),
                    ),
            )
        } else {
            self.controllers.iter().enumerate().fold(
                panel(cx).bg(bg.opacity(0.45)),
                |list, (ix, c)| {
                    let battery = battery_text(c.battery);
                    let low = c.battery.is_low();
                    let player = t_args(
                        "settings.controllers.player_slot",
                        &[("index", &(ix + 1).to_string())],
                    );
                    list.child(
                        h_flex()
                            .id(ElementId::Name(format!("controller-{ix}").into()))
                            .gap_3()
                            .px_3()
                            .py_2p5()
                            .items_center()
                            .when(ix > 0, |r| r.border_t_1().border_color(border))
                            .child(icon_badge(primary.opacity(0.16), primary))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .font_medium()
                                            .child(c.name.clone()),
                                    )
                                    .child(Tag::secondary().xsmall().outline().child(player)),
                            )
                            .child(Tag::secondary().xsmall().outline().child(kind_text(c.kind)))
                            .when_some(battery, |r, b| {
                                r.child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_1p5()
                                        .child(div().size_1p5().rounded_full().bg(if low {
                                            danger
                                        } else {
                                            success
                                        }))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(if low { danger } else { muted_fg })
                                                .child(b),
                                        ),
                                )
                            }),
                    )
                },
            )
        };
        v_flex().w_full().gap_2().child(header).child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_labels() {
        crate::i18n::register();
        assert_eq!(kind_label(ControllerKind::PlayStation5), "PlayStation 5");
        assert_eq!(kind_label(ControllerKind::SteamDeck), "Steam Deck");
        assert_eq!(battery_text(Battery::Wired).as_deref(), Some("Wired"));
        assert_eq!(battery_text(Battery::Unknown), None);
        assert_eq!(
            battery_text(Battery::Discharging(15)).as_deref(),
            Some("15%")
        );
    }
}
