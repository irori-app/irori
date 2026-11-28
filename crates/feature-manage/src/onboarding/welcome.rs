//! Step 1 of first-run onboarding: brand lockup, live language picker,
//! OS-adapted summary, and a compact 2x2 overview of Irori's core pillars.

use gpui_kit::assets::IconName;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div, rems,
};
use irori_ui::components::{BrandMark, BrandWordmark};
use irori_ui::i18n::{t, t_args};

use super::OnboardingView;

fn platform_welcome_description() -> SharedString {
    if cfg!(target_os = "macos") {
        t("manage.onboarding.welcome.description_macos")
    } else if cfg!(target_os = "windows") {
        t("manage.onboarding.welcome.description_windows")
    } else {
        t("manage.onboarding.welcome.description_linux")
    }
}

fn platform_play_highlight() -> SharedString {
    if cfg!(target_os = "macos") {
        t("manage.onboarding.highlight.play.body_macos")
    } else if cfg!(target_os = "windows") {
        t("manage.onboarding.highlight.play.body_windows")
    } else {
        t("manage.onboarding.highlight.play.body_linux")
    }
}

fn highlight_tile(icon: IconName, title: SharedString, body: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .flex_1()
        .min_w(rems(15.))
        .items_start()
        .gap_3()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            div()
                .flex_shrink_0()
                .p_2()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .child(Icon::new(icon).small().text_color(theme.primary)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(body),
                ),
        )
        .into_any_element()
}

impl OnboardingView {
    pub(super) fn render_welcome(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let palette_keys = if cfg!(target_os = "macos") {
            "⌘K"
        } else {
            "Ctrl+K"
        };
        let top = h_flex()
            .flex_wrap()
            .justify_between()
            .items_center()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2p5()
                    .child(BrandMark::new(rems(2.5)))
                    .child(BrandWordmark::new(rems(1.5), theme.foreground)),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t("manage.onboarding.welcome.language")),
                    )
                    .child(div().w(rems(12.)).child(Select::new(&self.language))),
            );
        let row_1 = h_flex()
            .flex_wrap()
            .gap_3()
            .child(highlight_tile(
                IconName::LibraryBig,
                t("manage.onboarding.highlight.library.title"),
                t("manage.onboarding.highlight.library.body"),
                cx,
            ))
            .child(highlight_tile(
                IconName::Joystick,
                t("manage.onboarding.highlight.emulation.title"),
                t("manage.onboarding.highlight.emulation.body"),
                cx,
            ));
        let row_2 = h_flex()
            .flex_wrap()
            .gap_3()
            .child(highlight_tile(
                IconName::Zap,
                t("manage.onboarding.highlight.play.title"),
                platform_play_highlight(),
                cx,
            ))
            .child(highlight_tile(
                IconName::Gamepad2,
                t("manage.onboarding.highlight.couch.title"),
                t_args(
                    "manage.onboarding.highlight.couch.body",
                    &[("keys", palette_keys)],
                ),
                cx,
            ));
        v_flex()
            .gap_4()
            .child(top)
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_2xl()
                            .font_bold()
                            .child(t("manage.onboarding.welcome.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(platform_welcome_description()),
                    ),
            )
            .child(v_flex().gap_3().child(row_1).child(row_2))
            .into_any_element()
    }
}
