//! Body section renderers for [`InstallSheet`]: notices, platform, account,
//! install location, optional components, DLCs, and collapsible advanced
//! options.

use std::collections::BTreeSet;

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Confirm;
use gpui_kit::component::input::Input;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::Select;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    alert::Alert, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Role, SharedString, Styled, div, point, px, relative, rems,
};
use irori_core::util::format_bytes;
use irori_core::{InstallInfo, InstallPlatform, Runner};
use irori_extras::anticheat::InstallGate;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::{Route, navigate};

use super::proton::ProtonDownload;
use super::{InstallSheet, nothing_to_install_text};
use crate::hooks::{self, AnticheatStatus};
use crate::i18n::platform_label;
use crate::load::{Load, language_name};
use crate::sizes::{DiskVerdict, bar_summary, disk_bar_fractions, drive_label, same_dir};
use crate::state::is_unsupported_third_party;

impl InstallSheet {
    pub(super) fn card(cx: &App) -> gpui_kit::Div {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .rounded(theme.radius_lg)
            .bg(theme.secondary.opacity(0.45))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
    }

    fn field_label(label: SharedString, cx: &App) -> AnyElement {
        div()
            .text_xs()
            .font_medium()
            .text_color(cx.theme().muted_foreground)
            .child(label)
            .into_any_element()
    }

    pub(super) fn render_notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Load::Failed(e) = &self.info {
            out.push(
                v_flex()
                    .gap_2()
                    .child(Alert::error(
                        "install-info-error",
                        t_args("game.install.info_failed", &[("error", e.as_ref())]),
                    ))
                    .child(
                        Button::new("retry-install-info")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("game.common.retry"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.fetch_info(window, cx)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if self.not_installable() {
            out.push(
                Alert::warning(
                    "not-installable",
                    t(nothing_to_install_text(self.game.runner())),
                )
                .into_any_element(),
            );
        }
        if let Some(app) = self.third_party() {
            if is_unsupported_third_party(&self.game) {
                out.push(
                    Alert::error(
                        "third-party",
                        t_args("game.install.third_party_unsupported", &[("app", &app)]),
                    )
                    .into_any_element(),
                );
            } else {
                out.push(
                    Alert::info(
                        "third-party",
                        t_args("game.install.third_party", &[("app", &app)]),
                    )
                    .into_any_element(),
                );
            }
        }
        let gate = self.anticheat_gate(cx);
        if let Some(report) = self.compat.as_ref().and_then(|c| c.anticheat.as_ref())
            && gate != InstallGate::Allowed
        {
            let allowed = gate == InstallGate::Confirm;
            let mut col = v_flex().gap_2().child(Alert::warning(
                "anticheat",
                anticheat_warning(report.status, &report.anticheats),
            ));
            if allowed {
                col = col.child(
                    Checkbox::new("anticheat-ack")
                        .label(t("game.install.install_anyway"))
                        .checked(self.anticheat_ack)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            this.anticheat_ack = *v;
                            cx.notify();
                        })),
                );
            } else {
                col = col
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("game.install.anticheat_disabled")),
                    )
                    .child(
                        Button::new("open-advanced")
                            .small()
                            .outline()
                            .label(t("game.install.open_settings"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::Settings("advanced/compat".into()), cx);
                            }),
                    );
            }
            out.push(col.into_any_element());
        } else if let Some(note) = self
            .compat
            .as_ref()
            .and_then(|c| c.anticheat.as_ref())
            .and_then(|r| r.online_only_note())
        {
            out.push(Alert::info("anticheat-online-only", note).into_any_element());
        }
        out
    }

    fn platform_chip_label(platform: InstallPlatform) -> (IconName, SharedString) {
        let native = irori_core::HostPlatform::current().native();
        match platform {
            InstallPlatform::Linux if native == InstallPlatform::Linux => (
                IconName::Terminal,
                t_args(
                    "game.install.native",
                    &[("platform", &platform_label(platform))],
                ),
            ),
            InstallPlatform::Mac if native == InstallPlatform::Mac => (
                IconName::Monitor,
                t_args(
                    "game.install.native",
                    &[("platform", &platform_label(platform))],
                ),
            ),
            InstallPlatform::Windows if platform.needs_wine() => {
                (IconName::AppWindow, t("game.install.windows_wine"))
            }
            InstallPlatform::Windows => (IconName::AppWindow, platform_label(platform)),
            InstallPlatform::Browser => (IconName::Globe, platform_label(platform)),
            _ => (IconName::Monitor, platform_label(platform)),
        }
    }

    pub(super) fn render_platform(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.platforms.len() < 2 || self.is_modify() {
            return None;
        }
        let theme = cx.theme();
        let active_bg = theme.primary.opacity(0.14);
        let active_border = theme.primary;
        let hover_bg = theme.secondary.opacity(0.8);
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let radius = theme.radius;
        let border = theme.border;
        let seg_bg = theme.secondary.opacity(0.6);

        let buttons = self.platforms.iter().enumerate().map(|(ix, &p)| {
            let active = p == self.platform;
            let (icon, label) = Self::platform_chip_label(p);
            h_flex()
                .id(("install-platform-opt", ix))
                .role(Role::Button)
                .keyboard_focusable(cx)
                .cursor_pointer()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded(radius)
                .border_1()
                .text_xs()
                .when(active, |el| {
                    el.bg(active_bg)
                        .border_color(active_border)
                        .text_color(fg)
                        .font_semibold()
                })
                .when(!active, |el| {
                    el.border_color(gpui_kit::transparent_black())
                        .text_color(muted)
                        .hover(move |s| s.bg(hover_bg).text_color(fg))
                })
                .child(Icon::new(icon).xsmall())
                .child(label)
                .on_action(cx.listener(move |this, _: &Confirm, window, cx| {
                    this.set_platform(p, window, cx);
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_platform(p, window, cx);
                }))
        });

        Some(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Gamepad2).small().text_color(muted))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(t("game.common.platform")),
                        ),
                )
                .child(
                    h_flex()
                        .p_0p5()
                        .gap_1()
                        .rounded(radius)
                        .bg(seg_bg)
                        .border_1()
                        .border_color(border)
                        .children(buttons),
                )
                .into_any_element(),
        )
    }

    /// Which account installs a game several accounts own. Updates and
    /// launches use the same account afterwards.
    pub(super) fn render_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.owners.is_empty() {
            return None;
        }
        let selected = self
            .owners
            .iter()
            .position(|(id, _)| Some(id) == self.account.as_ref());
        let owners = self.owners.clone();
        let muted = cx.theme().muted_foreground;
        Some(
            Self::card(cx)
                .debug_selector(|| "install-account".into())
                .p_3p5()
                .gap_2p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Users).small().text_color(muted))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(t("game.info.account")),
                        ),
                )
                .child(
                    RadioGroup::horizontal("install-account")
                        .children(owners.iter().map(|(_, name)| name.clone()))
                        .selected_index(selected)
                        .on_change(cx.listener(move |this, ix: &usize, _, cx| {
                            if let Some((id, _)) = owners.get(*ix) {
                                this.account = Some(id.clone());
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("game.install.account_hint")),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_location(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.is_modify() {
            return None;
        }
        let size = self.size();
        let space = self.disk.ready().copied();
        let verdict = DiskVerdict::evaluate(space, size.disk);
        let dir = self.install_dir(cx);
        let label = drive_label(&dir);

        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let primary = theme.primary;
        let primary_text = legible(
            primary,
            &[theme.background, theme.secondary],
            TEXT_CONTRAST,
            cx,
        );
        let danger = theme.danger;
        let success = legible(theme.success, &[theme.background], TEXT_CONTRAST, cx);
        let danger_text = legible(theme.danger, &[theme.background], TEXT_CONTRAST, cx);
        let radius = theme.radius;
        let chip_active_bg = primary.opacity(0.14);
        let chip_idle_bg = theme.background.opacity(0.55);
        let chip_hover_bg = theme.secondary;

        let app = irori_ui::models(cx).settings.read(cx).app.clone();
        let chips = (self.install_targets.len() > 1).then(|| {
            h_flex().items_center().gap_1p5().flex_wrap().children(
                self.install_targets.iter().enumerate().map(|(ix, target)| {
                    let resolved = irori_core::paths::expand_home(
                        &app.install_folder_in(&target.base_path, self.id.runner),
                    );
                    let active = same_dir(&resolved, &dir);
                    let name = target.label.clone();
                    let free_text = target.free.map(|free| {
                        t_args("game.disk.chip_free", &[("free", &format_bytes(free))])
                    });
                    h_flex()
                        .id(("library-folder-chip", ix))
                        .debug_selector(move || format!("library-folder-chip-{ix}"))
                        .role(Role::Button)
                        .keyboard_focusable(cx)
                        .cursor_pointer()
                        .items_center()
                        .gap_1p5()
                        .px_2p5()
                        .py_1()
                        .rounded(radius)
                        .border_1()
                        .text_xs()
                        .when(active, |el| {
                            el.bg(chip_active_bg)
                                .border_color(primary)
                                .text_color(fg)
                                .font_semibold()
                        })
                        .when(!active, |el| {
                            el.bg(chip_idle_bg)
                                .border_color(border)
                                .text_color(muted)
                                .hover(move |s| s.bg(chip_hover_bg).text_color(fg))
                        })
                        .child(name)
                        .when_some(free_text, |el, free| {
                            el.child(
                                div()
                                    .text_color(if active { primary_text } else { muted })
                                    .child(free),
                            )
                        })
                        .on_action(cx.listener(move |this, _: &Confirm, window, cx| {
                            this.select_install_target(ix, window, cx);
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_install_target(ix, window, cx);
                        }))
                }),
            )
        });

        let bar: AnyElement = match space {
            Some(s) => {
                let (used, install) = disk_bar_fractions(s, size.disk);
                let install_color = match verdict {
                    DiskVerdict::TooSmall { .. } | DiskVerdict::NotWritable => danger,
                    _ => primary,
                };
                h_flex()
                    .id("disk-bar")
                    .w_full()
                    .h(px(7.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme.muted)
                    .child(div().h_full().w(relative(used)).bg(muted.opacity(0.45)))
                    .child(
                        div()
                            .h_full()
                            .w(relative(install))
                            .when(size.disk > 0 && s.free > 0, |el| el.min_w(px(3.)))
                            .bg(install_color),
                    )
                    .into_any_element()
            }
            None if self.disk.is_loading() => Skeleton::new()
                .w_full()
                .h(px(7.))
                .rounded_full()
                .into_any_element(),
            None => div()
                .w_full()
                .h(px(7.))
                .rounded_full()
                .bg(theme.muted)
                .into_any_element(),
        };

        let (strip_summary, strip_color, error_line): (String, Hsla, Option<String>) = match &self
            .disk
        {
            Load::Failed(e) => (String::new(), danger_text, Some(e.to_string())),
            _ => match verdict {
                DiskVerdict::Fits { .. } => (bar_summary(verdict, space, size.disk), success, None),
                DiskVerdict::TooSmall { missing } => (
                    format!("{} {}", t("game.install.short_by"), format_bytes(missing)),
                    danger_text,
                    Some(bar_summary(verdict, space, size.disk)),
                ),
                DiskVerdict::NotWritable => (
                    String::new(),
                    danger_text,
                    Some(bar_summary(verdict, space, size.disk)),
                ),
                DiskVerdict::Unknown => (bar_summary(verdict, space, size.disk), muted, None),
            },
        };

        Some(
            Self::card(cx)
                .child(
                    v_flex()
                        .p_4()
                        .gap_3()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(IconName::Folder).small().text_color(muted),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .child(t("game.install.location_heading")),
                                        ),
                                )
                                .children(chips),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .child(div().flex_1().min_w_0().child(Input::new(&self.path)))
                                .child(
                                    Button::new("browse-install-dir")
                                        .outline()
                                        .icon(IconName::FolderOpen)
                                        .label(t("game.common.browse"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.browse_install_dir(window, cx)
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .border_t_1()
                        .border_color(border)
                        .bg(theme.background.opacity(0.45))
                        .px_4()
                        .py_2p5()
                        .gap_1p5()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Icon::new(IconName::HardDrive)
                                                .xsmall()
                                                .text_color(muted),
                                        )
                                        .when(!label.is_empty(), |row| {
                                            row.child(
                                                div()
                                                    .text_xs()
                                                    .font_medium()
                                                    .text_color(muted)
                                                    .child(label),
                                            )
                                        }),
                                )
                                .child(div().flex_1().min_w(rems(4.)).child(bar))
                                .when(!strip_summary.is_empty(), |row| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(strip_color)
                                            .child(strip_summary),
                                    )
                                }),
                        )
                        .when_some(error_line, |col, msg| {
                            col.child(div().text_xs().text_color(danger_text).child(msg))
                        }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_sdl(
        &self,
        info: &InstallInfo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // Optional components are chosen at install time only.
        if info.sdl.is_empty() || self.is_modify() {
            return None;
        }
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let selected_count = info
            .sdl
            .iter()
            .filter(|o| o.mandatory || self.sdl.contains(&o.id))
            .count();
        let total_count = info.sdl.len();
        let mut rows = v_flex();
        for (ix, opt) in info.sdl.iter().enumerate() {
            let id = opt.id.clone();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .px_3p5()
                    .py_2()
                    .when(ix > 0, |r| r.border_t_1().border_color(border.opacity(0.5)))
                    .child(
                        Checkbox::new(SharedString::from(format!("sdl-{}", opt.id)))
                            .label(opt.name.clone())
                            .checked(opt.mandatory || self.sdl.contains(&opt.id))
                            .disabled(opt.mandatory)
                            .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                if *v {
                                    this.sdl.insert(id.clone());
                                } else {
                                    this.sdl.remove(&id);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        Some(
            Self::card(cx)
                .child(
                    h_flex()
                        .w_full()
                        .px_3p5()
                        .py_2p5()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(IconName::Layers).small().text_color(muted))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(t("game.install.optional_components")),
                                )
                                .child(
                                    Tag::secondary()
                                        .xsmall()
                                        .outline()
                                        .child(format!("{selected_count} / {total_count}")),
                                ),
                        ),
                )
                .child(rows)
                .into_any_element(),
        )
    }

    pub(super) fn render_dlcs(
        &self,
        info: &InstallInfo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if info.dlcs.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let all: BTreeSet<String> = info.dlcs.iter().map(|d| d.app_name.clone()).collect();
        let all_selected = all.is_subset(&self.selected_dlcs);
        let selected_count = info
            .dlcs
            .iter()
            .filter(|d| self.selected_dlcs.contains(&d.app_name))
            .count();
        let total_count = info.dlcs.len();

        let mut rows = v_flex();
        for (ix, dlc) in info.dlcs.iter().enumerate() {
            let app = dlc.app_name.clone();
            let size = self
                .dlc_sizes
                .get(&dlc.app_name)
                .map(|s| format_bytes(s.download))
                .unwrap_or_default();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .px_3p5()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .when(ix > 0, |r| r.border_t_1().border_color(border.opacity(0.5)))
                    .child(
                        div().flex_1().min_w_0().child(
                            Checkbox::new(SharedString::from(format!("dlc-{}", dlc.app_name)))
                                .label(dlc.title.clone())
                                .checked(self.selected_dlcs.contains(&dlc.app_name))
                                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                    if *v {
                                        this.selected_dlcs.insert(app.clone());
                                    } else {
                                        this.selected_dlcs.remove(&app);
                                    }
                                    cx.notify();
                                })),
                        ),
                    )
                    .when(!size.is_empty(), |r| {
                        r.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .child(size),
                        )
                    }),
            );
        }

        Some(
            Self::card(cx)
                .child(
                    h_flex()
                        .w_full()
                        .px_3p5()
                        .py_2p5()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .flex_wrap()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(IconName::Package).small().text_color(muted))
                                .child(div().text_sm().font_semibold().child(t("game.install.dlc")))
                                .child(
                                    Tag::secondary()
                                        .xsmall()
                                        .outline()
                                        .child(format!("{selected_count} / {total_count}")),
                                ),
                        )
                        .child(
                            Checkbox::new("dlc-all")
                                .label(t("game.install.dlc_all"))
                                .checked(all_selected)
                                .on_change(cx.listener(move |this, v: &bool, _, cx| {
                                    if *v {
                                        this.selected_dlcs.extend(all.iter().cloned());
                                    } else {
                                        this.selected_dlcs.clear();
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .child(rows)
                .into_any_element(),
        )
    }

    fn options_summary(&self, cx: &App) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(info) = self.info.ready() {
            if info.languages.len() > 1
                && let Some(lang) = &self.language
            {
                parts.push(language_name(lang));
            }
            let has_branches = !info.branches.is_empty();
            let builds = info
                .builds
                .iter()
                .filter(|b| b.branch == self.branch)
                .count();
            if self.package_choice() {
                if let Some(name) = self.selected_build_name(info) {
                    parts.push(name);
                }
            } else if has_branches || builds > 0 {
                if let Some(branch) = &self.branch {
                    parts.push(branch.clone());
                } else if has_branches {
                    parts.push(t("game.install.branch_default").to_string());
                }
                if self.pin_build {
                    if let Some(name) = self.selected_build_name(info) {
                        parts.push(name);
                    }
                } else if builds > 0 && !has_branches {
                    parts.push(t("game.install.version_latest").to_string());
                }
            } else if self.is_modify() && matches!(self.id.runner, Runner::Legendary | Runner::Itch)
            {
                let label = if self.pin_build {
                    t("game.install.skip_updates")
                } else {
                    t("game.install.version_latest")
                };
                parts.push(label.to_string());
            }
        }
        if !self.is_modify()
            && self.platform.needs_wine()
            && let Some(wine) = &self.wine
            && !wine.name.trim().is_empty()
        {
            parts.push(wine.name.clone());
        }
        let _ = cx;
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn selected_build_name(&self, info: &InstallInfo) -> Option<String> {
        let id = self.build.as_deref()?;
        info.builds
            .iter()
            .find(|b| b.build_id == id)
            .map(|b| b.version_name.clone())
    }

    pub(super) fn render_more_options(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.info.ready();
        let has_language = info.is_some_and(|i| i.languages.len() > 1);
        let has_branches = info.is_some_and(|i| !i.branches.is_empty());
        let builds = info.map_or(0, |i| {
            i.builds.iter().filter(|b| b.branch == self.branch).count()
        });
        let has_password =
            info.is_some() && hooks::has_branch_password(cx) && self.id.runner == Runner::Gog;
        let has_package = self.package_choice() && builds > 0;
        let has_version = !self.package_choice() && (has_branches || builds > 0 || has_password);
        let has_skip_updates = info.is_some()
            && !self.package_choice()
            && !has_version
            && self.is_modify()
            && matches!(self.id.runner, Runner::Legendary | Runner::Itch);
        let has_wine = !self.is_modify() && self.platform.needs_wine();

        if !(has_language || has_package || has_version || has_skip_updates || has_wine) {
            return None;
        }

        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let hover_bg = theme.secondary.opacity(0.75);
        let summary = self.options_summary(cx);
        let chevron = if self.options_expanded {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };

        let ring = theme.ring;
        let header = h_flex()
            .id("more-options-toggle")
            .debug_selector(|| "more-options-toggle".into())
            .role(Role::Button)
            // Inside `Self::card` (which clips with `overflow_hidden`), draw an
            // inset focus ring so keyboard focus stays visible on every edge.
            .tab_index(0)
            .focus_visible(move |style| {
                style.shadow(vec![BoxShadow {
                    color: ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: true,
                }])
            })
            .cursor_pointer()
            .w_full()
            .px_3p5()
            .py_2p5()
            .items_center()
            .justify_between()
            .gap_3()
            .hover(move |s| s.bg(hover_bg))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| {
                this.options_expanded = !this.options_expanded;
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.options_expanded = !this.options_expanded;
                cx.notify();
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        Icon::new(IconName::SlidersHorizontal)
                            .small()
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(t("game.install.more_options")),
                    ),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when_some(summary, |row, text| {
                        row.child(
                            Tag::secondary()
                                .xsmall()
                                .outline()
                                .max_w(rems(18.))
                                .child(div().min_w_0().truncate().child(text)),
                        )
                    })
                    .child(Icon::new(chevron).small().text_color(muted)),
            );

        let mut card = Self::card(cx).child(header);
        if self.options_expanded {
            let mut body = v_flex()
                .debug_selector(|| "more-options-body".into())
                .w_full()
                .px_3p5()
                .pt_3()
                .pb_3p5()
                .gap_3()
                .border_t_1()
                .border_color(border);

            // Language & Branch sit side-by-side when both are available.
            if has_language && has_version && has_branches {
                body = body.child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .child(Self::field_label(t("game.common.language"), cx))
                                .child(
                                    Select::new(&self.language_select)
                                        .placeholder(t("game.common.language")),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1p5()
                                .child(Self::field_label(t("game.install.branch"), cx))
                                .child(
                                    Select::new(&self.branch_select)
                                        .placeholder(t("game.install.branch")),
                                ),
                        ),
                );
            } else {
                if has_language {
                    body = body.child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(Self::field_label(t("game.common.language"), cx))
                            .child(
                                Select::new(&self.language_select)
                                    .placeholder(t("game.common.language")),
                            ),
                    );
                }
                if has_version && has_branches {
                    body = body.child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(Self::field_label(t("game.install.branch"), cx))
                            .child(
                                Select::new(&self.branch_select)
                                    .placeholder(t("game.install.branch")),
                            ),
                    );
                }
            }

            if has_package {
                body = body.child(
                    v_flex()
                        .w_full()
                        .gap_1p5()
                        .child(Self::field_label(t("game.install.package"), cx))
                        .child(
                            Select::new(&self.build_select)
                                .placeholder(t("game.install.package"))
                                .disabled(builds < 2),
                        ),
                );
            } else if has_version {
                if builds > 0 {
                    body = body
                        .child(
                            Checkbox::new("pin-build")
                                .label(t("game.install.pin"))
                                .checked(self.pin_build)
                                .on_change(cx.listener(|this, v: &bool, window, cx| {
                                    this.pin_build = *v;
                                    this.fetch_info(window, cx);
                                })),
                        )
                        .when(self.pin_build, |c| {
                            c.child(
                                Select::new(&self.build_select)
                                    .placeholder(t("game.common.version"))
                                    .disabled(builds < 2),
                            )
                        });
                }
                if has_password {
                    body = body.child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.password).mask_toggle()),
                            )
                            .child(
                                Button::new("apply-branch-password")
                                    .outline()
                                    .label(t("game.install.unlock"))
                                    .loading(self.password_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_password(window, cx)
                                    })),
                            ),
                    );
                }
            } else if has_skip_updates {
                body = body.child(
                    Checkbox::new("skip-updates")
                        .label(t("game.install.skip_updates"))
                        .checked(self.pin_build)
                        .on_change(cx.listener(|this, v: &bool, _, cx| {
                            this.pin_build = *v;
                            cx.notify();
                        })),
                );
            }

            if has_wine {
                let has_above = has_language || has_package || has_version || has_skip_updates;
                let empty = self.wines.ready().is_some_and(|w| w.is_empty());
                let wine_block = v_flex()
                    .w_full()
                    .gap_2()
                    .when(has_above, |b| b.pt_3().border_t_1().border_color(border))
                    .child(Self::field_label(t("game.install.compatibility_tool"), cx))
                    .map(|col| {
                        if self.wines.is_loading() {
                            col.child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(Spinner::new().small())
                                    .child(t("game.install.looking_for_wine")),
                            )
                        } else if empty || matches!(self.proton, ProtonDownload::Running(_)) {
                            col.child(self.render_proton(cx))
                        } else {
                            col.child(
                                Select::new(&self.wine_select)
                                    .placeholder(t("game.install.wine_version")),
                            )
                        }
                    })
                    .child(Self::field_label(t("game.install.wine_folder"), cx))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.prefix)))
                            .child(
                                Button::new("browse-prefix")
                                    .outline()
                                    .icon(IconName::FolderOpen)
                                    .label(t("game.common.browse"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.browse_prefix(window, cx)
                                    })),
                            ),
                    );
                body = body.child(wine_block);
            }

            card = card.child(body);
        }

        Some(card.into_any_element())
    }
}

/// Install warning for a gated anticheat, naming the products when known.
fn anticheat_warning(status: AnticheatStatus, products: &[String]) -> SharedString {
    let key = match (status, products.is_empty()) {
        (AnticheatStatus::Broken, false) => "game.install.anticheat.broken",
        (AnticheatStatus::Broken, true) => "game.install.anticheat.broken_unnamed",
        (AnticheatStatus::Denied, false) => "game.install.anticheat.denied",
        (AnticheatStatus::Denied, true) => "game.install.anticheat.denied_unnamed",
        (_, false) => "game.install.anticheat.unknown",
        (_, true) => "game.install.anticheat.unknown_unnamed",
    };
    t_args(key, &[("products", &products.join(", "))])
}
