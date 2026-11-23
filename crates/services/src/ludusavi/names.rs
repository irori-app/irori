//! Names to look a game up by in Ludusavi when its title finds nothing.
//!
//! ROM names carry region, revision and dump tags (`Chrono Trigger (USA)
//! (Rev 1) [!]`) that no manifest entry has. Emulators' saves come from
//! secondary manifests that name each game after its platform or emulator
//! (`Tales of the Abyss (PS2)`, `Chrono Trigger (RetroArch)`), so a game
//! played in an emulator is found by its tag-free name plus such a suffix.

use std::path::Path;

use irori_core::GameInfo;

use super::client::TitleMatch;

/// The emulator whose manifest names games after itself rather than their
/// system (`… (RetroArch)`).
const RETROARCH_SUFFIX: &str = "retroarch";

/// `name` without its trailing bracketed tags: `Moon Cheese Caper (USA,
/// Europe) (Rev 1) [!]` → `Moon Cheese Caper`. Tags followed by more text
/// (`Racer (USA) v1.2`) stay, and a name that is only tags is kept whole.
pub fn without_tags(name: &str) -> &str {
    let mut rest = name.trim_end();
    while let Some(stripped) = without_last_tag(rest) {
        if stripped.is_empty() {
            break;
        }
        rest = stripped;
    }
    rest
}

/// `name` without its last bracketed group, `None` when it doesn't end in one.
fn without_last_tag(name: &str) -> Option<&str> {
    let name = name.trim_end();
    let open = match name.chars().last()? {
        ')' => '(',
        ']' => '[',
        _ => return None,
    };
    let start = name.rfind(open)?;
    Some(name[..start].trim_end())
}

/// Letters and digits only, lower case: how names are compared.
fn fold(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The ROM file's name without its extension, for a game played in an
/// emulator: it keeps the database name a renamed title lost.
fn rom_stem(game: &GameInfo) -> Option<String> {
    let rom = game.install.as_ref()?.executable.trim().to_string();
    if rom.is_empty() {
        return None;
    }
    // A Windows path read on another system still splits at its separators.
    let file = rom.rsplit(['/', '\\']).next().unwrap_or(&rom);
    Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
}

/// The game's title without tags, when that differs from the title.
pub fn untagged_title(game: &GameInfo) -> Option<String> {
    let stripped = without_tags(&game.title);
    (stripped != game.title.trim()).then(|| stripped.to_string())
}

/// Names to search Ludusavi for an emulated game's manifest entry, in
/// order: the title without tags, then the ROM's name without tags.
pub fn emulated_bases(game: &GameInfo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let stem = rom_stem(game);
    for name in [Some(game.title.as_str()), stem.as_deref()]
        .into_iter()
        .flatten()
    {
        let base = without_tags(name).to_string();
        if !base.is_empty() && !out.iter().any(|b| fold(b) == fold(&base)) {
            out.push(base);
        }
    }
    out
}

/// The entry among Ludusavi's ranked `found` that is `bases`' game under an
/// emulator or platform suffix. An entry naming the game exactly beats one
/// that keeps a tag of its own (`(Demo)`); then, with `retroarch` known, an
/// entry of RetroArch's manifest is preferred when the game plays in
/// RetroArch and avoided otherwise; then Ludusavi's order.
pub fn pick_emulated(
    found: &[TitleMatch],
    bases: &[String],
    retroarch: Option<bool>,
) -> Option<String> {
    let wanted: Vec<String> = bases.iter().map(|b| fold(b)).collect();
    found
        .iter()
        .filter_map(|m| {
            let rest = without_last_tag(&m.title)?;
            let suffix = m.title[rest.len()..].trim();
            let exact = wanted.contains(&fold(rest));
            if !exact && !wanted.contains(&fold(without_tags(rest))) {
                return None;
            }
            let other_emulator =
                retroarch.is_some_and(|ra| ra != (fold(suffix) == RETROARCH_SUFFIX));
            Some(((!exact, other_emulator), m))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, m)| m.title.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::InstalledInfo;

    fn found(titles: &[&str]) -> Vec<TitleMatch> {
        titles
            .iter()
            .map(|t| TitleMatch {
                title: t.to_string(),
                score: Some(0.9),
            })
            .collect()
    }

    #[test]
    fn strips_only_trailing_tags() {
        assert_eq!(
            without_tags("Moon Cheese Caper (USA, Europe) (Rev 1) [!]"),
            "Moon Cheese Caper"
        );
        assert_eq!(
            without_tags("Robot Gardener (1993)(Nintendo)(USA)(En)"),
            "Robot Gardener"
        );
        assert_eq!(
            without_tags("Turbo Goose Racer (USA) v1.2"),
            "Turbo Goose Racer (USA) v1.2"
        );
        assert_eq!(without_tags("(Prototype)"), "(Prototype)");
        assert_eq!(without_tags("Hades"), "Hades");
    }

    #[test]
    fn bases_are_the_title_and_the_rom_without_tags() {
        let game = GameInfo {
            title: "MK3".into(),
            install: Some(InstalledInfo {
                executable: "C:\\roms\\Great Adventure (NA).sfc".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(emulated_bases(&game), ["MK3", "Great Adventure"]);
        let tagged = GameInfo {
            title: "Turbo Goose Racer (NA)".into(),
            install: Some(InstalledInfo {
                executable: "/roms/Turbo Goose Racer (NA).sfc".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(emulated_bases(&tagged), ["Turbo Goose Racer"]);
        assert_eq!(
            untagged_title(&tagged).as_deref(),
            Some("Turbo Goose Racer")
        );
    }

    #[test]
    fn picks_the_suffixed_entry_of_the_emulator_it_plays_in() {
        let bases = vec!["Crash Bandicoot".to_string()];
        let ranked = found(&[
            "Crash Bandicoot 2 (PS1)",
            "Crash Bandicoot (RetroArch)",
            "Crash Bandicoot (PS1)",
        ]);
        assert_eq!(
            pick_emulated(&ranked, &bases, Some(false)).as_deref(),
            Some("Crash Bandicoot (PS1)")
        );
        assert_eq!(
            pick_emulated(&ranked, &bases, Some(true)).as_deref(),
            Some("Crash Bandicoot (RetroArch)")
        );
        assert_eq!(
            pick_emulated(&ranked, &bases, None).as_deref(),
            Some("Crash Bandicoot (RetroArch)")
        );
        assert_eq!(pick_emulated(&found(&["Hades"]), &bases, None), None);
    }

    #[test]
    fn a_manifest_entry_that_kept_a_tag_still_fits() {
        let bases = vec!["Koushien".to_string()];
        let ranked = found(&["Koushien (Demo) (PS1)"]);
        assert_eq!(
            pick_emulated(&ranked, &bases, None).as_deref(),
            Some("Koushien (Demo) (PS1)")
        );
    }
}
