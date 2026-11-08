//! The Steam friends window: the signed-in account's friends, their online
//! state and the game they play, with Steam's own ways to reach them (its
//! friends list, a chat, joining a lobby or server Steam reports).
//!
//! Steam is asked when the window opens, on Refresh and every
//! [`REFRESH_EVERY`] while it stays open; closing it stops that. The store
//! keeps the answer for a minute, so reopening at once asks nothing.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, Global, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Pixels, Render, SharedString, Size, Styled as _,
    StyledImage as _, Subscription, Task, Window, div, px, rems, size,
};
use irori_steam::{FriendLink, FriendState, Friends, SteamFriend, SteamStore};
use irori_ui::art::{LruImageCache, art_store};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};

use crate::common::notify_error;

/// How often an open window asks Steam again.
const REFRESH_EVERY: Duration = Duration::from_secs(3 * 60);

const ROW_REMS: f32 = 3.5;
const AVATAR_REMS: f32 = 2.25;

struct SteamHandle(Arc<SteamStore>);
impl Global for SteamHandle {}

/// Give the management screens the Steam store, enabling its friends
/// window (the backend exposes stores only as `dyn Store`).
pub fn install_steam_store(store: Arc<SteamStore>, cx: &mut App) {
    cx.set_global(SteamHandle(store));
}

pub(crate) fn steam_store(cx: &App) -> Option<Arc<SteamStore>> {
    cx.try_global::<SteamHandle>().map(|h| h.0.clone())
}

/// Whether the friends window has something to show: the shell gave the
/// Steam store and a Steam account is signed in.
pub fn steam_friends_available(cx: &App) -> bool {
    steam_store(cx).is_some_and(|s| s.has_account())
}

/// Open the Steam friends window.
pub fn open_steam_friends_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(store) = steam_store(cx) else {
        return;
    };
    let view = cx.new(|cx| FriendsDialog::new(store, window, cx));
    let title = t("manage.friends.title");
    let width = window.rem_size() * 34.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(title.clone()).width(width).child(view.clone())
    });
}

enum Load {
    Loading,
    Ready(Friends),
    Failed(SharedString),
}

/// One kind of row in the list, worded for the user.
fn state_text(friend: &SteamFriend) -> SharedString {
    match (&friend.playing, friend.state) {
        (Some((_, game)), _) if !game.is_empty() => {
            t_args("manage.friends.playing", &[("game", game)])
        }
        (Some(_), _) => t("manage.friends.in_game"),
        (None, FriendState::Online) => t("manage.friends.online"),
        (None, FriendState::Busy) => t("manage.friends.busy"),
        (None, FriendState::Away) => t("manage.friends.away"),
        (None, FriendState::Offline) => t("manage.friends.offline"),
    }
}

pub(crate) struct FriendsDialog {
    store: Arc<SteamStore>,
    load: Load,
    refreshing: bool,
    sizes: Rc<Vec<Size<Pixels>>>,
    sized_for: (usize, Pixels),
    images: Entity<LruImageCache>,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl FriendsDialog {
    pub(crate) fn new(store: Arc<SteamStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let refresh = cx.spawn_in(window, async move |this, cx| {
            let mut force = false;
            loop {
                if this.update(cx, |this, cx| this.reload(force, cx)).is_err() {
                    break;
                }
                force = true;
                cx.background_executor().timer(REFRESH_EVERY).await;
            }
        });
        let art = art_store(cx);
        Self {
            store,
            load: Load::Loading,
            refreshing: false,
            sizes: Rc::default(),
            sized_for: (usize::MAX, px(0.)),
            images: LruImageCache::new(48, cx),
            _refresh: refresh,
            _subscriptions: vec![cx.observe(&art, |_, _, cx| cx.notify())],
        }
    }

    fn reload(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        cx.notify();
        let store = self.store.clone();
        let task = irori_ui::run(cx, async move { store.friends(force).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                this.load = match result {
                    Ok(Some(friends)) => Load::Ready(friends),
                    Ok(None) => Load::Failed(t("manage.friends.signed_out")),
                    Err(e) => Load::Failed(format!("{e:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Show `friends` as if Steam had sent them (tests).
    #[cfg(test)]
    pub(crate) fn show(&mut self, friends: Friends, cx: &mut Context<Self>) {
        self.load = Load::Ready(friends);
        cx.notify();
    }

    /// The error shown, if any (tests).
    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<SharedString> {
        match &self.load {
            Load::Failed(message) => Some(message.clone()),
            _ => None,
        }
    }

    fn open(&self, link: FriendLink, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
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

    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Load::Ready(friends) = &self.load else {
            return Vec::new();
        };
        let rows: Vec<SteamFriend> =
            friends.list[range.start..range.end.min(friends.list.len())].to_vec();
        rows.iter()
            .map(|f| self.render_row(f, window, cx))
            .collect()
    }

    fn render_row(
        &self,
        friend: &SteamFriend,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let avatar_size = rems(AVATAR_REMS);
        let avatar = (!friend.avatar.is_empty())
            .then(|| {
                art_img(
                    &friend.avatar,
                    physical_width(avatar_size, window),
                    false,
                    cx,
                )
            })
            .flatten();
        let theme = cx.theme();
        let (muted, radius) = (theme.muted_foreground, theme.radius);
        let dot = match (&friend.playing, friend.state) {
            (Some(_), _) => theme.success,
            (None, FriendState::Online) => theme.info,
            (None, FriendState::Busy | FriendState::Away) => theme.warning,
            (None, FriendState::Offline) => theme.muted_foreground,
        };
        let id = friend.steamid;
        let join = friend.join;
        let profile = irori_ui::web_url(&friend.profile);
        h_flex()
            .id(ElementId::Name(format!("friend-{id}").into()))
            .debug_selector(move || format!("friend-{id}"))
            .h(rems(ROW_REMS))
            .gap_3()
            .px_2()
            .items_center()
            .child(
                div()
                    .relative()
                    .size(avatar_size)
                    .flex_shrink_0()
                    .rounded(radius)
                    .overflow_hidden()
                    .bg(theme.muted)
                    .when_some(avatar, |d, img| {
                        d.child(img.size_full().object_fit(ObjectFit::Cover))
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_medium()
                            .child(friend.name.clone()),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_1p5()
                            .items_center()
                            .text_sm()
                            .text_color(muted)
                            .child(div().size_2().flex_shrink_0().rounded_full().bg(dot))
                            .child(div().min_w_0().truncate().child(state_text(friend))),
                    ),
            )
            .when_some(join, |row, link| {
                row.child(
                    div()
                        .debug_selector(move || format!("friend-join-{id}"))
                        .child(
                            Button::new(ElementId::Name(format!("friend-join-{id}").into()))
                                .small()
                                .primary()
                                .icon(IconName::Play)
                                .label(t("manage.friends.join"))
                                .tooltip(t("manage.friends.join_hint"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open(link, window, cx)
                                })),
                        ),
                )
            })
            .child(
                Button::new(ElementId::Name(format!("friend-chat-{id}").into()))
                    .small()
                    .ghost()
                    .icon(IconName::MessageSquare)
                    .tooltip(t("manage.friends.message"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open(FriendLink::Message(id), window, cx)
                    })),
            )
            .when_some(profile, |row, url| {
                row.child(
                    Button::new(ElementId::Name(format!("friend-profile-{id}").into()))
                        .small()
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .tooltip(t("manage.friends.profile"))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .into_any_element()
    }
}

impl Render for FriendsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let count = match &self.load {
            Load::Ready(f) => f.list.len(),
            _ => 0,
        };
        let row_h = window.rem_size() * ROW_REMS;
        if self.sized_for != (count, row_h) {
            self.sizes = Rc::new(vec![size(px(0.), row_h); count]);
            self.sized_for = (count, row_h);
        }
        let toolbar =
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().text_sm().text_color(muted).child(
                    match &self.load {
                        Load::Ready(f) => {
                            let online = f
                                .list
                                .iter()
                                .filter(|f| f.state != FriendState::Offline || f.playing.is_some())
                                .count();
                            t_args(
                                "manage.friends.summary",
                                &[
                                    ("online", &online.to_string()),
                                    ("count", &f.list.len().to_string()),
                                ],
                            )
                        }
                        _ => SharedString::default(),
                    },
                ))
                .child(
                    Button::new("friends-refresh")
                        .small()
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label(t("manage.friends.refresh"))
                        .loading(self.refreshing)
                        .disabled(self.refreshing)
                        .on_click(cx.listener(|this, _, _, cx| this.reload(true, cx))),
                )
                .child(
                    Button::new("friends-open-steam")
                        .small()
                        .outline()
                        .icon(IconName::Users)
                        .label(t("manage.friends.open_steam"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open(FriendLink::FriendsList, window, cx)
                        })),
                );
        let body = match &self.load {
            Load::Loading => h_flex()
                .h(rems(6.))
                .justify_center()
                .items_center()
                .child(Spinner::new())
                .into_any_element(),
            Load::Failed(message) => Alert::error("friends-error", message.clone())
                .title(t("manage.friends.failed"))
                .into_any_element(),
            Load::Ready(_) if count == 0 => div()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child(t("manage.friends.empty"))
                .into_any_element(),
            Load::Ready(_) => div()
                .image_cache(self.images.clone())
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "steam-friends",
                        self.sizes.clone(),
                        |this, range, window, cx| this.render_rows(range, window, cx),
                    )
                    .h(rems((count.min(8) as f32) * ROW_REMS))
                    .w_full(),
                )
                .into_any_element(),
        };
        v_flex().gap_3().child(toolbar).child(body).child(
            h_flex()
                .gap_2()
                .items_start()
                .text_xs()
                .text_color(muted)
                .child(Icon::new(IconName::Info).xsmall())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(t("manage.friends.invite_hint")),
                ),
        )
    }
}
