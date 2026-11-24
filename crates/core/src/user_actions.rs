//! Actions the user adds beside a game's Play button (`iroriActions`, per
//! game): a mod manager, a map tool, a server browser, the game's wiki. Each
//! is a program of this computer, a Windows program run in the game's
//! prefix, or a link, and a program may count as playing the game.

use serde::{Deserialize, Serialize};

/// One action of a game. Irori-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserAction {
    /// Shown in menus; the program's file name or the link's host when empty.
    pub name: String,
    /// A program (a file, or a command on `PATH`) or a link
    /// (`https://…`, `steam://…`).
    pub target: String,
    /// A program's arguments, split like its system splits them.
    pub args: String,
    /// Run the program with the game's Wine or Proton build, in its prefix.
    pub in_prefix: bool,
    /// While the program runs the game counts as playing: its status says
    /// so and the time adds to its playtime.
    pub counts_as_playing: bool,
}

impl UserAction {
    /// Whether the target is a link: a scheme of two letters or more and a
    /// colon. One letter is a Windows drive (`C:\…`).
    pub fn is_link(&self) -> bool {
        let target = self.target.trim();
        let Some((scheme, _)) = target.split_once(':') else {
            return false;
        };
        scheme.len() >= 2
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    }

    /// What menus call it.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        let target = self.target.trim();
        if self.is_link() {
            let rest = target.split_once("://").map_or(target, |(_, rest)| rest);
            return rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(rest)
                .to_string();
        }
        target
            .rsplit(['/', '\\'])
            .find(|s| !s.is_empty())
            .unwrap_or(target)
            .to_string()
    }

    /// Whether there is anything to run.
    pub fn is_set(&self) -> bool {
        !self.target.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(target: &str) -> UserAction {
        UserAction {
            target: target.into(),
            ..Default::default()
        }
    }

    #[test]
    fn links_are_told_from_programs() {
        assert!(action("https://wiki.example.org/Hades").is_link());
        assert!(action("steam://rungameid/620").is_link());
        assert!(action("ror2mm://v1/install").is_link());
        assert!(!action(r"C:\Tools\Map.exe").is_link());
        assert!(!action("/usr/bin/protonup-qt").is_link());
        assert!(!action("modorganizer").is_link());
        assert!(!action("1:2").is_link());
    }

    #[test]
    fn an_unnamed_action_is_called_after_its_target() {
        assert_eq!(
            action("https://wiki.example.org/Hades").label(),
            "wiki.example.org"
        );
        assert_eq!(action(r"C:\Tools\Map Editor.exe").label(), "Map Editor.exe");
        assert_eq!(action("/opt/r2/r2modman").label(), "r2modman");
        let named = UserAction {
            name: " Mod manager ".into(),
            ..action("x")
        };
        assert_eq!(named.label(), "Mod manager");
        assert!(!action("  ").is_set());
    }
}
