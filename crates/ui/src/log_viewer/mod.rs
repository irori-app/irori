//! Unified virtualized log viewer shared by Settings -> Logs, Game Settings ->
//! Log, and the game log sheet (`open_log_sheet`).
//!
//! Features event-driven `notify` file tailing, structured parsing (timestamp,
//! level, module, message), consecutive duplicate collapsing, search match
//! highlighting with include/exclude toggle, module filtering, direct error
//! navigation, docked context inspector (±2 lines), default home-path
//! redaction (`~`) for copying and `dpaste.com` uploads, and 1-click launch
//! troubleshooting banners for game logs.

mod actions;
mod rows;
mod toolbar;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::VirtualListScrollHandle;
use gpui_kit::{
    AppContext as _, Context, Entity, Pixels, Render, ScrollStrategy, SharedString, Size,
    Subscription, Task, UniformListScrollHandle, Window, px,
};
use irori_core::GameId;
use irori_core::crash::{current_home_dir, redact_home_paths};
use irori_extras::logs::UploadedLog;

use crate::game_log::{
    self, DisplayRow, LevelFilter, LineLevel, LogStamp, ParsedLine, ParsedTail, build_rows,
    module_counts,
};
use crate::i18n::{language, t, t_args};
use crate::{backend, models};

/// Maximum lines retained from the tail of a log file.
pub const MAX_LINES: usize = 5000;
/// Debounce window for rapid bursts of log writes.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(90);
/// Retry interval only while the log's parent directory does not yet exist.
const MISSING_DIR_RETRY: Duration = Duration::from_secs(2);

/// How timestamps are displayed in log rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeMode {
    Compact,
    Full,
    Hidden,
}

impl TimeMode {
    pub fn next(self) -> Self {
        match self {
            TimeMode::Compact => TimeMode::Full,
            TimeMode::Full => TimeMode::Hidden,
            TimeMode::Hidden => TimeMode::Compact,
        }
    }
}

/// Configuration for [`LogViewer`].
#[derive(Debug, Clone)]
pub struct LogViewerOptions {
    /// Hide line numbers and keep action buttons compact (for the 48rem sheet).
    pub compact: bool,
    /// Render the Copy / Upload / Editor / Folder bar inside `LogViewer` itself.
    /// `LogsHubView` sets this to `false` and places those actions in its page header.
    pub show_action_bar: bool,
    /// Associated game when viewing a game's launch or install log.
    pub game_id: Option<GameId>,
}

impl Default for LogViewerOptions {
    fn default() -> Self {
        Self {
            compact: false,
            show_action_bar: true,
            game_id: None,
        }
    }
}

enum LoadState {
    Idle,
    Loading,
    Missing,
    Failed(SharedString),
    Loaded,
}

/// Unified GPUI log viewer entity.
pub struct LogViewer {
    path: PathBuf,
    /// Upload identifier kept in dpaste history (not translated).
    upload_name: Option<SharedString>,
    options: LogViewerOptions,
    home_dir: Option<PathBuf>,
    state: LoadState,
    stamp: Option<LogStamp>,
    file_size: u64,
    truncated: bool,
    lines: Arc<Vec<ParsedLine>>,
    warn_count: usize,
    err_count: usize,
    modules: Vec<(SharedString, usize)>,
    rows: Arc<Vec<DisplayRow>>,
    error_rows: Vec<usize>,
    query: Entity<gpui_kit::component::input::InputState>,
    exclude: bool,
    level: LevelFilter,
    selected_modules: HashSet<SharedString>,
    follow: bool,
    wrap: bool,
    time_mode: TimeMode,
    collapse_dups: bool,
    redact_home: bool,
    selected_row: Option<usize>,
    match_cursor: usize,
    error_cursor: Option<usize>,
    copied: bool,
    uploading: bool,
    uploads: Vec<UploadedLog>,
    deleting_uploads: HashSet<String>,
    diagnostic_hint: Option<SharedString>,
    diagnostic_dismissed: bool,
    diagnostic_stamp: Option<LogStamp>,
    scroll_uniform: UniformListScrollHandle,
    scroll_wrapped: VirtualListScrollHandle,
    wrapped_sizes: Rc<Vec<Size<Pixels>>>,
    wrapped_sized_for: (usize, Pixels, bool),
    watch_task: Option<Task<()>>,
    language: String,
    _subscriptions: Vec<Subscription>,
}

impl LogViewer {
    /// Create a new log viewer for `path`. Call [`Self::start`] when shown.
    pub fn new(
        path: PathBuf,
        upload_name: Option<SharedString>,
        options: LogViewerOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(t("ui.logs.search_placeholder"))
        });
        let q_sub = cx.subscribe(
            &query,
            |this: &mut Self, _, event: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                    this.match_cursor = 0;
                    this.selected_row = None;
                    this.refilter(cx);
                }
            },
        );
        let settings = models(cx).settings.clone();
        let s_sub = cx.observe(&settings, |_, _, cx| cx.notify());
        Self {
            path,
            upload_name,
            options,
            home_dir: current_home_dir(),
            state: LoadState::Idle,
            stamp: None,
            file_size: 0,
            truncated: false,
            lines: Arc::default(),
            warn_count: 0,
            err_count: 0,
            modules: Vec::new(),
            rows: Arc::default(),
            error_rows: Vec::new(),
            query,
            exclude: false,
            level: LevelFilter::All,
            selected_modules: HashSet::new(),
            follow: true,
            wrap: false,
            time_mode: TimeMode::Compact,
            collapse_dups: true,
            redact_home: true,
            selected_row: None,
            match_cursor: 0,
            error_cursor: None,
            copied: false,
            uploading: false,
            uploads: Vec::new(),
            deleting_uploads: HashSet::new(),
            diagnostic_hint: None,
            diagnostic_dismissed: false,
            diagnostic_stamp: None,
            scroll_uniform: UniformListScrollHandle::new(),
            scroll_wrapped: VirtualListScrollHandle::new(),
            wrapped_sizes: Rc::new(Vec::new()),
            wrapped_sized_for: (usize::MAX, px(0.), false),
            watch_task: None,
            language: language(),
            _subscriptions: vec![q_sub, s_sub],
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn can_copy(&self) -> bool {
        !self.rows.is_empty()
    }

    pub fn can_upload(&self) -> bool {
        self.upload_name.is_some()
            && !self.uploading
            && matches!(self.state, LoadState::Loaded)
            && !self.lines.is_empty()
    }

    pub fn is_copied(&self) -> bool {
        self.copied
    }

    pub fn is_uploading(&self) -> bool {
        self.uploading
    }

    pub fn uploads(&self) -> &[UploadedLog] {
        &self.uploads
    }

    pub fn is_deleting_upload(&self, url: &str) -> bool {
        self.deleting_uploads.contains(url)
    }

    pub fn redact_home(&self) -> bool {
        self.redact_home
    }

    pub fn set_redact_home(&mut self, redact: bool, cx: &mut Context<Self>) {
        if self.redact_home != redact {
            self.redact_home = redact;
            cx.notify();
        }
    }

    /// Switch the viewer to another log file (for example from the Logs Hub sidebar).
    pub fn set_source(
        &mut self,
        path: PathBuf,
        upload_name: Option<SharedString>,
        game_id: Option<GameId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.path == path && self.upload_name == upload_name && self.options.game_id == game_id {
            return;
        }
        let was_active = self.watch_task.is_some();
        self.watch_task = None;
        self.path = path;
        self.upload_name = upload_name;
        self.options.game_id = game_id;
        self.stamp = None;
        self.file_size = 0;
        self.truncated = false;
        self.lines = Arc::default();
        self.rows = Arc::default();
        self.warn_count = 0;
        self.err_count = 0;
        self.modules.clear();
        self.error_rows.clear();
        self.selected_modules.clear();
        self.selected_row = None;
        self.error_cursor = None;
        self.match_cursor = 0;
        self.diagnostic_hint = None;
        self.diagnostic_dismissed = false;
        self.diagnostic_stamp = None;
        self.state = LoadState::Idle;
        if was_active {
            self.start(window, cx);
        }
        cx.notify();
    }

    /// Start loading and following the log via event-driven OS notifications.
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state, LoadState::Idle) {
            self.state = LoadState::Loading;
        }
        self.load_uploads(window, cx);
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(path) = this.update(cx, |this, _| this.path.clone()) else {
                return;
            };
            // Initial read.
            let initial = Self::read_once(&path, None, cx).await;
            let Ok(follow) = this.update_in(cx, |this, window, cx| {
                this.apply_read(initial, window, cx);
                this.follow
            }) else {
                return;
            };
            if !follow {
                return;
            }
            // Event-driven filesystem watch loop (zero idle CPU).
            let mut watcher_pair = game_log::watch_log_file(&path);
            loop {
                if let Some((_, ref mut rx)) = watcher_pair {
                    if rx.recv().await.is_none() {
                        break;
                    }
                    // Coalesce burst writes before re-reading.
                    cx.background_executor().timer(WATCH_DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                } else {
                    cx.background_executor().timer(MISSING_DIR_RETRY).await;
                    watcher_pair = game_log::watch_log_file(&path);
                }
                let Ok((cur_path, stamp, follow)) =
                    this.update(cx, |this, _| (this.path.clone(), this.stamp, this.follow))
                else {
                    break;
                };
                if !follow || cur_path != path {
                    break;
                }
                let result = Self::read_once(&path, stamp, cx).await;
                if this
                    .update_in(cx, |this, window, cx| this.apply_read(result, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    async fn read_once(
        path: &Path,
        stamp: Option<LogStamp>,
        cx: &mut gpui_kit::AsyncWindowContext,
    ) -> std::io::Result<Option<(LogStamp, Option<ParsedTail>)>> {
        let path = path.to_path_buf();
        let task = match cx.update(|_, cx| {
            crate::run(cx, async move {
                Ok(game_log::read_structured_tail(&path, stamp, MAX_LINES).await)
            })
        }) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        match task.await {
            Ok(res) => res,
            Err(e) => Err(std::io::Error::other(e.to_string())),
        }
    }

    /// Stop watching the file (when the view is hidden or paused).
    pub fn stop(&mut self) {
        self.watch_task = None;
    }

    fn apply_read(
        &mut self,
        result: std::io::Result<Option<(LogStamp, Option<ParsedTail>)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(None) => return,
            Ok(Some((stamp, None))) => {
                self.stamp = Some(stamp);
                self.file_size = 0;
                self.truncated = false;
                self.lines = Arc::default();
                self.warn_count = 0;
                self.err_count = 0;
                self.modules.clear();
                self.state = LoadState::Missing;
            }
            Ok(Some((stamp, Some(tail)))) => {
                self.stamp = Some(stamp);
                self.file_size = tail.file_size;
                self.truncated = tail.truncated;
                let mut warns = 0;
                let mut errs = 0;
                for l in &tail.lines {
                    match l.level {
                        LineLevel::Warning => warns += 1,
                        LineLevel::Error => errs += 1,
                        LineLevel::Info => {}
                    }
                }
                self.warn_count = warns;
                self.err_count = errs;
                self.modules = module_counts(&tail.lines);
                self.lines = Arc::new(tail.lines);
                self.state = LoadState::Loaded;
                if self.err_count > 0 && self.options.game_id.is_some() {
                    self.maybe_diagnose_game(window, cx);
                }
            }
            Err(e) => {
                self.state = LoadState::Failed(SharedString::from(e.to_string()));
            }
        }
        self.refilter(cx);
        if self.follow && !self.rows.is_empty() {
            self.scroll_to_bottom();
        }
    }

    fn maybe_diagnose_game(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diagnostic_dismissed || self.diagnostic_stamp == self.stamp {
            return;
        }
        let Some(game_id) = self.options.game_id.clone() else {
            return;
        };
        self.diagnostic_stamp = self.stamp;
        let b = backend(cx);
        let task = crate::run(
            cx,
            async move { b.troubleshoot_launch(&game_id, None).await },
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(diag) = task.await else {
                return;
            };
            let hint = diag.findings.first().map(|f| {
                let first = match f.detail.as_ref() {
                    Some(irori_services::troubleshoot::Detail::Libraries { dlls, .. })
                    | Some(irori_services::troubleshoot::Detail::ProgramLibraries { dlls }) => {
                        dlls.first().map(String::as_str)
                    }
                    Some(irori_services::troubleshoot::Detail::AntiCheat { names, .. }) => {
                        names.first().map(String::as_str)
                    }
                    _ => None,
                };
                match first {
                    Some(detail) => t_args("ui.logs.diagnostic_detail", &[("detail", detail)]),
                    None => t("ui.logs.diagnostic_found"),
                }
            });
            let _ = this.update(cx, |this, cx| {
                this.diagnostic_hint = hint;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value();
        let rows = build_rows(
            &self.lines,
            self.level,
            &query,
            self.exclude,
            &self.selected_modules,
            self.collapse_dups,
        );
        self.error_rows = rows
            .iter()
            .enumerate()
            .filter_map(|(ix, r)| {
                self.lines
                    .get(r.entry_idx)
                    .is_some_and(|l| l.level == LineLevel::Error)
                    .then_some(ix)
            })
            .collect();
        self.rows = Arc::new(rows);
        self.wrapped_sized_for = (usize::MAX, px(0.), false);
        if self.selected_row.is_some_and(|ix| ix >= self.rows.len()) {
            self.selected_row = None;
        }
        cx.notify();
    }

    fn scroll_to_bottom(&self) {
        if self.rows.is_empty() {
            return;
        }
        if self.wrap {
            self.scroll_wrapped.scroll_to_bottom();
        } else {
            self.scroll_uniform
                .scroll_to_item(self.rows.len() - 1, ScrollStrategy::Bottom);
        }
    }

    pub(super) fn scroll_to_row(&mut self, row_idx: usize, cx: &mut Context<Self>) {
        if row_idx >= self.rows.len() {
            return;
        }
        self.selected_row = Some(row_idx);
        if self.wrap {
            self.scroll_wrapped
                .scroll_to_item(row_idx, ScrollStrategy::Center);
        } else {
            self.scroll_uniform
                .scroll_to_item(row_idx, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub(crate) fn maybe_redact(&self, text: &str) -> String {
        if self.redact_home {
            redact_home_paths(text, self.home_dir.as_deref())
        } else {
            text.to_string()
        }
    }
}

impl Render for LogViewer {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.render_view(window, cx)
    }
}
