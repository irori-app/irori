//! What the graphics mod panels of the Tools section share: the channel an
//! install reports its steps and download progress through, and the notice
//! after a removal that names the files Irori kept because they changed.

use std::sync::Arc;

use gpui_kit::SharedString;
use gpui_kit::component::notification::Notification;
use irori_core::install::ProgressSink;
use irori_reshade::{RemoveReport, Step};
use irori_ui::i18n::t_args;
use tokio::sync::mpsc::UnboundedReceiver;

/// What a running install reports.
pub(crate) enum Report {
    Step(Step),
    Percent(Option<f32>),
}

/// How an install reports its steps.
pub(crate) type StepSink = Arc<dyn Fn(Step) + Send + Sync>;

/// The step reporter and progress sink of an install, and the receiving end
/// the panel reads them from.
pub(crate) fn progress_channel() -> (StepSink, ProgressSink, UnboundedReceiver<Report>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Report>();
    let step_tx = tx.clone();
    let step: StepSink = Arc::new(move |s| {
        let _ = step_tx.send(Report::Step(s));
    });
    let progress: ProgressSink = Arc::new(move |p| {
        let _ = tx.send(Report::Percent(p.percent));
    });
    (step, progress, rx)
}

/// The success notice after removing a mod: `removed` as its text, or as
/// its title above the files that were kept.
pub(crate) fn removed_notification(removed: SharedString, report: &RemoveReport) -> Notification {
    if report.kept.is_empty() {
        return Notification::success(removed);
    }
    let files = report
        .kept
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ");
    let count = report.kept.len().to_string();
    let key = if report.kept.len() == 1 {
        "settings.reshade.kept_one"
    } else {
        "settings.reshade.kept_other"
    };
    Notification::success(t_args(key, &[("count", &count), ("files", &files)])).title(removed)
}
