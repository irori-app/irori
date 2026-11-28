//! "ROMs on this computer": the usual ROM folders Irori found (EmuDeck's,
//! RetroDECK's, a `ROMs` folder, at home and on other disks), offered a
//! system at a time. Nothing is added until the user says so; systems no
//! emulator here plays are named instead of offered.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, rems,
};
use irori_engines::systems;
use irori_services::{RomPlan, RomSource, RomSourceKind};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::common::{notify_error, notify_success};

/// What the search found, and which of its folders are ticked.
struct Found {
    sources: Vec<RomSource>,
    plan: RomPlan,
    picked: Vec<bool>,
}

pub(crate) struct RomSourcesDialog {
    found: Option<Found>,
    adding: bool,
}

/// Look for the usual ROM folders and offer the ones that play here.
pub fn open_rom_sources_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let dialog = cx.new(|cx| RomSourcesDialog::new(window, cx));
    let width = window.rem_size() * 36.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("manage.rom_sources.title"))
            .width(width)
            .child(dialog.clone())
    });
}

/// The name of who laid a ROM folder out.
pub(crate) fn source_name(kind: RomSourceKind) -> SharedString {
    match kind {
        RomSourceKind::EmuDeck => "EmuDeck".into(),
        RomSourceKind::RetroDeck => "RetroDECK".into(),
        RomSourceKind::Exo => t("manage.rom_sources.exo"),
        RomSourceKind::Folder => t("manage.rom_sources.folder"),
    }
}

impl RomSourcesDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.rom_sources().await) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((sources, plan)) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.found = Some(Found {
                    picked: vec![true; plan.add.len()],
                    sources,
                    plan,
                });
                cx.notify();
            });
        })
        .detach();
        Self {
            found: None,
            adding: false,
        }
    }

    fn picked(&self) -> Vec<PathBuf> {
        self.found
            .as_ref()
            .map(|f| {
                f.plan
                    .add
                    .iter()
                    .zip(&f.picked)
                    .filter(|(_, on)| **on)
                    .map(|((_, folder), _)| folder.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders = self.picked();
        if folders.is_empty() || self.adding {
            return;
        }
        self.adding = true;
        cx.notify();
        let count = folders.len();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.add_rom_folders(folders).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.adding = false;
                match result {
                    Ok(()) => {
                        window.close_dialog(cx);
                        notify_success(
                            if count == 1 {
                                t("manage.rom_sources.added_one")
                            } else {
                                t_args(
                                    "manage.rom_sources.added_other",
                                    &[("count", &count.to_string())],
                                )
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => notify_error(t("manage.rom_sources.add_failed"), &e, window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for RomSourcesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(found) = &self.found else {
            return v_flex()
                .py_6()
                .gap_3()
                .items_center()
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.rom_sources.looking")),
                );
        };
        let picked = found.picked.iter().filter(|on| **on).count();
        let mut list = v_flex().gap_3();
        for source in &found.sources {
            let rows: Vec<_> = found
                .plan
                .add
                .iter()
                .enumerate()
                .filter(|(_, (_, folder))| folder.starts_with(&source.root))
                .map(|(ix, (system, folder))| {
                    let name = systems::get(system).map_or(system.clone(), |s| s.name.clone());
                    let folder_name = folder
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    h_flex()
                        .gap_2()
                        .child(
                            Checkbox::new(SharedString::from(format!("rom-source-{ix}")))
                                .label(name)
                                .checked(found.picked.get(ix).copied().unwrap_or(false))
                                .disabled(self.adding)
                                .on_change(cx.listener(move |this, on: &bool, _, cx| {
                                    if let Some(f) = this.found.as_mut()
                                        && let Some(p) = f.picked.get_mut(ix)
                                    {
                                        *p = *on;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(folder_name),
                        )
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            list = list.child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_medium().truncate().child(t_args(
                        "manage.rom_sources.source",
                        &[
                            ("source", &source_name(source.kind)),
                            ("folder", &source.root.display().to_string()),
                        ],
                    )))
                    .children(rows),
            );
        }
        let nothing = found.plan.add.is_empty();
        v_flex()
            .gap_3()
            .child(div().text_sm().child(if found.sources.is_empty() {
                t("manage.rom_sources.none")
            } else if nothing {
                t("manage.rom_sources.nothing_new")
            } else {
                t("manage.rom_sources.intro")
            }))
            .when(!nothing, |d| {
                d.child(
                    div()
                        .id("rom-sources-list")
                        .max_h(rems(20.))
                        .overflow_y_scrollbar()
                        .child(list),
                )
            })
            .when(found.plan.already > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(if found.plan.already == 1 {
                            t("manage.rom_sources.already_one")
                        } else {
                            t_args(
                                "manage.rom_sources.already_other",
                                &[("count", &found.plan.already.to_string())],
                            )
                        }),
                )
            })
            .when(!found.plan.unplayable.is_empty(), |d| {
                d.child(Alert::info(
                    "rom-sources-unplayable",
                    t_args(
                        "manage.rom_sources.unplayable",
                        &[("systems", &found.plan.unplayable.join(", "))],
                    ),
                ))
            })
            .child(
                DialogFooter::new()
                    .child(
                        Button::new("rom-sources-cancel")
                            .ghost()
                            .label(if nothing {
                                t("manage.rom_sources.close")
                            } else {
                                t("manage.rom_sources.cancel")
                            })
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .when(!nothing, |d| {
                        d.child(
                            Button::new("rom-sources-add")
                                .primary()
                                .icon(IconName::FolderPlus)
                                .label(if picked == 1 {
                                    t("manage.rom_sources.add_one")
                                } else {
                                    t_args(
                                        "manage.rom_sources.add_other",
                                        &[("count", &picked.to_string())],
                                    )
                                })
                                .loading(self.adding)
                                .disabled(picked == 0 || self.adding)
                                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                        )
                    }),
            )
    }
}
