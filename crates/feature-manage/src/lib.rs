//! Management screens of Irori: the download queue, the Wine/Proton manager,
//! store accounts (with the browser login flow), store websites (in the store
//! window or the browser) and deals, and the
//! first-run onboarding.
//!
//! The shell owns routing and mounts these views for [`irori_ui::Route::Downloads`],
//! [`irori_ui::Route::WineManager`], [`irori_ui::Route::Accounts`] and
//! [`irori_ui::Route::Stores`]. It shows [`OnboardingView`] instead of the
//! library while [`should_show_onboarding`] is true, and removes it on
//! [`OnboardingEvent::Finished`]. Call [`init`] once at startup to register
//! this crate's translations.

mod accounts;
mod common;
mod deals;
mod downloads;
mod emulators;
mod format;
mod friends;
mod i18n;
mod launchers;
mod login;
mod onboarding;
mod steam_library;
mod store_window;
mod stores;
mod updates;
mod wine;
#[cfg(target_os = "linux")]
mod xbox;
/// Microsoft Store games play on Linux only; elsewhere there is no offline PC
/// to show.
#[cfg(not(target_os = "linux"))]
mod xbox {
    use gpui_kit::{App, Window};

    /// Always false: the Xbox store exists on Linux only.
    pub fn xbox_offline_available(_cx: &App) -> bool {
        false
    }

    /// Does nothing: the Xbox store exists on Linux only.
    pub fn open_offline_play_dialog(_window: &mut Window, _cx: &mut App) {}
}

pub use accounts::AccountsView;
pub use downloads::DownloadsView;
pub use emulators::{
    EmulatorsView, InstallTarget, choice_name, open_emulation_setup, open_engine_settings,
    start_install,
};
pub use friends::{install_steam_store, open_steam_friends_dialog, steam_friends_available};
pub use launchers::install_launchers_store;
pub use login::{open_account_login_dialog, open_login_dialog};
pub use onboarding::{
    OnboardingEvent, OnboardingView, mark_onboarding_done, should_show_onboarding,
};
pub use steam_library::{
    dismiss_steam_client_library_hint, install_steam_library_store, open_steam_library_dialog,
    steam_client_library_state, suggest_steam_client_library,
};
pub use store_window::{open_store_window, store_window_stores};
pub use stores::StoresView;
pub use updates::UpdatesView;
pub use wine::WineManagerView;
#[cfg(target_os = "linux")]
pub use xbox::install_xbox_store;
pub use xbox::{open_offline_play_dialog, xbox_offline_available};

pub use i18n::CATALOGS;

/// Register this crate's translations (`manage.` keys). Idempotent; the
/// views also call it on construction.
pub fn init(_cx: &mut gpui_kit::App) {
    i18n::register();
}

#[cfg(test)]
mod ui_tests;
