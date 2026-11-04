//! Home's Cinema layout: the focused game's art sharp in a wide band
//! between dark bars, its title over the band's lower edge, and the shelf as
//! a filmstrip of wide stills below. Up and Down change the shelf.
//!
//! The band follows the backdrop, which waits for the focus to rest, so
//! scrolling through the filmstrip fetches no art for each still passed.

use gpui_kit::assets::IconName;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, Styled as _, StyledImage as _, Window, div,
    linear_color_stop, linear_gradient, rems,
};
use irori_ui::components::{art_img, physical_width};
use irori_ui::motion::{fade_in, rise_in};

use super::home::InfoStyle;
use super::{BigPictureView, Colors};
use crate::layout::Metrics;
use crate::theme::CoverShape;

impl BigPictureView {
    pub(super) fn render_cinema(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cinema = m.cinema();
        let lens = self.lens();
        let focused = self.grid.focused(&lens);
        let (row, _) = focused.unwrap_or((0, 0));
        let game = self.tab_game().cloned();
        let key = game.as_ref().map(|g| g.id().key()).unwrap_or_default();

        let art = self
            .hero
            .as_ref()
            .and_then(|g| g.hero_art().map(str::to_owned))
            .and_then(|url| art_img(&url, physical_width(rems(m.width), window), false, cx));
        let info = game.as_ref().map(|game| {
            let style = InfoStyle {
                title: if m.compact { 2.25 } else { 3.25 },
                logo: if m.compact { 0.0 } else { 5.0 },
                lines: if m.compact { 0 } else { 2 },
                centered: false,
                max_w: m.width * 0.6,
            };
            self.game_info(game, style, c, window, cx)
        });
        let band = div()
            .id("bp-cinema-band")
            .relative()
            .w_full()
            .h(rems(cinema.band))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(c.muted)
            .when_some(art, |d, img| {
                d.child(fade_in(
                    ElementId::NamedInteger("bp-cinema-art".into(), self.hero_gen),
                    img.image_cache(&self.image_cache)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                ))
            })
            // Shade under the title, so it reads on any art.
            .child(div().absolute().inset_0().bg(linear_gradient(
                0.,
                linear_color_stop(c.background.opacity(0.9), 0.),
                linear_color_stop(c.background.opacity(0.), 0.65),
            )))
            .when_some(info, |d, info| {
                d.child(
                    div()
                        .absolute()
                        .left(rems(m.gutter))
                        .right(rems(m.gutter))
                        .bottom(rems(1.25))
                        .child(rise_in(
                            SharedString::from(format!("bp-cinema-info-{key}")),
                            info,
                            12.,
                        )),
                )
            });

        // Stills are always wide: a filmstrip of box art would read as
        // shelves.
        let still = CoverShape::Landscape;
        v_flex()
            .size_full()
            // The bars: the theme's background, nearly opaque over the
            // backdrop so the band reads as a screen.
            .bg(c.background.opacity(0.92))
            .child(div().h(rems(cinema.bar)).flex_shrink_0())
            .child(band)
            .child(div().h(rems(cinema.bar)).flex_shrink_0())
            .child(
                v_flex()
                    .w_full()
                    .h(rems(cinema.strip))
                    .flex_shrink_0()
                    .child(div().px(rems(m.gutter)).child(self.shelf_switcher(
                        row,
                        (IconName::ChevronUp, IconName::ChevronDown),
                        false,
                        c,
                        cx,
                    )))
                    .child(self.render_strip_covers(
                        row,
                        focused,
                        cinema.still,
                        still,
                        c,
                        m,
                        window,
                        cx,
                    )),
            )
            .into_any_element()
    }
}
