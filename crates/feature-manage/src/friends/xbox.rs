//! Xbox friends' rows: gamerpic, gamertag and what they do. Read only:
//! Microsoft documents no link that opens a chat or an invite, so the
//! window points to the Xbox app for those.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, Styled as _, StyledImage as _, Window, div, rems,
};
use irori_launchers::{XboxFriend, XboxOnlineState};
use irori_ui::components::{art_img, physical_width};
use irori_ui::i18n::{t, t_args};

use super::{AVATAR_REMS, FriendsDialog, ROW_REMS};

/// What a friend does, worded for the user.
pub(super) fn state_text(friend: &XboxFriend) -> SharedString {
    match (&friend.playing, friend.state) {
        (Some(game), _) => t_args("manage.friends.playing", &[("game", game)]),
        (None, XboxOnlineState::Online) => t("manage.friends.online"),
        (None, XboxOnlineState::Away) => t("manage.friends.away"),
        (None, XboxOnlineState::Offline) => t("manage.friends.offline"),
    }
}

/// Counted as online in the summary.
pub(super) fn is_online(friend: &XboxFriend) -> bool {
    friend.state != XboxOnlineState::Offline
}

impl FriendsDialog {
    pub(super) fn render_xbox_row(
        &self,
        friend: &XboxFriend,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let avatar_size = rems(AVATAR_REMS);
        let avatar = (!friend.picture.is_empty())
            .then(|| {
                art_img(
                    &friend.picture,
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
            (None, XboxOnlineState::Online) => theme.info,
            (None, XboxOnlineState::Away) => theme.warning,
            (None, XboxOnlineState::Offline) => theme.muted_foreground,
        };
        let id = friend.xuid.clone();
        h_flex()
            .id(ElementId::Name(format!("xbox-friend-{id}").into()))
            .debug_selector(move || format!("xbox-friend-{id}"))
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
                            .child(friend.gamertag.clone()),
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
            .into_any_element()
    }
}
