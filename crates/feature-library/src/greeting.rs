//! Time-of-day greeting and friendly OS first-name extraction for the library
//! header. Pure functions are separated from clock/environment reads so they
//! are deterministically unit-tested.

use chrono::Timelike as _;
use gpui_kit::SharedString;
use irori_ui::i18n::{t, t_args};

/// Broad time-of-day bucket for the library header greeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartOfDay {
    Morning,
    Afternoon,
    Evening,
}

/// Map a 24-hour local clock hour (`0..=23`) to a [`PartOfDay`].
pub fn part_of_day(hour: u32) -> PartOfDay {
    match hour {
        5..=11 => PartOfDay::Morning,
        12..=17 => PartOfDay::Afternoon,
        _ => PartOfDay::Evening,
    }
}

/// Current local [`PartOfDay`].
pub fn current_part_of_day() -> PartOfDay {
    part_of_day(chrono::Local::now().hour())
}

impl PartOfDay {
    /// Greeting text split cleanly around `{name}` so the name itself can be
    /// rendered as a clickable interactive span while respecting word order in
    /// every locale.
    pub fn split_greeting(self) -> (SharedString, SharedString) {
        const MARKER: &str = "\u{1F}";
        let key = match self {
            PartOfDay::Morning => "library.greeting.morning",
            PartOfDay::Afternoon => "library.greeting.afternoon",
            PartOfDay::Evening => "library.greeting.evening",
        };
        let rendered = t_args(key, &[("name", MARKER)]);
        match rendered.split_once(MARKER) {
            Some((before, after)) => (
                SharedString::from(before.to_owned()),
                SharedString::from(after.to_owned()),
            ),
            None => (
                SharedString::from(format!("{rendered} ")),
                SharedString::default(),
            ),
        }
    }

    /// Plain greeting when no user name can be resolved.
    pub fn plain_greeting(self) -> SharedString {
        t(match self {
            PartOfDay::Morning => "library.greeting.morning_plain",
            PartOfDay::Afternoon => "library.greeting.afternoon_plain",
            PartOfDay::Evening => "library.greeting.evening_plain",
        })
    }
}

/// Turn a raw OS username (such as `eduardo.junior`, `DOMAIN\john_doe`, or
/// `alice-smith`) into a clean capitalised first name (`Eduardo`, `John`,
/// `Alice`). Returns `None` if nothing usable remains.
pub fn extract_first_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let after_domain = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    let token = after_domain
        .split(['.', '_', '-', ' ', '\t'])
        .find(|part| !part.is_empty())?;
    let mut chars = token.chars();
    let first = chars.next()?;
    let rest: String = chars.collect();
    let mut out = String::new();
    for ch in first.to_uppercase() {
        out.push(ch);
    }
    let all_upper = !rest.is_empty() && rest.chars().all(|c| !c.is_lowercase());
    if all_upper {
        out.push_str(&rest.to_lowercase());
    } else {
        out.push_str(&rest);
    }
    Some(out)
}

/// First name derived from `USER`, else `USERNAME`.
pub fn os_first_name() -> Option<String> {
    let raw = std::env::var("USER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USERNAME").ok())?;
    extract_first_name(&raw)
}

/// Resolve the displayed greeting name: user's custom name (if non-empty)
/// takes precedence over [`os_first_name`].
pub fn resolve_name(custom: Option<&str>) -> Option<String> {
    if let Some(name) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(name.to_owned());
    }
    os_first_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_of_day_covers_all_hours() {
        for h in 0..=4 {
            assert_eq!(part_of_day(h), PartOfDay::Evening, "hour {h}");
        }
        for h in 5..=11 {
            assert_eq!(part_of_day(h), PartOfDay::Morning, "hour {h}");
        }
        for h in 12..=17 {
            assert_eq!(part_of_day(h), PartOfDay::Afternoon, "hour {h}");
        }
        for h in 18..=23 {
            assert_eq!(part_of_day(h), PartOfDay::Evening, "hour {h}");
        }
    }

    #[test]
    fn extracts_first_name_from_os_usernames() {
        assert_eq!(
            extract_first_name("eduardo.junior").as_deref(),
            Some("Eduardo")
        );
        assert_eq!(
            extract_first_name("CORP\\john_doe").as_deref(),
            Some("John")
        );
        assert_eq!(extract_first_name("ALICE").as_deref(), Some("Alice"));
        assert_eq!(extract_first_name("McFly").as_deref(), Some("McFly"));
        assert_eq!(
            extract_first_name("éloïse-marie").as_deref(),
            Some("Éloïse")
        );
        assert_eq!(extract_first_name("  ana silva ").as_deref(), Some("Ana"));
        assert_eq!(extract_first_name(""), None);
        assert_eq!(extract_first_name("   ._- "), None);
    }

    #[test]
    fn custom_name_overrides_os_name() {
        assert_eq!(resolve_name(Some("  Borges ")).as_deref(), Some("Borges"));
    }

    #[test]
    fn hearth_greetings_split_around_name_and_have_plain_forms() {
        crate::i18n::register();
        assert_eq!(
            PartOfDay::Morning.split_greeting(),
            (
                SharedString::from("The hearth is warm, "),
                SharedString::default()
            )
        );
        assert_eq!(
            PartOfDay::Afternoon.split_greeting(),
            (
                SharedString::from("Gather by the fire, "),
                SharedString::default()
            )
        );
        assert_eq!(
            PartOfDay::Evening.split_greeting(),
            (
                SharedString::from("Settle by the hearth, "),
                SharedString::default()
            )
        );
        assert_eq!(
            PartOfDay::Morning.plain_greeting(),
            SharedString::from("The hearth is warm")
        );
        assert_eq!(
            PartOfDay::Afternoon.plain_greeting(),
            SharedString::from("Gather by the fire")
        );
        assert_eq!(
            PartOfDay::Evening.plain_greeting(),
            SharedString::from("Settle by the hearth")
        );
    }
}
