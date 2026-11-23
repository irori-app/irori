//! The other launchers Irori imports from (Lutris, Bottles, Faugus
//! Launcher, Playnite): which have data on this computer, and one import
//! of each with its state, drawn the same way in onboarding, the empty
//! library and Getting started. The imports themselves live in the shell
//! (`irori_ui::hooks`); they only read the other launcher.

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::{
    AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, Task, div,
};
use irori_ui::hooks::{self, LutrisSummary};
use irori_ui::i18n::{t, t_args};

/// A launcher Irori imports games from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Launcher {
    Lutris,
    Bottles,
    Faugus,
    Playnite,
}

impl Launcher {
    pub const ALL: [Launcher; 4] = [
        Launcher::Lutris,
        Launcher::Bottles,
        Launcher::Faugus,
        Launcher::Playnite,
    ];

    /// Its own name; never translated.
    pub fn name(self) -> &'static str {
        match self {
            Launcher::Lutris => "Lutris",
            Launcher::Bottles => "Bottles",
            Launcher::Faugus => "Faugus Launcher",
            Launcher::Playnite => "Playnite",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Launcher::Lutris => "lutris",
            Launcher::Bottles => "bottles",
            Launcher::Faugus => "faugus",
            Launcher::Playnite => "playnite",
        }
    }

    /// Its mark where Irori has one, else the import icon.
    pub fn icon(self) -> Icon {
        match self {
            Launcher::Lutris => irori_ui::components::StoreLogo::Lutris.icon(),
            Launcher::Bottles | Launcher::Faugus | Launcher::Playnite => {
                Icon::new(IconName::Import)
            }
        }
    }

    /// What importing brings over, in a sentence.
    pub fn description(self) -> SharedString {
        match self {
            Launcher::Lutris => t("manage.onboarding.lutris.description"),
            Launcher::Bottles => t("manage.onboarding.bottles.description"),
            Launcher::Faugus => t("manage.onboarding.faugus.description"),
            Launcher::Playnite => t("manage.onboarding.playnite.description"),
        }
    }

    /// Whether it has data on this computer. Reads the disk (and Windows'
    /// registry for Playnite): call it off the UI thread.
    fn detected(self) -> bool {
        match self {
            Launcher::Lutris => hooks::lutris_detected(),
            Launcher::Bottles => hooks::bottles_detected(),
            Launcher::Faugus => hooks::faugus_detected(),
            Launcher::Playnite => irori_extras::playnite::is_installed(),
        }
    }

    fn import(self, cx: &mut App) -> Task<anyhow::Result<LutrisSummary>> {
        match self {
            Launcher::Lutris => hooks::import_from_lutris(cx),
            Launcher::Bottles => hooks::import_from_bottles(cx),
            Launcher::Faugus => hooks::import_from_faugus(cx),
            Launcher::Playnite => hooks::import_from_playnite(cx),
        }
    }
}

/// The launchers with data on this computer, in [`Launcher::ALL`]'s order.
/// Reads the disk: call it off the UI thread.
pub fn detect() -> Vec<Launcher> {
    Launcher::ALL.into_iter().filter(|l| l.detected()).collect()
}

/// Find the launchers off the UI thread.
pub fn detect_in_background(cx: &mut App) -> Task<anyhow::Result<Vec<Launcher>>> {
    let b = irori_ui::backend(cx);
    irori_ui::run(cx, async move { Ok(b.spawn_blocking(detect).await?) })
}

/// Where one import stands.
#[derive(Debug, Clone, Default)]
pub enum ImportState {
    #[default]
    Idle,
    Running,
    Done(LutrisSummary),
    Failed(SharedString),
}

/// One import per launcher found, run from any screen that lists them.
#[derive(Debug, Clone, Default)]
pub struct Imports {
    pub entries: Vec<(Launcher, ImportState)>,
}

impl Imports {
    pub fn new(found: &[Launcher]) -> Self {
        Self {
            entries: found.iter().map(|l| (*l, ImportState::Idle)).collect(),
        }
    }

    pub fn state(&self, launcher: Launcher) -> Option<&ImportState> {
        self.entries
            .iter()
            .find(|(l, _)| *l == launcher)
            .map(|(_, s)| s)
    }

    /// Mark `launcher` as importing and start it; `None` when it already
    /// runs or isn't listed.
    pub fn start(
        &mut self,
        launcher: Launcher,
        cx: &mut App,
    ) -> Option<Task<anyhow::Result<LutrisSummary>>> {
        let (_, state) = self.entries.iter_mut().find(|(l, _)| *l == launcher)?;
        if matches!(state, ImportState::Running) {
            return None;
        }
        *state = ImportState::Running;
        Some(launcher.import(cx))
    }

    /// Record how the import of `launcher` ended.
    pub fn finish(&mut self, launcher: Launcher, result: anyhow::Result<LutrisSummary>) {
        if let Some((_, state)) = self.entries.iter_mut().find(|(l, _)| *l == launcher) {
            *state = match result {
                Ok(summary) => ImportState::Done(summary),
                Err(e) => ImportState::Failed(format!("{e:#}").into()),
            };
        }
    }
}

/// The button, or the result, of one import. `run` starts it.
pub fn import_status(
    launcher: Launcher,
    state: &ImportState,
    run: impl Fn(&mut gpui_kit::Window, &mut App) + 'static,
) -> AnyElement {
    let id = launcher.id();
    let name = launcher.name();
    match state {
        ImportState::Idle | ImportState::Running => {
            Button::new(SharedString::from(format!("import-{id}")))
                .primary()
                .icon(IconName::Import)
                .label(t_args(
                    "manage.onboarding.launcher.run",
                    &[("launcher", name)],
                ))
                .loading(matches!(state, ImportState::Running))
                .on_click(move |_, window, cx| run(window, cx))
                .into_any_element()
        }
        ImportState::Done(summary) => {
            let summary = summary.clone();
            v_flex()
                .gap_2()
                .child(
                    Alert::success(
                        SharedString::from(format!("import-{id}-done")),
                        irori_ui::lutris_summary::summary_lines(&summary).join("\n"),
                    )
                    .title(t_args(
                        "manage.onboarding.launcher.done",
                        &[("launcher", name)],
                    )),
                )
                .child(
                    div().child(
                        Button::new(SharedString::from(format!("import-{id}-details")))
                            .label(t("manage.onboarding.lutris.details"))
                            .on_click(move |_, window, cx| {
                                irori_ui::lutris_summary::open_lutris_summary(
                                    summary.clone(),
                                    window,
                                    cx,
                                )
                            }),
                    ),
                )
                .into_any_element()
        }
        ImportState::Failed(message) => v_flex()
            .gap_2()
            .child(
                Alert::error(
                    SharedString::from(format!("import-{id}-failed")),
                    message.clone(),
                )
                .title(t_args(
                    "manage.onboarding.launcher.failed",
                    &[("launcher", name)],
                )),
            )
            .child(
                div().child(
                    Button::new(SharedString::from(format!("import-{id}-retry")))
                        .label(t("manage.onboarding.import.retry"))
                        .on_click(move |_, window, cx| run(window, cx)),
                ),
            )
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_runs_once_at_a_time_and_keeps_its_result() {
        let mut imports = Imports::new(&[Launcher::Bottles, Launcher::Faugus]);
        assert!(matches!(
            imports.state(Launcher::Bottles),
            Some(ImportState::Idle)
        ));
        assert!(imports.state(Launcher::Lutris).is_none());
        if let Some((_, s)) = imports
            .entries
            .iter_mut()
            .find(|(l, _)| *l == Launcher::Bottles)
        {
            *s = ImportState::Running;
        }
        imports.finish(
            Launcher::Bottles,
            Ok(LutrisSummary {
                imported: 2,
                ..Default::default()
            }),
        );
        assert!(matches!(
            imports.state(Launcher::Bottles),
            Some(ImportState::Done(s)) if s.imported == 2
        ));
        imports.finish(Launcher::Faugus, Err(anyhow::anyhow!("no games.json")));
        assert!(matches!(
            imports.state(Launcher::Faugus),
            Some(ImportState::Failed(m)) if m.contains("games.json")
        ));
    }
}
