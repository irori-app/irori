//! Minimalist `+ Add download` modal for direct HTTP/HTTPS links, `.torrent`
//! files, and `magnet:` links.
//!
//! Single smart input box with automatic kind detection, opt-in BitTorrent
//! consent and `aria2c` readiness check, torrent/magnet file picker, compact
//! destination folder row with free space, and collapsed-by-default options.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    Window, div,
};
use irori_core::process::CancellationToken;
use irori_core::util::format_bytes;
use irori_services::web_downloads::torrent_meta::classify_source;
use irori_services::{Aria2Status, DownloadSource, TorrentPreview, WebDownloadRequest};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, notify_success};

/// Whether clipboard or typed text looks like a direct link or magnet URI.
pub(crate) fn is_download_link_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains(['\n', '\r']) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("magnet:")
}

/// Open the shared `+ Add download` dialog, optionally pre-filled with a link,
/// magnet URI, or local `.torrent` path.
pub fn open_add_download_dialog(initial_input: Option<String>, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let prefill = initial_input.or_else(|| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .map(|text| text.trim().to_string())
            .filter(|text| is_download_link_text(text))
    });
    let dialog = cx.new(|cx| AddDownloadDialog::new(prefill, window, cx));
    let footer = cx.new(|cx| AddDownloadFooter::new(dialog.clone(), cx));
    let width =
        (window.rem_size() * 34.).min(window.viewport_size().width - window.rem_size() * 2.);
    let focus = Cell::new(true);
    window.open_dialog(cx, move |d, window, cx| {
        if focus.replace(false) {
            let input = dialog.read(cx).input.clone();
            input.focus_handle(cx).focus(window, cx);
        }
        let on_close = dialog.clone();
        d.title(t("manage.downloads.add.title"))
            .width(width)
            .child(dialog.clone())
            .footer(footer.clone())
            .on_close(move |_, _, cx| on_close.update(cx, |d, cx| d.cleanup(cx)))
    });
}

pub(super) enum PreviewState {
    Idle,
    Loading {
        source: String,
        is_magnet: bool,
    },
    Ready {
        source: String,
        preview: TorrentPreview,
        selected: BTreeSet<usize>,
    },
    Error {
        source: String,
        message: String,
    },
}

pub(super) struct AddDownloadDialog {
    pub(super) input: Entity<InputState>,
    pub(super) detected: Option<DownloadSource>,
    pub(super) aria2_status: Option<Aria2Status>,
    pub(super) checking_aria2: bool,
    pub(super) installing_aria2: bool,
    install_cancel: Option<CancellationToken>,
    pub(super) preview: PreviewState,
    preview_task: Option<Task<()>>,
    destination: PathBuf,
    free_space: Option<u64>,
    pub(super) more_open: bool,
    pub(super) auto_extract: bool,
    pub(super) delete_archive_after_extract: bool,
    pub(super) auto_add_to_library: bool,
    last_bt_enabled: bool,
    last_aria2_path: String,
    submitting: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl AddDownloadDialog {
    fn new(prefill: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = models(cx).settings.read(cx).app.clone();
        let prefs = &app.web_downloads;
        let destination = app.web_download_folder();
        let input = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder(t("manage.downloads.add.placeholder"));
            if let Some(text) = &prefill {
                state.set_value(text.clone(), window, cx);
            }
            state
        });
        let settings = models(cx).settings.clone();
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.on_input_changed(false, window, cx),
                        InputEvent::PressEnter { .. } if this.can_submit(cx) => {
                            this.submit(window, cx);
                        }
                        _ => {}
                    }
                }),
                cx.observe_in(&settings, window, |this, _, window, cx| {
                    this.on_settings_changed(window, cx);
                }),
            ];
        let mut dialog = Self {
            input,
            detected: None,
            aria2_status: None,
            checking_aria2: false,
            installing_aria2: false,
            install_cancel: None,
            preview: PreviewState::Idle,
            preview_task: None,
            destination,
            free_space: None,
            more_open: false,
            auto_extract: prefs.auto_extract,
            delete_archive_after_extract: prefs.delete_archive_after_extract,
            auto_add_to_library: prefs.auto_add_to_library,
            last_bt_enabled: prefs.bittorrent,
            last_aria2_path: prefs.aria2_path.clone(),
            submitting: false,
            error: None,
            _subscriptions: subscriptions,
        };
        dialog.refresh_free_space(cx);
        if prefill.is_some() {
            dialog.on_input_changed(true, window, cx);
        }
        dialog
    }

    fn cleanup(&mut self, cx: &mut Context<Self>) {
        self.preview_task = None;
        if let Some(cancel) = self.install_cancel.take() {
            cancel.cancel();
        }
        backend(cx).cancel_torrent_preview();
    }

    pub(super) fn bittorrent_enabled(&self, cx: &App) -> bool {
        models(cx).settings.read(cx).app.web_downloads.bittorrent
    }

    pub(super) fn is_bt_source(&self) -> bool {
        matches!(
            self.detected,
            Some(
                DownloadSource::LocalTorrent(_)
                    | DownloadSource::RemoteTorrentUrl(_)
                    | DownloadSource::Magnet { .. }
            )
        )
    }

    fn aria2_ready(&self) -> bool {
        matches!(self.aria2_status, Some(Aria2Status::Ready { .. }))
    }

    fn can_submit(&self, cx: &App) -> bool {
        if self.submitting || self.detected.is_none() {
            return false;
        }
        if self.is_bt_source() && (!self.bittorrent_enabled(cx) || !self.aria2_ready()) {
            return false;
        }
        if let PreviewState::Ready { selected, .. } = &self.preview
            && selected.is_empty()
        {
            return false;
        }
        true
    }

    fn on_settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = &models(cx).settings.read(cx).app.web_downloads;
        let bt_enabled = prefs.bittorrent;
        let aria2_path = prefs.aria2_path.clone();
        let bt_changed = bt_enabled != self.last_bt_enabled;
        let bt_just_enabled = !self.last_bt_enabled && bt_enabled;
        let aria2_path_changed = aria2_path != self.last_aria2_path;
        self.last_bt_enabled = bt_enabled;
        self.last_aria2_path = aria2_path;
        if !bt_changed && !aria2_path_changed {
            return;
        }
        if aria2_path_changed {
            self.aria2_status = None;
        }
        if self.is_bt_source() && bt_enabled {
            if aria2_path_changed || (self.aria2_status.is_none() && !self.checking_aria2) {
                self.check_aria2_status(window, cx);
            } else if bt_just_enabled && self.aria2_ready() {
                self.maybe_start_preview(true, window, cx);
            }
        }
        cx.notify();
    }

    fn on_input_changed(&mut self, immediate: bool, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().trim().to_string();
        self.error = None;
        self.detected = if raw.is_empty() {
            None
        } else {
            classify_source(&raw).ok()
        };

        let same_preview = match &self.preview {
            PreviewState::Loading { source, .. }
            | PreviewState::Ready { source, .. }
            | PreviewState::Error { source, .. } => source == &raw,
            PreviewState::Idle => false,
        };
        if !same_preview {
            self.preview_task = None;
            backend(cx).cancel_torrent_preview();
            self.preview = PreviewState::Idle;
        }

        if self.is_bt_source() {
            if self.aria2_status.is_none() && !self.checking_aria2 {
                self.check_aria2_status(window, cx);
            }
            self.maybe_start_preview(immediate, window, cx);
        }
        cx.notify();
    }

    pub(super) fn check_aria2_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checking_aria2 = true;
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.aria2_status().await });
        cx.spawn_in(window, async move |this, cx| {
            let status = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.checking_aria2 = false;
                this.aria2_status = status;
                if this.aria2_ready() {
                    this.maybe_start_preview(true, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn enable_bittorrent(&mut self, cx: &mut Context<Self>) {
        backend_op(cx, |b| {
            b.settings.update_app(|s| s.web_downloads.bittorrent = true);
        });
    }

    pub(super) fn install_aria2(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.installing_aria2 {
            return;
        }
        self.installing_aria2 = true;
        self.error = None;
        let cancel = CancellationToken::new();
        self.install_cancel = Some(cancel.clone());
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.install_aria2(cancel).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.installing_aria2 = false;
                this.install_cancel = None;
                match result {
                    Ok(_) => this.check_aria2_status(window, cx),
                    Err(err) => {
                        this.error = Some(format!("{err:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn maybe_start_preview(
        &mut self,
        immediate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.input.read(cx).value().trim().to_string();
        let Some(detected) = &self.detected else {
            return;
        };
        let is_magnet = match detected {
            DownloadSource::LocalTorrent(_) | DownloadSource::RemoteTorrentUrl(_) => false,
            DownloadSource::Magnet { .. } => {
                if !self.bittorrent_enabled(cx) || !self.aria2_ready() {
                    return;
                }
                true
            }
            DownloadSource::DirectUrl(_) => return,
        };
        if matches!(
            &self.preview,
            PreviewState::Loading { source: s, .. } | PreviewState::Ready { source: s, .. }
                if s == &source
        ) {
            return;
        }
        self.preview = PreviewState::Loading {
            source: source.clone(),
            is_magnet,
        };
        let b = backend(cx);
        let delay = if immediate {
            Duration::ZERO
        } else {
            Duration::from_millis(250)
        };
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let src = source.clone();
            let worker = b.clone();
            let req = b.spawn(async move { worker.preview_torrent_source(&src).await });
            let result = match req.await {
                Ok(res) => res,
                Err(err) => Err(anyhow::anyhow!("preview failed: {err}")),
            };
            let _ = this.update_in(cx, |this, _, cx| {
                if this.input.read(cx).value().trim() != source {
                    return;
                }
                match result {
                    Ok(preview) => {
                        let selected = preview.files.iter().map(|f| f.index).collect();
                        this.preview = PreviewState::Ready {
                            source,
                            preview,
                            selected,
                        };
                    }
                    Err(err) => {
                        this.preview = PreviewState::Error {
                            source,
                            message: format!("{err:#}"),
                        };
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn cancel_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_task = None;
        backend(cx).cancel_torrent_preview();
        self.preview = PreviewState::Idle;
        cx.notify();
    }

    fn pick_torrent_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("manage.downloads.add.pick_torrent")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.input.update(cx, |state, cx| {
                    state.set_value(path.display().to_string(), window, cx);
                });
                this.on_input_changed(true, window, cx);
            });
        })
        .detach();
    }

    fn pick_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("manage.downloads.add.choose_folder")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, _, cx| {
                this.destination = path;
                this.free_space = None;
                this.refresh_free_space(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_free_space(&mut self, cx: &mut Context<Self>) {
        let dest = self.destination.clone();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(tokio::task::spawn_blocking(move || b.free_space_at(&dest)).await?)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(free) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.free_space = free;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let source = self.input.read(cx).value().trim().to_string();
        let (title, selected_files, torrent_bytes, total_bytes) = match &self.preview {
            PreviewState::Ready {
                preview, selected, ..
            } => {
                let total: u64 = preview
                    .files
                    .iter()
                    .filter(|f| selected.contains(&f.index))
                    .map(|f| f.length)
                    .fold(0u64, u64::saturating_add);
                let indices = if selected.len() < preview.files.len() {
                    selected.iter().copied().collect()
                } else {
                    Vec::new()
                };
                (
                    Some(preview.name.clone()),
                    indices,
                    preview.torrent_bytes.clone(),
                    total,
                )
            }
            _ => (None, Vec::new(), None, 0),
        };
        let req = WebDownloadRequest {
            source,
            title,
            destination: Some(self.destination.clone()),
            selected_files,
            torrent_bytes,
            auto_extract: Some(self.auto_extract),
            delete_archive_after_extract: Some(
                self.auto_extract && self.delete_archive_after_extract,
            ),
            auto_add_to_library: Some(self.auto_add_to_library),
            total_bytes,
        };
        if matches!(self.preview, PreviewState::Loading { .. }) {
            self.preview_task = None;
            backend(cx).cancel_torrent_preview();
            self.preview = PreviewState::Idle;
        }
        self.submitting = true;
        self.error = None;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            tokio::task::spawn_blocking(move || b.enqueue_web_download(req)).await?
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.submitting = false;
                match result {
                    Ok(_) => {
                        this.cleanup(cx);
                        window.close_dialog(cx);
                        notify_success(t("manage.downloads.add.started"), window, cx);
                    }
                    Err(err) => {
                        this.error = Some(format!("{err:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn detected_kind_label(&self) -> Option<SharedString> {
        match &self.detected {
            Some(DownloadSource::DirectUrl(_)) => Some(t("manage.downloads.add.kind.link")),
            Some(DownloadSource::LocalTorrent(_) | DownloadSource::RemoteTorrentUrl(_)) => {
                Some(t("manage.downloads.add.kind.torrent"))
            }
            Some(DownloadSource::Magnet { .. }) => Some(t("manage.downloads.add.kind.magnet")),
            None => None,
        }
    }
}

impl Render for AddDownloadDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind_pill = self.detected_kind_label();
        let bt_notice = self.render_bt_notice(cx);
        let preview = self.render_preview(cx);
        let more_options = self.render_more_options(cx);
        let free_text = self.free_space.map(|free| {
            t_args(
                "manage.downloads.space.idle_free",
                &[("free", &format_bytes(free))],
            )
        });
        let radius = cx.theme().radius;
        let radius_lg = cx.theme().radius_lg;
        let bg = cx.theme().background;
        let border = cx.theme().border;
        let muted = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;

        v_flex()
            .id("add-download-form")
            .gap_3p5()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .items_center()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.input)))
                            .when_some(kind_pill, |row, pill| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(radius)
                                        .bg(muted)
                                        .text_xs()
                                        .font_medium()
                                        .text_color(muted_fg)
                                        .child(pill),
                                )
                            }),
                    )
                    .child(
                        Button::new("add-pick-torrent")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("manage.downloads.add.torrent_button"))
                            .tooltip(t("manage.downloads.add.pick_torrent"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_torrent_file(window, cx)
                            })),
                    ),
            )
            .children(bt_notice)
            .children(preview)
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .rounded(radius_lg)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Folder).small().text_color(muted_fg))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .child(self.destination.display().to_string()),
                            )
                            .when_some(free_text, |row, free| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child(format!("· {free}")),
                                )
                            }),
                    )
                    .child(
                        Button::new("add-change-folder")
                            .ghost()
                            .xsmall()
                            .label(t("manage.downloads.add.change_folder"))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.pick_destination(window, cx)
                                }),
                            ),
                    ),
            )
            .child(more_options)
    }
}

struct AddDownloadFooter {
    dialog: Entity<AddDownloadDialog>,
    _observe: Subscription,
}

impl AddDownloadFooter {
    fn new(dialog: Entity<AddDownloadDialog>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&dialog, |_, _, cx| cx.notify());
        Self {
            dialog,
            _observe: observe,
        }
    }
}

impl Render for AddDownloadFooter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.dialog.read(cx);
        let ready = state.can_submit(cx);
        let submitting = state.submitting;
        let error = state.error.clone();
        let dialog = self.dialog.clone();
        v_flex()
            .id("add-download-footer")
            .w_full()
            .gap_3()
            .when_some(error, |box_el, msg| {
                box_el.child(Alert::error("add-download-error", msg))
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("add-download-cancel")
                            .ghost()
                            .label(t("manage.downloads.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("add-download-submit")
                            .primary()
                            .label(t("manage.downloads.add.submit"))
                            .loading(submitting)
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                dialog.update(cx, |d, cx| d.submit(window, cx));
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::is_download_link_text;

    #[test]
    fn recognizes_http_https_and_magnet_links() {
        assert!(is_download_link_text("https://example.com/game.zip"));
        assert!(is_download_link_text(
            "  HTTP://files.example.org/demo.7z  "
        ));
        assert!(is_download_link_text(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!is_download_link_text(""));
        assert!(!is_download_link_text("file:///etc/passwd"));
        assert!(!is_download_link_text("just some notes"));
        assert!(!is_download_link_text("https://a.test\nhttps://b.test"));
    }
}
