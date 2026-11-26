//! The age-rating card under the primary action: the rating of the user's
//! region's board as a text badge ("PEGI" over "16"), its content
//! descriptors as the store wrote them, and the other boards' ratings.
//! Board names and ratings are never translated, and no board's logo is
//! drawn: the logos are the boards' trademarks.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, div, rems};
use irori_core::{AgeRating, GameInfo, age_rating};
use irori_ui::i18n::{t, t_args};

use super::GamePage;

/// What the page knows of a game's ratings: the store page's, then the
/// boards only the library data names.
pub(super) fn ratings(game: &GameInfo, page: Option<&[AgeRating]>) -> Vec<AgeRating> {
    let mut all = page.map(<[AgeRating]>::to_vec).unwrap_or_default();
    age_rating::merge(&mut all, &game.age_ratings);
    all
}

/// Content descriptors shown for `rating` in `ui_language`. Single-language
/// regional boards (ClassInd, USK, CERO, GRAC, CSRR) omit their descriptors
/// when the interface uses another language; the badge still shows.
pub(super) fn visible_descriptors<'a>(rating: &'a AgeRating, ui_language: &str) -> &'a [String] {
    if rating.system.descriptors_match_language(ui_language) {
        &rating.descriptors
    } else {
        &[]
    }
}

impl GamePage {
    /// The card, when any board rated the game.
    pub(super) fn render_age_rating(
        &self,
        game: &GameInfo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !irori_ui::age_rating::shown(cx) {
            return None;
        }
        let all = ratings(game, self.extra.ready().map(|e| e.age_ratings.as_slice()));
        let preferred = irori_ui::age_rating::preferred_systems(cx);
        let (first, others) = irori_ui::age_rating::arrange(&all, &preferred)?;
        let ink = self.ink;
        let theme = cx.theme();
        let badge = v_flex()
            .flex_shrink_0()
            .min_w(rems(3.5))
            .items_center()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_2()
            .border_color(ink.text)
            .text_color(ink.text)
            .child(div().text_xs().font_semibold().child(first.system.name()))
            .child(
                div()
                    .text_lg()
                    .font_extrabold()
                    .line_height(rems(1.4))
                    .child(first.rating.clone()),
            );
        let ui_language = irori_ui::i18n::language();
        let shown_descriptors = visible_descriptors(first, &ui_language);
        let descriptors = (!shown_descriptors.is_empty()).then(|| {
            div()
                .text_xs()
                .text_color(ink.text)
                .line_clamp(4)
                .child(shown_descriptors.join(&t("game.age_rating.separator")))
        });
        let also = (!others.is_empty()).then(|| {
            let list: Vec<String> = others.iter().map(|r| r.title()).collect();
            div()
                .text_xs()
                .text_color(ink.panel_muted)
                .line_clamp(2)
                .child(t_args(
                    "game.age_rating.also",
                    &[("ratings", &list.join(" · "))],
                ))
        });
        Some(
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .child(badge)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .text_color(ink.panel_muted)
                                .truncate()
                                .child(t("game.age_rating.title")),
                        )
                        .children(descriptors)
                        .children(also),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::RatingSystem;

    #[test]
    fn the_store_page_s_ratings_come_first() {
        let rating = |s, r: &str| AgeRating::new(s, r, Vec::new(), None).unwrap();
        let game = GameInfo {
            age_ratings: vec![
                rating(RatingSystem::Esrb, "t"),
                rating(RatingSystem::Pegi, "12"),
            ],
            ..Default::default()
        };
        let page = [rating(RatingSystem::Pegi, "16")];
        let titles: Vec<String> = ratings(&game, Some(&page))
            .iter()
            .map(AgeRating::title)
            .collect();
        assert_eq!(titles, ["PEGI 16", "ESRB T"]);
        assert_eq!(ratings(&game, None).len(), 2);
    }

    #[test]
    fn hides_single_language_regional_descriptors_in_other_ui_languages() {
        let class_ind = AgeRating::new(
            RatingSystem::ClassInd,
            "18",
            vec![
                "Conteúdo Sexual".into(),
                "Drogas".into(),
                "Violência Extrema".into(),
            ],
            Some(18),
        )
        .unwrap();
        assert!(visible_descriptors(&class_ind, "en").is_empty());
        assert_eq!(
            visible_descriptors(&class_ind, "pt_BR"),
            ["Conteúdo Sexual", "Drogas", "Violência Extrema"]
        );
        assert_eq!(
            visible_descriptors(&class_ind, "pt"),
            ["Conteúdo Sexual", "Drogas", "Violência Extrema"]
        );

        let pegi = AgeRating::new(
            RatingSystem::Pegi,
            "18",
            vec!["Violence".into(), "Bad Language".into()],
            Some(18),
        )
        .unwrap();
        assert_eq!(
            visible_descriptors(&pegi, "en"),
            ["Violence", "Bad Language"]
        );
        assert_eq!(
            visible_descriptors(&pegi, "fr"),
            ["Violence", "Bad Language"]
        );
    }
}
