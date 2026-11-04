//! The game page's log panel: the tail of the game's log beside its actions,
//! in place of the details, re-read while it shows so a session's output
//! appears as the game writes it. It follows the newest line unless the
//! wheel moved it up.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, StyledExt as _, VirtualListScrollHandle, v_flex, v_virtual_list,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Role,
    SharedString, Size, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px, size,
};
use irori_core::GameId;
use irori_ui::game_log::{self, LineLevel, LogStamp, LogTail};
use irori_ui::i18n::{t, t_args};

use super::{BigPictureView, Colors};

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

/// The log panel of one game.
pub(super) struct LogPanel {
    id: GameId,
    state: LogState,
    /// Row sizes for the lines, kept while their count and the rem size stay.
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    _refresh: Task<()>,
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
                    sizes: Rc::default(),
                    scroll: VirtualListScrollHandle::new(),
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

    pub(super) fn render_log(
        &mut self,
        c: Colors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                let line_h = window.rem_size() * LINE_H;
                if log.sizes.len() != tail.lines.len()
                    || log.sizes.first().is_some_and(|s| s.height != line_h)
                {
                    log.sizes = Rc::new(vec![size(px(0.), line_h); tail.lines.len()]);
                }
                let sizes = log.sizes.clone();
                let tail = tail.clone();
                v_virtual_list(cx.entity(), "bp-log-lines", sizes, move |_, range, _, _| {
                    range
                        .filter_map(|ix| {
                            let (level, text) = tail.lines.get(ix)?;
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
                div()
                    .px_2()
                    .text_sm()
                    .font_semibold()
                    .text_color(c.foreground.opacity(0.6))
                    .child(t("bigpicture.log.title")),
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
        match &self.log.as_ref()?.state {
            LogState::Ready(tail) => Some(tail.lines.len()),
            _ => None,
        }
    }
}

impl LogPanel {
    fn show(&mut self, tail: std::io::Result<Option<LogTail>>) {
        // Follow the newest line unless the wheel moved the list up from it.
        let max = self.scroll.max_offset().y;
        let follow =
            !matches!(self.state, LogState::Ready(_)) || -self.scroll.offset().y >= max - px(2.);
        self.state = match tail {
            Ok(Some(tail)) => LogState::Ready(Arc::new(tail)),
            Ok(None) => LogState::Missing,
            Err(e) => LogState::Failed(e.to_string().into()),
        };
        if follow {
            self.scroll.scroll_to_bottom();
        }
    }
}
