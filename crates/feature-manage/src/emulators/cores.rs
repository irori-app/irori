//! Dedicated modal for browsing, searching, installing, and removing
//! RetroArch cores without stretching the RetroArch card in the grid.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    div, rems,
};
use irori_engines::systems;
use irori_services::EngineStatus;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::ops::{EngineOps, cancel_install, core_key, core_name, engine_ops, start_install};
use super::{InstallTarget, PROGRESS_TICK, render_op};
use crate::common::notify_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreFilter {
    All,
    Installed,
    Available,
}

/// Open the RetroArch cores management modal.
pub(crate) fn open_cores_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let dialog = cx.new(|cx| CoresDialog::new(window, cx));
    let width = window.rem_size() * 42.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("manage.emulators.cores.dialog_title"))
            .width(width)
            .child(dialog.clone())
    });
}

struct CoresDialog {
    status: Option<EngineStatus>,
    search: Entity<InputState>,
    filter: CoreFilter,
    ops: Entity<EngineOps>,
    seen_revision: u64,
    ticking: bool,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CoresDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("manage.emulators.cores.search")));
        let ops = engine_ops(cx);
        let settings = models(cx).settings.clone();
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_in(&ops, window, |this, ops, window, cx| {
                let revision = ops.read(cx).revision;
                if revision != this.seen_revision {
                    this.seen_revision = revision;
                    this.load(window, cx);
                }
                this.ensure_ticker(window, cx);
                cx.notify();
            }),
            cx.observe_in(&settings, window, |this, _, window, cx| {
                this.load(window, cx)
            }),
        ];
        let seen_revision = ops.read(cx).revision;
        let mut this = Self {
            status: None,
            search,
            filter: CoreFilter::All,
            ops,
            seen_revision,
            ticking: false,
            _ticker: None,
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this.ensure_ticker(window, cx);
        this
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let statuses = b.engine_statuses().await;
            Ok(statuses.into_iter().find(|s| s.manifest.id == "retroarch"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(status) = task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticking || self.ops.read(cx).running.is_empty() {
            return;
        }
        self.ticking = true;
        self._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_TICK).await;
                let Ok(running) = this.update(cx, |this, cx| {
                    cx.notify();
                    !this.ops.read(cx).running.is_empty()
                }) else {
                    return;
                };
                if !running {
                    let _ = this.update(cx, |this, _| this.ticking = false);
                    return;
                }
            }
        }));
    }

    fn remove_core(&mut self, core: String, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let name = core_name(&core);
        let task = irori_ui::run(cx, async move { b.remove_core(&core).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    notify_error(
                        t_args("manage.emulators.core.remove_failed", &[("core", &name)]),
                        &e,
                        window,
                        cx,
                    );
                }
                this.load(window, cx);
            });
        })
        .detach();
    }

    fn pill(
        &self,
        id: &'static str,
        label: SharedString,
        count: usize,
        value: CoreFilter,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.filter == value;
        Button::new(id)
            .small()
            .label(format!("{label} ({count})"))
            .map(|b| if active { b.primary() } else { b.outline() })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.filter = value;
                cx.notify();
            }))
            .into_any_element()
    }
}

impl Render for CoresDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, background, radius) = (
            theme.muted_foreground,
            theme.border,
            theme.background,
            theme.radius,
        );

        let body: AnyElement = match &self.status {
            None => h_flex()
                .gap_2()
                .py_6()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.emulators.loading"))
                .into_any_element(),
            Some(status) => {
                let m = &status.manifest;
                let total = m.cores.len();
                let installed_count = m.cores.iter().filter(|c| status.has_core(&c.id)).count();
                let available_count = total.saturating_sub(installed_count);
                let query = self.search.read(cx).value().trim().to_lowercase();
                let running = self.ops.read(cx);

                let rows: Vec<AnyElement> = m
                    .cores
                    .iter()
                    .filter(|core| {
                        let installed = status.has_core(&core.id);
                        let matches_filter = match self.filter {
                            CoreFilter::All => true,
                            CoreFilter::Installed => installed,
                            CoreFilter::Available => !installed,
                        };
                        let matches_query = query.is_empty()
                            || core.name.to_lowercase().contains(&query)
                            || core.systems.iter().any(|id| {
                                systems::get(id).is_some_and(|sys| {
                                    sys.name.to_lowercase().contains(&query)
                                        || sys.short.to_lowercase().contains(&query)
                                })
                            });
                        matches_filter && matches_query
                    })
                    .map(|core| {
                        let key = core_key(&core.id);
                        let op = running.running.get(&key).map(|o| o.snapshot());
                        let installed = status.has_core(&core.id);
                        let systems_text = core
                            .systems
                            .iter()
                            .filter_map(|s| systems::get(s).map(|s| s.short.clone()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let action: AnyElement = match (&op, installed) {
                            (Some(_), _) => {
                                let k = key.clone();
                                Button::new("cancel-core")
                                    .ghost()
                                    .small()
                                    .label(t("manage.emulators.cancel"))
                                    .on_click(move |_, _, cx| cancel_install(&k, cx))
                                    .into_any_element()
                            }
                            (None, true) => {
                                let id = core.id.clone();
                                Button::new("remove-core")
                                    .ghost()
                                    .small()
                                    .label(t("manage.emulators.core.remove"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_core(id.clone(), window, cx)
                                    }))
                                    .into_any_element()
                            }
                            (None, false) => {
                                let id = core.id.clone();
                                Button::new("install-core")
                                    .primary()
                                    .small()
                                    .icon(IconName::Download)
                                    .label(t("manage.emulators.core.install"))
                                    .on_click(move |_, _, cx| {
                                        start_install(InstallTarget::Core { id: id.clone() }, cx)
                                    })
                                    .into_any_element()
                            }
                        };
                        h_flex()
                            .id(ElementId::Name(format!("core-{}", core.id).into()))
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(radius)
                            .bg(background)
                            .border_1()
                            .border_color(border)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_medium()
                                                    .truncate()
                                                    .child(core.name.clone()),
                                            )
                                            .when(installed, |d| {
                                                d.child(
                                                    Tag::success()
                                                        .xsmall()
                                                        .child(t("manage.emulators.tag.installed")),
                                                )
                                            }),
                                    )
                                    .child(div().text_xs().text_color(muted).truncate().child(
                                        t_args(
                                            "manage.emulators.core.plays",
                                            &[
                                                ("systems", &systems_text),
                                                ("license", &core.license),
                                            ],
                                        ),
                                    )),
                            )
                            .when_some(op, |d, live| d.child(render_op(&live, cx)))
                            .child(div().flex_shrink_0().child(action))
                            .into_any_element()
                    })
                    .collect();

                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .flex_wrap()
                                    .child(self.pill(
                                        "cores-filter-all",
                                        t("manage.emulators.filter.all"),
                                        total,
                                        CoreFilter::All,
                                        cx,
                                    ))
                                    .child(self.pill(
                                        "cores-filter-installed",
                                        t("manage.emulators.filter.installed"),
                                        installed_count,
                                        CoreFilter::Installed,
                                        cx,
                                    ))
                                    .child(self.pill(
                                        "cores-filter-available",
                                        t("manage.emulators.filter.available"),
                                        available_count,
                                        CoreFilter::Available,
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .w(rems(13.))
                                    .flex_shrink_0()
                                    .child(Input::new(&self.search).small()),
                            ),
                    )
                    .child(
                        div()
                            .id("retroarch-cores-list")
                            .max_h(rems(26.))
                            .overflow_y_scrollbar()
                            .child(if rows.is_empty() {
                                div()
                                    .py_4()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(t("manage.emulators.no_match"))
                                    .into_any_element()
                            } else {
                                v_flex().gap_2().children(rows).into_any_element()
                            }),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .id("retroarch-cores-dialog")
            .debug_selector(|| "retroarch-cores-dialog".into())
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.emulators.cores.description")),
            )
            .child(body)
            .child(
                DialogFooter::new().child(
                    Button::new("cores-close")
                        .label(t("manage.emulators.close"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}
