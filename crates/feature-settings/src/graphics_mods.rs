//! What the graphics mod panels of the Tools section share: the channel an
//! install reports its steps and download progress through, the texts of
//! steps and errors, the notice after a removal that names the files Irori
//! kept because they changed, and the event a panel sends when it changed
//! what another panel shows.

use std::sync::Arc;

use gpui_kit::SharedString;
use gpui_kit::component::notification::Notification;
use irori_core::install::ProgressSink;
use irori_reshade::{GraphicsMod, RemoveReport, ReshadeError, Step};
use irori_ui::i18n::{t, t_args};
use tokio::sync::mpsc::UnboundedReceiver;

/// A panel changed a graphics mod in a way the other panel shows: RenoDX
/// installed ReShade's build with add-on support, or removing ReShade took
/// RenoDX with it.
pub(crate) struct GraphicsModsChanged;

/// The message for a failed operation on `kind`, in the interface language.
pub(crate) fn error_text(e: &anyhow::Error, kind: GraphicsMod) -> SharedString {
    match ReshadeError::of(e) {
        Some(ReshadeError::Banned { exe }) => {
            t_args("settings.reshade.error.banned", &[("program", exe)])
        }
        Some(ReshadeError::NoApi) => t("settings.reshade.error.no_api"),
        Some(ReshadeError::Unsupported(api)) => t_args(
            "settings.reshade.error.unsupported",
            &[("api", api.label())],
        ),
        Some(ReshadeError::UnsupportedArch) => t("settings.reshade.error.arch"),
        Some(ReshadeError::DllTaken { name }) => t_args(
            match kind {
                GraphicsMod::Reshade => "settings.reshade.error.dll_taken",
                GraphicsMod::Renodx => "settings.renodx.error.file_taken",
            },
            &[("file", name)],
        ),
        Some(ReshadeError::OutsideGame) => t("settings.reshade.error.outside"),
        Some(ReshadeError::NotAddon) => t("settings.renodx.error.not_addon"),
        Some(ReshadeError::AddonArch) => t("settings.renodx.error.arch"),
        Some(ReshadeError::NoAddonBuild) => t("settings.renodx.error.no_build"),
        Some(ReshadeError::AddonTaken { name }) => {
            t_args("settings.renodx.error.addon_taken", &[("file", name)])
        }
        None => format!("{e:#}").into(),
    }
}

/// What an install is doing, in the interface language.
pub(crate) fn step_text(step: Step) -> SharedString {
    t(match step {
        Step::Checking => "settings.reshade.step.checking",
        Step::DownloadingReshade => "settings.reshade.step.reshade",
        Step::DownloadingShaders => "settings.reshade.step.shaders",
        Step::DownloadingCompiler => "settings.reshade.step.compiler",
        Step::DownloadingAddon => "settings.renodx.step.mod",
        Step::Writing => "settings.reshade.step.writing",
    })
}

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
