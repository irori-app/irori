//! Steam friends' rows: avatar, name, what they do, and Steam's own links
//! (join their lobby or server when Steam reports one, chat, profile).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_steam::{FriendLink, FriendState, SteamFriend};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};

use super::{AVATAR_REMS, FriendsDialog, ROW_REMS};

/// What a friend does, worded for the user.
pub(super) fn state_text(friend: &SteamFriend) -> SharedString {
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

/// Counted as online in the summary.
pub(super) fn is_online(friend: &SteamFriend) -> bool {
    friend.state != FriendState::Offline || friend.playing.is_some()
}

impl FriendsDialog {
    pub(super) fn render_steam_row(
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
                                    this.open_steam_link(link, window, cx)
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
                        this.open_steam_link(FriendLink::Message(id), window, cx)
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
