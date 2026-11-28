//! Unified Settings -> About & System view: identity and updates, scannable
//! hardware/display/tool diagnostics, help and feedback, and collapsible
//! third-party software notices.

mod sysinfo_cards;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, Div, ElementId, Entity,
    Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
};
use irori_runtime::sysinfo::{self, SystemInfo};
use irori_services::displays::DisplayReport;
use irori_ui::i18n::{t, t_args};

use crate::app_update::AppUpdatePanel;
use crate::feedback::{self, Feedback};
use crate::widgets::{muted, panel};

const CREDITS: [(&str, bool, &str); 6] = [
    ("settings.about.credit_gpui", true, "Apache-2.0"),
    ("settings.about.credit_lucide", true, "ISC"),
    ("settings.about.credit_themes", true, "Apache-2.0"),
    ("legendary, gogdl, nile", false, "GPL-3.0"),
    ("comet", false, "Apache-2.0"),
    ("umu-launcher", false, "GPL-3.0"),
];

/// Unified About & System panel rendered by `SettingsSection::About`.
pub(crate) struct AboutSystemPanel {
    app_update: Entity<AppUpdatePanel>,
    pub(super) info: Option<Arc<SystemInfo>>,
    pub(super) displays: Option<Arc<DisplayReport>>,
    pub(super) loading: bool,
    pub(super) copied_sysinfo: bool,
    copy_reset: Option<Task<()>>,
    pub(super) pci_expanded: bool,
    pub(super) modes_expanded: HashSet<usize>,
    credits_expanded: bool,
    reporting: bool,
}

impl AboutSystemPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            app_update: cx.new(AppUpdatePanel::new),
            info: None,
            displays: None,
            loading: false,
            copied_sysinfo: false,
            copy_reset: None,
            pci_expanded: false,
            modes_expanded: HashSet::new(),
            credits_expanded: false,
            reporting: false,
        }
    }

    /// Load system and display facts the first time the section opens.
    pub(crate) fn ensure_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.info.is_none() && !self.loading {
            self.load_sysinfo(window, cx);
        }
    }

    pub(super) fn load_sysinfo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let b = irori_ui::backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(tokio::join!(sysinfo::get(true), b.displays(true)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (info, displays) = task.await.ok().unzip();
            let _ = this.update(cx, |this, cx| {
                (this.loading, this.info, this.displays) = (false, info, displays);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn copy_sysinfo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = &self.info else { return };
        let mut text = sysinfo::format(info);
        if let Some(displays) = &self.displays {
            text.push('\n');
            text.push_str(&irori_services::displays::format(displays));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied_sysinfo = true;
        cx.notify();
        self.copy_reset = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                (this.copied_sysinfo, this.copy_reset) = (false, None);
                cx.notify();
            });
        }));
    }

    fn open_feedback(&mut self, way: Feedback, window: &mut Window, cx: &mut Context<Self>) {
        if way == Feedback::Report {
            if self.reporting {
                return;
            }
            self.reporting = true;
            cx.notify();
        }
        let task = feedback::open(way, window, cx);
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                this.reporting = false;
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn show_sysinfo_for_test(
        &mut self,
        info: Arc<SystemInfo>,
        displays: Arc<DisplayReport>,
        cx: &mut Context<Self>,
    ) {
        (self.info, self.displays, self.loading) = (Some(info), Some(displays), false);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn expand_disclosures_for_test(&mut self, cx: &mut Context<Self>) {
        (self.pci_expanded, self.credits_expanded) = (true, true);
        self.modes_expanded.insert(0);
        cx.notify();
    }

    fn render_feedback_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let logs = action_btn(
            "about-open-logs",
            IconName::ScrollText,
            "settings.about.view_logs",
        )
        .ghost()
        .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Settings("logs".into()), cx));
        let report = action_btn(
            "feedback-settings.about.feedback.report",
            IconName::ExternalLink,
            "settings.about.feedback.report_open",
        )
        .primary()
        .loading(self.reporting)
        .on_click(cx.listener(|this, _, w, cx| this.open_feedback(Feedback::Report, w, cx)));
        let actions = h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(logs)
            .child(report);
        let report_row = feedback_item(
            Feedback::Report.icon(),
            true,
            Feedback::Report.label_key(),
            Feedback::Report.help_key(),
            actions.into_any_element(),
            cx,
        )
        .flex_wrap()
        .py_3p5();

        let tour_btn = Button::new("about-tour")
            .outline()
            .xsmall()
            .label(t("settings.about.tour_start"))
            .on_click(|_, w, cx| irori_ui::tour::start(w, cx))
            .into_any_element();
        let mut tiles = vec![feedback_item(
            IconName::Compass,
            false,
            "settings.about.tour",
            "settings.about.tour_help",
            tour_btn,
            cx,
        )];
        for way in [Feedback::Idea, Feedback::Question, Feedback::KnownIssues] {
            let id = ElementId::Name(format!("feedback-{}", way.label_key()).into());
            let btn = Button::new(id)
                .ghost()
                .xsmall()
                .icon(IconName::ExternalLink)
                .label(t("settings.about.feedback.open"))
                .on_click(cx.listener(move |this, _, w, cx| this.open_feedback(way, w, cx)))
                .into_any_element();
            tiles.push(feedback_item(
                way.icon(),
                false,
                way.label_key(),
                way.help_key(),
                btn,
                cx,
            ));
        }

        let bdr = cx.theme().border;
        let grid = two_col_grid().border_t_1().border_color(bdr).children(
            tiles
                .into_iter()
                .enumerate()
                .map(|(i, d)| grid_border(d, i, bdr)),
        );
        v_flex()
            .gap_2()
            .pt_2()
            .child(irori_ui::style::caption(
                t("settings.about.feedback.title"),
                cx,
            ))
            .child(panel(cx).overflow_hidden().child(report_row).child(grid))
            .into_any_element()
    }

    fn render_credits_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (bdr, fg, open) = (
            cx.theme().border,
            cx.theme().muted_foreground,
            self.credits_expanded,
        );
        let count_tag = t_args("settings.about.credits_count_other", &[("count", "6")]);
        let chev = if open {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };
        let left = h_flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(Icon::new(IconName::Layers).small().text_color(fg))
            .child(trunc_sm(t("settings.about.third_party")).font_medium())
            .child(muted_tag(count_tag));
        let right = h_flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(xs_muted("GPL-3.0-or-later", cx))
            .child(Icon::new(chev).small().text_color(fg));
        let header = Button::new("about-credits-toggle")
            .ghost()
            .w_full()
            .h_10()
            .px_4()
            .accessibility_label(t("settings.about.third_party"))
            .child(
                row_between()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(left)
                    .child(right),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.credits_expanded = !this.credits_expanded;
                cx.notify();
            }));

        let mut box_ = Collapsible::new().w_full().open(open).child(header);
        if open {
            let rows = CREDITS
                .into_iter()
                .enumerate()
                .map(|(i, (lbl, is_key, lic))| {
                    let name: SharedString = if is_key { t(lbl) } else { lbl.into() };
                    let cell = row_between()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .min_w_0()
                        .child(trunc_sm(name).flex_1())
                        .child(mono_xs(lic, cx).flex_shrink_0());
                    grid_border(cell, i, bdr)
                });
            box_ = box_.content(irori_ui::motion::fade_in(
                "about-credits-content",
                two_col_grid().border_t_1().border_color(bdr).children(rows),
            ));
        }
        panel(cx)
            .overflow_hidden()
            .mt_1()
            .child(box_)
            .into_any_element()
    }
}

impl Render for AboutSystemPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(self.app_update.clone())
            .child(self.render_system_section(cx))
            .child(self.render_feedback_section(cx))
            .child(self.render_credits_section(cx))
    }
}

pub(super) fn action_btn(id: impl Into<ElementId>, icon: IconName, key: &str) -> Button {
    Button::new(id).small().icon(icon).label(t(key))
}
pub(super) fn muted_tag(label: impl Into<SharedString>) -> Tag {
    Tag::secondary().xsmall().outline().child(label.into())
}
pub(super) fn info_tag(label: impl Into<SharedString>) -> Tag {
    Tag::info().xsmall().outline().child(label.into())
}
pub(super) fn xs_muted(text: impl IntoElement, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
pub(super) fn mono_xs(text: impl IntoElement, cx: &App) -> Div {
    let th = cx.theme();
    div()
        .text_xs()
        .font_family(th.mono_font_family.clone())
        .text_color(th.muted_foreground)
        .truncate()
        .child(text)
}
pub(super) fn trunc_sm(text: impl IntoElement) -> Div {
    div().min_w_0().text_sm().truncate().child(text)
}
pub(super) fn row_between() -> Div {
    h_flex().items_center().justify_between().gap_2()
}
pub(super) fn split_row() -> Div {
    row_between().flex_wrap()
}
pub(super) fn two_col_grid() -> Div {
    div().grid().grid_cols(2).w_full()
}

pub(super) fn disclosure_toggle(
    id: impl Into<ElementId>,
    open: bool,
    label: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let icon = if open {
        IconName::ChevronUp
    } else {
        IconName::ChevronDown
    };
    h_flex().child(
        Button::new(id)
            .ghost()
            .xsmall()
            .compact()
            .icon(icon)
            .label(label)
            .on_click(on_click),
    )
}

fn grid_border(cell: Div, ix: usize, bdr: Hsla) -> Div {
    cell.when(ix % 2 == 1, |d| d.border_l_1().border_color(bdr))
        .when(ix >= 2, |d| d.border_t_1().border_color(bdr))
}

fn feedback_item(
    icon: IconName,
    featured: bool,
    title_key: &str,
    help_key: &str,
    trailing: AnyElement,
    cx: &App,
) -> Div {
    let th = cx.theme();
    let (fg, bg, bdr) = if featured {
        (
            th.primary,
            th.primary.opacity(0.12),
            th.primary.opacity(0.35),
        )
    } else {
        (th.muted_foreground, th.muted, th.border)
    };
    let badge = h_flex()
        .flex_none()
        .size_8()
        .rounded(th.radius)
        .bg(bg)
        .border_1()
        .border_color(bdr)
        .items_center()
        .justify_center()
        .child(Icon::new(icon).small().text_color(fg));
    let (title, help) = if featured {
        (
            div().text_sm().font_semibold().child(t(title_key)),
            muted(t(help_key), cx),
        )
    } else {
        (
            trunc_sm(t(title_key)).font_medium(),
            xs_muted(t(help_key), cx),
        )
    };
    let copy = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(title)
        .child(help);
    let lead = h_flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap_3()
        .child(badge)
        .child(copy);
    row_between()
        .min_w_0()
        .gap_3()
        .px_4()
        .py_3()
        .child(lead)
        .child(div().flex_shrink_0().child(trailing))
}
