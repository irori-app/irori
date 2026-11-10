//! UI integration tests: real views in headless windows over a backend with
//! no stores, under a temporary `IRORI_HOME`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Once};

use anyhow::{Result, bail};
use gpui_kit::{
    AppContext as _, Bounds, Modifiers, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, px, size,
};
use irori_core::{
    DynStore, ExtraInfo, GameId, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform,
    InstallRequest, LaunchPlan, LaunchRequest, LoginFlow, MAIN_ACCOUNT, OpContext, Outcome, Paths,
    RemoveArgs, Runner, Store, StoreAccount, UpdateRequest, UserInfo, async_trait,
};
use irori_services::{Backend, DownloadItem, DownloadKind, DownloadQueueSnapshot, DownloadState};
use irori_ui::models;

use crate::onboarding::OnboardingStep;
use crate::{
    AccountsView, DownloadsView, OnboardingEvent, OnboardingView, UpdatesView,
    should_show_onboarding,
};

fn setup(cx: &mut TestAppContext) {
    setup_with(cx, Vec::new);
}

fn setup_with(cx: &mut TestAppContext, stores: impl FnOnce() -> Vec<DynStore>) {
    // This integration test receives events from the real Tokio backend.
    cx.executor().allow_parking();
    static PATHS: Once = Once::new();
    PATHS.call_once(|| {
        let dir = tempfile::tempdir().expect("temp dir").keep();
        Paths::init(Paths::under(&dir));
    });
    let backend = Backend::start(|_, _| stores()).expect("backend starts");
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
    });
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is rendered"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn onboarding_walks_to_the_library(cx: &mut TestAppContext) {
    setup(cx);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| OnboardingView::with_heroic(false, window, cx))
        })
        .expect("window opens")
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&view, move |_, e: &OnboardingEvent, _| {
            sink.borrow_mut().push(*e)
        })
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    assert!(vcx.update(|_, cx| should_show_onboarding(cx)));

    let expected = [
        OnboardingStep::Welcome,
        OnboardingStep::Stores,
        OnboardingStep::InstallFolder,
        OnboardingStep::Done,
    ];
    for step in expected {
        assert_eq!(vcx.update(|_, cx| view.read(cx).step()), step);
        click(&mut vcx, "onboarding-next");
    }
    assert_eq!(*events.borrow(), vec![OnboardingEvent::Finished]);
    assert!(!vcx.update(|_, cx| should_show_onboarding(cx)));
}

fn queued(runner: Runner, app: &str, state: DownloadState) -> DownloadItem {
    DownloadItem {
        id: GameId::new(runner, app),
        title: format!("Game {app}"),
        kind: DownloadKind::Install(InstallRequest::default()),
        state,
        added_at: 0,
        started_at: None,
        finished_at: None,
        download_size: 1 << 30,
        error: None,
        progress: None,
    }
}

#[gpui_kit::test]
fn updates_page_lists_games_with_updates(cx: &mut TestAppContext) {
    setup(cx);
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| UpdatesView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let game = GameId::new(Runner::Gog, "a");
    assert!(vcx.debug_bounds("update-gog:a").is_none());

    // Drawn in the same step: the backend's own events replace the model's
    // updates with its real, empty set as soon as the test yields.
    vcx.update(|window, cx| {
        let library = models(cx).library.clone();
        library.update(cx, |l, _| l.updates = Arc::new([game].into()));
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(vcx.debug_bounds("update-gog:a").is_some());
}

#[gpui_kit::test]
fn downloads_show_items_of_every_store_in_queue_order(cx: &mut TestAppContext) {
    setup(cx);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("queue-zoom:b").is_none());

    vcx.update(|_, cx| {
        let downloads = models(cx).downloads.clone();
        downloads.update(cx, |d, cx| {
            d.queue = DownloadQueueSnapshot {
                active: vec![
                    queued(Runner::Gog, "a", DownloadState::Running),
                    queued(Runner::Zoom, "b", DownloadState::Queued),
                    queued(Runner::Sideload, "c", DownloadState::Queued),
                ],
                finished: vec![queued(Runner::Nile, "d", DownloadState::Failed)],
                paused: false,
            };
            cx.notify();
        });
    });
    vcx.run_until_parked();

    let current = vcx.update(|_, cx| {
        view.read(cx)
            .queue_for_tests()
            .current
            .as_ref()
            .map(|c| c.item.id.clone())
    });
    assert_eq!(current, Some(GameId::new(Runner::Gog, "a")));
    // ZOOM and manually added items render (Heroic hid them), in queue order.
    let zoom = vcx.debug_bounds("queue-zoom:b").expect("ZOOM row rendered");
    let other = vcx
        .debug_bounds("queue-sideload:c")
        .expect("sideload row rendered");
    assert!(zoom.top() < other.top());
    // The running item is the hero, not a queue row.
    assert!(vcx.debug_bounds("queue-gog:a").is_none());
}

/// A store with fixed accounts and no games.
struct TestStore {
    runner: Runner,
    accounts: Vec<StoreAccount>,
}

impl TestStore {
    /// A store nobody is signed in to.
    fn signed_out(runner: Runner) -> DynStore {
        Arc::new(Self {
            runner,
            accounts: Vec::new(),
        })
    }

    /// A GOG with two accounts: one signed in, one logged out that still has
    /// a game installed with it.
    fn two_accounts() -> DynStore {
        Arc::new(Self {
            runner: Runner::Gog,
            accounts: vec![
                StoreAccount {
                    slot: MAIN_ACCOUNT.into(),
                    account_id: "1".into(),
                    display_name: "Alice".into(),
                    signed_in: true,
                    games: 12,
                    ..Default::default()
                },
                StoreAccount {
                    slot: "2".into(),
                    account_id: "2".into(),
                    display_name: "Bob".into(),
                    installed: 1,
                    ..Default::default()
                },
            ],
        })
    }
}

#[async_trait]
impl Store for TestStore {
    fn runner(&self) -> Runner {
        self.runner
    }
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    fn user(&self) -> Option<UserInfo> {
        self.accounts
            .iter()
            .find(|a| a.signed_in)
            .map(|a| UserInfo {
                account_id: a.account_id.clone(),
                display_name: a.display_name.clone(),
            })
    }
    fn accounts(&self) -> Vec<StoreAccount> {
        self.accounts.clone()
    }
    fn can_add_account(&self) -> bool {
        !self.accounts.is_empty()
    }
    fn login_flow(&self) -> LoginFlow {
        LoginFlow::None
    }
    async fn login(&self, _: &str) -> Result<UserInfo> {
        bail!("unused")
    }
    async fn logout(&self) -> Result<()> {
        Ok(())
    }
    async fn refresh(&self, _: bool) -> Result<()> {
        Ok(())
    }
    fn games(&self) -> Vec<Arc<GameInfo>> {
        Vec::new()
    }
    fn game(&self, _: &str) -> Option<Arc<GameInfo>> {
        None
    }
    async fn extra_info(&self, _: &str) -> Result<ExtraInfo> {
        Ok(ExtraInfo::default())
    }
    async fn install_info(
        &self,
        _: &str,
        _: InstallPlatform,
        _: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        bail!("unused")
    }
    async fn updatable_games(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn install(&self, _: &str, _: &InstallRequest, _: OpContext) -> Result<Outcome> {
        bail!("unused")
    }
    async fn update(&self, _: &str, _: &UpdateRequest, _: OpContext) -> Result<Outcome> {
        bail!("unused")
    }
    async fn repair(&self, _: &str, _: OpContext) -> Result<Outcome> {
        bail!("unused")
    }
    async fn uninstall(&self, _: &str, _: RemoveArgs) -> Result<()> {
        bail!("unused")
    }
    async fn import(&self, _: &str, _: &Path, _: InstallPlatform) -> Result<()> {
        bail!("unused")
    }
    async fn set_install_path(&self, _: &str, _: &Path) -> Result<()> {
        bail!("unused")
    }
    async fn launch(&self, _: &str, _: &LaunchRequest, _: OpContext) -> Result<LaunchPlan> {
        bail!("unused")
    }
}

#[gpui_kit::test]
fn accounts_page_lists_every_account_of_a_store(cx: &mut TestAppContext) {
    setup_with(cx, || vec![TestStore::two_accounts()]);
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| AccountsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let alice = vcx
        .debug_bounds("account-gog-main")
        .expect("the signed-in account is listed");
    let bob = vcx
        .debug_bounds("account-gog-2")
        .expect("the logged-out account with an install is listed");
    assert!(alice.top() < bob.top(), "in the store's order");
    assert!(vcx.debug_bounds("add-account").is_some());
}

#[gpui_kit::test]
fn account_cards_are_all_as_tall_as_the_tallest(cx: &mut TestAppContext) {
    setup_with(cx, || {
        vec![
            TestStore::signed_out(Runner::Legendary),
            TestStore::two_accounts(),
            TestStore::signed_out(Runner::Nile),
            TestStore::signed_out(Runner::Zoom),
        ]
    });
    // Narrow enough for two rows of cards.
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(900.), px(800.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| AccountsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let mut card = |id: &'static str| {
        vcx.debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is rendered"))
    };
    let gog = card("account-gog");
    let cards = [
        card("account-legendary"),
        card("account-nile"),
        card("account-zoom"),
    ];
    assert!(
        cards.iter().any(|c| c.top() > gog.bottom()),
        "the cards take two rows"
    );
    for other in cards {
        assert_eq!(other.size.height, gog.size.height);
    }
}

/// Steam without an account offers to read its library from the Steam
/// client; once games were read, the card counts them and offers to remove
/// them.
#[gpui_kit::test]
fn steam_card_reads_the_library_from_the_client(cx: &mut TestAppContext) {
    use irori_core::{AppSettings, EventBus};

    let dir = tempfile::tempdir().expect("temp dir").keep();
    let steam = dir.join("Steam");
    std::fs::create_dir_all(steam.join("steamapps")).expect("mkdir");
    let data = steam.join("irori");
    std::fs::create_dir_all(data.join("cache")).expect("mkdir");
    let app = Arc::new(AppSettings::default());
    let dead = "http://127.0.0.1:9".to_string();
    let store = irori_steam::SteamStore::with_layout(
        EventBus::new(),
        Arc::new(move || app.clone()),
        steam,
        &data,
        irori_steam_api::Endpoints {
            api: dead.clone(),
            store: dead.clone(),
            assets: dead,
        },
    );
    let registered: DynStore = store.clone();
    setup_with(cx, move || vec![registered]);
    cx.update(|cx| crate::install_steam_library_store(store.clone(), cx));
    assert!(cx.update(|cx| crate::suggest_steam_client_library(cx)));
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| AccountsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("steam-library-read").is_some());
    assert!(vcx.debug_bounds("steam-library-forget").is_none());
    assert!(vcx.debug_bounds("steam-library-off").is_none());
    // Steam keeps no collections here, so the card says Irori can't see
    // the games hidden in Steam.
    assert!(vcx.debug_bounds("steam-hidden-note").is_some());
    let note = vcx.update(|_, cx| {
        crate::accounts::store_cards(cx)
            .into_iter()
            .find(|c| c.runner == Runner::Steam)
            .and_then(|c| c.steam_hidden)
    });
    assert_eq!(
        note,
        Some(irori_ui::i18n::t("manage.steam_hidden.unknown_local"))
    );
}

/// The Rockstar card of a prefix adopted with a game and a signed-in
/// account offers Close and Remove from Irori; removing brings Set up back
/// and leaves the prefix on disk.
#[cfg(not(windows))]
#[gpui_kit::test]
fn rockstar_card_after_adopting_a_prefix(cx: &mut TestAppContext) {
    use irori_core::{AppSettings, EventBus, WineInstallation};
    use irori_launchers::{LauncherKind, LaunchersStore};

    setup(cx);
    let dir = tempfile::tempdir().expect("temp dir").keep();
    let drive = dir.join("Rockstar/drive_c");
    let program = drive.join(LauncherKind::Rockstar.exe_path());
    std::fs::create_dir_all(program.parent().expect("parent")).expect("mkdir");
    std::fs::write(&program, b"MZ").expect("write");
    let log = drive.join("users/me/Documents/Rockstar Games/Launcher/launcher.log");
    std::fs::create_dir_all(log.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &log,
        "[t] [DISPLAY] [Main ] [launcher] Presence Event - Signed In\n",
    )
    .expect("write");

    let app = Arc::new(AppSettings {
        third_party_launchers: true,
        ..Default::default()
    });
    let store = LaunchersStore::with_dirs(
        EventBus::new(),
        Arc::new(move || app.clone()),
        dir.join("config"),
        dir.join("cache"),
    );
    let wine = WineInstallation {
        bin: "/usr/bin/wine".into(),
        name: "Wine".into(),
        ..Default::default()
    };
    let adopted = store.clone();
    let prefix = dir.join("Rockstar");
    let b = cx.update(|cx| irori_ui::backend(cx));
    b.settings.update_app(|s| s.third_party_launchers = true);
    futures::executor::block_on(b.spawn(async move {
        adopted
            .adopt(LauncherKind::Rockstar, prefix, Some(wine))
            .await
    }))
    .expect("task runs")
    .expect("adopts the prefix");

    cx.update(|cx| crate::install_launchers_store(store, cx));
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| crate::launchers::LaunchersSection::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("launcher-rockstar-signed-in").is_some());
    assert!(vcx.debug_bounds("launcher-rockstar-close").is_some());
    assert!(vcx.debug_bounds("launcher-rockstar-set-up").is_none());
    assert!(
        vcx.debug_bounds("launcher-xbox-windows-only").is_some(),
        "the Xbox app's card says it is Windows-only"
    );
    assert!(
        vcx.debug_bounds("launcher-ea-installed-only").is_some(),
        "the EA app's card says why owned games it hasn't installed aren't listed"
    );
    // The note takes the card's width, so its text wraps there instead of
    // keeping its own length and running over the neighbouring card. (The
    // test platform's glyphs are too narrow for the text to overflow, so
    // the width is what is checked.)
    vcx.simulate_resize(size(px(1400.), px(900.)));
    vcx.run_until_parked();
    let card = vcx
        .debug_bounds("launcher-xbox-card")
        .expect("the Xbox card");
    let note = vcx
        .debug_bounds("launcher-xbox-windows-only")
        .expect("the note");
    assert!(
        note.left() >= card.left() && note.right() <= card.right(),
        "{note:?} stays within {card:?}"
    );
    assert!(
        note.size.width >= card.size.width * 0.8,
        "{note:?} fills {card:?}"
    );
    if std::env::var_os(irori_launchers::XBOX_CLIENT_ID_VAR).is_none() {
        let sign_in = vcx
            .debug_bounds("launcher-xbox-sign-in")
            .expect("the Xbox account's sign-in, on every system");
        assert!(sign_in.right() <= card.right());
    }

    click(&mut vcx, "launcher-rockstar-remove");
    for _ in 0..50 {
        if vcx.debug_bounds("launcher-rockstar-set-up").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        vcx.run_until_parked();
    }
    assert!(vcx.debug_bounds("launcher-rockstar-set-up").is_some());
    assert!(vcx.debug_bounds("launcher-rockstar-close").is_none());
    assert!(program.is_file(), "the prefix stays on disk");
}

#[gpui_kit::test]
fn steam_friends_window_lists_friends_and_offers_only_real_joins(cx: &mut TestAppContext) {
    use irori_core::{AppSettings, EventBus};
    use irori_steam::{FriendLink, FriendState, Friends, SteamFriend, SteamStore};

    setup(cx);
    let dir = tempfile::tempdir().expect("temp dir").keep();
    let app = Arc::new(AppSettings::default());
    let store = SteamStore::with_layout(
        EventBus::new(),
        Arc::new(move || app.clone()),
        dir.join("Steam"),
        &dir.join("data"),
        irori_steam_api::Endpoints {
            api: "http://127.0.0.1:9".into(),
            store: "http://127.0.0.1:9".into(),
            assets: "http://127.0.0.1:9".into(),
        },
    );
    cx.update(|cx| crate::install_steam_store(store.clone(), cx));
    assert!(
        !cx.update(|cx| crate::steam_friends_available(cx)),
        "no account, no friends window"
    );
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                crate::friends::FriendsDialog::new(
                    Some(store),
                    None,
                    crate::FriendsSource::Steam,
                    window,
                    cx,
                )
            })
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    // The store answers on the backend's runtime, outside the test executor.
    let mut explained = false;
    for _ in 0..200 {
        vcx.run_until_parked();
        if vcx.update(|_, cx| view.read(cx).failure()).is_some() {
            explained = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(explained, "signed out explains itself");

    let friend =
        |steamid: u64, playing: Option<(u32, &str)>, join: Option<FriendLink>| SteamFriend {
            steamid,
            name: format!("Friend {steamid}"),
            avatar: String::new(),
            profile: format!("https://steamcommunity.com/profiles/{steamid}"),
            state: FriendState::Online,
            playing: playing.map(|(a, n)| (a, n.to_string())),
            join,
        };
    let friends = Friends {
        list: vec![
            friend(
                1,
                Some((730, "Counter-Strike 2")),
                Some(FriendLink::JoinLobby {
                    app: 730,
                    lobby: 5,
                    friend: 1,
                }),
            ),
            friend(2, Some((620, "Portal 2")), None),
            friend(3, None, None),
        ],
        read_at: None,
    };
    vcx.update(|_, cx| view.update(cx, |v, cx| v.show(friends, cx)));
    vcx.run_until_parked();
    for selector in ["friend-1", "friend-2", "friend-3"] {
        assert!(vcx.debug_bounds(selector).is_some(), "{selector} is listed");
    }
    assert!(vcx.debug_bounds("friend-join-1").is_some());
    assert!(vcx.debug_bounds("friend-join-2").is_none());
    assert!(vcx.debug_bounds("friend-join-3").is_none());

    // Without the launchers store there is no Xbox tab; the other sources
    // explain themselves.
    let (tabs, shown) = vcx.update(|_, cx| view.read(cx).shown_tabs());
    assert_eq!(
        (tabs, shown),
        (
            vec![crate::FriendsSource::Steam, crate::FriendsSource::Other],
            crate::FriendsSource::Steam
        )
    );
    vcx.update(|_, cx| view.update(cx, |v, cx| v.show_tab(crate::FriendsSource::Other, cx)));
    vcx.run_until_parked();
    for selector in [
        "friends-other-discord",
        "friends-other-epic",
        "friends-other-gog",
        "friends-other-launchers",
    ] {
        assert!(vcx.debug_bounds(selector).is_some(), "{selector} is shown");
    }
    assert!(vcx.debug_bounds("friend-1").is_none());
}

#[gpui_kit::test]
fn xbox_friends_have_a_tab_of_their_own(cx: &mut TestAppContext) {
    use irori_core::{AppSettings, EventBus};
    use irori_launchers::{LaunchersStore, XboxFriend, XboxFriends, XboxOnlineState};

    setup(cx);
    if std::env::var_os(irori_launchers::XBOX_CLIENT_ID_VAR).is_some() {
        // A packager's build may turn the Xbox account off.
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir").keep();
    let app = Arc::new(AppSettings::default());
    let store = LaunchersStore::with_dirs(
        EventBus::new(),
        Arc::new(move || app.clone()),
        dir.join("config"),
        dir.join("cache"),
    );
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                crate::friends::FriendsDialog::new(
                    None,
                    Some(store),
                    crate::FriendsSource::Xbox,
                    window,
                    cx,
                )
            })
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    let mut explained = false;
    for _ in 0..200 {
        vcx.run_until_parked();
        if vcx.update(|_, cx| view.read(cx).failure()).is_some() {
            explained = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(explained, "signed out explains itself");
    let (tabs, shown) = vcx.update(|_, cx| view.read(cx).shown_tabs());
    assert_eq!(
        (tabs, shown),
        (
            vec![crate::FriendsSource::Xbox, crate::FriendsSource::Other],
            crate::FriendsSource::Xbox
        )
    );

    let friend = |xuid: &str, playing: Option<&str>| XboxFriend {
        xuid: xuid.into(),
        gamertag: format!("Tag {xuid}"),
        picture: String::new(),
        state: XboxOnlineState::Online,
        playing: playing.map(str::to_string),
        follows_back: true,
        favorite: false,
    };
    let friends = XboxFriends {
        list: vec![friend("31", Some("Halo Infinite")), friend("32", None)],
        read_at: None,
    };
    vcx.update(|_, cx| view.update(cx, |v, cx| v.show_xbox(friends, cx)));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("xbox-friend-31").is_some());
    assert!(vcx.debug_bounds("xbox-friend-32").is_some());
    // Read only: nothing to join or message.
    assert!(vcx.debug_bounds("friend-join-31").is_none());
}
