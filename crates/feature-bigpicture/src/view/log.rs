//! The game page's log panel: the tail of the game's log beside its actions,
//! in place of the details, re-read while it shows so a session's output
//! appears as the game writes it. It follows the newest line unless it was
//! scrolled up from it (the wheel, or the shoulders a page at a time).
//!
//! The page's rows under Show log filter it: errors and warnings only, and
//! a text the lines must contain, typed in a field that sits outside the
//! `BigPicture` key context like the Library's search.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Role, ScrollStrategy, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, point, px, rems,
    size,
};
use irori_core::GameId;
use irori_ui::game_log::{self, LineLevel, LogStamp, LogTail, visible_lines};
use irori_ui::i18n::{t, t_args};

use super::{BigPictureView, Colors};
use crate::layout::{CHIPS, Metrics, page_offset};

/// Lines kept: a screenful many times over, little next to a full log.
const MAX_LINES: usize = 2000;
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// Line height, in rem.
const LINE_H: f32 = 1.35;

enum LogState {
    Reading,
    /// No log yet: the game never ran.
    Missing,
    Failed(SharedString),
    Ready(Arc<LogTail>),
}

/// What the log panel's lines are filtered by and what they were derived
/// from: the tail read (by address), errors only, the trimmed text.
type VisibleKey = (usize, bool, String);

/// The log panel of one game.
pub(super) struct LogPanel {
    id: GameId,
    state: LogState,
    /// Indices of the tail's lines that pass the filters, derived when the
    /// tail or the filters change.
    visible: Rc<Vec<usize>>,
    visible_key: Option<VisibleKey>,
    /// Row sizes for the shown lines, kept while their count and the rem
    /// size stay.
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    /// Scroll to the newest shown line at the next draw, which knows how
    /// many lines show.
    to_end: bool,
    _refresh: Task<()>,
}

/// The log's filters, kept for the session like Show log.
pub(super) struct LogFilter {
    pub(super) errors_only: bool,
    pub(super) input: Entity<InputState>,
    /// The text field has the keyboard.
    pub(super) editing: bool,
}

impl LogFilter {
    /// The filters and the subscription that follows the text field.
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<BigPictureView>,
    ) -> (Self, Subscription) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("bigpicture.log.filter_placeholder"))
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.close_log_filter(window, cx)
                }
                _ => {}
            },
        );
        let filter = Self {
            errors_only: false,
            input,
            editing: false,
        };
        (filter, subscription)
    }

    /// The text the lines must contain, trimmed; empty for every line.
    pub(super) fn query(&self, cx: &gpui_kit::App) -> String {
        self.input.read(cx).value().trim().to_string()
    }
}

impl BigPictureView {
    /// Keep a log panel for the game `id`, or none.
    pub(super) fn sync_log(&mut self, id: Option<&GameId>, cx: &mut Context<Self>) {
        match id {
            None => self.log = None,
            Some(id) if self.log.as_ref().is_some_and(|l| &l.id == id) => {}
            Some(id) => {
                let path = irori_ui::backend(cx).game_log(id);
                self.log = Some(LogPanel {
                    id: id.clone(),
                    state: LogState::Reading,
                    visible: Rc::default(),
                    visible_key: None,
                    sizes: Rc::default(),
                    scroll: VirtualListScrollHandle::new(),
                    to_end: false,
                    _refresh: Self::refresh_log(path, cx),
                });
            }
        }
    }

    /// Re-read the log every [`REFRESH_EVERY`] while the panel lives
    /// (dropping it stops this), when its size or time changed.
    fn refresh_log(path: PathBuf, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            // The stamp last read; a log still missing reads as unchanged too.
            let mut last: Option<Option<LogStamp>> = None;
            loop {
                let path = path.clone();
                let Ok(read) = this.update(cx, |_, cx| {
                    irori_ui::run(cx, async move {
                        let stamp = game_log::stamp(&path).await;
                        if last == Some(stamp) {
                            return Ok((stamp, None));
                        }
                        let tail = game_log::read_tail(&path, MAX_LINES).await;
                        Ok((stamp, Some(tail)))
                    })
                }) else {
                    break;
                };
                if let Ok((stamp, Some(tail))) = read.await {
                    last = Some(stamp);
                    let alive = this.update(cx, |this, cx| {
                        if let Some(log) = &mut this.log {
                            log.show(tail);
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        })
    }

    /// Scroll the log a page toward its end (`down`) or its start.
    pub(super) fn scroll_log_page(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(log) = &self.log else {
            return;
        };
        let scroll = &log.scroll;
        let offset = scroll.offset();
        let y = page_offset(
            f32::from(offset.y),
            f32::from(scroll.bounds().size.height),
            f32::from(scroll.max_offset().y),
            down,
        );
        scroll.set_offset(point(offset.x, px(y)));
        cx.notify();
    }

    /// Give the keyboard to the log's filter field.
    pub(super) fn open_log_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log_filter.editing = true;
        self.log_filter
            .input
            .update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Leave the filter field for the page, keeping its text.
    pub(super) fn close_log_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.log_filter.editing {
            return;
        }
        self.log_filter.editing = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_log_filter(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id("bp-log-filter")
            .key_context(crate::SEARCH_CONTEXT)
            .on_action(cx.listener(|this, _: &crate::CloseSearch, window, cx| {
                this.close_log_filter(window, cx)
            }))
            .absolute()
            .left_0()
            .right_0()
            .top(rems(m.top_bar))
            .h(rems(CHIPS))
            .px(rems(m.gutter))
            .items_center()
            .bg(c.background.opacity(0.9))
            .child(
                div()
                    .w_full()
                    .max_w(rems(40.))
                    .text_xl()
                    .child(Input::new(&self.log_filter.input).large().cleanable(true)),
            )
            .into_any_element()
    }

    pub(super) fn render_log(
        &mut self,
        c: Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let errors_only = self.log_filter.errors_only;
        let query = self.log_filter.query(cx);
        let Some(log) = &mut self.log else {
            return div().into_any_element();
        };
        let note = |text: SharedString, color| {
            div()
                .p_2()
                .text_base()
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let filtering = errors_only || !query.is_empty();
        // "12 of 340 lines" while a filter hides some.
        let mut count: Option<SharedString> = None;
        let body = match &log.state {
            LogState::Reading => note(t("bigpicture.log.reading"), c.muted_foreground),
            LogState::Missing => note(t("bigpicture.log.empty"), c.muted_foreground),
            LogState::Failed(e) => note(
                t_args("bigpicture.log.read_failed", &[("error", e.as_ref())]),
                c.danger,
            ),
            LogState::Ready(tail) if tail.lines.is_empty() => {
                note(t("bigpicture.log.empty"), c.muted_foreground)
            }
            LogState::Ready(tail) => {
                let key = (Arc::as_ptr(tail) as usize, errors_only, query);
                if log.visible_key.as_ref() != Some(&key) {
                    // Other lines show after a filter changed: start from the
                    // newest again.
                    log.to_end |= log
                        .visible_key
                        .as_ref()
                        .is_some_and(|old| (old.1, &old.2) != (key.1, &key.2));
                    log.visible = Rc::new(visible_lines(&tail.lines, key.1, &key.2, false));
                    log.visible_key = Some(key);
                }
                if log.to_end && !log.visible.is_empty() {
                    log.to_end = false;
                    log.scroll
                        .scroll_to_item(log.visible.len() - 1, ScrollStrategy::Top);
                }
                if filtering {
                    count = Some(t_args(
                        "bigpicture.log.filtered",
                        &[
                            ("shown", &log.visible.len().to_string()),
                            ("total", &tail.lines.len().to_string()),
                        ],
                    ));
                }
                if log.visible.is_empty() {
                    note(t("bigpicture.log.no_match"), c.muted_foreground)
                } else {
                    let line_h = window.rem_size() * LINE_H;
                    if log.sizes.len() != log.visible.len()
                        || log.sizes.first().is_some_and(|s| s.height != line_h)
                    {
                        log.sizes = Rc::new(vec![size(px(0.), line_h); log.visible.len()]);
                    }
                    let sizes = log.sizes.clone();
                    let tail = tail.clone();
                    let visible = log.visible.clone();
                    v_virtual_list(cx.entity(), "bp-log-lines", sizes, move |_, range, _, _| {
                        range
                            .filter_map(|ix| {
                                let (level, text) = tail.lines.get(*visible.get(ix)?)?;
                                let color = match level {
                                    LineLevel::Error => c.danger,
                                    LineLevel::Warning => c.yellow,
                                    LineLevel::Info => c.foreground.opacity(0.85),
                                };
                                Some(
                                    div()
                                        .id(ix)
                                        .px_2()
                                        .w_full()
                                        .whitespace_nowrap()
                                        .truncate()
                                        .text_color(color)
                                        .child(text.clone())
                                        .into_any_element(),
                                )
                            })
                            .collect()
                    })
                    .track_scroll(&log.scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .text_sm()
                    .font_family(cx.theme().mono_font_family.clone())
                    .into_any_element()
                }
            }
        };
        v_flex()
            .id("bp-page-log")
            .role(Role::Log)
            .aria_label(t("bigpicture.log.title"))
            .flex_1()
            .min_w_0()
            .h_full()
            .p_4()
            .gap_2()
            .rounded(c.radius_lg)
            .bg(c.background.opacity(0.75))
            .border_1()
            .border_color(c.foreground.opacity(0.08))
            .child(
                h_flex()
                    .px_2()
                    .gap_4()
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .text_color(c.foreground.opacity(0.6))
                            .child(t("bigpicture.log.title")),
                    )
                    .when_some(count, |d, count| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_color(c.muted_foreground)
                                .child(count),
                        )
                    }),
            )
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
impl BigPictureView {
    /// Lines the log panel shows, once it has read the log.
    pub(super) fn log_lines(&self) -> Option<usize> {
        let log = self.log.as_ref()?;
        match &log.state {
            LogState::Ready(_) if log.visible_key.is_some() => Some(log.visible.len()),
            LogState::Ready(tail) => Some(tail.lines.len()),
            _ => None,
        }
    }

    /// The log's scroll offset, from its top.
    pub(super) fn log_offset(&self) -> Option<f32> {
        Some(-f32::from(self.log.as_ref()?.scroll.offset().y))
    }
}

impl LogPanel {
    fn show(&mut self, tail: std::io::Result<Option<LogTail>>) {
        // Follow the newest line unless the list was scrolled up from it.
        let max = self.scroll.max_offset().y;
        let follow =
            !matches!(self.state, LogState::Ready(_)) || -self.scroll.offset().y >= max - px(2.);
        self.state = match tail {
            Ok(Some(tail)) => LogState::Ready(Arc::new(tail)),
            Ok(None) => LogState::Missing,
            Err(e) => LogState::Failed(e.to_string().into()),
        };
        // The list may not have been drawn yet (it has no length then), so
        // the next draw scrolls.
        self.to_end |= follow;
    }
}
