//! Stores -> Deals tab: Free to claim giveaways, regional country/currency
//! selection, global live search, Official vs Keyshops filtering, ProtonDB
//! compatibility badges with tooltips, and GG.deals price comparison.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::IndexPath;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{Select, SelectItem, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Global, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Task, Window,
    div, px, rems,
};
use irori_core::{DealsLayout, sort_key};
use irori_extras::anticheat;
use irori_extras::discounts::{
    self, CatalogLocale, CatalogProduct, DEALS_REGIONS, DealStore, DealsRegion, FreeGame,
    GgDealsPrice,
};
use irori_extras::wiki::{self, ProtonDbTier};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::{SectionTitle, art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{backend_op, card, card_grid, notify_error, notify_success, wrapped_tooltip};

/// Deals rendered per "Show more" step.
const PAGE: usize = 40;
/// Cap on lazy per-batch ProtonDB/GG.deals lookups per page.
const ENRICH_BATCH: usize = 36;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DealSort {
    #[default]
    Featured,
    Discount,
    LowestPrice,
    TitleAsc,
}

impl DealSort {
    const ALL: [DealSort; 4] = [
        DealSort::Featured,
        DealSort::Discount,
        DealSort::LowestPrice,
        DealSort::TitleAsc,
    ];

    fn id(self) -> &'static str {
        match self {
            DealSort::Featured => "featured",
            DealSort::Discount => "discount",
            DealSort::LowestPrice => "lowest_price",
            DealSort::TitleAsc => "title_asc",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "discount" => DealSort::Discount,
            "lowest_price" => DealSort::LowestPrice,
            "title_asc" => DealSort::TitleAsc,
            _ => DealSort::Featured,
        }
    }

    fn label(self) -> SharedString {
        match self {
            DealSort::Featured => t("manage.deals.sort.featured"),
            DealSort::Discount => t("manage.deals.sort.discount"),
            DealSort::LowestPrice => t("manage.deals.sort.lowest_price"),
            DealSort::TitleAsc => t("manage.deals.sort.title_asc"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ShopFilter {
    #[default]
    Official,
    All,
    Keyshops,
}

#[derive(Debug, Clone, PartialEq)]
struct SelectChoice {
    value: SharedString,
    title: SharedString,
}

impl SelectItem for SelectChoice {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &SharedString {
        &self.value
    }
}

#[derive(Clone, Default)]
pub(crate) enum DealsState {
    #[default]
    Idle,
    Loading,
    Loaded {
        free_games: Arc<Vec<FreeGame>>,
        products: Arc<Vec<CatalogProduct>>,
        failed: Vec<&'static str>,
    },
    Failed(SharedString),
    Unavailable,
}

#[derive(Clone, Default)]
enum SearchState {
    #[default]
    Idle,
    Loading(String),
    Loaded {
        results: Arc<Vec<CatalogProduct>>,
        failed: Vec<&'static str>,
    },
}

/// Session cache of deals, global search, ProtonDB tiers and GG.deals prices.
pub(crate) struct DealsCache {
    pub region_code: String,
    pub state: DealsState,
    search_state: SearchState,
    protondb: HashMap<String, ProtonDbTier>,
    protondb_requested: HashSet<String>,
    ggdeals: HashMap<String, GgDealsPrice>,
    ggdeals_requested: HashSet<String>,
    ggdeals_key_saved: bool,
    anticheat_warmed: bool,
}

impl Default for DealsCache {
    fn default() -> Self {
        Self {
            region_code: String::new(),
            state: DealsState::Idle,
            search_state: SearchState::Idle,
            protondb: HashMap::new(),
            protondb_requested: HashSet::new(),
            ggdeals: HashMap::new(),
            ggdeals_requested: HashSet::new(),
            ggdeals_key_saved: false,
            anticheat_warmed: false,
        }
    }
}

struct DealsCacheHandle(Entity<DealsCache>);
impl Global for DealsCacheHandle {}

fn cache(cx: &mut App) -> Entity<DealsCache> {
    if let Some(h) = cx.try_global::<DealsCacheHandle>() {
        return h.0.clone();
    }
    let entity = cx.new(|_| DealsCache::default());
    cx.set_global(DealsCacheHandle(entity.clone()));
    entity
}

fn active_region(cx: &App) -> &'static DealsRegion {
    let setting = &models(cx).settings.read(cx).app.deals_region;
    discounts::resolve_region(setting, irori_ui::age_rating::system_locale())
}

/// Interleave featured feeds round-robin so the first screen shows a mix of
/// Steam, GOG, GMG and Humble rather than hundreds from one store first.
pub(crate) fn interleave_featured(groups: Vec<Vec<CatalogProduct>>) -> Vec<CatalogProduct> {
    let max_len = groups.iter().map(Vec::len).max().unwrap_or(0);
    let total: usize = groups.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(total);
    let mut iters: Vec<_> = groups.into_iter().map(Vec::into_iter).collect();
    for _ in 0..max_len {
        for it in &mut iters {
            if let Some(item) = it.next() {
                out.push(item);
            }
        }
    }
    out
}

fn load(entity: Entity<DealsCache>, region: &'static DealsRegion, cx: &mut App) {
    let is_same_region = entity.read(cx).region_code == region.code;
    if is_same_region && matches!(entity.read(cx).state, DealsState::Loading) {
        return;
    }
    let warm_ac = cfg!(target_os = "linux") && !entity.read(cx).anticheat_warmed;
    entity.update(cx, |c, cx| {
        if c.region_code != region.code {
            c.ggdeals.clear();
            c.ggdeals_requested.clear();
            c.search_state = SearchState::Idle;
        }
        c.region_code = region.code.to_string();
        c.state = DealsState::Loading;
        if warm_ac {
            c.anticheat_warmed = true;
        }
        cx.notify();
    });
    let b = backend(cx);
    let task = irori_ui::run(cx, async move {
        let locale = CatalogLocale::from(region);
        let ac_warm = async {
            if warm_ac {
                let _ = anticheat::database().await;
            }
        };
        let (free_res, steam, gog, gmg, humble, key_saved, _) = futures::join!(
            discounts::free_games(region),
            discounts::steam_discounts(region),
            discounts::gog_discounts(&locale, None),
            discounts::gmg_discounts(Some(region.currency_code)),
            discounts::humble_discounts(),
            b.ggdeals_key_saved(),
            ac_warm,
        );
        Ok((free_res, steam, gog, gmg, humble, key_saved))
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        entity.update(cx, |c, cx| {
            // Ignore stale load if user switched regions while in flight.
            if c.region_code != region.code {
                return;
            }
            match result {
                Err(e) => c.state = DealsState::Failed(format!("{e:#}").into()),
                Ok(((free_list, free_failed), steam, gog, gmg, humble, key_saved)) => {
                    c.ggdeals_key_saved = key_saved;
                    let mut groups = Vec::new();
                    let mut failed = free_failed;
                    for (name, feed) in [
                        ("Steam", steam),
                        ("GOG", gog),
                        ("GreenManGaming", gmg),
                        ("Humble", humble),
                    ] {
                        match feed {
                            Ok(list) => groups.push(list),
                            Err(e) => {
                                tracing::warn!("deals from {name}: {e:#}");
                                failed.push(name);
                            }
                        }
                    }
                    let products = interleave_featured(groups);
                    c.state = if products.is_empty() && free_list.is_empty() && failed.len() >= 4 {
                        DealsState::Unavailable
                    } else {
                        DealsState::Loaded {
                            free_games: Arc::new(free_list),
                            products: Arc::new(products),
                            failed,
                        }
                    };
                }
            }
            cx.notify();
        });
    })
    .detach();
}

fn run_global_search(
    entity: Entity<DealsCache>,
    query: String,
    region: &'static DealsRegion,
    cx: &mut App,
) {
    let trimmed = query.trim().to_string();
    if trimmed.len() < 2 {
        entity.update(cx, |c, cx| {
            if !matches!(c.search_state, SearchState::Idle) {
                c.search_state = SearchState::Idle;
                cx.notify();
            }
        });
        return;
    }
    let cached_products = match &entity.read(cx).state {
        DealsState::Loaded { products, .. } => products.clone(),
        _ => Arc::default(),
    };
    entity.update(cx, |c, cx| {
        c.search_state = SearchState::Loading(trimmed.clone());
        cx.notify();
    });
    let q = trimmed.clone();
    let task = irori_ui::run(cx, async move {
        Ok(discounts::search_global(&q, region, &cached_products).await)
    });
    cx.spawn(async move |cx| {
        let res = task.await;
        entity.update(cx, |c, cx| {
            let still_current = match &c.search_state {
                SearchState::Loading(current) => current == &trimmed,
                _ => false,
            };
            if !still_current || c.region_code != region.code {
                return;
            }
            match res {
                Ok((results, failed)) => {
                    c.search_state = SearchState::Loaded {
                        results: Arc::new(results),
                        failed,
                    };
                }
                Err(e) => {
                    tracing::warn!("global deals search failed: {e:#}");
                    c.search_state = SearchState::Loaded {
                        results: Arc::default(),
                        failed: vec!["Search"],
                    };
                }
            }
            cx.notify();
        });
    })
    .detach();
}

fn enrich_visible(
    entity: Entity<DealsCache>,
    app_ids: Vec<String>,
    region: &'static DealsRegion,
    cx: &mut App,
) {
    let (missing_proton, missing_ggdeals) = entity.update(cx, |c, _| {
        let mut p_ids = Vec::new();
        let mut g_ids = Vec::new();
        for id in app_ids {
            if cfg!(target_os = "linux")
                && p_ids.len() < ENRICH_BATCH
                && c.protondb_requested.insert(id.clone())
            {
                p_ids.push(id.clone());
            }
            if c.ggdeals_key_saved
                && g_ids.len() < ENRICH_BATCH
                && c.ggdeals_requested.insert(id.clone())
            {
                g_ids.push(id);
            }
        }
        (p_ids, g_ids)
    });

    if !missing_proton.is_empty() {
        let target = entity.clone();
        let task = irori_ui::run(
            cx,
            async move { Ok(wiki::protondb_batch(missing_proton).await) },
        );
        cx.spawn(async move |cx| {
            if let Ok(tiers) = task.await
                && !tiers.is_empty()
            {
                target.update(cx, |c, cx| {
                    c.protondb.extend(tiers);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    if !missing_ggdeals.is_empty() {
        let b = backend(cx);
        let target = entity;
        let region_code = region.code;
        let task = irori_ui::run(cx, async move {
            let Some(key) = b.ggdeals_api_key().await else {
                return Ok(HashMap::new());
            };
            discounts::ggdeals_prices(&key, region, &missing_ggdeals).await
        });
        cx.spawn(async move |cx| {
            if let Ok(prices) = task.await
                && !prices.is_empty()
            {
                target.update(cx, |c, cx| {
                    if c.region_code == region_code {
                        c.ggdeals.extend(prices);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}

/// Show or hide the Stores page's deals (`iroriDeals`).
pub fn set_deals_shown(shown: bool, window: &mut Window, cx: &mut App) {
    crate::common::backend_op(cx, move |b| b.settings.update_app(|s| s.deals = shown));
    let message = if shown {
        t("manage.deals.shown_notice")
    } else {
        t("manage.deals.hidden_notice")
    };
    window.push_notification(Notification::info(message), cx);
}

/// Discount percentage from `"-70%"`.
pub(crate) fn discount_percent(p: &CatalogProduct) -> u32 {
    p.price
        .discount
        .trim()
        .trim_start_matches('-')
        .trim_end_matches('%')
        .trim()
        .parse()
        .unwrap_or(0)
}

fn final_amount(p: &CatalogProduct) -> f64 {
    p.price
        .final_money
        .as_ref()
        .and_then(|m| m.amount.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(f64::INFINITY)
}

/// Whether a deal relates to a library title: exact match or edition/DLC prefix match.
pub(crate) fn relates_to_library(title: &str, owned: &HashSet<String>) -> bool {
    let key = sort_key(title);
    if owned.contains(&key) {
        return true;
    }
    owned.iter().any(|o| {
        o.chars().count() >= 4
            && key.starts_with(o.as_str())
            && key[o.len()..].starts_with([' ', ':', '-', '—', '–'])
    })
}

pub(crate) fn is_owned_base_game(title: &str, owned: &HashSet<String>) -> bool {
    owned.contains(&sort_key(title))
}

pub(crate) fn has_divergent_currency(
    product: &CatalogProduct,
    region: &DealsRegion,
) -> Option<String> {
    let cur = product
        .price
        .final_money
        .as_ref()
        .map(|m| m.currency.as_str())
        .filter(|c| !c.is_empty())?;
    (!cur.eq_ignore_ascii_case(region.currency_code)).then(|| cur.to_uppercase())
}

fn matches_shop_filter(
    product: &CatalogProduct,
    shop: ShopFilter,
    ggdeals_key_saved: bool,
    ggdeals: &HashMap<String, GgDealsPrice>,
) -> bool {
    if !ggdeals_key_saved {
        return match shop {
            ShopFilter::All => true,
            ShopFilter::Official => matches!(product.store, DealStore::Steam | DealStore::Gog),
            ShopFilter::Keyshops => {
                matches!(
                    product.store,
                    DealStore::Gmg | DealStore::Humble | DealStore::Other
                )
            }
        };
    }
    match shop {
        ShopFilter::Official | ShopFilter::All => true,
        ShopFilter::Keyshops => product
            .steam_app_id
            .as_deref()
            .and_then(|id| ggdeals.get(id))
            .is_none_or(|p| p.keyshop.is_some()),
    }
}

pub(crate) struct DealQuery<'a> {
    pub store: Option<DealStore>,
    pub shop: ShopFilter,
    pub sort: DealSort,
    pub query: &'a str,
    pub global_search_active: bool,
    pub library_related: bool,
    pub hide_owned: bool,
    pub owned: &'a HashSet<String>,
    pub ggdeals_key_saved: bool,
    pub ggdeals: &'a HashMap<String, GgDealsPrice>,
}

pub(crate) fn filter_and_sort_deals(
    products: &[CatalogProduct],
    filter: &DealQuery<'_>,
) -> Vec<usize> {
    let q = filter.query.trim().to_lowercase();
    let mut ix: Vec<usize> = products
        .iter()
        .enumerate()
        .filter(|(_, p)| filter.store.is_none_or(|s| p.store == s))
        .filter(|(_, p)| {
            matches_shop_filter(p, filter.shop, filter.ggdeals_key_saved, filter.ggdeals)
        })
        .filter(|(_, p)| {
            filter.global_search_active || q.is_empty() || p.title.to_lowercase().contains(&q)
        })
        .filter(|(_, p)| !filter.library_related || relates_to_library(&p.title, filter.owned))
        .filter(|(_, p)| !filter.hide_owned || !is_owned_base_game(&p.title, filter.owned))
        .map(|(i, _)| i)
        .collect();
    match filter.sort {
        DealSort::Featured => {}
        DealSort::Discount => {
            ix.sort_by_key(|&i| std::cmp::Reverse(discount_percent(&products[i])));
        }
        DealSort::LowestPrice => {
            ix.sort_by(|&a, &b| {
                final_amount(&products[a])
                    .partial_cmp(&final_amount(&products[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        DealSort::TitleAsc => {
            ix.sort_by(|&a, &b| sort_key(&products[a].title).cmp(&sort_key(&products[b].title)));
        }
    }
    ix
}

fn store_label(store: DealStore, custom_name: Option<&str>) -> String {
    match store {
        DealStore::Steam => "Steam".into(),
        DealStore::Gog => "GOG".into(),
        DealStore::Gmg => "GreenManGaming".into(),
        DealStore::Humble => "Humble".into(),
        DealStore::Other => custom_name.unwrap_or("Store").to_string(),
    }
}

fn store_short_label(store: DealStore, custom_name: Option<&str>) -> String {
    match store {
        DealStore::Steam => "Steam".into(),
        DealStore::Gog => "GOG".into(),
        DealStore::Gmg => "GMG".into(),
        DealStore::Humble => "Humble".into(),
        DealStore::Other => custom_name.unwrap_or("Store").to_string(),
    }
}

fn protondb_tag_and_tip(tier: ProtonDbTier) -> Option<(Tag, SharedString)> {
    let (tag, tip) = match tier {
        ProtonDbTier::Native => (Tag::info(), t("manage.deals.protondb.native")),
        ProtonDbTier::Platinum => (Tag::success(), t("manage.deals.protondb.platinum")),
        ProtonDbTier::Gold => (Tag::warning(), t("manage.deals.protondb.gold")),
        ProtonDbTier::Silver => (Tag::secondary(), t("manage.deals.protondb.silver")),
        ProtonDbTier::Bronze => (Tag::secondary(), t("manage.deals.protondb.bronze")),
        ProtonDbTier::Borked => (Tag::danger(), t("manage.deals.protondb.borked")),
        ProtonDbTier::Pending | ProtonDbTier::Unknown => return None,
    };
    Some((tag.xsmall().child(tier.label()), tip))
}

fn render_protondb_badge(
    id_prefix: &str,
    title: &str,
    steam_app_id: Option<&str>,
    tiers: &HashMap<String, ProtonDbTier>,
) -> Option<AnyElement> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let broken_ac = anticheat::cached_broken_anticheat(title, steam_app_id);
    let tier = steam_app_id.and_then(|id| tiers.get(id).copied());
    if tier.is_none() && !broken_ac {
        return None;
    }
    let proton_url = steam_app_id
        .map(discounts::protondb_app_url)
        .unwrap_or_else(|| discounts::protondb_search_url(title));
    let mut row = h_flex().gap_1().items_center();
    if let Some((tag, tip)) = tier.and_then(protondb_tag_and_tip) {
        let open_url = proton_url.clone();
        row = row.child(
            div()
                .id(ElementId::Name(format!("{id_prefix}-pdb").into()))
                .cursor_pointer()
                .tooltip(move |window, cx| wrapped_tooltip(tip.clone(), window, cx))
                .on_click(move |_, _, cx| cx.open_url(&open_url))
                .child(tag),
        );
    }
    if broken_ac {
        let tip = t("manage.deals.anticheat_tooltip");
        row = row.child(
            div()
                .id(ElementId::Name(format!("{id_prefix}-ac").into()))
                .cursor_pointer()
                .tooltip(move |window, cx| wrapped_tooltip(tip.clone(), window, cx))
                .on_click(move |_, _, cx| cx.open_url(&proton_url))
                .child(Tag::danger().xsmall().child(t("manage.deals.anticheat"))),
        );
    }
    Some(row.into_any_element())
}

fn open_ggdeals_key_dialog(
    cache_entity: Entity<DealsCache>,
    key_saved: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t("manage.deals.ggdeals.key_placeholder"))
            .masked(true)
    });
    let weak_cache = cache_entity.downgrade();
    window.open_dialog(cx, move |dialog, _, cx| {
        let input = input.clone();
        let weak_cache = weak_cache.clone();
        let (border, muted_bg, muted_fg, radius) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted.opacity(0.35),
                theme.muted_foreground,
                theme.radius,
            )
        };
        dialog
            .title(t("manage.deals.ggdeals.dialog_title"))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .child(t("manage.deals.ggdeals.dialog_description")),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .p_3()
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(muted_bg)
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child(t("manage.deals.ggdeals.step1")),
                                    )
                                    .child(
                                        Link::new("ggdeals-register")
                                            .href("https://gg.deals/register/")
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .text_sm()
                                                    .child(t("manage.deals.ggdeals.register_link"))
                                                    .child(
                                                        Icon::new(IconName::ExternalLink).xsmall(),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child(t("manage.deals.ggdeals.step2")),
                                    )
                                    .child(
                                        Link::new("ggdeals-get-key")
                                            .href("https://gg.deals/api/")
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .text_sm()
                                                    .child(t("manage.deals.ggdeals.get_key"))
                                                    .child(
                                                        Icon::new(IconName::ExternalLink).xsmall(),
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(t("manage.deals.ggdeals.step3")),
                            ),
                    )
                    .child(Input::new(&input)),
            )
            .footer({
                let save_input = input.clone();
                let save_cache = weak_cache.clone();
                let remove_cache = weak_cache.clone();
                let mut footer = DialogFooter::new();
                if key_saved {
                    footer =
                        footer.child(
                            Button::new("ggdeals-key-remove")
                                .outline()
                                .label(t("manage.deals.ggdeals.remove_key"))
                                .on_click(move |_, window, cx| {
                                    let b = backend(cx);
                                    let target = remove_cache.clone();
                                    let task = irori_ui::run(cx, async move {
                                        b.set_ggdeals_api_key(None).await
                                    });
                                    window
                                        .spawn(cx, async move |cx| {
                                            let res = task.await;
                                            let _ = cx.update(|window, cx| match res {
                                                Ok(()) => {
                                                    let _ = target.update(cx, |c, cx| {
                                                        c.ggdeals_key_saved = false;
                                                        c.ggdeals.clear();
                                                        c.ggdeals_requested.clear();
                                                        cx.notify();
                                                    });
                                                    notify_success(
                                                        t("manage.deals.ggdeals.key_removed"),
                                                        window,
                                                        cx,
                                                    );
                                                    window.close_dialog(cx);
                                                }
                                                Err(e) => notify_error(
                                                    t("manage.deals.ggdeals.key_save_failed"),
                                                    &e,
                                                    window,
                                                    cx,
                                                ),
                                            });
                                        })
                                        .detach();
                                }),
                        );
                }
                footer.child(
                    Button::new("ggdeals-key-save")
                        .primary()
                        .label(t("manage.deals.ggdeals.save_key"))
                        .on_click(move |_, window, cx| {
                            let val = save_input.read(cx).value().trim().to_string();
                            if val.is_empty() {
                                return;
                            }
                            let b = backend(cx);
                            let target = save_cache.clone();
                            let task =
                                irori_ui::run(
                                    cx,
                                    async move { b.set_ggdeals_api_key(Some(val)).await },
                                );
                            window
                                .spawn(cx, async move |cx| {
                                    let res = task.await;
                                    let _ = cx.update(|window, cx| match res {
                                        Ok(()) => {
                                            let _ = target.update(cx, |c, cx| {
                                                c.ggdeals_key_saved = true;
                                                c.ggdeals.clear();
                                                c.ggdeals_requested.clear();
                                                cx.notify();
                                            });
                                            notify_success(
                                                t("manage.deals.ggdeals.key_saved"),
                                                window,
                                                cx,
                                            );
                                            window.close_dialog(cx);
                                        }
                                        Err(e) => notify_error(
                                            t("manage.deals.ggdeals.key_save_failed"),
                                            &e,
                                            window,
                                            cx,
                                        ),
                                    });
                                })
                                .detach();
                        }),
                )
            })
    });
}

fn region_items(system_locale: Option<&str>) -> Vec<SelectChoice> {
    let auto_region = discounts::resolve_region("", system_locale);
    let mut items = vec![SelectChoice {
        value: "".into(),
        title: t_args("manage.deals.region.auto", &[("code", auto_region.code)]),
    }];
    items.extend(DEALS_REGIONS.iter().map(|r| SelectChoice {
        value: r.code.into(),
        title: r.label.into(),
    }));
    items
}

fn region_selected_index(setting: &str) -> IndexPath {
    let row = if setting.trim().is_empty() || setting.eq_ignore_ascii_case("auto") {
        0
    } else {
        DEALS_REGIONS
            .iter()
            .position(|r| r.code.eq_ignore_ascii_case(setting.trim()))
            .map(|i| i + 1)
            .unwrap_or(0)
    };
    IndexPath::default().row(row)
}

fn sort_items() -> Vec<SelectChoice> {
    DealSort::ALL
        .into_iter()
        .map(|s| SelectChoice {
            value: s.id().into(),
            title: s.label(),
        })
        .collect()
}

/// The deals tab of the Stores page.
pub(crate) struct DealsSection {
    cache: Entity<DealsCache>,
    search: Entity<InputState>,
    region_select: Entity<SelectState<Vec<SelectChoice>>>,
    sort_select: Entity<SelectState<Vec<SelectChoice>>>,
    store_filter: Option<DealStore>,
    shop_filter: ShopFilter,
    sort: DealSort,
    library_related: bool,
    hide_owned: bool,
    owned: HashSet<String>,
    shown: usize,
    visible: Vec<usize>,
    grid_width: Rc<Cell<Pixels>>,
    free_grid_width: Rc<Cell<Pixels>>,
    placeholder_language: String,
    images: Entity<LruImageCache>,
    search_debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DealsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cache = cache(cx);
        crate::i18n::register();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("manage.deals.search")));
        let reg_setting = models(cx).settings.read(cx).app.deals_region.clone();
        let reg_choices = region_items(irori_ui::age_rating::system_locale());
        let reg_ix = region_selected_index(&reg_setting);
        let region_select = cx.new(|cx| SelectState::new(reg_choices, Some(reg_ix), window, cx));
        let sort_choices = sort_items();
        let sort_select = cx.new(|cx| {
            SelectState::new(sort_choices, Some(IndexPath::default().row(0)), window, cx)
        });
        let library = models(cx).library.clone();
        let settings = models(cx).settings.clone();
        let art = art_store(cx);
        let subscriptions = vec![
            cx.observe(&art, |_, _, cx| cx.notify()),
            cx.observe_in(&cache, window, |this, _, window, cx| {
                this.refilter(cx);
                this.schedule_enrichment(window, cx);
                cx.notify();
            }),
            cx.observe_in(&settings, window, |this, s, window, cx| {
                let setting = s.read(cx).app.deals_region.clone();
                let target_region =
                    discounts::resolve_region(&setting, irori_ui::age_rating::system_locale());
                let reg_ix = region_selected_index(&setting);
                this.region_select.update(cx, |sel, cx| {
                    sel.set_selected_index(Some(reg_ix), window, cx)
                });
                if this.cache.read(cx).region_code != target_region.code {
                    load(this.cache.clone(), target_region, cx);
                }
                cx.notify();
            }),
            cx.observe(&library, |this, _, cx| {
                this.owned = owned_titles(cx);
                if this.library_related || this.hide_owned {
                    this.refilter(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&region_select, window, |_, _, e, _, cx| {
                let gpui_kit::component::select::SelectEvent::Confirm(val) = e;
                if let Some(code) = val {
                    let next = code.to_string();
                    backend_op(cx, move |b| {
                        b.settings.update_app(|s| s.deals_region = next);
                    });
                }
            }),
            cx.subscribe(&sort_select, |this, _, e, cx| {
                let gpui_kit::component::select::SelectEvent::Confirm(val) = e;
                if let Some(id) = val {
                    this.sort = DealSort::from_id(id.as_ref());
                    this.shown = PAGE;
                    this.refilter(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(
                &search,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.shown = PAGE;
                        this.refilter(cx);
                        let q = this.search.read(cx).value().to_string();
                        let cache = this.cache.clone();
                        let region = active_region(cx);
                        this.search_debounce = Some(cx.spawn_in(window, async move |_, cx| {
                            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                            let _ = cx.update(|_, cx| {
                                run_global_search(cache, q, region, cx);
                            });
                        }));
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        this.search_debounce = None;
                        let q = this.search.read(cx).value().to_string();
                        let region = active_region(cx);
                        run_global_search(this.cache.clone(), q, region, cx);
                    }
                    _ => {}
                },
            ),
        ];
        let initial_region = active_region(cx);
        if matches!(cache.read(cx).state, DealsState::Idle)
            || cache.read(cx).region_code != initial_region.code
        {
            load(cache.clone(), initial_region, cx);
        }
        let mut this = Self {
            cache,
            search,
            region_select,
            sort_select,
            store_filter: None,
            shop_filter: ShopFilter::Official,
            sort: DealSort::Featured,
            library_related: false,
            hide_owned: false,
            owned: owned_titles(cx),
            shown: PAGE,
            visible: Vec::new(),
            grid_width: Rc::new(Cell::new(px(0.))),
            free_grid_width: Rc::new(Cell::new(px(0.))),
            placeholder_language: irori_ui::i18n::language(),
            images: LruImageCache::new(96, cx),
            search_debounce: None,
            _subscriptions: subscriptions,
        };
        this.refilter(cx);
        this.schedule_enrichment(window, cx);
        this
    }

    fn is_global_search_active(&self, cx: &App) -> bool {
        self.search.read(cx).value().trim().len() >= 2
    }

    fn active_products<'a>(
        &self,
        cx: &'a App,
    ) -> Option<(&'a [CatalogProduct], Vec<&'static str>)> {
        let c = self.cache.read(cx);
        if self.is_global_search_active(cx)
            && let SearchState::Loaded {
                results, failed, ..
            } = &c.search_state
        {
            return Some((results.as_slice(), failed.clone()));
        }
        if let DealsState::Loaded {
            products, failed, ..
        } = &c.state
        {
            return Some((products.as_slice(), failed.clone()));
        }
        None
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let Some((products, _)) = self.active_products(cx) else {
            self.visible.clear();
            return;
        };
        let query = self.search.read(cx).value();
        let global_search_active = self.is_global_search_active(cx);
        let c = self.cache.read(cx);
        let shop = if !c.ggdeals_key_saved && self.shop_filter == ShopFilter::Official {
            // Without a GG.deals key, "Official shops" falls back to all curated direct stores.
            ShopFilter::All
        } else {
            self.shop_filter
        };
        self.visible = filter_and_sort_deals(
            products,
            &DealQuery {
                store: self.store_filter,
                shop,
                sort: self.sort,
                query: &query,
                global_search_active,
                library_related: self.library_related,
                hide_owned: self.hide_owned,
                owned: &self.owned,
                ggdeals_key_saved: c.ggdeals_key_saved,
                ggdeals: &c.ggdeals,
            },
        );
    }

    fn schedule_enrichment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let region = active_region(cx);
        let mut ids = Vec::new();
        {
            let c = self.cache.read(cx);
            if let DealsState::Loaded { free_games, .. } = &c.state {
                for g in free_games.iter() {
                    if let Some(id) = &g.steam_app_id {
                        ids.push(id.clone());
                    }
                }
            }
            if let Some((products, _)) = self.active_products(cx) {
                for &ix in self.visible.iter().take(self.shown) {
                    if let Some(p) = products.get(ix)
                        && let Some(id) = &p.steam_app_id
                    {
                        ids.push(id.clone());
                    }
                }
            }
        }
        if !ids.is_empty() {
            let target = self.cache.clone();
            window.defer(cx, move |_, cx| {
                enrich_visible(target, ids, region, cx);
            });
        }
    }

    fn render_free_card(&self, g: &FreeGame, window: &mut Window, cx: &mut App) -> AnyElement {
        let (muted, muted_bg, radius) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.muted, theme.radius)
        };
        let width = rems(6.5);
        let art = g
            .image_url
            .as_ref()
            .and_then(|url| art_img(url, physical_width(width, window), false, cx));
        let compat = {
            let cache = self.cache.read(cx);
            render_protondb_badge(
                &format!("free-{}", g.id),
                &g.title,
                g.steam_app_id.as_deref(),
                &cache.protondb,
            )
        };
        let when_text: Option<SharedString> = g.ends_at.as_deref().map(|raw| {
            let day = irori_ui::dates::localized_day(raw);
            if g.upcoming {
                t_args("manage.deals.free.starts_on", &[("date", &day)])
            } else {
                t_args("manage.deals.free.until", &[("date", &day)])
            }
        });
        let claim_url = g.claim_url.clone();
        card(cx)
            .id(ElementId::Name(format!("free-{}", g.id).into()))
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .w(width)
                            .h(rems(3.7))
                            .flex_shrink_0()
                            .rounded(radius)
                            .overflow_hidden()
                            .bg(muted_bg)
                            .when_some(art, |this, img| {
                                this.child(
                                    img.size_full().rounded(radius).object_fit(ObjectFit::Cover),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .flex_wrap()
                                    .child(if g.upcoming {
                                        Tag::secondary()
                                            .xsmall()
                                            .child(t("manage.deals.free.coming_soon"))
                                    } else {
                                        Tag::success().xsmall().child(t("manage.deals.free.badge"))
                                    })
                                    .child(Tag::secondary().xsmall().child(g.store.display_name()))
                                    .children(compat),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .line_clamp(1)
                                    .child(g.title.clone()),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .when_some(g.original_price.as_ref(), |row, price| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .line_through()
                                        .text_color(muted)
                                        .child(price.clone()),
                                )
                            })
                            .when_some(when_text, |row, when| {
                                row.child(div().text_xs().text_color(muted).truncate().child(when))
                            }),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("free-claim-{}", g.id).into()))
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label(if g.upcoming {
                                t("manage.deals.free.view")
                            } else {
                                t("manage.deals.free.claim")
                            })
                            .on_click(move |_, _, cx| cx.open_url(&claim_url)),
                    ),
            )
            .into_any_element()
    }

    fn render_ggdeals_line(
        &self,
        p: &CatalogProduct,
        ggdeals: &HashMap<String, GgDealsPrice>,
        cx: &App,
    ) -> Option<AnyElement> {
        let info = p.steam_app_id.as_deref().and_then(|id| ggdeals.get(id))?;
        let muted = cx.theme().muted_foreground;
        let mut parts = Vec::new();
        match self.shop_filter {
            ShopFilter::Official => {
                if let Some(r) = &info.retail {
                    parts.push(t_args(
                        "manage.deals.ggdeals.official_price",
                        &[("price", r)],
                    ));
                }
                if let Some(l) = &info.historical_retail {
                    parts.push(t_args("manage.deals.ggdeals.low_price", &[("price", l)]));
                }
            }
            ShopFilter::Keyshops => {
                if let Some(k) = &info.keyshop {
                    parts.push(t_args(
                        "manage.deals.ggdeals.keyshop_price",
                        &[("price", k)],
                    ));
                }
                if let Some(l) = info
                    .historical_keyshop
                    .as_ref()
                    .or(info.historical_retail.as_ref())
                {
                    parts.push(t_args("manage.deals.ggdeals.low_price", &[("price", l)]));
                }
            }
            ShopFilter::All => {
                if let Some(r) = &info.retail {
                    parts.push(t_args(
                        "manage.deals.ggdeals.official_price",
                        &[("price", r)],
                    ));
                }
                if let Some(k) = &info.keyshop {
                    parts.push(t_args(
                        "manage.deals.ggdeals.keyshop_price",
                        &[("price", k)],
                    ));
                }
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(
            div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(parts.join(" · "))
                .into_any_element(),
        )
    }

    fn render_deal_grid(
        &self,
        p: &CatalogProduct,
        region: &'static DealsRegion,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let width = rems(18.);
        let art = p
            .cover_horizontal
            .clone()
            .or_else(|| p.cover_vertical.clone())
            .and_then(|url| art_img(&url, physical_width(width, window), false, cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let percent = discount_percent(p);
        let cache = self.cache.read(cx);
        let compat = render_protondb_badge(
            &format!("grid-{}", p.id),
            &p.title,
            p.steam_app_id.as_deref(),
            &cache.protondb,
        );
        let gg_line = self.render_ggdeals_line(p, &cache.ggdeals, cx);
        let divergent_cur = has_divergent_currency(p, region);
        let gg_url = p
            .steam_app_id
            .as_deref()
            .and_then(|id| cache.ggdeals.get(id))
            .and_then(|g| g.url.clone())
            .unwrap_or_else(|| discounts::gg_deals_search_url(&p.title, region));
        let in_library = is_owned_base_game(&p.title, &self.owned);

        card(cx)
            .id(ElementId::Name(format!("deal-grid-{}", p.id).into()))
            .p_0()
            .gap_0()
            .overflow_hidden()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(rems(9.5))
                    .bg(theme.muted)
                    .when_some(art, |this, img| {
                        this.child(img.size_full().object_fit(ObjectFit::Cover))
                    })
                    .child(
                        div().absolute().top_2().left_2().child(
                            Tag::secondary()
                                .xsmall()
                                .child(store_short_label(p.store, p.store_name.as_deref())),
                        ),
                    )
                    .when(percent > 0, |this| {
                        this.child(div().absolute().top_2().right_2().child(
                            Tag::success().xsmall().child(t_args(
                                "manage.deals.discount",
                                &[("percent", &percent.to_string())],
                            )),
                        ))
                    })
                    .when_some(compat, |this, badges| {
                        this.child(div().absolute().bottom_2().right_2().child(badges))
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                Link::new("open")
                                    .when_some(irori_ui::web_url(&p.store_link), |link, url| {
                                        link.href(url)
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .line_clamp(2)
                                            .child(p.title.clone()),
                                    ),
                            )
                            .when(in_library || p.store == DealStore::Gog, |col| {
                                col.child(
                                    h_flex()
                                        .gap_1p5()
                                        .when(in_library, |r| {
                                            r.child(
                                                Tag::info()
                                                    .xsmall()
                                                    .child(t("manage.deals.in_library")),
                                            )
                                        })
                                        .when(p.store == DealStore::Gog, |r| {
                                            r.child(
                                                Tag::secondary()
                                                    .xsmall()
                                                    .child(t("manage.deals.installable_short")),
                                            )
                                        }),
                                )
                            })
                            .children(gg_line),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .min_w_0()
                                    .when(!p.price.base.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .line_through()
                                                .text_color(muted)
                                                .child(p.price.base.clone()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(p.price.r#final.clone()),
                                    )
                                    .when_some(divergent_cur, |row, cur| {
                                        row.child(Tag::secondary().xsmall().child(cur))
                                    }),
                            )
                            .child(
                                Button::new(ElementId::Name(
                                    format!("ggdeals-grid-{}", p.id).into(),
                                ))
                                .ghost()
                                .xsmall()
                                .icon(IconName::ExternalLink)
                                .label("GG.deals")
                                .tooltip(t("manage.deals.ggdeals.compare_tooltip"))
                                .on_click(move |_, _, cx| cx.open_url(&gg_url)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_deal_list(
        &self,
        p: &CatalogProduct,
        region: &'static DealsRegion,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let width = rems(6.);
        let art = p
            .cover_horizontal
            .clone()
            .or_else(|| p.cover_vertical.clone())
            .and_then(|url| art_img(&url, physical_width(width, window), false, cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let percent = discount_percent(p);
        let cache = self.cache.read(cx);
        let compat = render_protondb_badge(
            &format!("list-{}", p.id),
            &p.title,
            p.steam_app_id.as_deref(),
            &cache.protondb,
        );
        let gg_line = self.render_ggdeals_line(p, &cache.ggdeals, cx);
        let divergent_cur = has_divergent_currency(p, region);
        let gg_url = p
            .steam_app_id
            .as_deref()
            .and_then(|id| cache.ggdeals.get(id))
            .and_then(|g| g.url.clone())
            .unwrap_or_else(|| discounts::gg_deals_search_url(&p.title, region));

        h_flex()
            .id(ElementId::Name(format!("deal-list-{}", p.id).into()))
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .child(
                div()
                    .w(width)
                    .h(rems(3.4))
                    .flex_shrink_0()
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .when_some(art, |this, img| {
                        this.child(
                            img.size_full()
                                .rounded(theme.radius)
                                .object_fit(ObjectFit::Cover),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        Link::new("open")
                            .when_some(irori_ui::web_url(&p.store_link), |link, url| link.href(url))
                            .child(div().font_medium().truncate().child(p.title.clone())),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(store_label(p.store, p.store_name.as_deref())),
                            )
                            .children(compat)
                            .children(gg_line),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .items_center()
                    .when(percent > 0, |this| {
                        this.child(Tag::success().xsmall().child(t_args(
                            "manage.deals.discount",
                            &[("percent", &percent.to_string())],
                        )))
                    })
                    .when(!p.price.base.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .line_through()
                                .text_color(muted)
                                .child(p.price.base.clone()),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(p.price.r#final.clone()),
                    )
                    .when_some(divergent_cur, |row, cur| {
                        row.child(Tag::secondary().xsmall().child(cur))
                    })
                    .child(
                        Button::new(ElementId::Name(format!("ggdeals-list-{}", p.id).into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .label("GG.deals")
                            .tooltip(t("manage.deals.ggdeals.compare_tooltip"))
                            .on_click(move |_, _, cx| cx.open_url(&gg_url)),
                    ),
            )
            .into_any_element()
    }
}

fn owned_titles(cx: &App) -> HashSet<String> {
    models(cx)
        .library
        .read(cx)
        .snapshot
        .games
        .iter()
        .map(|g| sort_key(&g.title))
        .collect()
}

impl Render for DealsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = irori_ui::i18n::language();
        if language != self.placeholder_language {
            self.placeholder_language = language;
            self.search.update(cx, |s, cx| {
                s.set_placeholder(t("manage.deals.search"), window, cx);
            });
            let setting = models(cx).settings.read(cx).app.deals_region.clone();
            let reg_choices = region_items(irori_ui::age_rating::system_locale());
            let reg_ix = region_selected_index(&setting);
            self.region_select.update(cx, |sel, cx| {
                sel.set_items(reg_choices, window, cx);
                sel.set_selected_index(Some(reg_ix), window, cx);
            });
            let sort_choices = sort_items();
            let sort_ix = DealSort::ALL
                .iter()
                .position(|&s| s == self.sort)
                .unwrap_or(0);
            self.sort_select.update(cx, |sel, cx| {
                sel.set_items(sort_choices, window, cx);
                sel.set_selected_index(Some(IndexPath::default().row(sort_ix)), window, cx);
            });
        }

        let region = active_region(cx);
        let layout = models(cx).settings.read(cx).app.deals_layout;
        let cache = self.cache.read(cx);
        let state = cache.state.clone();
        let search_state = cache.search_state.clone();
        let ggdeals_key_saved = cache.ggdeals_key_saved;
        let muted = cx.theme().muted_foreground;

        let free_strip: Option<AnyElement> = match &state {
            DealsState::Loaded { free_games, .. } if !free_games.is_empty() => {
                let has_gp = free_games.iter().any(|g| g.from_gamerpower);
                let cards: Vec<AnyElement> = free_games
                    .iter()
                    .map(|g| self.render_free_card(g, window, cx))
                    .collect();
                Some(
                    v_flex()
                        .gap_3()
                        .child(
                            SectionTitle::new(t("manage.deals.free.title")).when(has_gp, |s| {
                                s.trailing(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t("manage.deals.free.attribution")),
                                )
                            }),
                        )
                        .child(card_grid(&self.free_grid_width, 20., cards, window))
                        .into_any_element(),
                )
            }
            _ => None,
        };

        let gg_browse_url = discounts::gg_deals_browse_url(region);
        let cache_for_dialog = self.cache.clone();
        let header_actions = h_flex()
            .gap_2()
            .flex_wrap()
            .items_center()
            .child(
                div()
                    .w(rems(12.))
                    .child(Select::new(&self.region_select).small()),
            )
            .child(
                Button::new("deals-ggdeals-browse")
                    .outline()
                    .small()
                    .icon(IconName::ExternalLink)
                    .label("GG.deals")
                    .tooltip(t("manage.deals.ggdeals.browse_tooltip"))
                    .on_click(move |_, _, cx| cx.open_url(&gg_browse_url)),
            )
            .child(
                Button::new("deals-ggdeals-key")
                    .outline()
                    .small()
                    .icon(IconName::KeyRound)
                    .tooltip(t("manage.deals.ggdeals.key_tooltip"))
                    .label(if ggdeals_key_saved {
                        t("manage.deals.ggdeals.key_active")
                    } else {
                        t("manage.deals.ggdeals.key_button")
                    })
                    .on_click(move |_, window, cx| {
                        open_ggdeals_key_dialog(
                            cache_for_dialog.clone(),
                            ggdeals_key_saved,
                            window,
                            cx,
                        );
                    }),
            )
            .child(
                Button::new("deals-reload")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label(t("manage.deals.reload"))
                    .disabled(matches!(state, DealsState::Loading))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        load(this.cache.clone(), region, cx);
                    })),
            )
            .child(
                Button::new("deals-hide")
                    .ghost()
                    .small()
                    .icon(IconName::EyeOff)
                    .label(t("manage.deals.hide"))
                    .on_click(|_, window, cx| set_deals_shown(false, window, cx)),
            );

        let global_active = self.is_global_search_active(cx);
        let search_loading = matches!(search_state, SearchState::Loading(_));
        let active_snapshot: Option<(Vec<CatalogProduct>, Vec<&'static str>, bool, bool)> =
            self.active_products(cx).map(|(products, failed)| {
                let empty_products = products.is_empty();
                let has_other = products.iter().any(|p| p.store == DealStore::Other);
                let visible_products: Vec<CatalogProduct> = self
                    .visible
                    .iter()
                    .take(self.shown)
                    .map(|&i| products[i].clone())
                    .collect();
                (visible_products, failed.to_vec(), empty_products, has_other)
            });

        let mut store_tabs: Vec<Option<DealStore>> = vec![
            None,
            Some(DealStore::Steam),
            Some(DealStore::Gog),
            Some(DealStore::Gmg),
            Some(DealStore::Humble),
        ];
        if active_snapshot
            .as_ref()
            .is_some_and(|(_, _, _, has_other)| *has_other)
        {
            store_tabs.push(Some(DealStore::Other));
        }
        let selected_store_ix = store_tabs
            .iter()
            .position(|&s| s == self.store_filter)
            .unwrap_or(0);

        let shop_options = if ggdeals_key_saved {
            [
                (ShopFilter::Official, t("manage.deals.shop_type.official")),
                (ShopFilter::All, t("manage.deals.shop_type.all")),
                (ShopFilter::Keyshops, t("manage.deals.shop_type.keyshops")),
            ]
        } else {
            [
                (ShopFilter::All, t("manage.deals.shop_type.all")),
                (ShopFilter::Official, t("manage.deals.shop_type.direct")),
                (ShopFilter::Keyshops, t("manage.deals.shop_type.resellers")),
            ]
        };
        let selected_shop_ix = shop_options
            .iter()
            .position(|(f, _)| *f == self.shop_filter)
            .unwrap_or(0);

        let body: AnyElement = match (&state, active_snapshot) {
            (DealsState::Idle | DealsState::Loading, None) => h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.deals.loading"))
                .into_any_element(),
            (DealsState::Failed(message), None) => Alert::error("deals-error", message.clone())
                .title(t("manage.deals.failed"))
                .into_any_element(),
            (DealsState::Unavailable, None) => {
                Alert::error("deals-error", t("manage.deals.unavailable")).into_any_element()
            }
            (_, Some((visible_products, failed, empty_products, _))) => {
                let total = self.visible.len();
                let cards: Vec<AnyElement> = visible_products
                    .iter()
                    .map(|product| match layout {
                        DealsLayout::Grid => self.render_deal_grid(product, region, window, cx),
                        DealsLayout::List => self.render_deal_list(product, region, window, cx),
                    })
                    .collect();
                let store_tabs_for_click = store_tabs.clone();
                let shop_options_for_click = shop_options.clone();
                v_flex()
                    .gap_3()
                    .child(
                        TabBar::new("deal-stores")
                            .segmented()
                            .small()
                            .selected_index(selected_store_ix)
                            .children(store_tabs.iter().map(|s| {
                                Tab::new().label(
                                    s.map(|s| {
                                        SharedString::from(store_short_label(
                                            s,
                                            Some(&t("manage.deals.other_stores")),
                                        ))
                                    })
                                    .unwrap_or_else(|| t("manage.deals.all_stores")),
                                )
                            }))
                            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                                this.store_filter =
                                    store_tabs_for_click.get(*ix).copied().flatten();
                                this.shown = PAGE;
                                this.refilter(cx);
                                this.schedule_enrichment(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .flex_wrap()
                            .items_center()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w(rems(16.))
                                    .max_w(rems(28.))
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div().flex_1().child(
                                            Input::new(&self.search)
                                                .small()
                                                .prefix(Icon::new(IconName::Search).small())
                                                .cleanable(true),
                                        ),
                                    )
                                    .when(search_loading, |r| r.child(Spinner::new().small())),
                            )
                            .child(
                                TabBar::new("deal-shop-type")
                                    .segmented()
                                    .small()
                                    .selected_index(selected_shop_ix)
                                    .children(
                                        shop_options
                                            .into_iter()
                                            .map(|(_, label)| Tab::new().label(label)),
                                    )
                                    .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                                        if let Some((f, _)) = shop_options_for_click.get(*ix) {
                                            this.shop_filter = *f;
                                            this.shown = PAGE;
                                            this.refilter(cx);
                                            this.schedule_enrichment(window, cx);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .w(rems(11.))
                                    .child(Select::new(&self.sort_select).small()),
                            )
                            .child(
                                ButtonGroup::new("deals-layout")
                                    .small()
                                    .outline()
                                    .child(
                                        Button::new("deals-layout-grid")
                                            .icon(IconName::LayoutGrid)
                                            .tooltip(t("manage.deals.layout.grid"))
                                            .selected(layout == DealsLayout::Grid),
                                    )
                                    .child(
                                        Button::new("deals-layout-list")
                                            .icon(IconName::List)
                                            .tooltip(t("manage.deals.layout.list"))
                                            .selected(layout == DealsLayout::List),
                                    )
                                    .on_click(move |clicked: &Vec<usize>, _, cx| {
                                        let next = match clicked.first() {
                                            Some(1) => DealsLayout::List,
                                            _ => DealsLayout::Grid,
                                        };
                                        backend_op(cx, move |b| {
                                            b.settings.update_app(|s| s.deals_layout = next);
                                        });
                                    }),
                            )
                            .child(
                                Switch::new("deals-library-related")
                                    .label(t("manage.deals.library_related"))
                                    .tooltip(t("manage.deals.library_related_tooltip"))
                                    .checked(self.library_related)
                                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                                        this.library_related = *on;
                                        this.shown = PAGE;
                                        this.refilter(cx);
                                        this.schedule_enrichment(window, cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Switch::new("deals-hide-owned")
                                    .label(t("manage.deals.hide_owned"))
                                    .tooltip(t("manage.deals.hide_owned_tooltip"))
                                    .checked(self.hide_owned)
                                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                                        this.hide_owned = *on;
                                        this.shown = PAGE;
                                        this.refilter(cx);
                                        this.schedule_enrichment(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(!failed.is_empty(), |this| {
                        this.child(Alert::warning(
                            "deals-partial",
                            t_args("manage.deals.partial", &[("stores", &failed.join(", "))]),
                        ))
                    })
                    .child(if cards.is_empty() {
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(if global_active {
                                t("manage.deals.search_empty")
                            } else if empty_products {
                                t("manage.deals.none")
                            } else {
                                t("manage.deals.no_match")
                            })
                            .into_any_element()
                    } else if layout == DealsLayout::Grid {
                        card_grid(&self.grid_width, 17.5, cards, window).into_any_element()
                    } else {
                        v_flex()
                            .w_full()
                            .max_w(rems(68.))
                            .gap_2()
                            .children(cards)
                            .into_any_element()
                    })
                    .when(total > self.shown, |this| {
                        this.child(
                            div().child(
                                Button::new("deals-more")
                                    .ghost()
                                    .small()
                                    .label(t_args(
                                        "manage.deals.show_more",
                                        &[("count", &(total - self.shown).to_string())],
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.shown += PAGE;
                                        this.schedule_enrichment(window, cx);
                                        cx.notify();
                                    })),
                            ),
                        )
                    })
                    .into_any_element()
            }
            (_, None) => div().into_any_element(),
        };

        v_flex()
            .gap_5()
            .image_cache(self.images.clone())
            .children(free_strip)
            .child(
                v_flex()
                    .gap_3()
                    .child(SectionTitle::new(t("manage.deals.title")).trailing(header_actions))
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_extras::discounts::CatalogPrice;

    fn deal(title: &str, store: DealStore, discount: &str, final_amt: &str) -> CatalogProduct {
        CatalogProduct {
            id: format!("{title}-{}", store_label(store, None)),
            title: title.into(),
            store,
            price: CatalogPrice {
                discount: discount.into(),
                final_money: Some(discounts::Money {
                    amount: final_amt.into(),
                    currency: "USD".into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn filters_related_hides_owned_and_sorts() {
        let products = vec![
            deal(
                "The Witcher 3: Wild Hunt - Hearts of Stone",
                DealStore::Gog,
                "-50%",
                "4.99",
            ),
            deal("Hades", DealStore::Steam, "-70%", "7.49"),
            deal("Unrelated Game", DealStore::Humble, "-90%", "1.99"),
            deal("Witchery", DealStore::Gog, "-10%", "9.00"),
        ];
        let owned: HashSet<String> = ["The Witcher 3: Wild Hunt", "Hades"]
            .into_iter()
            .map(sort_key)
            .collect();
        let gg = HashMap::new();

        let base = |sort| DealQuery {
            store: None,
            shop: ShopFilter::All,
            sort,
            query: "",
            global_search_active: false,
            library_related: false,
            hide_owned: false,
            owned: &owned,
            ggdeals_key_saved: false,
            ggdeals: &gg,
        };

        // Featured preserves order.
        assert_eq!(
            filter_and_sort_deals(&products, &base(DealSort::Featured)),
            vec![0, 1, 2, 3]
        );
        // Discount puts -90% first.
        assert_eq!(
            filter_and_sort_deals(&products, &base(DealSort::Discount)),
            vec![2, 1, 0, 3]
        );
        // Related + Hide owned keeps only the Witcher 3 DLC!
        assert_eq!(
            filter_and_sort_deals(
                &products,
                &DealQuery {
                    library_related: true,
                    hide_owned: true,
                    ..base(DealSort::Discount)
                },
            ),
            vec![0]
        );
        // LowestPrice puts $1.99 first.
        assert_eq!(
            filter_and_sort_deals(&products, &base(DealSort::LowestPrice)),
            vec![2, 0, 1, 3]
        );
        // Steam filter finds Hades only.
        assert_eq!(
            filter_and_sort_deals(
                &products,
                &DealQuery {
                    store: Some(DealStore::Steam),
                    ..base(DealSort::Featured)
                },
            ),
            vec![1]
        );
    }

    #[test]
    fn detects_divergent_currency_and_interleaves() {
        let br = discounts::resolve_region("BR", None);
        let humble = deal("X", DealStore::Humble, "-50%", "5.00");
        assert_eq!(has_divergent_currency(&humble, br).as_deref(), Some("USD"));

        let interleaved = interleave_featured(vec![
            vec![
                deal("S1", DealStore::Steam, "-10%", "1"),
                deal("S2", DealStore::Steam, "-10%", "1"),
            ],
            vec![deal("G1", DealStore::Gog, "-10%", "1")],
        ]);
        assert_eq!(
            interleaved
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            vec!["S1", "G1", "S2"]
        );
    }
}
