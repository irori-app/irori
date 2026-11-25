//! Storefront hub ([`Route::Stores`](irori_ui::Route::Stores)): underlined tabs
//! for Deals (Free to claim, regional deals, global search), Stores (official
//! websites and sign-in), and Launchers (Battle.net, EA, Ubisoft, Rockstar, Xbox).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use irori_core::Runner;
use irori_ui::components::PageHeader;
use irori_ui::i18n::t;
use irori_ui::models;

use crate::accounts::{ACCOUNT_STORES, StoreCard, store_cards, store_icon};
use crate::common::{card, card_grid};
use crate::deals::DealsSection;
use crate::login::open_login_dialog;
use crate::store_window::open_store_window;

/// A store's website.
pub(crate) struct StoreSite {
    pub url: &'static str,
    pub blurb: SharedString,
}

pub(crate) fn store_site(runner: Runner) -> Option<StoreSite> {
    Some(match runner {
        Runner::Legendary => StoreSite {
            url: "https://store.epicgames.com/",
            blurb: t("manage.stores.blurb.epic"),
        },
        Runner::Gog => StoreSite {
            url: "https://www.gog.com/",
            blurb: t("manage.stores.blurb.gog"),
        },
        Runner::Nile => StoreSite {
            url: "https://gaming.amazon.com/",
            blurb: t("manage.stores.blurb.amazon"),
        },
        Runner::Zoom => StoreSite {
            url: "https://www.zoom-platform.com/",
            blurb: t("manage.stores.blurb.zoom"),
        },
        Runner::Itch => StoreSite {
            url: "https://itch.io/",
            blurb: t("manage.stores.blurb.itch"),
        },
        Runner::Xbox => StoreSite {
            url: "https://www.xbox.com/games/store",
            blurb: t("manage.stores.blurb.xbox"),
        },
        // Steam's games come from its own client and store opens in the browser.
        Runner::Steam | Runner::Sideload | Runner::Emulation | Runner::Launchers => {
            return None;
        }
    })
}

fn website_url_and_blurb(runner: Runner) -> Option<(&'static str, SharedString)> {
    if runner == Runner::Steam {
        return Some((
            "https://store.steampowered.com/",
            t("manage.stores.blurb.steam"),
        ));
    }
    store_site(runner).map(|s| (s.url, s.blurb))
}

/// Whether the user wants deals on the Stores page (`iroriDeals`).
fn deals_shown(cx: &gpui_kit::App) -> bool {
    models(cx).settings.read(cx).app.deals
}

/// Narrowest a store card gets before the grid drops a column, in rem.
const MIN_CARD_REM: f32 = 17.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoresTab {
    Deals,
    Stores,
    Launchers,
}

/// Store websites, launchers and deals under underlined tabs.
pub struct StoresView {
    tab: StoresTab,
    cards: Vec<StoreCard>,
    /// Width of the store cards' grid at the last layout.
    grid_width: Rc<Cell<Pixels>>,
    /// Built only while `iroriDeals` is on, so nothing is asked for while
    /// the user has turned deals off.
    deals: Option<Entity<DealsSection>>,
    launchers: Entity<crate::launchers::LaunchersSection>,
    _subscriptions: Vec<Subscription>,
}

impl StoresView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let m = models(cx).clone();
        let subscriptions = vec![
            cx.observe(&m.accounts, |this, _, cx| this.rebuild(cx)),
            cx.observe_in(&m.settings, window, |this, _, window, cx| {
                this.rebuild(cx);
                this.show_deals(window, cx);
            }),
        ];
        let deals = deals_shown(cx).then(|| cx.new(|cx| DealsSection::new(window, cx)));
        let launchers = cx.new(|cx| crate::launchers::LaunchersSection::new(window, cx));
        let tab = if deals.is_some() {
            StoresTab::Deals
        } else {
            StoresTab::Stores
        };
        Self {
            tab,
            cards: store_cards(cx),
            grid_width: Rc::new(Cell::new(px(0.))),
            deals,
            launchers,
            _subscriptions: subscriptions,
        }
    }

    /// Builds or drops the deals as `iroriDeals` changes.
    fn show_deals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (deals_shown(cx), self.deals.is_some()) {
            (true, false) => {
                self.deals = Some(cx.new(|cx| DealsSection::new(window, cx)));
                cx.notify();
            }
            (false, true) => {
                self.deals = None;
                if self.tab == StoresTab::Deals {
                    self.tab = StoresTab::Stores;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let cards = store_cards(cx);
        if cards != self.cards {
            self.cards = cards;
            cx.notify();
        }
    }

    fn tabs(&self) -> Vec<StoresTab> {
        let mut tabs = Vec::with_capacity(3);
        if self.deals.is_some() {
            tabs.push(StoresTab::Deals);
        }
        tabs.push(StoresTab::Stores);
        tabs.push(StoresTab::Launchers);
        tabs
    }

    fn render_store(&self, data: &StoreCard, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (url, blurb) = website_url_and_blurb(data.runner)?;
        let theme = cx.theme();
        let runner = data.runner;
        let is_steam = runner == Runner::Steam;
        let connected = data.user.is_some() || data.steam_client.is_some();
        let can_login = data.user.is_none() && !data.disabled;

        Some(
            card(cx)
                .id(ElementId::Name(format!("store-{}", runner.id()).into()))
                .debug_selector(move || format!("store-{}", runner.id()))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(store_icon(runner).text_color(theme.muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_semibold()
                                .truncate()
                                .child(runner.store_name()),
                        )
                        .when(connected, |hdr| {
                            hdr.child(Tag::success().xsmall().child(t("manage.stores.connected")))
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(blurb),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("open-store")
                                .small()
                                .outline()
                                .icon(IconName::Store)
                                .label(t("manage.stores.open_store"))
                                .on_click(move |_, window, cx| {
                                    if is_steam {
                                        cx.open_url(url);
                                    } else {
                                        open_store_window(runner, None, window, cx);
                                    }
                                }),
                        )
                        .child(
                            Button::new("open-browser")
                                .ghost()
                                .small()
                                .icon(IconName::ExternalLink)
                                .accessibility_label(t("manage.stores.open_browser"))
                                .tooltip(t("manage.stores.open_browser"))
                                .on_click(move |_, _, cx| cx.open_url(url)),
                        )
                        .child(div().flex_1())
                        .when(can_login, |row| {
                            row.child(
                                Button::new("login")
                                    .small()
                                    .label(t("manage.stores.login"))
                                    .on_click(move |_, window, cx| {
                                        open_login_dialog(runner, window, cx);
                                    }),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

impl Render for StoresView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs();
        let selected_ix = tabs.iter().position(|&t| t == self.tab).unwrap_or(0);
        let tabs_for_click = tabs.clone();
        let body: AnyElement = match self.tab {
            StoresTab::Deals => match &self.deals {
                Some(deals) => deals.clone().into_any_element(),
                None => div().into_any_element(),
            },
            StoresTab::Stores => {
                let mut sorted: Vec<StoreCard> = self
                    .cards
                    .iter()
                    .filter(|c| ACCOUNT_STORES.contains(&c.runner) && !c.hidden)
                    .cloned()
                    .collect();
                // Show Steam first among storefronts for fast access.
                sorted.sort_by_key(|c| u8::from(c.runner != Runner::Steam));
                let cards: Vec<AnyElement> = sorted
                    .iter()
                    .filter_map(|c| self.render_store(c, cx))
                    .collect();
                card_grid(&self.grid_width, MIN_CARD_REM, cards, window).into_any_element()
            }
            StoresTab::Launchers => self.launchers.clone().into_any_element(),
        };

        v_flex()
            .id("stores-page")
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .px_6()
                    .py_6()
                    .gap_5()
                    .child(
                        PageHeader::new(t("manage.stores.title"))
                            .description(t("manage.stores.description")),
                    )
                    .child(
                        TabBar::new("stores-tabs")
                            .underline()
                            .large()
                            .selected_index(selected_ix)
                            .children(tabs.iter().map(|tab| {
                                Tab::new().label(match tab {
                                    StoresTab::Deals => t("manage.stores.tab.deals"),
                                    StoresTab::Stores => t("manage.stores.tab.stores"),
                                    StoresTab::Launchers => t("manage.stores.tab.launchers"),
                                })
                            }))
                            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                                if let Some(&next) = tabs_for_click.get(*ix) {
                                    this.tab = next;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(body),
            )
    }
}
