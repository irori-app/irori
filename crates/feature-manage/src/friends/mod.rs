//! The friends window: the friends of the accounts Irori reaches through a
//! service that lets other programs read them, one tab each, and "Other
//! sources" saying why the rest aren't listed.
//!
//! * Steam: the signed-in account's friends, their online state and game,
//!   with Steam's own ways to reach them (its friends list, a chat, joining
//!   a lobby or server Steam reports).
//! * Xbox: the people the Xbox account follows, through Microsoft's
//!   documented Xbox Live social, profile and presence services. Read only.
//!
//! A source is asked when its tab shows, on Refresh and every
//! [`REFRESH_EVERY`] while the window stays open on it; closing it stops
//! that. The stores keep an answer for a minute in memory and drop it at
//! sign-out, so reopening at once asks nothing.

mod other;
mod steam;
mod xbox;

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _,
    Pixels, Render, SharedString, Size, Styled as _, Subscription, Task, Window, div, px, rems,
    size,
};
use irori_launchers::{LaunchersStore, XboxFriends};
use irori_steam::{FriendLink, Friends, SteamStore};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::i18n::{t, t_args};

use crate::common::notify_error;

/// How often an open window asks its source again.
const REFRESH_EVERY: Duration = Duration::from_secs(3 * 60);

const ROW_REMS: f32 = 3.5;
const AVATAR_REMS: f32 = 2.25;

struct SteamHandle(Arc<SteamStore>);
impl Global for SteamHandle {}

/// Give the management screens the Steam store, enabling its friends
/// (the backend exposes stores only as `dyn Store`).
pub fn install_steam_store(store: Arc<SteamStore>, cx: &mut App) {
    cx.set_global(SteamHandle(store));
}

pub(crate) fn steam_store(cx: &App) -> Option<Arc<SteamStore>> {
    cx.try_global::<SteamHandle>().map(|h| h.0.clone())
}

/// Where friends come from: a tab of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FriendsSource {
    Steam,
    Xbox,
    /// Why other services' friends aren't listed.
    Other,
}

/// A Steam account is signed in.
pub fn steam_friends_available(cx: &App) -> bool {
    steam_store(cx).is_some_and(|s| s.has_account())
}

/// An Xbox account is signed in.
pub fn xbox_friends_available(cx: &App) -> bool {
    crate::launchers::launchers_store(cx).is_some_and(|s| s.has_xbox_account())
}

/// Whether the friends window has friends to show.
pub fn friends_available(cx: &App) -> bool {
    steam_friends_available(cx) || xbox_friends_available(cx)
}

/// Open the friends window on Steam's tab.
pub fn open_steam_friends_dialog(window: &mut Window, cx: &mut App) {
    open_friends_dialog(FriendsSource::Steam, window, cx);
}

/// Open the friends window on `source`'s tab (or the first one there is).
pub fn open_friends_dialog(source: FriendsSource, window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let steam = steam_store(cx);
    let xbox = crate::launchers::launchers_store(cx);
    let view = cx.new(|cx| FriendsDialog::new(steam, xbox, source, window, cx));
    let title = t("manage.friends.title");
    let width = window.rem_size() * 34.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(title.clone()).width(width).child(view.clone())
    });
}

/// The tabs for the stores Irori has: a store that can't show friends
/// (no Steam store, no Microsoft app to sign in to Xbox with) has none.
/// "Other sources" is always last.
pub(crate) fn tabs(steam: bool, xbox: bool) -> Vec<FriendsSource> {
    let mut tabs = Vec::new();
    if steam {
        tabs.push(FriendsSource::Steam);
    }
    if xbox {
        tabs.push(FriendsSource::Xbox);
    }
    tabs.push(FriendsSource::Other);
    tabs
}

/// The tab to open on: the one asked for when shown, else the first.
pub(crate) fn first_tab(tabs: &[FriendsSource], wanted: FriendsSource) -> FriendsSource {
    if tabs.contains(&wanted) {
        wanted
    } else {
        tabs[0]
    }
}

enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(SharedString),
}

impl<T> Load<T> {
    fn failure(&self) -> Option<SharedString> {
        match self {
            Load::Failed(message) => Some(message.clone()),
            _ => None,
        }
    }
}

pub(crate) struct FriendsDialog {
    steam: Option<Arc<SteamStore>>,
    xbox: Option<Arc<LaunchersStore>>,
    tabs: Vec<FriendsSource>,
    tab: FriendsSource,
    steam_friends: Load<Friends>,
    xbox_friends: Load<XboxFriends>,
    /// Sources being asked now.
    refreshing: Vec<FriendsSource>,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (FriendsSource, usize, Pixels),
    images: Entity<LruImageCache>,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl FriendsDialog {
    pub(crate) fn new(
        steam: Option<Arc<SteamStore>>,
        xbox: Option<Arc<LaunchersStore>>,
        source: FriendsSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Xbox has a tab when Irori can sign in to it at all.
        let xbox = xbox.filter(|x| x.xbox_account_status().available);
        let tabs = tabs(steam.is_some(), xbox.is_some());
        let tab = first_tab(&tabs, source);
        let refresh = cx.spawn_in(window, async move |this, cx| {
            let mut force = false;
            loop {
                if this
                    .update(cx, |this, cx| this.reload(this.tab, force, cx))
                    .is_err()
                {
                    break;
                }
                force = true;
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        });
        let art = art_store(cx);
        Self {
            steam,
            xbox,
            tabs,
            tab,
            steam_friends: Load::Idle,
            xbox_friends: Load::Idle,
            refreshing: Vec::new(),
            sizes: Rc::default(),
            sized_for: (FriendsSource::Other, usize::MAX, px(0.)),
            images: LruImageCache::new(48, cx),
            _refresh: refresh,
            _subscriptions: vec![cx.observe(&art, |_, _, cx| cx.notify())],
        }
    }

    fn select(&mut self, tab: FriendsSource, cx: &mut Context<Self>) {
        self.tab = tab;
        let idle = match tab {
            FriendsSource::Steam => matches!(self.steam_friends, Load::Idle),
            FriendsSource::Xbox => matches!(self.xbox_friends, Load::Idle),
            FriendsSource::Other => false,
        };
        if idle {
            self.reload(tab, false, cx);
        }
        cx.notify();
    }

    /// Ask `source` for its friends (again, with `force`).
    fn reload(&mut self, source: FriendsSource, force: bool, cx: &mut Context<Self>) {
        if self.refreshing.contains(&source) {
            return;
        }
        match source {
            FriendsSource::Steam => {
                let Some(store) = self.steam.clone() else {
                    return;
                };
                if matches!(self.steam_friends, Load::Idle) {
                    self.steam_friends = Load::Loading;
                }
                let task = irori_ui::run(cx, async move { store.friends(force).await });
                self.finish(source, cx, task, |this, result| {
                    this.steam_friends = match result {
                        Ok(Some(friends)) => Load::Ready(friends),
                        Ok(None) => Load::Failed(t("manage.friends.signed_out")),
                        Err(e) => Load::Failed(format!("{e:#}").into()),
                    }
                });
            }
            FriendsSource::Xbox => {
                let Some(store) = self.xbox.clone() else {
                    return;
                };
                if matches!(self.xbox_friends, Load::Idle) {
                    self.xbox_friends = Load::Loading;
                }
                let task = irori_ui::run(cx, async move { store.xbox_friends(force).await });
                self.finish(source, cx, task, |this, result| {
                    this.xbox_friends = match result {
                        Ok(Some(friends)) => Load::Ready(friends),
                        Ok(None) => Load::Failed(t("manage.friends.xbox.signed_out")),
                        Err(e) => Load::Failed(format!("{e:#}").into()),
                    }
                });
            }
            FriendsSource::Other => return,
        }
        self.refreshing.push(source);
        cx.notify();
    }

    fn finish<T: 'static>(
        &mut self,
        source: FriendsSource,
        cx: &mut Context<Self>,
        task: Task<anyhow::Result<T>>,
        apply: impl FnOnce(&mut Self, anyhow::Result<T>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing.retain(|s| *s != source);
                apply(this, result);
                cx.notify();
            });
        })
        .detach();
    }

    /// Show `friends` as if Steam had sent them (tests).
    #[cfg(test)]
    pub(crate) fn show(&mut self, friends: Friends, cx: &mut Context<Self>) {
        self.steam_friends = Load::Ready(friends);
        cx.notify();
    }

    /// Show `friends` as if Xbox had sent them (tests).
    #[cfg(test)]
    pub(crate) fn show_xbox(&mut self, friends: XboxFriends, cx: &mut Context<Self>) {
        self.xbox_friends = Load::Ready(friends);
        cx.notify();
    }

    /// Switch tabs (tests).
    #[cfg(test)]
    pub(crate) fn show_tab(&mut self, tab: FriendsSource, cx: &mut Context<Self>) {
        self.select(tab, cx);
    }

    /// The tabs and the one shown (tests).
    #[cfg(test)]
    pub(crate) fn shown_tabs(&self) -> (Vec<FriendsSource>, FriendsSource) {
        (self.tabs.clone(), self.tab)
    }

    /// The error the shown tab explains, if any (tests).
    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<SharedString> {
        match self.tab {
            FriendsSource::Steam => self.steam_friends.failure(),
            FriendsSource::Xbox => self.xbox_friends.failure(),
            FriendsSource::Other => None,
        }
    }

    fn open_steam_link(&self, link: FriendLink, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.steam.clone() else {
            return;
        };
        let task = irori_ui::run(cx, async move { store.open_friend_link(link).await });
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    notify_error(t("manage.friends.open_failed"), &e, window, cx)
                });
            }
        })
        .detach();
    }

    /// Rows of the shown tab, and how many are online.
    fn counts(&self) -> Option<(usize, usize)> {
        match (self.tab, &self.steam_friends, &self.xbox_friends) {
            (FriendsSource::Steam, Load::Ready(f), _) => Some((
                f.list.len(),
                f.list.iter().filter(|f| steam::is_online(f)).count(),
            )),
            (FriendsSource::Xbox, _, Load::Ready(f)) => Some((
                f.list.len(),
                f.list.iter().filter(|f| xbox::is_online(f)).count(),
            )),
            _ => None,
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match (self.tab, &self.steam_friends, &self.xbox_friends) {
            (FriendsSource::Steam, Load::Ready(f), _) => {
                let rows = f.list[range.start..range.end.min(f.list.len())].to_vec();
                rows.iter()
                    .map(|f| self.render_steam_row(f, window, cx))
                    .collect()
            }
            (FriendsSource::Xbox, _, Load::Ready(f)) => {
                let rows = f.list[range.start..range.end.min(f.list.len())].to_vec();
                rows.iter()
                    .map(|f| self.render_xbox_row(f, window, cx))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.tabs.len() > 1).then(|| {
            let selected = self.tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
            TabBar::new("friends-sources")
                .segmented()
                .small()
                .selected_index(selected)
                .children(self.tabs.iter().map(|tab| {
                    Tab::new().label(match tab {
                        FriendsSource::Steam => SharedString::from("Steam"),
                        FriendsSource::Xbox => SharedString::from("Xbox"),
                        FriendsSource::Other => t("manage.friends.other.tab"),
                    })
                }))
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    if let Some(tab) = this.tabs.get(*ix).copied() {
                        this.select(tab, cx);
                    }
                }))
                .into_any_element()
        })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let tab = self.tab;
        let refreshing = self.refreshing.contains(&tab);
        h_flex()
            .gap_2()
            .items_center()
            .child(div().flex_1().min_w_0().text_sm().text_color(muted).child(
                match self.counts() {
                    Some((count, online)) => t_args(
                        "manage.friends.summary",
                        &[
                            ("online", &online.to_string()),
                            ("count", &count.to_string()),
                        ],
                    ),
                    None => SharedString::default(),
                },
            ))
            .child(
                Button::new("friends-refresh")
                    .small()
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .label(t("manage.friends.refresh"))
                    .loading(refreshing)
                    .disabled(refreshing)
                    .on_click(cx.listener(move |this, _, _, cx| this.reload(tab, true, cx))),
            )
            .when(tab == FriendsSource::Steam, |row| {
                row.child(
                    Button::new("friends-open-steam")
                        .small()
                        .outline()
                        .icon(IconName::Users)
                        .label(t("manage.friends.open_steam"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_steam_link(FriendLink::FriendsList, window, cx)
                        })),
                )
            })
            .into_any_element()
    }

    fn render_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (loading, failure) = match self.tab {
            FriendsSource::Steam => (
                matches!(self.steam_friends, Load::Idle | Load::Loading),
                self.steam_friends.failure(),
            ),
            FriendsSource::Xbox => (
                matches!(self.xbox_friends, Load::Idle | Load::Loading),
                self.xbox_friends.failure(),
            ),
            FriendsSource::Other => (false, None),
        };
        let count = self.counts().map_or(0, |(count, _)| count);
        let row_h = window.rem_size() * ROW_REMS;
        if self.sized_for != (self.tab, count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (self.tab, count, row_h);
        }
        if loading {
            return h_flex()
                .h(rems(6.))
                .justify_center()
                .items_center()
                .child(Spinner::new())
                .into_any_element();
        }
        if let Some(message) = failure {
            return Alert::error("friends-error", message)
                .title(t("manage.friends.failed"))
                .into_any_element();
        }
        if count == 0 {
            return div()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child(match self.tab {
                    FriendsSource::Xbox => t("manage.friends.xbox.empty"),
                    _ => t("manage.friends.empty"),
                })
                .into_any_element();
        }
        div()
            .image_cache(self.images.clone())
            .child(
                v_virtual_list(
                    cx.entity(),
                    "friends-list",
                    self.sizes.clone(),
                    |this, range, window, cx| this.render_rows(range, window, cx),
                )
                .h(rems((count.min(8) as f32) * ROW_REMS))
                .w_full(),
            )
            .into_any_element()
    }
}

impl Render for FriendsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let tabs = self.render_tabs(cx);
        if self.tab == FriendsSource::Other {
            return v_flex()
                .gap_3()
                .children(tabs)
                .child(self.render_other_sources(cx));
        }
        let hint = match self.tab {
            FriendsSource::Xbox => t("manage.friends.xbox.hint"),
            _ => t("manage.friends.invite_hint"),
        };
        v_flex()
            .gap_3()
            .children(tabs)
            .child(self.render_toolbar(cx))
            .child(self.render_list(window, cx))
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).xsmall())
                    .child(div().flex_1().min_w_0().child(hint)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_follow_the_stores_irori_has() {
        use FriendsSource::*;
        assert_eq!(tabs(true, true), [Steam, Xbox, Other]);
        assert_eq!(tabs(false, true), [Xbox, Other]);
        assert_eq!(tabs(false, false), [Other]);
        assert_eq!(first_tab(&tabs(true, true), Xbox), Xbox);
        assert_eq!(first_tab(&tabs(false, true), Steam), Xbox);
        assert_eq!(first_tab(&tabs(false, false), Steam), Other);
    }

    #[test]
    fn every_other_source_is_explained() {
        let ids: Vec<&str> = other::SOURCES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["discord", "epic", "gog", "launchers"]);
    }
}
