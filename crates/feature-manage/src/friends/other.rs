//! "Other sources": why the friends of other services aren't listed. Each
//! was checked: Discord's friends list needs a permission Discord grants
//! only to apps it approves, Epic Games keeps its friends service to its
//! own launcher, GOG publishes no friends API, and the other launchers keep
//! friends inside their own apps. Nothing is fetched here.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div,
};
use irori_ui::i18n::t;

use super::FriendsDialog;

/// Each source with its name, in display order. Names are brands and stay
/// as they are.
pub(super) const SOURCES: &[(&str, &str)] = &[
    ("discord", "Discord"),
    ("epic", "Epic Games"),
    ("gog", "GOG"),
    (
        "launchers",
        "Battle.net · EA app · Ubisoft Connect · Rockstar",
    ),
];

/// Why `source`'s friends aren't listed.
fn explanation(source: &str) -> SharedString {
    match source {
        "discord" => t("manage.friends.other.discord"),
        "epic" => t("manage.friends.other.epic"),
        "gog" => t("manage.friends.other.gog"),
        _ => t("manage.friends.other.launchers"),
    }
}

impl FriendsDialog {
    pub(super) fn render_other_sources(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("manage.friends.other.intro")),
            )
            .children(SOURCES.iter().map(|(id, name)| {
                h_flex()
                    .debug_selector(move || format!("friends-other-{id}"))
                    .gap_3()
                    .items_start()
                    .child(
                        Icon::new(IconName::Info)
                            .small()
                            .flex_shrink_0()
                            .text_color(muted),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(div().font_medium().child(*name))
                            .child(div().text_sm().text_color(muted).child(explanation(id))),
                    )
            }))
            .into_any_element()
    }
}
