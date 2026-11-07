//! Which board's age rating the interface shows: the one of the user's
//! region, told by the interface language and the system's locale (an
//! English interface on an Australian system shows ACB), else another.

use irori_core::{AgeRating, RatingSystem, age_rating};
use once_cell::sync::Lazy;

/// The system's locale (`en-AU`), read once: changing it needs a restart
/// of most programs anyway.
static SYSTEM_LOCALE: Lazy<Option<String>> = Lazy::new(sys_locale::get_locale);

/// The boards to show first, best first, for the current interface
/// language.
pub fn preferred_systems() -> Vec<RatingSystem> {
    age_rating::preferred_systems(&crate::i18n::language(), SYSTEM_LOCALE.as_deref())
}

/// The rating to show first and the game's other boards' ratings, in
/// `preferred` order. `None` when the game has no rating.
pub fn arrange<'a>(
    ratings: &'a [AgeRating],
    preferred: &[RatingSystem],
) -> Option<(&'a AgeRating, Vec<&'a AgeRating>)> {
    let first = age_rating::pick(ratings, preferred)?;
    let rank = |r: &AgeRating| {
        preferred
            .iter()
            .position(|s| *s == r.system)
            .unwrap_or(usize::MAX)
    };
    let mut others: Vec<&AgeRating> = ratings
        .iter()
        .filter(|r| r.system != first.system && r.system != RatingSystem::Unknown)
        .collect();
    others.sort_by_key(|r| rank(r));
    Some((first, others))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_region_s_board_first_and_the_others_after() {
        let rating = |s: RatingSystem, r: &str| AgeRating::new(s, r, Vec::new(), None).unwrap();
        let ratings = vec![
            rating(RatingSystem::Usk, "16"),
            rating(RatingSystem::Esrb, "m"),
            rating(RatingSystem::Pegi, "18"),
        ];
        let preferred = age_rating::preferred_systems("fr", None);
        let (first, others) = arrange(&ratings, &preferred).unwrap();
        assert_eq!(first.title(), "PEGI 18");
        let others: Vec<String> = others.iter().map(|r| r.title()).collect();
        assert_eq!(others, ["ESRB M", "USK 16"]);
        assert!(arrange(&[], &preferred).is_none());
    }
}
