//! "More like this": games of the library in the same series, by the same
//! developer or publisher, or sharing genres. Worked out once per library
//! revision and per genre list, not per frame.

use std::sync::Arc;

use gpui_kit::component::StyledExt as _;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_core::GameInfo;
use irori_services::related::{Related, RelatedReason, related_games};
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::KeyboardFocusable as _;
use irori_ui::{Route, models, navigate};

use super::GamePage;
use crate::load::Load;

/// Related games shown at most.
const SHOWN: usize = 8;
const COVER_REM: f32 = 7.5;

/// The last list worked out, with what it was worked out from.
#[derive(Default)]
pub(super) struct RelatedCache {
    key: Option<(u64, usize, Vec<String>)>,
    list: Arc<Vec<Related>>,
}

impl GamePage {
    fn related(&mut self, game: &GameInfo, cx: &mut Context<Self>) -> Arc<Vec<Related>> {
        let genres = match &self.extra {
            Load::Ready(extra) => extra.genres.clone(),
            _ => Vec::new(),
        };
        let (snapshot, user) = {
            let library = models(cx).library.read(cx);
            (library.snapshot.clone(), library.user.clone())
        };
        let key = (
            snapshot.revision,
            Arc::as_ptr(&user) as usize,
            genres.clone(),
        );
        if self.related.key.as_ref() != Some(&key) {
            self.related.list = Arc::new(related_games(game, &genres, &snapshot, &user, SHOWN));
            self.related.key = Some(key);
        }
        self.related.list.clone()
    }

    /// The section, or `None` when nothing in the library is related.
    pub(super) fn render_related(
        &mut self,
        game: &GameInfo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let list = self.related(game, cx);
        if list.is_empty() {
            return None;
        }
        let ink = self.ink;
        let tiles = list.iter().map(|r| {
            let id = r.game.id();
            let reason = match &r.reason {
                RelatedReason::Series => t("game.related.series"),
                RelatedReason::Developer(name) => {
                    t_args("game.related.developer", &[("name", name)])
                }
                RelatedReason::Publisher(name) => {
                    t_args("game.related.publisher", &[("name", name)])
                }
                RelatedReason::Genres(genres) => t_args(
                    "game.related.genres",
                    &[("genres", &genres.join(&t("game.related.separator")))],
                ),
            };
            v_flex()
                .id(ElementId::Name(format!("related:{}", id.key()).into()))
                .keyboard_focusable(cx)
                .w(rems(COVER_REM))
                .gap_1p5()
                .cursor_pointer()
                .rounded_lg()
                .child(GameCover::new(r.game.clone()).width(rems(COVER_REM)))
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .text_color(ink.text)
                        .truncate()
                        .child(r.game.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(ink.muted)
                        .truncate()
                        .child(reason),
                )
                .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx))
        });
        Some(
            v_flex()
                .w_full()
                .gap_4()
                .child(super::section_title(t("game.related.title"), ink.text))
                .child(div().w_full().flex().flex_wrap().gap_4().children(tiles))
                .into_any_element(),
        )
    }
}
