use super::chrome::{CompatBadgeTone, protondb_tone};
use super::*;

/// Epic games the EA app or Ubisoft Connect installs have no sizes; the
/// modal must offer Install, not "nothing to install" (#167).
#[test]
fn launcher_games_have_something_to_install() {
    let ea = GameInfo {
        runner: Some(Runner::Legendary),
        third_party_managed_app: Some("The EA App".into()),
        is_ea_managed: true,
        ..Default::default()
    };
    let info = InstallInfo {
        third_party_managed_app: Some("The EA App".into()),
        ..Default::default()
    };
    assert!(!nothing_to_install(&info, &ea));
    assert!(!nothing_to_install(&InstallInfo::default(), &ea));

    let goodies = GameInfo {
        runner: Some(Runner::Gog),
        ..Default::default()
    };
    assert!(nothing_to_install(&InstallInfo::default(), &goodies));
    let sized = InstallInfo {
        download_size: 1,
        ..Default::default()
    };
    assert!(!nothing_to_install(&sized, &goodies));
}

#[test]
fn only_gog_mentions_gog_bonus_content() {
    assert_eq!(
        nothing_to_install_text(Runner::Gog),
        "game.install.nothing_to_install"
    );
    for runner in [Runner::Legendary, Runner::Nile, Runner::Zoom, Runner::Itch] {
        assert_eq!(
            nothing_to_install_text(runner),
            "game.install.nothing_to_install_other"
        );
    }
}

#[test]
fn native_linux_is_the_default_platform() {
    let game = GameInfo {
        is_linux_native: true,
        platforms: vec![InstallPlatform::Windows, InstallPlatform::Linux],
        ..Default::default()
    };
    assert_eq!(
        installable_platforms(&game),
        if cfg!(target_os = "linux") {
            vec![InstallPlatform::Linux, InstallPlatform::Windows]
        } else {
            vec![InstallPlatform::Windows]
        }
    );
    let windows_only = GameInfo {
        platforms: vec![InstallPlatform::Windows, InstallPlatform::Mac],
        ..Default::default()
    };
    assert_eq!(
        installable_platforms(&windows_only),
        if cfg!(target_os = "macos") {
            vec![InstallPlatform::Mac, InstallPlatform::Windows]
        } else {
            vec![InstallPlatform::Windows]
        }
    );
    assert_eq!(
        installable_platforms(&GameInfo::default()),
        vec![InstallPlatform::Windows]
    );
}

#[test]
fn more_options_auto_expands_for_modify_or_missing_wine() {
    assert!(should_auto_expand_options(
        InstallMode::Modify,
        false,
        false,
        &ProtonDownload::Offered
    ));
    assert!(!should_auto_expand_options(
        InstallMode::Install,
        false,
        false,
        &ProtonDownload::Offered
    ));
    assert!(!should_auto_expand_options(
        InstallMode::Install,
        true,
        false,
        &ProtonDownload::Offered
    ));
    assert!(should_auto_expand_options(
        InstallMode::Install,
        true,
        true,
        &ProtonDownload::Offered
    ));
    assert!(should_auto_expand_options(
        InstallMode::Install,
        true,
        false,
        &ProtonDownload::Finding
    ));
    assert!(should_auto_expand_options(
        InstallMode::Install,
        true,
        false,
        &ProtonDownload::Running("ge".into())
    ));
    assert!(should_auto_expand_options(
        InstallMode::Install,
        true,
        false,
        &ProtonDownload::Failed("network".into())
    ));
    // On Windows (needs_wine == false), missing wine does not force expand.
    assert!(!should_auto_expand_options(
        InstallMode::Install,
        false,
        true,
        &ProtonDownload::Offered
    ));
}

#[test]
fn protondb_tone_is_case_insensitive() {
    assert_eq!(protondb_tone("Platinum"), CompatBadgeTone::Success);
    assert_eq!(protondb_tone("native"), CompatBadgeTone::Success);
    assert_eq!(protondb_tone("Gold"), CompatBadgeTone::Warning);
    assert_eq!(protondb_tone("gold"), CompatBadgeTone::Warning);
    assert_eq!(protondb_tone("Silver"), CompatBadgeTone::Warning);
    assert_eq!(protondb_tone("Bronze"), CompatBadgeTone::Warning);
    assert_eq!(protondb_tone("Borked"), CompatBadgeTone::Danger);
    assert_eq!(protondb_tone("Unknown"), CompatBadgeTone::Neutral);
}
