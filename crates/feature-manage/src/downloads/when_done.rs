//! "When downloads finish": put the computer to sleep or power it off once
//! the queue is done. The choice is the backend's, for this run only; when
//! the queue empties the backend counts a minute down, which every screen
//! shows with a way to cancel, and it never acts while a game runs.
//!
//! Also "Turn the screen off" while downloads run (the desktop turns it on
//! again at the next input; `irori_services::screen`), and the download
//! commands the command palette offers: pause, resume, the screen and this
//! choice, so all of it is reachable from the keyboard and a controller.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::{
    AnyElement, App, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div,
};
use irori_services::{DownloadQueueSnapshot, PowerAction, PowerCountdown};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

/// The actions the system allows here, asked once per run the first time
/// they're needed (asking logind is cheap, but not something to do at idle).
#[derive(Default)]
struct PowerChoices {
    actions: Option<Vec<PowerAction>>,
    asking: bool,
}

impl Global for PowerChoices {}

/// Of what [`irori_services::Backend::power_actions`] offers, what a
/// finished queue may do: sleep and power off. Returns `None` while the system
/// is still answering so callers don't flash a fallback button before redraw.
fn power_choices_loaded(cx: &mut App) -> Option<Vec<PowerAction>> {
    let state = cx.default_global::<PowerChoices>();
    if let Some(actions) = &state.actions {
        return Some(actions.clone());
    }
    if !state.asking {
        state.asking = true;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.power_actions().await) });
        cx.spawn(async move |cx| {
            let offered = task.await.unwrap_or_default();
            cx.update(|cx| {
                let choices = cx.default_global::<PowerChoices>();
                choices.actions = Some(finish_actions(&offered));
                choices.asking = false;
                cx.refresh_windows();
            });
        })
        .detach();
    }
    None
}

/// Of what [`irori_services::Backend::power_actions`] offers, what a
/// finished queue may do: sleep and power off. Empty until the system has
/// answered; the answer redraws the windows.
pub(crate) fn power_choices(cx: &mut App) -> Vec<PowerAction> {
    power_choices_loaded(cx).unwrap_or_default()
}

/// The finished-queue choices among what the system offers, in their order.
pub(crate) fn finish_actions(offered: &[PowerAction]) -> Vec<PowerAction> {
    [PowerAction::Sleep, PowerAction::PowerOff]
        .into_iter()
        .filter(|a| offered.contains(a))
        .collect()
}

/// The name of a choice in the menu.
pub(crate) fn choice_label(choice: Option<PowerAction>) -> SharedString {
    match choice {
        Some(PowerAction::Sleep) => t("manage.downloads.when_done.sleep"),
        Some(PowerAction::PowerOff) => t("manage.downloads.when_done.power_off"),
        _ => t("manage.downloads.when_done.nothing"),
    }
}

/// Seconds left of a countdown, rounded up so it never reads 0 before it
/// acts.
pub(crate) fn seconds_left(countdown: &PowerCountdown, now: Instant) -> u64 {
    let left = countdown.at.saturating_duration_since(now);
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

/// "Sleeping in 42 s" / "Powering off in 42 s".
pub(crate) fn countdown_text(countdown: &PowerCountdown, now: Instant) -> SharedString {
    let secs = seconds_left(countdown, now).to_string();
    match countdown.action {
        PowerAction::PowerOff => t_args(
            "manage.downloads.when_done.power_off_in",
            &[("seconds", &secs)],
        ),
        _ => t_args("manage.downloads.when_done.sleep_in", &[("seconds", &secs)]),
    }
}

/// How often a countdown on screen is redrawn.
pub(crate) const COUNTDOWN_TICK: Duration = Duration::from_secs(1);

/// Keep the computer on: cancel the countdown and the choice.
pub fn keep_on(cx: &App) {
    let b = backend(cx);
    b.set_when_downloads_done(None);
}

/// Unified power & display menu button shown while downloads run: labeled
/// `Stay on` (with caret) by default, `When done: Sleep` / `Power off` when
/// armed, or a direct `Turn the screen off` button when no power action is
/// available. Combines "Turn the screen off" into the menu when supported.
pub(crate) fn power_menu_button(queue: &DownloadQueueSnapshot, cx: &mut App) -> Option<AnyElement> {
    if queue.active.is_empty() {
        return None;
    }
    let choices = power_choices_loaded(cx)?;
    let can_screen_off = offers_screen_off(queue, cx);
    if choices.is_empty() && !can_screen_off {
        return None;
    }
    if choices.is_empty() && can_screen_off {
        return Some(
            Button::new("when-done")
                .outline()
                .icon(IconName::MonitorOff)
                .label(t("manage.downloads.screen_off"))
                .tooltip(t("manage.downloads.screen_off_tooltip"))
                .on_click(|_, _, cx| turn_screen_off(cx))
                .into_any_element(),
        );
    }
    let current = queue.when_done;
    let icon = match current {
        Some(PowerAction::PowerOff) => IconName::Power,
        _ => IconName::Moon,
    };
    let mut btn = Button::new("when-done")
        .outline()
        .icon(icon)
        .dropdown_caret(true);
    btn = if current.is_some() {
        btn.label(t_args(
            "manage.downloads.when_done.button",
            &[("choice", &choice_label(current))],
        ))
    } else {
        btn.label(choice_label(None)).tooltip(t_args(
            "manage.downloads.when_done.button",
            &[("choice", &choice_label(None))],
        ))
    };
    Some(
        btn.dropdown_menu(move |mut menu, _, _| {
            for choice in std::iter::once(None).chain(choices.iter().copied().map(Some)) {
                menu = menu.item(
                    PopupMenuItem::new(choice_label(choice))
                        .checked(choice == current)
                        .on_click(move |_, _, cx| {
                            backend(cx).set_when_downloads_done(choice);
                        }),
                );
            }
            if can_screen_off {
                menu = menu.separator().item(
                    PopupMenuItem::new(t("manage.downloads.screen_off"))
                        .icon(IconName::MonitorOff)
                        .on_click(|_, _, cx| turn_screen_off(cx)),
                );
            }
            menu
        })
        .into_any_element(),
    )
}

/// The countdown with its Cancel, while one runs.
pub(crate) fn countdown_banner(countdown: &PowerCountdown, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .id("power-countdown")
        .gap_3()
        .px_4()
        .py_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.warning)
        .child(Icon::new(IconName::Power).small().text_color(theme.warning))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_medium()
                .child(countdown_text(countdown, Instant::now())),
        )
        .child(
            Button::new("power-countdown-cancel")
                .small()
                .primary()
                .label(t("manage.downloads.when_done.cancel"))
                .on_click(|_, _, cx| keep_on(cx)),
        )
        .into_any_element()
}

/// Whether "Turn the screen off" is offered: downloads run, and this
/// desktop turns the screen on again by itself.
pub(crate) fn offers_screen_off(queue: &DownloadQueueSnapshot, cx: &App) -> bool {
    !queue.paused && !queue.active.is_empty() && backend(cx).can_turn_screen_off()
}

/// Turn the screen off now; a failure is told as a notice, since the screen
/// may already be dark.
pub fn turn_screen_off(cx: &mut App) {
    let b = backend(cx);
    let task = {
        let b = b.clone();
        irori_ui::run(cx, async move { b.turn_screen_off().await })
    };
    // Written now, in the interface language, for the notice.
    let title = t("manage.downloads.screen_off_failed").to_string();
    cx.spawn(async move |_| {
        if let Err(e) = task.await {
            b.bus.notice(
                irori_core::Notice::new(irori_core::NoticeLevel::Warning, title)
                    .message(format!("{e:#}")),
            );
        }
    })
    .detach();
}

/// A download command of the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadCommand {
    Pause,
    Resume,
    /// Turn the screen off while the downloads run.
    ScreenOff,
    /// What to do once the queue is done (`None`: nothing).
    WhenDone(Option<PowerAction>),
    /// Cancel the countdown before sleeping or powering off.
    KeepOn,
}

/// The download commands that apply now, with their label and icon.
pub fn download_commands(cx: &mut App) -> Vec<(SharedString, IconName, DownloadCommand)> {
    let queue = models(cx).downloads.read(cx).queue.clone();
    let mut rows = Vec::new();
    if queue.countdown.is_some() {
        rows.push((
            t("manage.downloads.command.keep_on"),
            IconName::Power,
            DownloadCommand::KeepOn,
        ));
    }
    if queue.active.is_empty() {
        return rows;
    }
    if offers_screen_off(&queue, cx) {
        rows.push((
            t("manage.downloads.command.screen_off"),
            IconName::MonitorOff,
            DownloadCommand::ScreenOff,
        ));
    }
    rows.push(if queue.paused {
        (
            t("manage.downloads.command.resume"),
            IconName::Play,
            DownloadCommand::Resume,
        )
    } else {
        (
            t("manage.downloads.command.pause"),
            IconName::Pause,
            DownloadCommand::Pause,
        )
    });
    for choice in std::iter::once(None).chain(power_choices(cx).into_iter().map(Some)) {
        if choice == queue.when_done {
            continue;
        }
        let (label, icon) = match choice {
            Some(PowerAction::PowerOff) => {
                (t("manage.downloads.command.power_off"), IconName::Power)
            }
            Some(_) => (t("manage.downloads.command.sleep"), IconName::Moon),
            None => (t("manage.downloads.command.nothing"), IconName::Clock),
        };
        rows.push((label, icon, DownloadCommand::WhenDone(choice)));
    }
    rows
}

/// Words the palette also finds the download commands by.
pub fn download_command_keywords() -> SharedString {
    t("manage.downloads.command.keywords")
}

/// Run a download command of the palette.
pub fn run_download_command(command: DownloadCommand, cx: &mut App) {
    let b = backend(cx);
    match command {
        DownloadCommand::Pause => b.pause_downloads(),
        DownloadCommand::Resume => b.resume_downloads(),
        DownloadCommand::ScreenOff => turn_screen_off(cx),
        DownloadCommand::WhenDone(choice) => b.set_when_downloads_done(choice),
        DownloadCommand::KeepOn => b.set_when_downloads_done(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sleep_and_power_off_finish_a_queue() {
        use PowerAction::*;
        assert_eq!(
            finish_actions(&[PowerOff, Restart, Sleep, LogOut]),
            [Sleep, PowerOff]
        );
        assert_eq!(finish_actions(&[Restart]), []);
    }

    #[test]
    fn countdown_reads_whole_seconds_rounded_up() {
        crate::i18n::register();
        let now = Instant::now();
        let countdown = PowerCountdown {
            action: PowerAction::Sleep,
            at: now + Duration::from_millis(41_200),
        };
        assert_eq!(seconds_left(&countdown, now), 42);
        assert_eq!(countdown_text(&countdown, now), "Sleeping in 42 s");
        let done = PowerCountdown {
            action: PowerAction::PowerOff,
            at: now,
        };
        assert_eq!(seconds_left(&done, now + Duration::from_secs(3)), 0);
        assert_eq!(countdown_text(&done, now), "Powering off in 0 s");
    }
}
