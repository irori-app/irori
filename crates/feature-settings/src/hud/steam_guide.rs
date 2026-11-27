//! Visual step-by-step dialog showing where to paste Irori's Steam launch
//! options inside Steam's game Properties window.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, ClipboardItem, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    WeakEntity, Window, div, px, rems,
};
use irori_ui::i18n::{t, t_args};

use super::HudPanel;

#[derive(Clone)]
struct GuideCopy {
    title: SharedString,
    subtitle: SharedString,
    crumbs: [SharedString; 5],
    steps: [(SharedString, SharedString, IconName); 4],
    mock_title: SharedString,
    mock_general: SharedString,
    mock_updates: SharedString,
    mock_installed_files: SharedString,
    mock_compatibility: SharedString,
    mock_launch_options: SharedString,
    mock_paste_here: SharedString,
    got_it: SharedString,
}

impl GuideCopy {
    fn load(game_title: Option<&str>) -> Self {
        let step1_body = match game_title {
            Some(title) if !title.trim().is_empty() => t_args(
                "settings.hud.steam_guide.step1.body_game",
                &[("title", title)],
            ),
            _ => t("settings.hud.steam_guide.step1.body"),
        };
        let mock_title = match game_title {
            Some(title) if !title.trim().is_empty() => t_args(
                "settings.hud.steam_guide.mock.title_game",
                &[("title", title)],
            ),
            _ => t("settings.hud.steam_guide.mock.title"),
        };
        Self {
            title: t("settings.hud.steam_guide.title"),
            subtitle: t("settings.hud.steam_guide.subtitle"),
            crumbs: [
                t("settings.hud.steam_guide.crumb.library"),
                t("settings.hud.steam_guide.crumb.right_click"),
                t("settings.hud.steam_guide.crumb.properties"),
                t("settings.hud.steam_guide.crumb.general"),
                t("settings.hud.steam_guide.crumb.launch_options"),
            ],
            steps: [
                (
                    t("settings.hud.steam_guide.step1.title"),
                    step1_body,
                    IconName::LibraryBig,
                ),
                (
                    t("settings.hud.steam_guide.step2.title"),
                    t("settings.hud.steam_guide.step2.body"),
                    IconName::Gamepad2,
                ),
                (
                    t("settings.hud.steam_guide.step3.title"),
                    t("settings.hud.steam_guide.step3.body"),
                    IconName::Settings,
                ),
                (
                    t("settings.hud.steam_guide.step4.title"),
                    t("settings.hud.steam_guide.step4.body"),
                    IconName::ClipboardPaste,
                ),
            ],
            mock_title,
            mock_general: t("settings.hud.steam_guide.mock.general"),
            mock_updates: t("settings.hud.steam_guide.mock.updates"),
            mock_installed_files: t("settings.hud.steam_guide.mock.installed_files"),
            mock_compatibility: t("settings.hud.steam_guide.mock.compatibility"),
            mock_launch_options: t("settings.hud.steam_guide.mock.launch_options"),
            mock_paste_here: t("settings.hud.steam_guide.mock.paste_here"),
            got_it: t("settings.hud.steam_guide.got_it"),
        }
    }
}

/// Opens a centered visual guide explaining where to paste `snippet` in Steam.
pub(super) fn open_steam_launch_guide_dialog(
    panel: WeakEntity<HudPanel>,
    snippet: SharedString,
    game_title: Option<SharedString>,
    copy_label: SharedString,
    copied_msg: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    let copy = GuideCopy::load(game_title.as_deref());
    let width = window.rem_size() * 38.;
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let bg = theme.background;
        let secondary = theme.secondary;
        let muted_bg = theme.muted;
        let border = theme.border;
        let primary = theme.primary;
        let primary_fg = theme.primary_foreground;
        let fg = theme.foreground;
        let muted_fg = theme.muted_foreground;
        let radius = theme.radius;
        let radius_lg = theme.radius_lg;
        let mono_font = theme.mono_font_family.clone();

        let title_el = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(rems(1.75))
                    .rounded(radius)
                    .bg(primary.opacity(0.14))
                    .text_color(primary)
                    .child(Icon::new(IconName::Gamepad2).small()),
            )
            .child(copy.title.clone());

        let mut breadcrumb = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_2()
            .rounded(radius)
            .bg(secondary)
            .border_1()
            .border_color(border);

        for (idx, crumb) in copy.crumbs.iter().enumerate() {
            let is_last = idx + 1 == copy.crumbs.len();
            if idx > 0 {
                breadcrumb = breadcrumb.child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_fg),
                );
            }
            let pill = if is_last {
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(radius)
                    .bg(primary.opacity(0.16))
                    .border_1()
                    .border_color(primary.opacity(0.45))
                    .text_xs()
                    .font_semibold()
                    .text_color(primary)
                    .child(crumb.clone())
            } else {
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(radius)
                    .bg(muted_bg)
                    .text_xs()
                    .font_medium()
                    .text_color(fg)
                    .child(crumb.clone())
            };
            breadcrumb = breadcrumb.child(pill);
        }

        let step_card = |index: usize| {
            let (step_title, step_body, icon) = &copy.steps[index];
            let number = SharedString::from(format!("{}", index + 1));
            v_flex()
                .flex_1()
                .min_w_0()
                .p_3()
                .gap_1p5()
                .rounded(radius)
                .bg(secondary)
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .flex_shrink_0()
                                        .size(rems(1.35))
                                        .rounded_full()
                                        .bg(primary.opacity(0.16))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(primary)
                                        .child(number),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .truncate()
                                        .child(step_title.clone()),
                                ),
                        )
                        .child(Icon::new(*icon).xsmall().text_color(muted_fg)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(step_body.clone()),
                )
        };

        let steps_grid = v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .gap_2()
                    .child(step_card(0))
                    .child(step_card(1)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .gap_2()
                    .child(step_card(2))
                    .child(step_card(3)),
            );

        let window_dot = |alpha: f32| {
            div()
                .size(px(7.))
                .rounded_full()
                .bg(muted_fg.opacity(alpha))
        };

        let steam_mockup = v_flex()
            .w_full()
            .rounded(radius_lg)
            .bg(bg)
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .bg(secondary)
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .child(window_dot(0.35))
                                    .child(window_dot(0.25))
                                    .child(window_dot(0.2)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(muted_fg)
                                    .truncate()
                                    .child(copy.mock_title.clone()),
                            ),
                    )
                    .child(Icon::new(IconName::X).xsmall().text_color(muted_fg)),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .child(
                        v_flex()
                            .w(rems(8.5))
                            .flex_shrink_0()
                            .p_2()
                            .gap_1()
                            .bg(secondary.opacity(0.55))
                            .border_r_1()
                            .border_color(border)
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(radius)
                                    .bg(primary.opacity(0.16))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(primary)
                                    .truncate()
                                    .child(copy.mock_general.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .text_color(muted_fg.opacity(0.75))
                                    .truncate()
                                    .child(copy.mock_updates.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .text_color(muted_fg.opacity(0.75))
                                    .truncate()
                                    .child(copy.mock_installed_files.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .text_color(muted_fg.opacity(0.75))
                                    .truncate()
                                    .child(copy.mock_compatibility.clone()),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .p_3()
                            .gap_2()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(muted_fg)
                                            .child(copy.mock_launch_options.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(primary)
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(primary_fg)
                                            .child(copy.mock_paste_here.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .p_2p5()
                                    .rounded(radius)
                                    .bg(muted_bg)
                                    .border_1()
                                    .border_color(primary)
                                    .overflow_hidden()
                                    .text_xs()
                                    .font_family(mono_font)
                                    .text_color(fg)
                                    .child(snippet.clone()),
                            ),
                    ),
            );

        let panel_copy = panel.clone();
        let snippet_copy = snippet.clone();
        let copied_copy = copied_msg.clone();

        dialog.title(title_el).w(width).child(
            v_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_fg)
                        .child(copy.subtitle.clone()),
                )
                .child(breadcrumb)
                .child(steps_grid)
                .child(steam_mockup)
                .child(
                    DialogFooter::new()
                        .child(
                            Button::new("hud-steam-guide-copy")
                                .primary()
                                .icon(IconName::Copy)
                                .label(copy_label.clone())
                                .on_click(move |_, window, cx| {
                                    let _ = panel_copy.update(cx, |this, cx| {
                                        this.sync_live_mangohud_config(cx);
                                    });
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        snippet_copy.to_string(),
                                    ));
                                    window.push_notification(
                                        Notification::success(copied_copy.clone()),
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("hud-steam-guide-close")
                                .outline()
                                .label(copy.got_it.clone())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
                .into_any_element(),
        )
    });
}
