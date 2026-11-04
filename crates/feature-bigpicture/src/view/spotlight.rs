//! Home's Spotlight layout: the focused game large (its cover beside its
//! title, facts and description) above one shelf in a strip. Up and Down
//! change the shelf the strip shows.

use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, ClickEvent, Context, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, point, px, rems,
};
use irori_ui::components::{art_img, physical_width};
use irori_ui::motion::rise_in;

use super::home::InfoStyle;
use super::{BigPictureView, Colors};
use crate::layout::Metrics;

impl BigPictureView {
    pub(super) fn render_spotlight(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = m.spotlight();
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (row, _) = focused.unwrap_or((0, 0));
        let shape = self.look.theme().covers.shape;
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();

        let feature = game.as_ref().map(|game| {
            let art = shape
                .art(game)
                .and_then(|url| art_img(url, physical_width(rems(s.feature_w), window), false, cx));
            let id = game.id();
            div()
                .id("bp-spotlight-cover")
                .role(Role::Button)
                .aria_label(SharedString::from(game.title.clone()))
                .flex_shrink_0()
                .w(rems(s.feature_w))
                .h(rems(s.feature_h))
                .overflow_hidden()
                .rounded(c.radius_lg)
                .bg(c.muted)
                .cursor_pointer()
                .shadow(vec![BoxShadow {
                    color: c.ring.opacity(0.35),
                    offset: point(px(0.), px(16.)),
                    blur_radius: px(64.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .when_some(art, |d, img| {
                    d.child(
                        img.image_cache(&self.image_cache)
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_page(id.clone(), cx);
                    this.focus.focus(window, cx);
                }))
        });
        let info = game.as_ref().map(|game| {
            let style = InfoStyle {
                title: if m.compact { 2.5 } else { 4.0 },
                logo: if m.compact { 4.0 } else { 7.0 },
                lines: if m.compact { 2 } else { 4 },
                centered: false,
                max_w: m.width * 0.5,
            };
            self.game_info(game, style, c, window, cx)
        });
        let hero = h_flex()
            .w_full()
            .h(rems(s.hero))
            .flex_shrink_0()
            .px(rems(m.gutter))
            .gap(rems(m.gutter))
            .items_center()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_center()
                    .when_some(info, |d, info| d.child(info)),
            )
            .when_some(feature, |d, f| d.child(f));

        let strip = v_flex()
            .w_full()
            .h(rems(s.strip))
            .flex_shrink_0()
            .child(div().px(rems(m.gutter)).child(self.shelf_switcher(
                row,
                (IconName::ChevronUp, IconName::ChevronDown),
                false,
                c,
                cx,
            )))
            .when(!self.rows.is_empty(), |d| {
                d.child(
                    // The strip's own heading is the switcher above.
                    div().flex_1().min_h_0().overflow_hidden().child(
                        self.render_strip_covers(row, focused, s.tile, shape, c, m, window, cx),
                    ),
                )
            });

        v_flex()
            .size_full()
            .child(rise_in(
                SharedString::from(format!("bp-spotlight-{key}")),
                hero,
                12.,
            ))
            .child(strip)
            .into_any_element()
    }
}
