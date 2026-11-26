//! Crash reports. Release builds abort on a panic, so a crash used to leave
//! no trace: the window vanished and the log ended mid-session. A panic hook
//! now writes a report next to the log before the abort, and the next start
//! offers an actionable notification and sheet to review or share it.

use std::path::PathBuf;

use gpui_kit::{App, BorrowAppContext as _, Global};
use irori_core::{Paths, crash, util};

struct PendingCrash(Option<PathBuf>);

impl Global for PendingCrash {}

/// Write a crash report for every panic, then run the default hook (which
/// prints to stderr). Call once, before other threads start.
pub(crate) fn install() {
    let dir = Paths::get().logs_dir();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = report(
            info.payload_as_str().unwrap_or("(no message)"),
            info.location().map(|l| l.to_string()).as_deref(),
            &backtrace.to_string(),
        );
        // Plain file writes: the logger may be the thing that panicked.
        if let Some(path) = crash::write_report(&dir, util::unix_now(), &text) {
            eprintln!("Irori crashed; report saved to {}", path.display());
        }
        previous(info);
    }));
}

/// The text of a crash report.
fn report(message: &str, location: Option<&str>, backtrace: &str) -> String {
    let thread = std::thread::current();
    format!(
        "Irori {version} crashed\n\
         OS: {os} {arch}\n\
         Thread: {thread}\n\
         Location: {location}\n\
         Message: {message}\n\
         \n\
         Backtrace:\n{backtrace}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        thread = thread.name().unwrap_or("unnamed"),
        location = location.unwrap_or("unknown"),
    )
}

/// The newest unreported crash report, marked as reported once taken.
pub(crate) fn take_last_crash() -> Option<PathBuf> {
    crash::take_unreported(&Paths::get().logs_dir())
}

/// Stash `path` on GPUI globals so the first shell window can show it.
pub(crate) fn install_pending(path: Option<PathBuf>, cx: &mut App) {
    cx.set_global(PendingCrash(path));
}

/// Take the pending startup crash report once for the active shell window.
pub(crate) fn take_pending(cx: &mut App) -> Option<PathBuf> {
    cx.try_global::<PendingCrash>()?;
    cx.update_global::<PendingCrash, _>(|pending, _| pending.0.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_say_what_failed_where() {
        let text = report("index out of bounds", Some("src/a.rs:3:9"), "0: main");
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("Location: src/a.rs:3:9"));
        assert!(text.contains("Message: index out of bounds"));
        assert!(text.contains("0: main"));
    }
}
