//! Game log sheet: opens the unified [`LogViewer`] from `irori_ui` inside a
//! 48rem sheet so game logs share the same structured layout, search, module
//! filtering, context inspector, privacy redaction and diagnostic banners as
//! `Settings -> Logs`.

use std::path::PathBuf;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, AppContext as _, ParentElement as _, Styled as _, Window, div, rems};
use irori_core::GameId;
use irori_ui::backend;
pub use irori_ui::game_log::{LineLevel, LogTail, parse_tail};
use irori_ui::i18n::{t, t_args};
pub use irori_ui::log_viewer::{LogViewer, LogViewerOptions};

/// Lines kept by launch-problem quick reads.
const MAX_LINES: usize = 5000;

pub(crate) async fn read_tail(path: PathBuf) -> anyhow::Result<Option<LogTail>> {
    irori_ui::game_log::read_tail(&path, MAX_LINES)
        .await
        .map_err(|e| {
            anyhow::anyhow!(t_args(
                "game.logs.reading_path",
                &[
                    ("path", &path.display().to_string()),
                    ("error", &e.to_string()),
                ],
            ))
        })
}

/// Open the log of a game's last session in a sheet using the unified viewer.
pub fn open_log_sheet(id: GameId, window: &mut Window, cx: &mut App) {
    let game_title = backend(cx).game(&id).map(|g| g.title.clone());
    let title = match &game_title {
        Some(title) => t_args("game.logs.title", &[("title", title)]),
        None => t("game.logs.title_plain"),
    };
    let upload_name = format!(
        "Log of {}",
        game_title.unwrap_or_else(|| id.app_name.to_string())
    );
    let path = backend(cx).game_log(&id);
    let viewer = cx.new(|cx| {
        let mut v = LogViewer::new(
            path,
            Some(upload_name.into()),
            LogViewerOptions {
                compact: true,
                show_action_bar: true,
                game_id: Some(id.clone()),
            },
            window,
            cx,
        );
        v.start(window, cx);
        v
    });
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title(title.clone())
            .size(rems(48.))
            .child(div().w_full().h(rems(38.)).child(viewer.clone()))
    });
}
