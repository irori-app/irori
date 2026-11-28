use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, ClipboardItem, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, px, rems,
};

use super::{LoadState, LogViewer, TimeMode};
use crate::game_log::LevelFilter;
use crate::i18n::{t, t_args};
use crate::models;

impl LogViewer {
    pub(super) fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cur_lang = crate::i18n::language();
        if cur_lang != self.language {
            self.language = cur_lang;
            cx.defer_in(window, |this, window, cx| {
                let placeholder = t("ui.logs.search_placeholder");
                this.query
                    .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            });
        }
        v_flex()
            .id("unified-log-viewer")
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap_2()
            .when(self.options.show_action_bar, |v| {
                v.child(self.render_standalone_actions(cx))
            })
            .child(self.render_toolbar(cx))
            .children(self.render_banners(cx))
            .child(self.render_log_card(window, cx))
            .children(self.render_inspector(cx))
            .child(self.render_footer(cx))
            .into_any_element()
    }

    fn render_standalone_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let compact = self.options.compact;
        h_flex()
            .w_full()
            .gap_2()
            .flex_wrap()
            .items_center()
            .child(
                Button::new("lv-copy")
                    .small()
                    .outline()
                    .icon(if self.copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .label(if self.copied {
                        t("ui.logs.copied")
                    } else {
                        t("ui.logs.copy")
                    })
                    .disabled(!self.can_copy())
                    .on_click(cx.listener(|this, _, window, cx| this.copy(window, cx))),
            )
            .when(self.upload_name.is_some(), |row| {
                row.child(
                    Button::new("lv-upload")
                        .small()
                        .outline()
                        .icon(IconName::Upload)
                        .label(t("ui.logs.upload"))
                        .loading(self.uploading)
                        .disabled(!self.can_upload())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.confirm_upload(window, cx);
                        })),
                )
            })
            .when(!self.uploads.is_empty(), |row| {
                row.child(self.render_uploads_popover("lv-uploads-pop", cx))
            })
            .child(div().flex_1())
            .child(
                Button::new("lv-editor")
                    .small()
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .tooltip(t("ui.logs.open_in_editor"))
                    .disabled(!matches!(self.state, LoadState::Loaded))
                    .on_click(cx.listener(|this, _, window, cx| this.open_in_editor(window, cx))),
            )
            .child(
                ButtonGroup::new("lv-folder-group")
                    .outline()
                    .small()
                    .child(
                        Button::new("lv-reveal")
                            .icon(IconName::FileSearch)
                            .when(!compact, |b| b.label(t("ui.logs.show_file")))
                            .tooltip(t("ui.logs.show_file"))
                            .on_click(cx.listener(|this, _, _, cx| this.reveal_file(cx))),
                    )
                    .child(
                        Button::new("lv-folder")
                            .icon(IconName::FolderOpen)
                            .when(!compact, |b| b.label(t("ui.logs.logs_folder")))
                            .tooltip(t("ui.logs.logs_folder"))
                            .on_click(|_, window, cx| Self::open_logs_folder(window, cx)),
                    ),
            )
            .into_any_element()
    }

    pub fn render_uploads_popover(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let count = self.uploads.len();
        let entity = cx.entity();
        Popover::new(id)
            .anchor(Anchor::TopRight)
            .trigger(
                Button::new(ElementId::Name(format!("{id}-btn").into()))
                    .small()
                    .ghost()
                    .icon(IconName::Clock)
                    .when(count > 0, |b| b.label(count.to_string()))
                    .tooltip(t("ui.logs.recent_uploads")),
            )
            .content(move |_, _, cx| {
                let uploads = entity.read(cx).uploads.clone();
                let deleting = entity.read(cx).deleting_uploads.clone();
                let theme = cx.theme();
                let (border, muted, link) = (theme.border, theme.muted_foreground, theme.primary);
                let now = irori_core::util::unix_now();
                if uploads.is_empty() {
                    return div()
                        .w(rems(16.))
                        .p_2()
                        .text_xs()
                        .text_color(muted)
                        .child(t("ui.logs.no_recent_uploads"))
                        .into_any_element();
                }
                v_flex()
                    .w(rems(20.))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(t("ui.logs.recent_uploads")),
                    )
                    .children(uploads.into_iter().enumerate().map(|(ix, u)| {
                        let age_h = now.saturating_sub(u.uploaded_at) / 3600;
                        let age = if age_h == 0 {
                            t("ui.logs.upload_age_recent")
                        } else {
                            t_args("ui.logs.upload_age_hours", &[("hours", &age_h.to_string())])
                        };
                        let copy_url = u.url.clone();
                        let del_token = u.delete_token.clone();
                        let is_del = deleting.contains(&u.delete_token);
                        let ent = entity.clone();
                        h_flex()
                            .gap_2()
                            .py_1()
                            .items_center()
                            .when(ix > 0, |r| r.border_t_1().border_color(border))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(link)
                                            .truncate()
                                            .child(u.url.clone()),
                                    )
                                    .child(div().text_xs().text_color(muted).child(age)),
                            )
                            .child(
                                Button::new(("lv-up-copy", ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .tooltip(t("ui.logs.copy_link"))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_url.clone(),
                                        ));
                                    }),
                            )
                            .child(
                                Button::new(("lv-up-del", ix))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Trash)
                                    .loading(is_del)
                                    .tooltip(t("ui.logs.delete_upload"))
                                    .on_click(move |_, window, cx| {
                                        ent.update(cx, |this, cx| {
                                            this.forget_upload(del_token.clone(), window, cx);
                                        });
                                    }),
                            )
                    }))
                    .into_any_element()
            })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let level = self.level;
        let query_str = self.query.read(cx).value();
        let has_query = !query_str.trim().is_empty();
        let show_match_nav = has_query && !self.exclude && !self.rows.is_empty();
        let disable_logs = models(cx).settings.read(cx).app.disable_logs;
        let time_label = match self.time_mode {
            TimeMode::Compact => t("ui.logs.time_compact"),
            TimeMode::Full => t("ui.logs.time_full"),
            TimeMode::Hidden => t("ui.logs.time_hidden"),
        };
        let mod_count = self.selected_modules.len();
        let mod_label = if mod_count > 0 {
            t_args(
                "ui.logs.modules_count",
                &[("count", &mod_count.to_string())],
            )
        } else {
            t("ui.logs.modules")
        };

        let all_lbl = format!("{} {}", t("ui.logs.level_all"), self.lines.len());
        let warn_lbl = format!("{} {}", t("ui.logs.level_warnings"), self.warn_count);
        let err_lbl = format!("{} {}", t("ui.logs.level_errors"), self.err_count);

        h_flex()
            .w_full()
            .gap_1p5()
            .flex_wrap()
            .items_center()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Input::new(&self.query)
                            .id("log-search")
                            .small()
                            .aria_label(t("ui.logs.search_placeholder"))
                            .prefix(Icon::new(IconName::Search).small())
                            .cleanable(true)
                            .w(rems(13.5)),
                    )
                    .child(
                        Button::new("lv-exclude")
                            .small()
                            .outline()
                            .label("!")
                            .selected(self.exclude)
                            .tooltip(t("ui.logs.exclude_tooltip"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.exclude = !this.exclude;
                                this.selected_row = None;
                                this.refilter(cx);
                            })),
                    )
                    .when(show_match_nav, |r| {
                        let pos = (self.match_cursor % self.rows.len()) + 1;
                        r.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{pos}/{}", self.rows.len())),
                        )
                        .child(
                            Button::new("lv-match-prev")
                                .xsmall()
                                .ghost()
                                .icon(IconName::ChevronUp)
                                .tooltip(t("ui.logs.prev_match"))
                                .on_click(cx.listener(|this, _, _, cx| this.jump_match(false, cx))),
                        )
                        .child(
                            Button::new("lv-match-next")
                                .xsmall()
                                .ghost()
                                .icon(IconName::ChevronDown)
                                .tooltip(t("ui.logs.next_match"))
                                .on_click(cx.listener(|this, _, _, cx| this.jump_match(true, cx))),
                        )
                    }),
            )
            .child(
                ButtonGroup::new("log-level")
                    .outline()
                    .small()
                    .child(
                        Button::new("level-all")
                            .label(all_lbl)
                            .selected(level == LevelFilter::All),
                    )
                    .child(
                        Button::new("level-warnings")
                            .label(warn_lbl)
                            .selected(level == LevelFilter::Warnings),
                    )
                    .child(
                        Button::new("level-errors")
                            .label(err_lbl)
                            .selected(level == LevelFilter::Errors),
                    )
                    .on_click(cx.listener(|this, ix: &Vec<usize>, _, cx| {
                        let level = match ix.first() {
                            Some(1) => LevelFilter::Warnings,
                            Some(2) => LevelFilter::Errors,
                            _ => LevelFilter::All,
                        };
                        this.level = level;
                        this.error_cursor = None;
                        this.refilter(cx);
                    })),
            )
            .when(
                !self.error_rows.is_empty() && level == LevelFilter::All,
                |r| {
                    let err_btn_label = match self.error_cursor {
                        Some(c) => format!(
                            "{}/{}",
                            (c % self.error_rows.len()) + 1,
                            self.error_rows.len()
                        ),
                        None => format!("({})", self.error_rows.len()),
                    };
                    r.child(
                        ButtonGroup::new("lv-err-jump")
                            .outline()
                            .small()
                            .child(
                                Button::new("lv-err-prev")
                                    .icon(IconName::ChevronUp)
                                    .tooltip(t("ui.logs.prev_error"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.jump_error(false, cx)),
                                    ),
                            )
                            .child(
                                Button::new("lv-err-next")
                                    .icon(IconName::ChevronDown)
                                    .label(t_args("ui.logs.jump_error", &[("pos", &err_btn_label)]))
                                    .tooltip(t("ui.logs.next_error"))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.jump_error(true, cx)),
                                    ),
                            ),
                    )
                },
            )
            .when(!self.modules.is_empty(), |r| {
                r.child(self.render_modules_popover(mod_label, cx))
            })
            .child(div().flex_1())
            .child(if disable_logs {
                Button::new("log-follow")
                    .small()
                    .warning()
                    .icon(IconName::TriangleAlert)
                    .label(t("ui.logs.recording_off"))
                    .tooltip(t("ui.logs.recording_off_tooltip"))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_write_logs(true, cx)))
            } else {
                Button::new("log-follow")
                    .small()
                    .outline()
                    .selected(self.follow)
                    .icon(if self.follow {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .label(if self.follow {
                        t("ui.logs.live")
                    } else {
                        t("ui.logs.paused")
                    })
                    .tooltip(t("ui.logs.follow_tooltip"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_follow(window, cx)))
            })
            .child(
                ButtonGroup::new("lv-display-opts")
                    .outline()
                    .small()
                    .child(
                        Button::new("lv-wrap")
                            .icon(IconName::TextWrap)
                            .selected(self.wrap)
                            .tooltip(t("ui.logs.wrap_tooltip"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wrap = !this.wrap;
                                this.wrapped_sized_for = (usize::MAX, px(0.), false);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("lv-time")
                            .icon(IconName::Clock)
                            .label(time_label)
                            .selected(self.time_mode != TimeMode::Hidden)
                            .tooltip(t("ui.logs.time_tooltip"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.time_mode = this.time_mode.next();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("lv-dups")
                            .icon(IconName::Layers)
                            .selected(self.collapse_dups)
                            .tooltip(t("ui.logs.collapse_tooltip"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.collapse_dups = !this.collapse_dups;
                                this.selected_row = None;
                                this.refilter(cx);
                            })),
                    ),
            )
    }

    fn render_modules_popover(
        &self,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let selected_any = !self.selected_modules.is_empty();
        let entity = cx.entity();
        Popover::new("lv-modules-pop")
            .anchor(Anchor::TopLeft)
            .trigger(
                Button::new("lv-modules-btn")
                    .small()
                    .outline()
                    .selected(selected_any)
                    .icon(IconName::ListFilter)
                    .label(label),
            )
            .content(move |_, _, cx| {
                let mods = entity.read(cx).modules.clone();
                let active = entity.read(cx).selected_modules.clone();
                let theme = cx.theme();
                let muted = theme.muted_foreground;
                let ent_clear = entity.clone();
                v_flex()
                    .w(rems(18.))
                    .max_h(rems(20.))
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(muted)
                                    .child(t("ui.logs.modules")),
                            )
                            .when(!active.is_empty(), |r| {
                                r.child(
                                    Button::new("lv-mods-clear")
                                        .xsmall()
                                        .ghost()
                                        .label(t("ui.logs.clear"))
                                        .on_click(move |_, _, cx| {
                                            ent_clear.update(cx, |this, cx| {
                                                this.selected_modules.clear();
                                                this.refilter(cx);
                                            });
                                        }),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .id("lv-mods-list")
                            .flex_1()
                            .min_h_0()
                            .gap_0p5()
                            .overflow_y_scrollbar()
                            .children(mods.into_iter().enumerate().map(|(ix, (m, count))| {
                                let is_sel = active.contains(&m);
                                let mod_name = m.clone();
                                let ent = entity.clone();
                                Button::new(("lv-mod-item", ix))
                                    .xsmall()
                                    .ghost()
                                    .selected(is_sel)
                                    .w_full()
                                    .label(format!("{m} ({count})"))
                                    .on_click(move |_, _, cx| {
                                        ent.update(cx, |this, cx| {
                                            if !this.selected_modules.remove(&mod_name) {
                                                this.selected_modules.insert(mod_name.clone());
                                            }
                                            this.refilter(cx);
                                        });
                                    })
                            })),
                    )
                    .into_any_element()
            })
    }

    fn render_banners(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let disable_logs = models(cx).settings.read(cx).app.disable_logs;
        let mut out = Vec::new();
        if disable_logs {
            out.push(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.warning.opacity(0.4))
                    .bg(theme.warning.opacity(0.1))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .small()
                            .text_color(theme.warning),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .truncate()
                            .child(t("ui.logs.disabled_banner")),
                    )
                    .child(
                        Button::new("lv-reenable")
                            .xsmall()
                            .outline()
                            .label(t("ui.logs.reenable_action"))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.toggle_write_logs(true, cx)),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if let (Some(hint), Some(game_id)) = (&self.diagnostic_hint, &self.options.game_id)
            && !self.diagnostic_dismissed
        {
            let gid = game_id.clone();
            out.push(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.danger.opacity(0.4))
                    .bg(theme.danger.opacity(0.1))
                    .child(Icon::new(IconName::Wrench).small().text_color(theme.danger))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .truncate()
                            .child(hint.clone()),
                    )
                    .child(
                        Button::new("lv-troubleshoot")
                            .xsmall()
                            .primary()
                            .label(t("ui.logs.open_troubleshoot"))
                            .on_click(move |_, window, cx| {
                                crate::hooks::open_troubleshooter(gid.clone(), window, cx);
                            }),
                    )
                    .child(
                        Button::new("lv-diag-dismiss")
                            .xsmall()
                            .ghost()
                            .icon(IconName::X)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.diagnostic_dismissed = true;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if self.truncated {
            out.push(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.info.opacity(0.35))
                    .bg(theme.info.opacity(0.08))
                    .child(Icon::new(IconName::Info).small().text_color(theme.info))
                    .child(div().flex_1().min_w_0().text_xs().truncate().child(t_args(
                        "ui.logs.truncated_banner",
                        &[("lines", &super::MAX_LINES.to_string())],
                    )))
                    .child(
                        Button::new("lv-open-full")
                            .xsmall()
                            .outline()
                            .label(t("ui.logs.open_full"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_in_editor(window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }
}
