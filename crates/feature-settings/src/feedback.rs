//! Help and feedback (Settings → About and the command palette): report a
//! problem on Irori's issue tracker with this computer's facts filled in,
//! suggest an idea or ask a question there too, and read the known issues.
//!
//! Each opens GitHub's new-issue form in the browser with its label and a
//! body to fill in; nothing is sent until the user submits it there. The
//! repository has no discussions, and GitHub applies a label from the
//! address only for people who may triage, so every body also says first
//! what kind of issue it is. The facts it fills in are the version,
//! how Irori was installed, the system, desktop and graphics: never a path,
//! a user or account name or a log, which the user adds themselves.

use gpui_kit::assets::IconName;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Task, Window};
use irori_runtime::sysinfo::{self, SystemInfo};
use irori_services::app_update::InstallKind;
use irori_ui::i18n::t;

/// Irori's repository, whose issues take all three kinds of feedback.
pub const REPOSITORY: &str = "https://github.com/irori-app/irori";

/// The most of a report's body put in the address: browsers and GitHub
/// refuse very long addresses, and a long system section only pushes the
/// user's own words down.
const BODY_LIMIT: usize = 1500;

/// One way to reach the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feedback {
    /// A new issue with the system filled in.
    Report,
    /// A new issue labelled as an enhancement.
    Idea,
    /// A new issue labelled as a question.
    Question,
    /// The open issues, to see whether a problem is known.
    KnownIssues,
}

impl Feedback {
    pub const ALL: [Feedback; 4] = [
        Feedback::Report,
        Feedback::Idea,
        Feedback::Question,
        Feedback::KnownIssues,
    ];

    /// Translation key of its name.
    pub fn label_key(self) -> &'static str {
        match self {
            Feedback::Report => "settings.about.feedback.report",
            Feedback::Idea => "settings.about.feedback.idea",
            Feedback::Question => "settings.about.feedback.question",
            Feedback::KnownIssues => "settings.about.feedback.known_issues",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Feedback::Report => IconName::Bug,
            Feedback::Idea => IconName::Lightbulb,
            Feedback::Question => IconName::CircleQuestionMark,
            Feedback::KnownIssues => IconName::ListChecks,
        }
    }

    /// Translation key of the sentence saying what it does.
    pub fn help_key(self) -> &'static str {
        match self {
            Feedback::Report => "settings.about.feedback.report_help",
            Feedback::Idea => "settings.about.feedback.idea_help",
            Feedback::Question => "settings.about.feedback.question_help",
            Feedback::KnownIssues => "settings.about.feedback.known_issues_help",
        }
    }

    /// The repository's label for issues of this kind.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Feedback::Report => Some("bug"),
            Feedback::Idea => Some("enhancement"),
            Feedback::Question => Some("question"),
            Feedback::KnownIssues => None,
        }
    }

    /// The address; a report's without this computer's facts (see
    /// [`report_url`]).
    pub fn url(self) -> String {
        match self {
            Feedback::Report => new_issue("bug", &report_template("")),
            Feedback::Idea => new_issue("enhancement", IDEA_BODY),
            Feedback::Question => new_issue("question", QUESTION_BODY),
            Feedback::KnownIssues => format!("{REPOSITORY}/issues"),
        }
    }
}

/// An idea's body; its first line says the kind, for when the label from
/// the address isn't applied.
const IDEA_BODY: &str = "Kind: idea (enhancement)\n\n### What Irori could do\n\n\n\n\
                         ### Why it would help\n\n";

/// A question's body, its first line as [`IDEA_BODY`]'s.
const QUESTION_BODY: &str = "Kind: question\n\n### Your question\n\n\n\n\
                             ### What you tried\n\n";

/// The new-issue form with `label` and `body` filled in.
fn new_issue(label: &str, body: &str) -> String {
    let mut url =
        url::Url::parse(&format!("{REPOSITORY}/issues/new")).expect("the issue address is valid");
    url.query_pairs_mut()
        .append_pair("labels", label)
        .append_pair("body", body);
    url.into()
}

/// What a report says about this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportFacts {
    pub version: String,
    /// How Irori was installed, in words (`Flatpak`, `Homebrew`…).
    pub install: String,
    /// The system and its version.
    pub os: String,
    pub kernel: Option<String>,
    /// The desktop and its session type (`GNOME (wayland)`).
    pub desktop: Option<String>,
    /// Each graphics card with its driver.
    pub gpus: Vec<String>,
    pub vulkan: Option<bool>,
    /// `Steam Deck OLED`, when it is one.
    pub device: Option<String>,
}

impl ReportFacts {
    /// The facts of this computer as `sysinfo` found them.
    pub fn new(info: &SystemInfo, install: &InstallKind) -> Self {
        let os = match &info.os.version {
            Some(version) if !version.is_empty() => format!("{} {version}", info.os.name),
            _ => info.os.name.clone(),
        };
        let desktop = match (&info.desktop, &info.session_type) {
            (Some(d), Some(s)) => Some(format!("{d} ({s})")),
            (Some(d), None) => Some(d.clone()),
            (None, Some(s)) => Some(s.clone()),
            (None, None) => None,
        };
        let gpus = info
            .gpus
            .iter()
            .map(|g| {
                let name = [g.vendor_name.as_deref(), g.device_name.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                let name = if name.is_empty() {
                    format!("{}:{}", g.vendor_id, g.device_id)
                } else {
                    name
                };
                match (&g.driver, &g.driver_version) {
                    (Some(d), Some(v)) => format!("{name} ({d} {v})"),
                    (Some(d), None) => format!("{name} ({d})"),
                    _ => name,
                }
            })
            .collect();
        let device = info.steam_deck.map(|(model, _)| {
            match model {
                sysinfo::SteamDeckModel::Lcd => "Steam Deck LCD",
                sysinfo::SteamDeckModel::Oled => "Steam Deck OLED",
            }
            .to_string()
        });
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            install: install_words(install),
            os,
            kernel: (!info.os.kernel.is_empty()).then(|| info.os.kernel.clone()),
            desktop,
            gpus,
            vulkan: cfg!(target_os = "linux").then(|| info.vulkan.available()),
            device,
        }
    }
}

/// How Irori was installed, in the words of an issue.
fn install_words(install: &InstallKind) -> String {
    match install {
        InstallKind::Flatpak => "Flatpak",
        InstallKind::SystemPackage => "system package",
        InstallKind::Nix => "Nix",
        InstallKind::Source => "built from source",
        // The packager's text may name anything; the kind is enough.
        InstallKind::Packaged(_) => "distribution package",
        InstallKind::Homebrew => "Homebrew",
        InstallKind::Scoop => "Scoop",
        InstallKind::Winget => "winget",
        InstallKind::Manual => "download from the releases page",
    }
    .to_string()
}

/// The body of a new issue: the user's three questions, then the system.
/// Issues are read in English, so the template is too.
pub fn report_body(facts: &ReportFacts) -> String {
    let mut system = vec![
        format!("- Irori: {} ({})", facts.version, facts.install),
        format!("- System: {}", facts.os),
    ];
    if let Some(kernel) = &facts.kernel {
        system.push(format!("- Kernel: {kernel}"));
    }
    if let Some(desktop) = &facts.desktop {
        system.push(format!("- Desktop: {desktop}"));
    }
    if let Some(device) = &facts.device {
        system.push(format!("- Device: {device}"));
    }
    for gpu in &facts.gpus {
        system.push(format!("- GPU: {gpu}"));
    }
    if let Some(vulkan) = facts.vulkan {
        system.push(format!(
            "- Vulkan: {}",
            if vulkan { "available" } else { "not available" }
        ));
    }
    let mut lines = String::new();
    for line in system {
        // Leave out whole lines past the limit, never half of one.
        if report_template(&lines).len() + line.len() + 1 > BODY_LIMIT {
            break;
        }
        lines.push_str(&line);
        lines.push('\n');
    }
    report_template(&lines)
}

/// A report's body around the lines of its System section. Its first line
/// says the kind, for when the label from the address isn't applied.
fn report_template(system: &str) -> String {
    format!(
        "Kind: problem report (bug)\n\n### What happened\n\n\n\n\
         ### Steps to reproduce\n\n1. \n\n### What you expected\n\n\n\n\
         ### System\n\n{system}\n\nLogs help: Settings → Logs shows them. Paste \
         the part around the problem, after removing anything private."
    )
}

/// The new-issue address with the report's body filled in.
pub fn report_url(facts: &ReportFacts) -> String {
    new_issue("bug", &report_body(facts))
}

/// Open one of the ways to reach the project in the browser. A report
/// first reads this computer's facts, which takes a moment the first time;
/// the task ends when the browser was asked to open it.
pub fn open(feedback: Feedback, window: &mut Window, cx: &mut App) -> Task<()> {
    if feedback != Feedback::Report {
        cx.open_url(&feedback.url());
        return Task::ready(());
    }
    let task = irori_ui::run(cx, async move {
        let info = sysinfo::get(false).await;
        let install = tokio::task::spawn_blocking(InstallKind::detect)
            .await
            .unwrap_or(InstallKind::Manual);
        anyhow::Ok(report_url(&ReportFacts::new(&info, &install)))
    });
    window.spawn(cx, async move |cx| {
        let url = task.await;
        cx.update(|window, cx| match url {
            Ok(url) => cx.open_url(&url),
            Err(e) => window.push_notification(
                Notification::error(format!("{e:#}"))
                    .title(t("settings.about.feedback.report_failed")),
                cx,
            ),
        })
        .ok();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ReportFacts {
        ReportFacts {
            version: "1.2.0".into(),
            install: "Flatpak".into(),
            os: "Fedora Linux 41".into(),
            kernel: Some("6.11.4".into()),
            desktop: Some("GNOME (wayland)".into()),
            gpus: vec!["AMD Radeon RX 6800 (amdgpu)".into()],
            vulkan: Some(true),
            device: None,
        }
    }

    #[test]
    fn a_report_asks_the_three_questions_then_says_the_system() {
        let body = report_body(&facts());
        let asks = ["### What happened", "### Steps", "### What you expected"];
        let mut last = 0;
        for heading in asks.iter().chain(&["### System"]) {
            let at = body.find(heading).expect(heading);
            assert!(at >= last, "{heading} out of order");
            last = at;
        }
        assert!(body.contains("- Irori: 1.2.0 (Flatpak)"));
        assert!(body.contains("- Desktop: GNOME (wayland)"));
        assert!(body.contains("- GPU: AMD Radeon RX 6800 (amdgpu)"));
        assert!(body.contains("- Vulkan: available"));
    }

    #[test]
    fn a_long_system_is_cut_at_a_line_within_the_limit() {
        let mut many = facts();
        many.gpus = (0..200).map(|i| format!("GPU number {i}")).collect();
        let body = report_body(&many);
        assert!(body.len() <= BODY_LIMIT);
        assert!(body.contains("- GPU: GPU number 0\n"));
        assert!(!body.contains("GPU number 199"));
        assert!(body.ends_with("anything private."));
    }

    #[test]
    fn the_report_address_carries_the_body_encoded() {
        let url = report_url(&facts());
        assert!(url.starts_with("https://github.com/irori-app/irori/issues/new?"));
        let parsed = url::Url::parse(&url).unwrap();
        let body = parsed
            .query_pairs()
            .find(|(k, _)| k == "body")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(body, report_body(&facts()));
        assert!(!url.contains(' '), "spaces are encoded");
    }

    #[test]
    fn the_packagers_own_words_stay_out() {
        let words = install_words(&InstallKind::Packaged("/home/me/secret".into()));
        assert_eq!(words, "distribution package");
    }

    #[test]
    fn every_way_has_an_address_on_the_repository() {
        for way in Feedback::ALL {
            assert!(way.url().starts_with(REPOSITORY), "{way:?}");
            assert!(!way.url().contains("discussions"), "{way:?}");
        }
    }

    fn query(url: &str, key: &str) -> Option<String> {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }

    #[test]
    fn ideas_and_questions_are_labelled_issues_that_say_their_kind() {
        for (way, label, kind) in [
            (Feedback::Idea, "enhancement", "Kind: idea"),
            (Feedback::Question, "question", "Kind: question"),
            (Feedback::Report, "bug", "Kind: problem report"),
        ] {
            let url = way.url();
            assert!(
                url.starts_with(&format!("{REPOSITORY}/issues/new?")),
                "{way:?}"
            );
            assert_eq!(query(&url, "labels").as_deref(), Some(label));
            assert_eq!(way.label(), Some(label));
            // The label applies only for triagers; the body says it anyway.
            assert!(query(&url, "body").unwrap().starts_with(kind), "{way:?}");
        }
        let report = report_url(&facts());
        assert_eq!(query(&report, "labels").as_deref(), Some("bug"));
        let body = query(&report, "body").unwrap();
        assert!(body.starts_with("Kind: problem report"));
        assert!(body.contains("- Irori: 1.2.0 (Flatpak)"));
        assert_eq!(Feedback::KnownIssues.label(), None);
    }
}
