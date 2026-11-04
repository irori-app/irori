//! The game page's Mods tab: the game's mods in load order, where they come
//! from, which files they fight over, and installing more from a file,
//! Thunderstore or Nexus Mods. The work happens in `irori_services::mods`.

mod details;
mod fomod;
mod list;
mod nexus;
mod thunderstore;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Render, SharedString, Size, Styled, Task, Window, div, px,
};
use irori_core::{Event, GameId, GameInfo};
use irori_mods::DeployMethod;
use irori_mods::thunderstore::Community;
use irori_services::{InstallOutcome, ModProgress, ModsView};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::load::Load;
use crate::ops::{notify_error, notify_success, open_path, pick_path, run_reporting};

pub use nexus::open_nxm_dialog;

/// Whether the game gets a Mods tab.
pub(crate) fn applies(game: &GameInfo) -> bool {
    irori_services::supports_mods(game)
}

/// An install in progress, shown above the list.
#[derive(Clone)]
pub(crate) struct Busy {
    label: SharedString,
    progress: Option<ModProgress>,
}

/// The Mods tab of one game.
pub struct ModsPanel {
    id: GameId,
    view: Load<Arc<ModsView>>,
    /// The mod whose details show under the list.
    selected: Option<String>,
    busy: Option<Busy>,
    community: Load<Option<Community>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
    _events: Task<()>,
    /// Redraws while a download runs, so its bar moves.
    _ticker: Option<Task<()>>,
}

impl ModsPanel {
    pub fn new(game: Arc<GameInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = game.id();
        let mut rx = backend(cx).subscribe();
        let watched = id.clone();
        let events = cx.spawn_in(window, async move |this, cx| {
            loop {
                match rx.recv().await {
                    Ok(Event::ModsChanged(changed)) if changed == watched => {}
                    Ok(Event::GameExited { id, .. }) if id == watched => {}
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
                if this
                    .update_in(cx, |this, window, cx| this.reload(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            id,
            view: Load::Idle,
            selected: None,
            busy: None,
            community: Load::Idle,
            sizes: Rc::new(Vec::new()),
            sized_for: (usize::MAX, px(0.)),
            _events: events,
            _ticker: None,
        };
        this.reload(window, cx);
        this.load_community(window, cx);
        this
    }

    pub(crate) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.ready().is_none() {
            self.view = Load::Loading;
        }
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move { b.mods(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(view) => {
                        if this
                            .selected
                            .as_ref()
                            .is_some_and(|s| view.list.get(s).is_none())
                        {
                            this.selected = None;
                        }
                        this.view = Load::Ready(Arc::new(view));
                    }
                    Err(e) => this.view = Load::Failed(format!("{e:#}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_community(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.community = Load::Loading;
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(cx, async move { b.mod_community(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.community = Load::from_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn running(&self, cx: &App) -> bool {
        backend(cx).is_running(&self.id)
    }

    /// Run a change to the mods and report failure; the bus reloads.
    pub(crate) fn change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        failed: SharedString,
        f: impl FnOnce(
            irori_services::Backend,
            GameId,
        ) -> futures::future::BoxFuture<'static, anyhow::Result<()>>,
    ) {
        let fut = f(backend(cx), self.id.clone());
        let task = irori_ui::run(cx, fut);
        cx.spawn_in(window, async move |this, cx| {
            if let Err(e) = task.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    notify_error(window, cx, failed, &e);
                    this.reload(window, cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn set_enabled(
        &mut self,
        mod_id: String,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_local(cx, |v| {
            if let Some(m) = v.list.get_mut(&mod_id) {
                m.enabled = on;
            }
        });
        self.change(window, cx, t("game.mods.change_failed"), move |b, id| {
            Box::pin(async move { b.set_mod_enabled(&id, &mod_id, on).await })
        });
    }

    /// Move `mod_id` to position `to` in the load order.
    pub(crate) fn move_mod(
        &mut self,
        mod_id: &str,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut moved = false;
        self.edit_local(cx, |v| moved = v.list.move_to(mod_id, to));
        if !moved {
            return;
        }
        let Some(order) = self
            .view
            .ready()
            .map(|v| v.list.mods.iter().map(|m| m.id.clone()).collect::<Vec<_>>())
        else {
            return;
        };
        self.change(window, cx, t("game.mods.change_failed"), move |b, id| {
            Box::pin(async move { b.reorder_mods(&id, order).await })
        });
    }

    /// Apply a change to the shown list right away; the backend's answer
    /// replaces it.
    fn edit_local(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ModsView)) {
        if let Load::Ready(view) = &mut self.view {
            f(Arc::make_mut(view));
            cx.notify();
        }
    }

    pub(crate) fn confirm_remove(
        &mut self,
        mod_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .view
            .ready()
            .and_then(|v| v.list.get(&mod_id))
            .map(|m| m.name.clone())
        else {
            return;
        };
        let this = cx.entity();
        let title = t_args("game.mods.remove_title", &[("name", &name)]);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, mod_id) = (this.clone(), mod_id.clone());
            alert
                .title(title.clone())
                .description(t("game.mods.remove_description"))
                .confirm()
                .ok_text(t("game.mods.remove"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let mod_id = mod_id.clone();
                    this.update(cx, |p, cx| {
                        p.change(window, cx, t("game.mods.remove_failed"), move |b, id| {
                            Box::pin(async move { b.remove_mod(&id, &mod_id).await })
                        })
                    });
                    true
                })
        });
    }

    fn confirm_clear_written(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(t("game.mods.clear_written_title"))
                .description(t("game.mods.clear_written_description"))
                .confirm()
                .ok_text(t("game.mods.clear_written"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    this.update(cx, |p, cx| {
                        p.change(window, cx, t("game.mods.change_failed"), |b, id| {
                            Box::pin(async move { b.clear_written_mod_files(&id).await })
                        })
                    });
                    true
                })
        });
    }

    fn set_method(&mut self, links: bool, window: &mut Window, cx: &mut Context<Self>) {
        let method = if links {
            DeployMethod::Links
        } else {
            DeployMethod::Auto
        };
        self.edit_local(cx, |v| v.list.method = method);
        self.change(window, cx, t("game.mods.change_failed"), move |b, id| {
            Box::pin(async move { b.set_mod_method(&id, method).await })
        });
    }

    /// Show `busy` until `task` ends, then report how it went.
    pub(crate) fn run_install(
        &mut self,
        busy: Busy,
        task: Task<anyhow::Result<InstallOutcome>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_busy(busy, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.stop_busy(cx);
                match result {
                    Ok(InstallOutcome::Installed { name }) => notify_success(
                        window,
                        cx,
                        t_args("game.mods.installed", &[("name", &name)]),
                    ),
                    Ok(InstallOutcome::Choose(prompt)) => {
                        fomod::open_fomod_dialog(prompt, window, cx)
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.install_failed"), &e),
                }
            });
        })
        .detach();
    }

    pub(crate) fn start_busy(&mut self, busy: Busy, window: &mut Window, cx: &mut Context<Self>) {
        let ticks = busy.progress.is_some();
        self.busy = Some(busy);
        if ticks {
            self._ticker = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    pub(crate) fn stop_busy(&mut self, cx: &mut Context<Self>) {
        self.busy = None;
        self._ticker = None;
        cx.notify();
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    fn add_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pick = pick_path(window, cx, false, t("game.mods.pick_archive"));
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = pick.await else { return };
            let _ = this.update_in(cx, |this, window, cx| this.install_file(path, window, cx));
        })
        .detach();
    }

    fn install_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let id = self.id.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let task = irori_ui::run(cx, async move { b.install_mod_archive(&id, path).await });
        let busy = Busy {
            label: t_args("game.mods.installing", &[("name", &name)]),
            progress: None,
        };
        self.run_install(busy, task, window, cx);
    }

    fn open_thunderstore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let community = self.community.ready().cloned().flatten();
        thunderstore::open_browser(cx.entity(), community, window, cx);
    }

    fn open_nexus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        nexus::open_nexus_dialog(cx.entity(), window, cx);
    }

    pub(crate) fn id(&self) -> &GameId {
        &self.id
    }

    pub(crate) fn view(&self) -> Option<&Arc<ModsView>> {
        self.view.ready()
    }

    pub(crate) fn community(&self) -> Option<&Community> {
        self.community.ready().and_then(Option::as_ref)
    }

    pub(crate) fn set_community(
        &mut self,
        community: Option<Community>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = community.as_ref().map(|c| c.id.clone());
        self.community = Load::Ready(community);
        cx.notify();
        self.change(window, cx, t("game.mods.change_failed"), move |b, id| {
            Box::pin(async move { b.set_mod_community(&id, choice).await })
        });
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.running(cx);
        let busy = self.is_busy();
        let theme = cx.theme();
        let (total, on) = self
            .view
            .ready()
            .map(|v| {
                (
                    v.list.mods.len(),
                    v.list.mods.iter().filter(|m| m.enabled).count(),
                )
            })
            .unwrap_or((0, 0));
        let summary = if total == 0 {
            t("game.mods.summary_none")
        } else {
            t_args(
                "game.mods.summary",
                &[("enabled", &on.to_string()), ("total", &total.to_string())],
            )
        };
        let community_name = self.community().map(|c| c.name.clone());
        let entity = cx.entity();
        let add = Button::new("mods-add")
            .primary()
            .small()
            .icon(IconName::Plus)
            .label(t("game.mods.add"))
            .disabled(running || busy)
            .dropdown_menu(move |menu, _, _| {
                let (file, store, nexus) = (entity.clone(), entity.clone(), entity.clone());
                menu.item(
                    PopupMenuItem::new(t("game.mods.add_file"))
                        .icon(IconName::FileArchive)
                        .on_click(move |_, window, cx| {
                            file.update(cx, |p, cx| p.add_from_file(window, cx))
                        }),
                )
                .item(
                    PopupMenuItem::new(match &community_name {
                        Some(name) => t_args("game.mods.add_thunderstore_for", &[("game", name)]),
                        None => t("game.mods.add_thunderstore"),
                    })
                    .icon(IconName::Globe)
                    .on_click(move |_, window, cx| {
                        store.update(cx, |p, cx| p.open_thunderstore(window, cx))
                    }),
                )
                .item(
                    PopupMenuItem::new(t("game.mods.add_nexus"))
                        .icon(IconName::Link)
                        .on_click(move |_, window, cx| {
                            nexus.update(cx, |p, cx| p.open_nexus(window, cx))
                        }),
                )
            });
        let folder = self.view.ready().map(|v| v.folder.clone());
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().text_lg().font_semibold().child(t("game.mods.title")))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(summary),
                            ),
                    )
                    .child(h_flex().gap_2().flex_shrink_0().child(add).when_some(
                        folder,
                        |row, dir| {
                            row.child(
                                Button::new("mods-folder")
                                    .ghost()
                                    .small()
                                    .icon(IconName::FolderOpen)
                                    .tooltip(t("game.mods.open_folder"))
                                    .on_click(move |_, window, cx| {
                                        let dir = dir.clone();
                                        let make = dir.clone();
                                        run_reporting(
                                            window,
                                            cx,
                                            t("game.ops.open_failed"),
                                            async move {
                                                tokio::fs::create_dir_all(&make).await?;
                                                Ok(())
                                            },
                                            move |(), window, cx| open_path(dir, window, cx),
                                        )
                                        .detach();
                                    }),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.help")),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        if let Some(busy) = &self.busy {
            let fraction = busy.progress.as_ref().and_then(|p| p.fraction());
            let done = busy
                .progress
                .as_ref()
                .map(|p| p.done.load(Ordering::Relaxed))
                .unwrap_or(0);
            return Some(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(gpui_kit::component::spinner::Spinner::new().small())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(busy.label.clone()),
                            )
                            .when(done > 0, |row| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(irori_core::util::format_bytes(done)),
                                )
                            }),
                    )
                    .when_some(fraction, |col, f| {
                        col.child(Progress::new("mods-progress").value(f * 100.))
                    })
                    .into_any_element(),
            );
        }
        if self.running(cx) && self.view.ready().is_some_and(|v| v.list.any_enabled()) {
            return Some(
                h_flex()
                    .gap_2()
                    .p_3()
                    .rounded(theme.radius_lg)
                    .bg(theme.secondary)
                    .items_center()
                    .child(
                        Icon::new(IconName::Play)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(t("game.mods.applied_while_playing")),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    fn render_options(&self, view: &ModsView, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let running = self.running(cx);
        let links = view.list.method == DeployMethod::Links;
        let method_text = if view.overlay_available && !links {
            t("game.mods.method_overlay")
        } else {
            t("game.mods.method_links")
        };
        v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        Icon::new(IconName::ShieldCheck)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(method_text),
                    ),
            )
            .when(view.overlay_available, |col| {
                col.child(
                    Switch::new("mods-links")
                        .small()
                        .label(t("game.mods.use_links"))
                        .checked(links)
                        .disabled(running)
                        .on_change(cx.listener(|this, on: &bool, window, cx| {
                            this.set_method(*on, window, cx)
                        })),
                )
            })
            .when(view.written_files > 0, |col| {
                col.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t_args(
                                    "game.mods.written_files",
                                    &[("count", &view.written_files.to_string())],
                                )),
                        )
                        .child(
                            Button::new("mods-clear-written")
                                .xsmall()
                                .ghost()
                                .label(t("game.mods.clear_written"))
                                .disabled(running)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_clear_written(window, cx)
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let community = self
            .community()
            .filter(|c| c.supported())
            .map(|c| c.name.clone());
        v_flex()
            .items_center()
            .gap_3()
            .py_8()
            .px_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_dashed()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Puzzle)
                    .large()
                    .text_color(theme.muted_foreground),
            )
            .child(div().font_medium().child(t("game.mods.empty_title")))
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.empty_body")),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .justify_center()
                    .when_some(community, |row, name| {
                        row.child(
                            Button::new("mods-empty-thunderstore")
                                .primary()
                                .small()
                                .icon(IconName::Globe)
                                .label(t_args("game.mods.browse_community", &[("game", &name)]))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_thunderstore(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("mods-empty-file")
                            .outline()
                            .small()
                            .icon(IconName::FileArchive)
                            .label(t("game.mods.add_file"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_from_file(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ModsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let status = self.render_status(cx);
        let body: AnyElement = match self.view.clone() {
            Load::Idle | Load::Loading => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t("game.mods.loading"))
                .into_any_element(),
            Load::Failed(e) => v_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t_args("game.mods.load_failed", &[("error", e.as_ref())])),
                )
                .child(
                    Button::new("mods-retry")
                        .small()
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label(t("game.common.retry"))
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .into_any_element(),
            Load::Ready(view) if view.list.mods.is_empty() => v_flex()
                .gap_4()
                .child(self.render_empty(cx))
                .child(self.render_options(&view, cx))
                .into_any_element(),
            Load::Ready(view) => {
                let list = self.render_list(&view, window, cx);
                let details = self.render_details(&view, window, cx);
                v_flex()
                    .gap_4()
                    .child(list)
                    .children(details)
                    .child(self.render_options(&view, cx))
                    .into_any_element()
            }
        };
        v_flex()
            .id("mods-panel")
            .w_full()
            .max_w(gpui_kit::rems(52.))
            .gap_4()
            .child(header)
            .children(status)
            .child(body)
    }
}

/// `one` or `other` for `count`, with `{count}` and `args` filled in.
pub(crate) fn counted(one: &str, other: &str, count: usize, args: &[(&str, &str)]) -> SharedString {
    let count_text = count.to_string();
    let mut all: Vec<(&str, &str)> = vec![("count", &count_text)];
    all.extend_from_slice(args);
    t_args(if count == 1 { one } else { other }, &all)
}

/// A short text for where a mod came from.
pub(crate) fn source_label(source: &irori_mods::ModSource) -> SharedString {
    match source {
        irori_mods::ModSource::Archive { .. } => t("game.mods.source_file"),
        irori_mods::ModSource::Thunderstore { .. } => "Thunderstore".into(),
        irori_mods::ModSource::Nexus { .. } => "Nexus Mods".into(),
    }
}

/// Open the Mods tab's panel for `game` in a sheet (from the game menu).
pub fn open_mods_sheet(game: Arc<GameInfo>, window: &mut Window, cx: &mut App) {
    let title: SharedString = t_args("game.mods.sheet_title", &[("title", &game.title)]);
    let panel: Entity<ModsPanel> = cx.new(|cx| ModsPanel::new(game, window, cx));
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title(title.clone())
            .size(gpui_kit::rems(40.))
            .child(div().p_1().child(panel.clone()))
    });
}
