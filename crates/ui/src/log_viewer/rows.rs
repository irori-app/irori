use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, HighlightStyle, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Window, div, px, rems, size, uniform_list,
};

use super::{LoadState, LogViewer, TimeMode};
use crate::components::EmptyState;
use crate::game_log::{
    DisplayRow, LevelFilter, LineLevel, ParsedLine, find_highlight_ranges, format_file_size,
    module_color_bucket,
};
use crate::i18n::{t, t_args};

impl LogViewer {
    pub(super) fn render_log_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (border, radius, panel_bg, bg, muted_fg, danger) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.radius_lg,
                theme.muted,
                theme.background,
                theme.muted_foreground,
                theme.danger,
            )
        };

        let empty: Option<AnyElement> = match &self.state {
            LoadState::Idle | LoadState::Loading => Some(
                div()
                    .flex_1()
                    .p_6()
                    .text_sm()
                    .text_color(muted_fg)
                    .child(t("ui.logs.loading"))
                    .into_any_element(),
            ),
            LoadState::Missing => Some(
                EmptyState::new(IconName::ScrollText, t("ui.logs.empty_title"))
                    .description(t("ui.logs.empty_description"))
                    .action(
                        Button::new("lv-empty-folder")
                            .small()
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("ui.logs.logs_folder"))
                            .on_click(|_, window, cx| Self::open_logs_folder(window, cx)),
                    )
                    .into_any_element(),
            ),
            LoadState::Failed(err) => Some(
                div()
                    .flex_1()
                    .p_6()
                    .text_sm()
                    .text_color(danger)
                    .child(t_args("ui.logs.read_failed", &[("error", err)]))
                    .into_any_element(),
            ),
            LoadState::Loaded if self.rows.is_empty() => Some(
                EmptyState::new(IconName::Search, t("ui.logs.no_match_title"))
                    .description(t("ui.logs.no_match_description"))
                    .action(
                        Button::new("lv-clear-empty")
                            .small()
                            .outline()
                            .label(t("ui.logs.reset_filters"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_filters(window, cx);
                            })),
                    )
                    .into_any_element(),
            ),
            LoadState::Loaded => None,
        };

        let body: AnyElement = match empty {
            Some(el) => el,
            None => self.render_rows_list(window, cx),
        };

        let show_head = matches!(self.state, LoadState::Loaded) && !self.rows.is_empty();
        v_flex()
            .id("log-body")
            .flex_1()
            .min_h_0()
            .w_full()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(bg)
            .overflow_hidden()
            .when(show_head, |v| {
                let show_time = self.time_mode != TimeMode::Hidden;
                let time_w = if self.time_mode == TimeMode::Full {
                    rems(12.5)
                } else {
                    rems(5.5)
                };
                let mod_w = if self.options.compact {
                    rems(7.5)
                } else {
                    rems(10.)
                };
                v.child(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .bg(panel_bg)
                        .border_b_1()
                        .border_color(border)
                        .text_xs()
                        .font_semibold()
                        .text_color(muted_fg)
                        .when(!self.options.compact, |r| {
                            r.child(div().w(rems(2.5)).flex_shrink_0().text_right().child("#"))
                        })
                        .when(show_time, |r| {
                            r.child(div().w(time_w).flex_shrink_0().child(t("ui.logs.col_time")))
                        })
                        .child(
                            div()
                                .w(rems(2.8))
                                .flex_shrink_0()
                                .child(t("ui.logs.col_level")),
                        )
                        .child(
                            div()
                                .w(mod_w)
                                .flex_shrink_0()
                                .child(t("ui.logs.col_module")),
                        )
                        .child(div().flex_1().min_w_0().child(t("ui.logs.col_message"))),
                )
            })
            .child(body)
            .into_any_element()
    }

    fn render_rows_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.wrap {
            let line_h = window.rem_size() * 1.3;
            let key = (self.rows.len(), line_h, self.options.compact);
            if self.wrapped_sized_for != key {
                let chars_per_line: usize = if self.options.compact { 54 } else { 92 };
                let sizes: Vec<gpui_kit::Size<gpui_kit::Pixels>> = self
                    .rows
                    .iter()
                    .map(|r| {
                        let len = self
                            .lines
                            .get(r.entry_idx)
                            .map(|l| l.message.len())
                            .unwrap_or(0);
                        let wrapped_lines = (len / chars_per_line + 1).clamp(1, 8) as f32;
                        size(px(0.), line_h * wrapped_lines + px(4.))
                    })
                    .collect();
                self.wrapped_sizes = Rc::new(sizes);
                self.wrapped_sized_for = key;
            }
            let view = cx.entity();
            return v_virtual_list(
                view,
                "log-lines-wrapped",
                self.wrapped_sizes.clone(),
                |this, range, _, cx| this.render_row_range(range, cx),
            )
            .track_scroll(&self.scroll_wrapped)
            .flex_1()
            .min_h_0()
            .w_full()
            .py_1()
            .into_any_element();
        }

        let entity = cx.entity();
        uniform_list("log-lines", self.rows.len(), move |range, _, cx| {
            entity.update(cx, |this, cx| this.render_row_range(range, cx))
        })
        .track_scroll(&self.scroll_uniform)
        .flex_1()
        .min_h_0()
        .w_full()
        .py_1()
        .into_any_element()
    }

    fn render_row_range(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let palette = RowPalette {
            mono: theme.mono_font_family.clone(),
            fg: theme.foreground,
            muted: theme.muted_foreground,
            info: theme.info,
            warning: theme.warning,
            danger: theme.danger,
            selected_bg: theme.list_active,
            hover_bg: theme.list_hover,
            surface: theme.secondary,
            mod_colors: [
                theme.info,
                theme.cyan,
                theme.magenta,
                theme.success,
                theme.warning,
                theme.primary,
            ],
            highlight_bg: theme.warning.opacity(0.32),
        };
        let query = if self.exclude {
            String::new()
        } else {
            self.query.read(cx).value().to_string()
        };
        range
            .filter_map(|row_idx| {
                let row = *self.rows.get(row_idx)?;
                let line = self.lines.get(row.entry_idx)?;
                Some(self.render_single_row(row_idx, row, line, &query, &palette, cx))
            })
            .collect()
    }

    fn render_single_row(
        &self,
        row_idx: usize,
        row: DisplayRow,
        line: &ParsedLine,
        query: &str,
        p: &RowPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected = self.selected_row == Some(row_idx);
        let (lvl_label, lvl_color, row_bg, rail_color) = match line.level {
            LineLevel::Error => (
                "ERR",
                p.danger,
                Some(p.danger.opacity(0.085)),
                Some(p.danger),
            ),
            LineLevel::Warning => (
                "WARN",
                p.warning,
                Some(p.warning.opacity(0.06)),
                Some(p.warning),
            ),
            LineLevel::Info => ("INFO", p.info, None, None),
        };
        let bg = if is_selected {
            Some(p.selected_bg)
        } else {
            row_bg
        };
        let show_time = self.time_mode != TimeMode::Hidden;
        let time_w = if self.time_mode == TimeMode::Full {
            rems(12.5)
        } else {
            rems(5.5)
        };
        let time_text: SharedString = match self.time_mode {
            TimeMode::Full => line.ts_full.clone().unwrap_or_else(|| "-".into()),
            TimeMode::Compact => line.ts_short.clone().unwrap_or_else(|| "-".into()),
            TimeMode::Hidden => SharedString::default(),
        };
        let mod_w = if self.options.compact {
            rems(7.5)
        } else {
            rems(10.)
        };
        let mod_color = p.mod_colors[module_color_bucket(&line.module)];
        let msg_text = self.maybe_redact(&line.message);
        let msg_el: AnyElement = if !query.trim().is_empty() {
            let ranges = find_highlight_ranges(&msg_text, query);
            if ranges.is_empty() {
                SharedString::from(msg_text).into_any_element()
            } else {
                let style = HighlightStyle {
                    background_color: Some(p.highlight_bg),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                };
                StyledText::new(msg_text)
                    .with_highlights(ranges.into_iter().map(|r| (r, style)))
                    .into_any_element()
            }
        } else {
            SharedString::from(msg_text).into_any_element()
        };

        let mod_for_click = line.module.clone();
        h_flex()
            .id(("log-row", row_idx))
            .w_full()
            .px_3()
            .py_0p5()
            .gap_2()
            .items_center()
            .border_l_2()
            .border_color(rail_color.unwrap_or(Hsla::transparent_black()))
            .when_some(bg, |d, c| d.bg(c))
            .hover(move |s| s.bg(p.hover_bg))
            .cursor_pointer()
            .font_family(p.mono.clone())
            .text_xs()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_row = if this.selected_row == Some(row_idx) {
                    None
                } else {
                    Some(row_idx)
                };
                cx.notify();
            }))
            .when(!self.options.compact, |r| {
                r.child(
                    div()
                        .w(rems(2.5))
                        .flex_shrink_0()
                        .text_right()
                        .text_color(p.muted.opacity(0.65))
                        .child((line.index + 1).to_string()),
                )
            })
            .when(show_time, |r| {
                r.child(
                    div()
                        .w(time_w)
                        .flex_shrink_0()
                        .truncate()
                        .text_color(p.muted)
                        .child(time_text),
                )
            })
            .child(
                div()
                    .w(rems(2.8))
                    .flex_shrink_0()
                    .px_1()
                    .rounded(px(4.))
                    .bg(lvl_color.opacity(0.14))
                    .text_color(lvl_color)
                    .font_semibold()
                    .text_center()
                    .child(lvl_label),
            )
            .child(
                div()
                    .id(("log-mod", row_idx))
                    .w(mod_w)
                    .flex_shrink_0()
                    .px_1()
                    .rounded(px(4.))
                    .bg(p.surface.opacity(0.65))
                    .text_color(mod_color)
                    .font_semibold()
                    .truncate()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if this.selected_modules.len() == 1
                            && this.selected_modules.contains(&mod_for_click)
                        {
                            this.selected_modules.clear();
                        } else {
                            this.selected_modules.clear();
                            this.selected_modules.insert(mod_for_click.clone());
                        }
                        this.selected_row = None;
                        this.refilter(cx);
                    }))
                    .child(line.module_short.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(match line.level {
                        LineLevel::Error => p.danger,
                        LineLevel::Warning => p.warning,
                        LineLevel::Info => p.fg,
                    })
                    .when(!self.wrap, |d| d.whitespace_nowrap().truncate())
                    .child(msg_el),
            )
            .when(row.count > 1, |r| {
                r.child(
                    div()
                        .flex_shrink_0()
                        .px_1p5()
                        .rounded(px(99.))
                        .bg(p.surface)
                        .text_color(p.muted)
                        .font_semibold()
                        .child(format!("×{}", row.count)),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let row_idx = self.selected_row?;
        let row = *self.rows.get(row_idx)?;
        let line = self.lines.get(row.entry_idx)?;
        let theme = cx.theme();
        let start = row.entry_idx.saturating_sub(2);
        let end = (row.entry_idx + 2).min(self.lines.len().saturating_sub(1));
        let mono = theme.mono_font_family.clone();
        let mod_name = line.module.clone();

        Some(
            v_flex()
                .id("log-inspector")
                .w_full()
                .p_2p5()
                .gap_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .child(div().text_xs().font_semibold().child(t_args(
                            "ui.logs.inspector_line",
                            &[("line", &(line.index + 1).to_string())],
                        )))
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(theme.secondary)
                                .text_xs()
                                .font_family(mono.clone())
                                .child(line.module.clone()),
                        )
                        .when_some(line.ts_full.clone(), |r, ts| {
                            r.child(
                                div()
                                    .text_xs()
                                    .font_family(mono.clone())
                                    .text_color(theme.muted_foreground)
                                    .child(ts),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            Button::new("lv-insp-copy-line")
                                .xsmall()
                                .outline()
                                .icon(IconName::Copy)
                                .label(t("ui.logs.copy_line"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.copy_line(row_idx, window, cx);
                                })),
                        )
                        .child(
                            Button::new("lv-insp-copy-snippet")
                                .xsmall()
                                .outline()
                                .icon(IconName::Copy)
                                .label(t("ui.logs.copy_snippet"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.copy_context_snippet(row_idx, window, cx);
                                })),
                        )
                        .child(
                            Button::new("lv-insp-solo")
                                .xsmall()
                                .outline()
                                .label(t("ui.logs.filter_module"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected_modules.clear();
                                    this.selected_modules.insert(mod_name.clone());
                                    this.refilter(cx);
                                })),
                        )
                        .child(
                            Button::new("lv-insp-close")
                                .xsmall()
                                .ghost()
                                .icon(IconName::X)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected_row = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .p_2()
                        .gap_0p5()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .font_family(mono)
                        .text_xs()
                        .children(self.lines[start..=end].iter().map(|ctx_line| {
                            let focus = ctx_line.index == line.index;
                            div()
                                .w_full()
                                .px_1()
                                .rounded(px(3.))
                                .when(focus, |d| d.bg(theme.list_active).font_semibold())
                                .text_color(if focus {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground
                                })
                                .child(format!(
                                    "{}: {}",
                                    ctx_line.index + 1,
                                    self.maybe_redact(&ctx_line.raw)
                                ))
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let has_filters = !self.query.read(cx).value().trim().is_empty()
            || self.level != LevelFilter::All
            || !self.selected_modules.is_empty();
        let path_display = self.maybe_redact(&self.path.display().to_string());
        let size_text = format_file_size(self.file_size);
        let counts_text = if has_filters {
            t_args(
                "ui.logs.footer_filtered",
                &[
                    ("shown", &self.rows.len().to_string()),
                    ("total", &self.lines.len().to_string()),
                ],
            )
        } else if self.collapse_dups && self.rows.len() < self.lines.len() {
            t_args(
                "ui.logs.footer_collapsed",
                &[
                    ("shown", &self.rows.len().to_string()),
                    ("raw", &self.lines.len().to_string()),
                ],
            )
        } else {
            t_args(
                "ui.logs.footer_total",
                &[("count", &self.rows.len().to_string())],
            )
        };

        h_flex()
            .w_full()
            .gap_2()
            .flex_wrap()
            .items_center()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .id("lv-footer-path")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .font_family(theme.mono_font_family.clone())
                    .on_click(cx.listener(|this, _, _, cx| this.reveal_file(cx)))
                    .child(path_display),
            )
            .child(div().flex_shrink_0().child(size_text))
            .child(div().flex_shrink_0().child(counts_text))
            .when(has_filters, |r| {
                r.child(
                    Button::new("lv-footer-reset")
                        .xsmall()
                        .ghost()
                        .label(t("ui.logs.reset_filters"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.reset_filters(window, cx);
                        })),
                )
            })
            .child(
                Button::new("lv-privacy-toggle")
                    .xsmall()
                    .outline()
                    .selected(self.redact_home)
                    .icon(if self.redact_home {
                        IconName::ShieldCheck
                    } else {
                        IconName::ShieldOff
                    })
                    .label(if self.redact_home {
                        t("ui.logs.privacy_on")
                    } else {
                        t("ui.logs.privacy_off")
                    })
                    .tooltip(t("ui.logs.privacy_tooltip"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.redact_home = !this.redact_home;
                        cx.notify();
                    })),
            )
    }
}

struct RowPalette {
    mono: SharedString,
    fg: Hsla,
    muted: Hsla,
    info: Hsla,
    warning: Hsla,
    danger: Hsla,
    selected_bg: Hsla,
    hover_bg: Hsla,
    surface: Hsla,
    mod_colors: [Hsla; 6],
    highlight_bg: Hsla,
}
