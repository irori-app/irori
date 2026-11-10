//! Which package of a Store product to install. A product can offer several
//! (a PC and an Xbox build of one bundle, editions of a game); the install
//! dialog lists them as builds and passes the chosen content ID back as
//! `InstallRequest::build`.

use irori_core::BuildInfo;
use irori_msstore_api::ops::package_plan::{Choice, Plan};

/// Packages that install as a game: supported and not an add-on.
fn installable(plan: &Plan) -> impl Iterator<Item = &Choice> {
    plan.choices
        .iter()
        .filter(|c| c.supported && c.kind != "Durable")
}

/// The package to install: `selected` (a content ID) when it is one of the
/// product's installable packages, else the only one, else the first PC game.
pub fn choose<'a>(plan: &'a Plan, selected: Option<&str>) -> Option<&'a Choice> {
    if let Some(selected) = selected.map(str::to_ascii_lowercase) {
        return installable(plan).find(|c| c.content_id.to_ascii_lowercase() == selected);
    }
    installable(plan)
        .find(|c| c.kind == "Game" && c.platform == "Windows.Desktop")
        .or_else(|| installable(plan).find(|c| c.kind == "Game"))
        .or_else(|| installable(plan).next())
}

/// The choice offered in the install dialog, when there is more than one.
pub fn builds(plan: &Plan) -> Vec<BuildInfo> {
    let choices: Vec<&Choice> = installable(plan).collect();
    if choices.len() < 2 {
        return Vec::new();
    }
    let mut out: Vec<BuildInfo> = choices
        .into_iter()
        .map(|c| BuildInfo {
            build_id: c.content_id.clone(),
            version_name: label(c),
            date: String::new(),
            branch: None,
        })
        .collect();
    // The default first, so the dialog starts on it.
    if let Some(default) = choose(plan, None)
        && let Some(at) = out.iter().position(|b| b.build_id == default.content_id)
    {
        let item = out.remove(at);
        out.insert(0, item);
    }
    out
}

/// "Title (PC)" or the title with the package's platforms.
fn label(choice: &Choice) -> String {
    let platform = match choice.platform.as_str() {
        "Windows.Desktop" => "PC",
        "Windows.Xbox" | "Xbox.One" => "Xbox",
        other => other,
    };
    if platform.is_empty() {
        choice.title.clone()
    } else {
        format!("{} ({platform})", choice.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(cid: &str, title: &str, platform: &str, kind: &str, supported: bool) -> Choice {
        Choice {
            product_id: "9NBLGGH4R315".into(),
            content_id: cid.into(),
            title: title.into(),
            platform: platform.into(),
            format: "MSIXVC".into(),
            kind: kind.into(),
            supported,
            parent_product_id: None,
            main_package_family_name: None,
            download_bytes: None,
            install_bytes: None,
        }
    }

    #[test]
    fn one_package_needs_no_choice() {
        let plan = Plan {
            choices: vec![choice("a", "Game", "Windows.Desktop", "Game", true)],
        };
        assert!(builds(&plan).is_empty());
        assert_eq!(choose(&plan, None).unwrap().content_id, "a");
    }

    #[test]
    fn several_packages_offer_the_pc_game_first() {
        let plan = Plan {
            choices: vec![
                choice("x", "Game", "Xbox.One", "Game", true),
                choice("p", "Game", "Windows.Desktop", "Game", true),
                choice("d", "Expansion", "Windows.Desktop", "Durable", true),
                choice("u", "Old", "Windows.Desktop", "Game", false),
            ],
        };
        let offered = builds(&plan);
        assert_eq!(offered.len(), 2);
        assert_eq!(offered[0].build_id, "p");
        assert_eq!(offered[0].version_name, "Game (PC)");
        assert_eq!(offered[1].version_name, "Game (Xbox)");
        assert_eq!(choose(&plan, None).unwrap().content_id, "p");
        assert_eq!(choose(&plan, Some("X")).unwrap().content_id, "x");
        // An add-on or an unsupported package is never installed as the game.
        assert!(choose(&plan, Some("d")).is_none());
        assert!(choose(&plan, Some("u")).is_none());
    }
}
