//! Pinned modal chrome views ([`InstallHeader`] and [`InstallFooter`])
//! observing [`InstallSheet`].

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Window, div, linear_color_stop, linear_gradient,
    px, rems,
};
use irori_core::Runner;
use irori_core::util::format_bytes;
use irori_ui::components::{GameCover, StoreLogo};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{ICON_CONTRAST, TEXT_CONTRAST, art_tint, legible};
use irori_ui::theme::reduce_transparency;

use super::InstallSheet;
use crate::hooks::SteamDeckStatus;
use crate::sizes::DiskVerdict;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompatBadgeTone {
    Success,
    Warning,
    Danger,
    Neutral,
}

pub(super) fn protondb_tone(tier: &str) -> CompatBadgeTone {
    match tier.trim().to_ascii_lowercase().as_str() {
        "platinum" | "native" => CompatBadgeTone::Success,
        "gold" | "silver" | "bronze" => CompatBadgeTone::Warning,
        "borked" => CompatBadgeTone::Danger,
        _ => CompatBadgeTone::Neutral,
    }
}

/// Pinned Hero Header of the install / modify modal: artwork cover, title,
/// badges, ambient art tint, close button and live 3-metric storage summary.
pub(super) struct InstallHeader {
    sheet: Entity<InstallSheet>,
    _observe: Vec<Subscription>,
}

impl InstallHeader {
    pub(super) fn new(sheet: Entity<InstallSheet>, cx: &mut Context<Self>) -> Self {
        let art = irori_ui::art::art_store(cx);
        let observe = vec![
            cx.observe(&sheet, |_, _, cx| cx.notify()),
            cx.observe(&art, |_, _, cx| cx.notify()),
        ];
        Self {
            sheet,
            _observe: observe,
        }
    }

    fn render_store_pill(runner: Runner, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let store_name: SharedString = runner.store_name().into();
        let pill = h_flex()
            .id("install-store-badge")
            .items_center()
            .justify_center()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.secondary.opacity(0.65))
            .border_1()
            .border_color(theme.border)
            .tooltip(move |window, cx| Tooltip::new(store_name.clone()).build(window, cx));
        match StoreLogo::of_runner(runner) {
            Some(logo) => pill
                .child(logo.icon().size(px(20.)).text_color(theme.foreground))
                .into_any_element(),
            None => pill
                .child(
                    div()
                        .px_0p5()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(runner.short_name()),
                )
                .into_any_element(),
        }
    }

    fn render_metric(
        icon: IconName,
        accent: Hsla,
        label: SharedString,
        value: Option<(SharedString, Hsla)>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let accent_ink = legible(
            accent,
            &[theme.background, theme.secondary],
            ICON_CONTRAST,
            cx,
        );
        h_flex()
            .flex_1()
            .min_w(rems(8.))
            .items_center()
            .gap_3()
            .px_3p5()
            .py_2p5()
            .rounded(theme.radius)
            .bg(theme.secondary.opacity(0.45))
            .border_1()
            .border_color(theme.border.opacity(0.75))
            .child(
                h_flex()
                    .size(px(32.))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(theme.radius)
                    .bg(accent.opacity(0.14))
                    .border_1()
                    .border_color(accent.opacity(0.28))
                    .child(Icon::new(icon).size(px(16.)).text_color(accent_ink)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(SharedString::from(label.to_uppercase())),
                    )
                    .child(match value {
                        Some((text, color)) => div()
                            .text_lg()
                            .font_bold()
                            .text_color(color)
                            .truncate()
                            .child(text)
                            .into_any_element(),
                        None => Skeleton::new()
                            .h_5()
                            .w(rems(5.))
                            .rounded_sm()
                            .into_any_element(),
                    }),
            )
            .into_any_element()
    }
}

impl Render for InstallHeader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.sheet.read(cx);
        let game = state.game.clone();
        let modify = state.is_modify();
        let info_loading = state.info.is_loading();
        let disk_loading = state.disk.is_loading();
        let size = state.size();
        let space = state.disk.ready().copied();
        let verdict = state.disk_verdict();
        let compat = state.compat.clone();

        let tint = (!reduce_transparency(cx))
            .then(|| {
                game.hero_art()
                    .and_then(|url| irori_ui::art::ambient(url, cx))
                    .map(|a| art_tint(a.rgb, cx))
            })
            .flatten();

        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        let bg = theme.background;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let info_color = theme.info;
        let primary_color = theme.primary;
        let success_color = legible(theme.success, &[bg], TEXT_CONTRAST, cx);
        let danger_color = legible(theme.danger, &[bg], TEXT_CONTRAST, cx);

        let subtitle: Option<SharedString> = if modify {
            let base = t("game.install.subtitle_modify");
            Some(
                match game
                    .install
                    .as_ref()
                    .map(|i| i.version.trim())
                    .filter(|v| !v.is_empty())
                {
                    Some(ver) => format!("{base} · {ver}").into(),
                    None => base,
                },
            )
        } else {
            game.developer
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    game.publisher
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .map(|s| SharedString::from(s.to_string()))
        };

        let mut compat_pills: Vec<AnyElement> = Vec::new();
        if cfg!(target_os = "linux")
            && let Some(c) = &compat
        {
            if let Some(tier) = &c.protondb_tier {
                let tier_text = crate::page::compat::tier_label(tier);
                let tip = t_args("game.header.protondb", &[("tier", &tier_text)]);
                let tag = match protondb_tone(tier) {
                    CompatBadgeTone::Success => Tag::success(),
                    CompatBadgeTone::Warning => Tag::warning(),
                    CompatBadgeTone::Danger => Tag::danger(),
                    CompatBadgeTone::Neutral => Tag::secondary(),
                };
                compat_pills.push(
                    div()
                        .id("install-protondb-badge")
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(tag.xsmall().outline().font_medium().child(tier_text))
                        .into_any_element(),
                );
            }
            if let Some(deck) = c.steam_deck.filter(|d| *d != SteamDeckStatus::Unknown) {
                let deck_label = deck.label();
                let tip = t_args("game.header.deck", &[("status", &deck_label)]);
                let tag = match deck {
                    SteamDeckStatus::Verified => Tag::success(),
                    SteamDeckStatus::Playable => Tag::warning(),
                    SteamDeckStatus::Unsupported => Tag::danger(),
                    SteamDeckStatus::Unknown => Tag::secondary(),
                };
                compat_pills.push(
                    div()
                        .id("install-deck-badge")
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(
                            tag.xsmall()
                                .outline()
                                .font_medium()
                                .child(format!("Deck {deck_label}")),
                        )
                        .into_any_element(),
                );
            }
        }

        let download_value = (!info_loading).then(|| {
            let text = if size.download == 0 {
                t("game.install.unknown")
            } else {
                format_bytes(size.download).into()
            };
            (text, fg)
        });
        let disk_value = (!info_loading).then(|| {
            let text = if size.disk == 0 {
                t("game.install.unknown")
            } else {
                format_bytes(size.disk).into()
            };
            (text, fg)
        });
        let (third_icon, third_accent, third_label, third_value) = if info_loading || disk_loading {
            (
                IconName::CircleCheck,
                theme.success,
                t("game.install.free_after"),
                None,
            )
        } else {
            match verdict {
                DiskVerdict::Fits { left_after } => (
                    IconName::CircleCheck,
                    theme.success,
                    t("game.install.free_after"),
                    Some((format_bytes(left_after).into(), success_color)),
                ),
                DiskVerdict::TooSmall { missing } => (
                    IconName::TriangleAlert,
                    theme.danger,
                    t("game.install.short_by"),
                    Some((format_bytes(missing).into(), danger_color)),
                ),
                DiskVerdict::NotWritable => (
                    IconName::TriangleAlert,
                    theme.danger,
                    t("game.install.free_after"),
                    Some((
                        space
                            .map(|s| format_bytes(s.free).into())
                            .unwrap_or_else(|| t("game.install.unknown")),
                        danger_color,
                    )),
                ),
                DiskVerdict::Unknown => (
                    IconName::CircleCheck,
                    if space.is_some() {
                        theme.success
                    } else {
                        muted
                    },
                    t("game.install.free_after"),
                    Some((
                        space
                            .map(|s| format_bytes(s.free).into())
                            .unwrap_or_else(|| t("game.install.unknown")),
                        if space.is_some() {
                            success_color
                        } else {
                            muted
                        },
                    )),
                ),
            }
        };

        div()
            .id("install-header")
            .debug_selector(|| "install-header".into())
            .relative()
            .w_full()
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .overflow_hidden()
            .when_some(tint, |el, tint| {
                let top_alpha = if dark { 0.24 } else { 0.13 };
                el.child(div().absolute().inset_0().bg(linear_gradient(
                    140.,
                    linear_color_stop(tint.opacity(top_alpha), 0.),
                    linear_color_stop(tint.opacity(0.), 0.78),
                )))
            })
            .child(
                h_flex()
                    .relative()
                    .w_full()
                    .px_6()
                    .py_5()
                    .gap_5()
                    .items_stretch()
                    .child(
                        div()
                            .flex_shrink_0()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(border)
                            .overflow_hidden()
                            .shadow_md()
                            .child(
                                GameCover::new(game.clone())
                                    .width(rems(6.))
                                    .radius(theme.radius),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_start()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1p5()
                                            .child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_2p5()
                                                    .flex_wrap()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .text_2xl()
                                                            .font_extrabold()
                                                            .text_color(fg)
                                                            .line_clamp(2)
                                                            .child(game.title.clone()),
                                                    )
                                                    .child(Self::render_store_pill(
                                                        game.runner(),
                                                        cx,
                                                    ))
                                                    .when(modify, |row| {
                                                        row.child(
                                                            Tag::primary()
                                                                .xsmall()
                                                                .outline()
                                                                .font_semibold()
                                                                .child(t(
                                                                    "game.install.modify_badge",
                                                                )),
                                                        )
                                                    }),
                                            )
                                            .when(
                                                !compat_pills.is_empty() || subtitle.is_some(),
                                                |col| {
                                                    col.child(
                                                        h_flex()
                                                            .min_w_0()
                                                            .items_center()
                                                            .gap_2()
                                                            .flex_wrap()
                                                            .children(compat_pills)
                                                            .when_some(subtitle, |row, sub| {
                                                                row.child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .text_sm()
                                                                        .font_medium()
                                                                        .text_color(muted)
                                                                        .truncate()
                                                                        .child(sub),
                                                                )
                                                            }),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        Button::new("install-modal-close")
                                            .flex_shrink_0()
                                            .small()
                                            .ghost()
                                            .icon(IconName::X)
                                            .accessibility_label(t("game.install.close"))
                                            .tooltip(t("game.install.close"))
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_stretch()
                                    .gap_3()
                                    .child(Self::render_metric(
                                        IconName::CloudDownload,
                                        info_color,
                                        t("game.install.download"),
                                        download_value,
                                        cx,
                                    ))
                                    .child(Self::render_metric(
                                        IconName::HardDriveDownload,
                                        primary_color,
                                        t("game.install.size_on_disk"),
                                        disk_value,
                                        cx,
                                    ))
                                    .child(Self::render_metric(
                                        third_icon,
                                        third_accent,
                                        third_label,
                                        third_value,
                                        cx,
                                    )),
                            ),
                    ),
            )
    }
}

/// Modal footer: secondary actions and the primary `Install · <size>` / `Save changes` button.
pub struct InstallFooter {
    sheet: Entity<InstallSheet>,
    _observe: Subscription,
}

impl InstallFooter {
    pub(super) fn new(sheet: Entity<InstallSheet>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&sheet, |_, _, cx| cx.notify());
        Self {
            sheet,
            _observe: observe,
        }
    }
}

impl Render for InstallFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.sheet.read(cx);
        let blocker = state.blocker(cx);
        let label = state.primary_label();
        let modify = state.is_modify();
        let has_changes = state.has_changes();
        let submitting = state.submitting;
        let id = state.id.clone();
        let sheet = self.sheet.clone();
        let theme = cx.theme();

        h_flex()
            .w_full()
            .px_6()
            .py_4()
            .gap_3()
            .flex_wrap()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(!modify, |row| {
                        row.child(
                            Button::new("import-instead")
                                .debug_selector(|| "import-instead".into())
                                .ghost()
                                .icon(IconName::FolderInput)
                                .label(t("game.install.import_existing"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    crate::manage::open_import_dialog(id.clone(), window, cx);
                                }),
                        )
                    })
                    .when(modify && has_changes && blocker.is_none(), |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t("game.install.changes_ready")),
                        )
                    }),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when_some(blocker.filter(|_| !submitting), |row, reason| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t(reason)),
                        )
                    })
                    .when(modify, |row| {
                        row.child(
                            Button::new("install-cancel")
                                .outline()
                                .label(t("game.common.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                    })
                    .child(
                        Button::new("install-submit")
                            .primary()
                            .when(!modify, |b| b.icon(Icon::new(IconName::Download)))
                            .label(label)
                            .loading(submitting)
                            .disabled(blocker.is_some())
                            .on_click(move |_, window, cx| {
                                sheet.update(cx, |s, cx| s.submit(window, cx));
                            }),
                    ),
            )
    }
}
