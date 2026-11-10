//! The hint shown while a controller button is held to open Big Picture: a
//! small pill at the bottom of the window whose bar fills over the hold.
//! With reduced motion the bar is left out (GPUI would draw it full at once,
//! which reads as done) and only the words show.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, relative,
};
use irori_ui::i18n::t;

use super::{GamepadModel, HoldHint};

/// Shows [`GamepadModel::hold`]. The shell keeps one over its pages.
pub struct HoldHintView {
    pub(super) hint: Option<HoldHint>,
    _subscription: Option<Subscription>,
}

impl HoldHintView {
    /// The hint view, following the controller model when controller
    /// support is installed (it never shows otherwise).
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx: &mut Context<Self>| {
            let subscription = super::model(cx).map(|model| {
                cx.observe(
                    &model,
                    |this: &mut Self, model: Entity<GamepadModel>, cx| {
                        let hint = model.read(cx).hold();
                        if this.hint != hint {
                            this.hint = hint;
                            cx.notify();
                        }
                    },
                )
            });
            Self {
                hint: None,
                _subscription: subscription,
            }
        })
    }
}

impl Render for HoldHintView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::register_catalogs();
        let Some(hint) = self.hint else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let still = irori_ui::style::reduce_motion(cx);
        let label = t("bigpicture.hold.hint");
        // A new hold restarts the bar.
        let id = SharedString::from(format!("bp-hold-{:?}", hint.since));
        let bar = div().h_1().w_full().rounded_full().bg(theme.muted).child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme.primary)
                .w(relative(0.))
                .with_animation(id, Animation::new(hint.duration), |bar, f| {
                    bar.w(relative(f))
                }),
        );
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_6()
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .id("bp-hold-hint")
                    .role(Role::Status)
                    .aria_label(label.clone())
                    .min_w_0()
                    .max_w_full()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Gamepad2))
                            .child(div().min_w_0().truncate().child(label)),
                    )
                    .when(!still, |d| d.child(bar)),
            )
            .into_any_element()
    }
}
