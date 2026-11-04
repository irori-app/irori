//! Browsing a game's Thunderstore community and installing packages with
//! their dependencies. When Irori doesn't recognise the game, the user
//! picks its community from the list first.

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Task, Window, div, px, rems, size,
};
use irori_mods::ModSource;
use irori_mods::thunderstore::{Community, PackageSummary};
use irori_services::InstallOutcome;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use super::{Busy, ModsPanel};
use crate::load::Load;

const RESULT_ROW_REMS: f32 = 4.5;
const COMMUNITY_ROW_REMS: f32 = 2.5;
/// Rows shown before a list scrolls.
const VISIBLE_RESULTS: usize = 8;
const VISIBLE_COMMUNITIES: usize = 12;
/// Wait this long after typing before searching.
const SEARCH_DELAY: Duration = Duration::from_millis(350);

/// Open the Thunderstore browser for the panel's game.
pub(super) fn open_browser(
    panel: Entity<ModsPanel>,
    community: Option<Community>,
    window: &mut Window,
    cx: &mut App,
) {
    let browser = cx.new(|cx| Browser::new(panel, community, window, cx));
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title("Thunderstore")
            .size(rems(34.))
            .child(browser.clone())
    });
}

struct Browser {
    panel: Entity<ModsPanel>,
    community: Option<Community>,
    /// Every community, for choosing one.
    communities: Load<Arc<Vec<Community>>>,
    query: Entity<InputState>,
    results: Load<Vec<PackageSummary>>,
    page: u32,
    has_more: bool,
    total: u64,
    loading_more: bool,
    search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Browser {
    fn new(
        panel: Entity<ModsPanel>,
        community: Option<Community>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("game.mods.ts_search")));
        let subscriptions = vec![cx.subscribe_in(
            &query,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::Change => this.schedule_search(window, cx),
                InputEvent::PressEnter { .. } => this.search_now(window, cx),
                _ => {}
            },
        )];
        let mut this = Self {
            panel,
            community,
            communities: Load::Idle,
            query,
            results: Load::Idle,
            page: 1,
            has_more: false,
            total: 0,
            loading_more: false,
            search: None,
            _subscriptions: subscriptions,
        };
        if this.community.is_some() {
            this.search_now(window, cx);
        } else {
            this.load_communities(window, cx);
        }
        this
    }

    fn load_communities(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.communities = Load::Loading;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.thunderstore_ecosystem().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.communities =
                    Load::from_result(result.map(|eco| Arc::new(eco.communities.clone())));
                cx.notify();
            });
        })
        .detach();
    }

    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.community.is_none() {
            cx.notify();
            return;
        }
        self.search = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let _ = this.update_in(cx, |this, window, cx| this.search_now(window, cx));
        }));
    }

    fn search_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = 1;
        self.results = Load::Loading;
        self.fetch(window, cx);
    }

    fn fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(community) = self.community.clone() else {
            return;
        };
        let query = self.query.read(cx).value().to_string();
        let page = self.page;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            b.search_thunderstore(&community.id, &query, page).await
        });
        self.search = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_more = false;
                match result {
                    Ok(found) => {
                        this.has_more = found.has_more;
                        this.total = found.total;
                        match (&mut this.results, page) {
                            (Load::Ready(list), p) if p > 1 => list.extend(found.results),
                            _ => this.results = Load::Ready(found.results),
                        }
                    }
                    Err(e) => this.results = Load::Failed(format!("{e:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page += 1;
        self.loading_more = true;
        self.fetch(window, cx);
    }

    fn choose(&mut self, community: Community, window: &mut Window, cx: &mut Context<Self>) {
        self.community = Some(community.clone());
        self.panel
            .update(cx, |p, cx| p.set_community(Some(community), window, cx));
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
        self.search_now(window, cx);
    }

    fn change_game(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.community = None;
        self.results = Load::Idle;
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
        if self.communities.ready().is_none() {
            self.load_communities(window, cx);
        }
        cx.notify();
    }

    fn install(&mut self, package: &PackageSummary, window: &mut Window, cx: &mut Context<Self>) {
        let Some(community) = self.community.clone() else {
            return;
        };
        let (namespace, name) = (package.namespace.clone(), package.name.clone());
        let label = t_args("game.mods.installing", &[("name", &package.display_name())]);
        self.panel.update(cx, |panel, cx| {
            let b = backend(cx);
            let id = panel.id().clone();
            let progress = irori_services::ModProgress::default();
            let busy = Busy {
                label,
                progress: Some(progress.clone()),
            };
            let task = irori_ui::run(cx, async move {
                let names = b
                    .install_thunderstore(&id, &community, &namespace, &name, progress)
                    .await?;
                Ok(InstallOutcome::Installed {
                    name: names.join(", "),
                })
            });
            panel.run_install(busy, task, window, cx);
        });
        cx.notify();
    }

    /// Lower-cased `Namespace-Name` of every installed package.
    fn installed(&self, cx: &App) -> HashSet<String> {
        self.panel
            .read(cx)
            .view()
            .map(|v| {
                v.list
                    .mods
                    .iter()
                    .filter_map(|m| match &m.source {
                        ModSource::Thunderstore {
                            namespace, name, ..
                        } => Some(format!("{namespace}-{name}").to_lowercase()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn render_results(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(results) = self.results.ready().cloned() else {
            return Vec::new();
        };
        let installed = self.installed(cx);
        let busy = self.panel.read(cx).is_busy() || self.panel.read(cx).running(cx);
        range
            .filter_map(|ix| {
                let p = results.get(ix)?.clone();
                let is_installed = installed.contains(&p.full_name().to_lowercase());
                let art = p
                    .icon_url
                    .as_deref()
                    .and_then(|u| irori_ui::components::art_img(u, 96, true, cx));
                let theme = cx.theme();
                let icon: AnyElement = match art {
                    Some(img) => img.size(rems(3.)).rounded(theme.radius).into_any_element(),
                    None => div()
                        .size(rems(3.))
                        .rounded(theme.radius)
                        .bg(theme.secondary)
                        .into_any_element(),
                };
                let meta = t_args(
                    "game.mods.ts_meta",
                    &[
                        ("author", &p.namespace),
                        ("downloads", &compact(p.download_count)),
                    ],
                );
                let package = p.clone();
                Some(
                    h_flex()
                        .id(ElementId::Name(format!("ts-{}", p.full_name()).into()))
                        .h(rems(RESULT_ROW_REMS))
                        .px_2()
                        .gap_3()
                        .items_center()
                        .when(ix > 0, |r| r.border_t_1().border_color(theme.border))
                        .child(div().flex_shrink_0().child(icon))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_sm()
                                                .font_semibold()
                                                .child(p.display_name()),
                                        )
                                        .when(p.is_deprecated, |r| {
                                            r.child(
                                                Tag::warning()
                                                    .xsmall()
                                                    .child(t("game.mods.ts_deprecated")),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(meta),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(p.description.clone()),
                                ),
                        )
                        .child(if is_installed {
                            Button::new(("ts-update", ix))
                                .small()
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .label(t("game.mods.ts_reinstall"))
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.install(&package, window, cx)
                                }))
                                .into_any_element()
                        } else {
                            Button::new(("ts-install", ix))
                                .small()
                                .outline()
                                .icon(IconName::Download)
                                .label(t("game.mods.install"))
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.install(&package, window, cx)
                                }))
                                .into_any_element()
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn matching_communities(&self, cx: &App) -> Vec<Community> {
        let query = self.query.read(cx).value().to_lowercase();
        self.communities
            .ready()
            .map(|list| {
                list.iter()
                    .filter(|c| c.supported())
                    .filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn render_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let found = Rc::new(self.matching_communities(cx));
        let body: AnyElement = match &self.communities {
            Load::Ready(_) if found.is_empty() => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t("game.mods.ts_no_community"))
                .into_any_element(),
            Load::Ready(_) => {
                let shown = found.len().min(VISIBLE_COMMUNITIES) as f32;
                let row_h = window.rem_size() * COMMUNITY_ROW_REMS;
                let sizes = Rc::new(vec![size(px(0.), row_h); found.len()]);
                let rows = found.clone();
                v_virtual_list(
                    cx.entity(),
                    "ts-communities",
                    sizes,
                    move |_, range, _, cx| {
                        let theme = cx.theme();
                        range
                            .filter_map(|ix| {
                                let c = rows.get(ix)?.clone();
                                Some(
                                    h_flex()
                                        .id(("ts-community", ix))
                                        .h(rems(COMMUNITY_ROW_REMS))
                                        .px_3()
                                        .items_center()
                                        .rounded(theme.radius)
                                        .hover(|s| s.bg(theme.muted))
                                        .cursor_pointer()
                                        .child(div().truncate().text_sm().child(c.name.clone()))
                                        .on_click(cx.listener(
                                            move |this: &mut Self, _, window, cx| {
                                                this.choose(c.clone(), window, cx)
                                            },
                                        ))
                                        .into_any_element(),
                                )
                            })
                            .collect()
                    },
                )
                .w_full()
                .h(rems(COMMUNITY_ROW_REMS * shown))
                .into_any_element()
            }
            Load::Failed(e) => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t_args("game.mods.ts_failed", &[("error", e.as_ref())]))
                .into_any_element(),
            _ => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t("game.mods.loading"))
                .into_any_element(),
        };
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.ts_pick_community")),
            )
            .child(Input::new(&self.query).prefix(Icon::new(IconName::Search).small()))
            .child(body)
            .into_any_element()
    }
}

/// `1234567` → `1.2M`.
fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(community) = self.community.clone() else {
            return div().child(self.render_picker(window, cx));
        };
        let theme = cx.theme();
        let header =
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_semibold()
                                .truncate()
                                .child(community.name.clone()),
                        )
                        .when(self.total > 0, |col| {
                            col.child(div().text_xs().text_color(theme.muted_foreground).child(
                                t_args("game.mods.ts_count", &[("count", &self.total.to_string())]),
                            ))
                        }),
                )
                .child(
                    Button::new("ts-change")
                        .xsmall()
                        .ghost()
                        .label(t("game.mods.ts_change_game"))
                        .on_click(cx.listener(|this, _, window, cx| this.change_game(window, cx))),
                );
        let body: AnyElement = if !community.supported() {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t_args(
                    "game.mods.ts_unsupported",
                    &[("game", &community.name), ("loader", &community.loader)],
                ))
                .into_any_element()
        } else {
            match &self.results {
                Load::Ready(list) if list.is_empty() => div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.ts_no_results"))
                    .into_any_element(),
                Load::Ready(list) => {
                    let row_h = window.rem_size() * RESULT_ROW_REMS;
                    let sizes = Rc::new(vec![size(px(0.), row_h); list.len()]);
                    let more = self.has_more.then(|| {
                        Button::new("ts-more")
                            .small()
                            .ghost()
                            .w_full()
                            .label(t("game.mods.ts_more"))
                            .loading(self.loading_more)
                            .disabled(self.loading_more)
                            .on_click(cx.listener(|this, _, window, cx| this.load_more(window, cx)))
                    });
                    let shown = list.len().min(VISIBLE_RESULTS) as f32;
                    v_flex()
                        .gap_2()
                        .child(
                            div().w_full().h(rems(RESULT_ROW_REMS * shown)).child(
                                v_virtual_list(
                                    cx.entity(),
                                    "ts-results",
                                    sizes,
                                    |this, range, _, cx| this.render_results(range, cx),
                                )
                                .size_full(),
                            ),
                        )
                        .children(more)
                        .into_any_element()
                }
                Load::Failed(e) => v_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(t_args("game.mods.ts_failed", &[("error", e.as_ref())])),
                    )
                    .child(
                        Button::new("ts-retry")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("game.common.retry"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.search_now(window, cx)),
                            ),
                    )
                    .into_any_element(),
                _ => div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.loading"))
                    .into_any_element(),
            }
        };
        let note: SharedString = t("game.mods.ts_note");
        v_flex()
            .gap_3()
            .child(header)
            .child(Input::new(&self.query).prefix(Icon::new(IconName::Search).small()))
            .child(body)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note),
            )
    }
}
