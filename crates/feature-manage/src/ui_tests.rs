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
use crate::{AccountsView, DownloadsView, OnboardingEvent, OnboardingView, should_show_onboarding};

pub(crate) fn setup(cx: &mut TestAppContext) {
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
    let backend = Backend::start_for_tests(|_, _| stores()).expect("backend starts");
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
fn downloads_page_centers_empty_state_and_lists_game_updates(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1000.), px(800.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let game = GameId::new(Runner::Gog, "a");
    assert!(vcx.debug_bounds("update-gog:a").is_none());

    // The idle empty state must be vertically centered in the 800 px viewport,
    // not collapsed at the top of a scroll container.
    let empty = vcx
        .debug_bounds("downloads-empty")
        .expect("idle empty state is rendered");
    assert!(
        empty.center().y > px(300.) && empty.center().y < px(600.),
        "empty state center {:?} should be vertically centered in 800 px window",
        empty.center()
    );

    // Drawn in the same step: the backend's own events replace the model's
    // updates with its real, empty set as soon as the test yields.
    vcx.update(|window, cx| {
        let library = models(cx).library.clone();
        library.update(cx, |l, _| l.updates = Arc::new([game].into()));
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(vcx.debug_bounds("update-gog:a").is_some());
    assert!(vcx.debug_bounds("downloads-empty").is_none());

    // Clear game updates and surface an Irori self-update: the top banner renders
    // cleanly without showing the empty state, then returns to empty when cleared.
    vcx.update(|window, cx| {
        let library = models(cx).library.clone();
        library.update(cx, |l, _| l.updates = Arc::new(Default::default()));
        let app_update = models(cx).app_update.clone();
        app_update.update(cx, |u, _| {
            u.status.kind = irori_services::InstallKind::Manual;
            u.status.available = Some(irori_services::Release {
                version: "9.9.9".into(),
                url: "https://github.com/irori-app/irori/releases/tag/v9.9.9".into(),
            });
            u.status.install = irori_services::SelfInstall::Ready {
                version: "9.9.9".into(),
            };
        });
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("downloads-empty").is_none());

    vcx.update(|window, cx| {
        let app_update = models(cx).app_update.clone();
        app_update.update(cx, |u, _| {
            u.status.available = None;
            u.status.install = irori_services::SelfInstall::Idle;
        });
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(vcx.debug_bounds("irori-self-update-banner").is_none());
    assert!(vcx.debug_bounds("downloads-empty").is_some());
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
                ..Default::default()
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

#[gpui_kit::test]
fn downloads_header_wraps_cleanly_in_narrow_window(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(800.), px(700.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    let update_game = GameId::new(Runner::Gog, "upd");
    vcx.update(|window, cx| {
        let downloads = models(cx).downloads.clone();
        downloads.update(cx, |d, cx| {
            d.queue = DownloadQueueSnapshot {
                active: vec![
                    queued(Runner::Gog, "a", DownloadState::Running),
                    queued(Runner::Zoom, "b", DownloadState::Queued),
                ],
                paused: false,
                ..Default::default()
            };
            cx.notify();
        });
        let library = models(cx).library.clone();
        library.update(cx, |l, cx| {
            l.updates = Arc::new([update_game].into());
            cx.notify();
        });
        window.refresh();
        let _ = window.draw(cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    let title = vcx
        .debug_bounds("downloads-title")
        .expect("header title renders");
    let actions = vcx
        .debug_bounds("downloads-header-actions")
        .expect("header actions render");
    let settings = vcx
        .debug_bounds("downloads-settings")
        .expect("settings button renders");

    assert!(
        title.size.width >= px(100.),
        "title should not collapse on an 800 px window: {title:?}"
    );
    assert!(
        actions.right() <= px(800.),
        "header actions must stay within 800 px window: {actions:?}"
    );
    assert!(
        settings.right() <= px(800.),
        "settings gear must stay within 800 px window: {settings:?}"
    );

    click(&mut vcx, "downloads-settings");
    let route = vcx.read(|cx| irori_ui::nav::navigator(cx).read(cx).current().clone());
    assert_eq!(route, irori_ui::Route::Settings("downloads".into()));
}

#[gpui_kit::test]
fn downloads_history_only_shows_idle_banner_and_opens_game_page(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1000.), px(800.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    vcx.update(|window, cx| {
        let downloads = models(cx).downloads.clone();
        downloads.update(cx, |d, cx| {
            d.queue = DownloadQueueSnapshot {
                active: Vec::new(),
                finished: vec![queued(Runner::Gog, "dawn", DownloadState::Done)],
                paused: false,
                ..Default::default()
            };
            cx.notify();
        });
        window.refresh();
        let _ = window.draw(cx);
    });

    assert!(vcx.debug_bounds("downloads-empty").is_none());
    assert!(
        vcx.debug_bounds("downloads-idle-banner").is_some(),
        "compact idle banner should stay visible above finished history"
    );

    click(&mut vcx, "history-open-gog:dawn-0");
    let route = vcx.read(|cx| irori_ui::nav::navigator(cx).read(cx).current().clone());
    assert_eq!(
        route,
        irori_ui::Route::Game(GameId::new(Runner::Gog, "dawn"))
    );
}

#[gpui_kit::test]
fn downloads_empty_renders_multiple_disk_pills(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1000.), px(800.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    vcx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_install_disks_for_tests(vec![
                irori_services::QueueDisk {
                    folder: std::path::PathBuf::from("/home/edward/Games/Heroic"),
                    label: Some("/ · Reletech P400 EVO 2TB".into()),
                    needed: 0,
                    free: Some(1_300_u64 << 30),
                    total: Some(1_900_u64 << 30),
                },
                irori_services::QueueDisk {
                    folder: std::path::PathBuf::from("/mnt/Asgard"),
                    label: Some("/mnt/Asgard".into()),
                    needed: 0,
                    free: Some(177_u64 << 30),
                    total: Some(938_u64 << 30),
                },
            ]);
            cx.notify();
        });
        window.refresh();
        let _ = window.draw(cx);
    });

    let first = vcx
        .debug_bounds("downloads-idle-disk-0")
        .expect("first disk pill rendered");
    let second = vcx
        .debug_bounds("downloads-idle-disk-1")
        .expect("second disk pill rendered");
    assert!(
        first.right() <= second.left() || first.bottom() <= second.top(),
        "disk pills should not overlap: first={first:?}, second={second:?}"
    );
}

/// A store with fixed accounts and no games.
struct TestStore {
    runner: Runner,
    accounts: Vec<StoreAccount>,
    user: Option<UserInfo>,
}

impl TestStore {
    /// A store nobody is signed in to.
    fn signed_out(runner: Runner) -> DynStore {
        Arc::new(Self {
            runner,
            accounts: Vec::new(),
            user: None,
        })
    }

    /// A store with one signed-in account.
    fn signed_in(runner: Runner, name: &str) -> DynStore {
        Arc::new(Self {
            runner,
            accounts: vec![StoreAccount {
                slot: MAIN_ACCOUNT.into(),
                account_id: "1".into(),
                display_name: name.into(),
                signed_in: true,
                games: 8,
                ..Default::default()
            }],
            user: None,
        })
    }

    /// A local store detected on disk (such as Steam without a web account).
    fn local_found(runner: Runner) -> DynStore {
        Arc::new(Self {
            runner,
            accounts: Vec::new(),
            user: Some(UserInfo {
                account_id: "local".into(),
                display_name: "On disk".into(),
            }),
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
            user: None,
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
        self.user.clone().or_else(|| {
            self.accounts
                .iter()
                .find(|a| a.signed_in)
                .map(|a| UserInfo {
                    account_id: a.account_id.clone(),
                    display_name: a.display_name.clone(),
                })
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
    // Narrow enough for two rows in the available section.
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
    // Card grids measure their width during paint and call `window.refresh()`.
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    vcx.run_until_parked();
    let mut card = |id: &'static str| {
        vcx.debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is rendered"))
    };
    let gog = card("account-gog");
    let legendary = card("account-legendary");
    let nile = card("account-nile");
    let zoom = card("account-zoom");
    assert!(
        gog.bottom() < legendary.top(),
        "connected stores appear above available stores"
    );
    assert!(
        zoom.top() > legendary.bottom(),
        "available cards take two rows on narrow windows"
    );
    for other in [nile, zoom] {
        assert_eq!(other.size.height, legendary.size.height);
    }
}

#[gpui_kit::test]
fn accounts_page_separates_sections_and_features_steam_on_wide_window(cx: &mut TestAppContext) {
    setup_with(cx, || {
        vec![
            TestStore::signed_in(Runner::Legendary, "EpicPlayer"),
            TestStore::two_accounts(),
            TestStore::signed_out(Runner::Nile),
            TestStore::signed_out(Runner::Zoom),
            TestStore::signed_out(Runner::Itch),
            TestStore::local_found(Runner::Steam),
        ]
    });
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(2200.), px(1000.)),
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
    // Card grids measure their width during paint and call `window.refresh()`.
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    vcx.run_until_parked();
    let mut card = |id: &'static str| {
        vcx.debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is rendered"))
    };
    let legendary = card("account-legendary");
    let gog = card("account-gog");
    let steam = card("account-steam");
    let nile = card("account-nile");
    let zoom = card("account-zoom");
    let itch = card("account-itch");

    assert_eq!(legendary.top(), gog.top(), "2-column connected grid");
    assert!(
        steam.top() > gog.bottom(),
        "featured Steam card sits below the connected account grid"
    );
    assert!(
        steam.size.width > gog.size.width * 1.5,
        "Steam spans full width instead of squeezing into one column"
    );
    assert!(
        nile.top() > steam.bottom(),
        "available section follows connected"
    );
    assert_eq!(nile.top(), zoom.top());
    assert!(
        itch.top() > zoom.bottom(),
        "turned-off stores appear in the inactive section below available stores"
    );
}

#[gpui_kit::test]
fn accounts_page_caps_available_grid_columns_on_ultrawide(cx: &mut TestAppContext) {
    setup_with(cx, || {
        vec![
            TestStore::signed_out(Runner::Legendary),
            TestStore::signed_out(Runner::Gog),
            TestStore::signed_out(Runner::Nile),
            TestStore::signed_out(Runner::Zoom),
        ]
    });
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(2400.), px(900.)),
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
    // Card grids measure their width during paint and call `window.refresh()`.
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    vcx.run_until_parked();
    let mut card = |id: &'static str| {
        vcx.debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} is rendered"))
    };
    let legendary = card("account-legendary");
    let gog = card("account-gog");
    let nile = card("account-nile");
    let zoom = card("account-zoom");
    assert_eq!(legendary.top(), gog.top());
    assert_eq!(gog.top(), nile.top());
    assert!(
        zoom.top() > legendary.bottom(),
        "4 available cards wrap to a second row even on ultrawide screens"
    );
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
            login: dead.clone(),
            assets: dead,
        },
    );
    let registered: DynStore = store.clone();
    setup_with(cx, move || vec![registered]);
    cx.update(|cx| crate::install_steam_library_store(store.clone(), cx));
    // The store finds Steam in `init`, on the backend's own runtime, which a
    // busy runner may not have reached yet.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !store.client_library_state().steam_found && std::time::Instant::now() < deadline {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
            login: "http://127.0.0.1:9".into(),
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

#[gpui_kit::test]
fn steam_sign_in_view_renders_both_tabs_and_validates_api_key(cx: &mut TestAppContext) {
    use crate::login::steam::{Mode, SteamSignInView, SteamSignedIn};

    setup(cx);
    let emitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = emitted.clone();
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let view = cx.new(|cx| SteamSignInView::new(window, cx));
            cx.subscribe(&view, move |_, event: &SteamSignedIn, _| {
                sink.lock().unwrap().push(event.0.clone());
            })
            .detach();
            view
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    // Default tab is Steam website, showing the full-width tab bar with
    // equal-width tabs and either the open button (when irori-sign-in is
    // present) or the API key fallback button.
    let bar = vcx
        .debug_bounds("steam-sign-in-mode")
        .expect("steam mode tab bar rendered");
    let web_tab = vcx
        .debug_bounds("steam-tab-web")
        .expect("steam web tab rendered");
    let api_tab = vcx
        .debug_bounds("steam-tab-api")
        .expect("steam api tab rendered");
    assert_eq!(web_tab.top(), api_tab.top());
    assert!((web_tab.size.width - api_tab.size.width).abs() < px(2.));
    assert!((web_tab.size.width + api_tab.size.width) > bar.size.width * 0.9);
    assert!(
        vcx.debug_bounds("steam-web-open").is_some()
            || vcx.debug_bounds("steam-web-use-api-key").is_some()
    );

    // Switch to the guided Steam Web API key tab.
    vcx.update(|window, cx| {
        view.update(cx, |v, cx| v.set_mode(Mode::ApiKey, window, cx));
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("steam-api-open-key-page").is_some());
    assert!(vcx.debug_bounds("steam-api-open-privacy-page").is_some());
    assert!(vcx.debug_bounds("steam-api-connect").is_some());

    // Invalid API key reports an inline error and does not emit.
    vcx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_api_inputs_for_test("76561197960265729", "too-short", window, cx);
        });
    });
    vcx.run_until_parked();
    click(&mut vcx, "steam-api-connect");
    vcx.run_until_parked();
    assert!(vcx.update(|_, cx| view.read(cx).error().is_some()));
    assert!(emitted.lock().unwrap().is_empty());

    // Valid SteamID64 + 32-char hex API key emits the API key login payload.
    vcx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_api_inputs_for_test(
                "76561197960265729",
                "0123456789ABCDEF0123456789ABCDEF",
                window,
                cx,
            );
        });
    });
    vcx.run_until_parked();
    click(&mut vcx, "steam-api-connect");
    vcx.run_until_parked();
    assert!(vcx.update(|_, cx| view.read(cx).error().is_none()));
    let sent = emitted.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("apiKey"));
    assert!(sent[0].contains("76561197960265729"));
}

#[gpui_kit::test]
fn steam_library_dialog_keeps_cancel_button_inside_footer(cx: &mut TestAppContext) {
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
            login: dead.clone(),
            assets: dead,
        },
    );
    let registered: DynStore = store.clone();
    setup_with(cx, move || vec![registered]);
    cx.update(|cx| crate::install_steam_library_store(store.clone(), cx));
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| AccountsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        crate::steam_library::open_steam_library_dialog(window, cx);
    });
    vcx.run_until_parked();

    let footer = vcx
        .debug_bounds("steam-library-dialog-footer")
        .expect("dialog footer rendered");
    let cancel = vcx
        .debug_bounds("steam-library-dialog-cancel")
        .expect("dialog cancel button rendered");
    assert!(
        cancel.left() >= footer.left(),
        "cancel button must stay within dialog footer left edge"
    );
    assert!(
        cancel.right() <= footer.right(),
        "cancel button must stay within dialog footer right edge"
    );
}

#[gpui_kit::test]
fn wine_manager_fills_width_and_switches_large_tabs(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|_| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            Default::default(),
            size(px(2400.), px(1000.)),
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| crate::WineManagerView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    let viewport = vcx.update(|window, _| window.viewport_size());
    let header = vcx
        .debug_bounds("wine-header")
        .expect("wine header is rendered");
    assert!(
        header.size.width >= px(2300.) && header.size.width >= viewport.width - px(2.),
        "wine manager fills width: header={:?}, viewport={:?}",
        header.size.width,
        viewport.width
    );

    click(&mut vcx, "wine-tab-system");
    assert!(vcx.debug_bounds("wine-content").is_some());

    click(&mut vcx, "wine-tab-prefixes");
    assert!(vcx.debug_bounds("wine-header").is_some());
}

#[gpui_kit::test]
fn emulators_page_constrains_width_and_hides_search_on_achievements(cx: &mut TestAppContext) {
    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(2400.), px(1000.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, _view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| crate::EmulatorsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    let content = vcx
        .debug_bounds("emulators-content")
        .expect("emulators content is rendered");
    assert!(
        content.size.width < px(2000.),
        "emulators page keeps a comfortable max width on ultrawide windows"
    );
    assert!(vcx.debug_bounds("emulators-search").is_some());

    click(&mut vcx, "emulators-tab-systems");
    assert!(vcx.debug_bounds("emulators-search").is_some());

    click(&mut vcx, "emulators-tab-achievements");
    assert!(
        vcx.debug_bounds("emulators-search").is_none(),
        "search input is hidden on the Achievements tab"
    );
}

fn open_emulators_window(cx: &mut TestAppContext, w: f32, h: f32) -> VisualTestContext {
    let (handle, _) = cx.update(|cx| {
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        gpui_kit::open_window(opts, cx, |w, cx| {
            cx.new(|cx| crate::EmulatorsView::new(w, cx))
        })
        .expect("window opens")
    });
    VisualTestContext::from_window(handle, cx)
}

#[gpui_kit::test]
fn emulators_page_opens_retroarch_cores_dialog(cx: &mut TestAppContext) {
    setup(cx);
    let mut vcx = open_emulators_window(cx, 1200., 1200.);
    for _ in 0..200 {
        vcx.run_until_parked();
        if vcx.debug_bounds("manage-retroarch-cores").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        vcx.debug_bounds("manage-retroarch-cores").is_some(),
        "RetroArch card renders Manage cores button"
    );
    click(&mut vcx, "manage-retroarch-cores");
    assert!(
        vcx.debug_bounds("retroarch-cores-dialog").is_some(),
        "clicking Manage cores opens the dedicated RetroArch cores modal"
    );
}

#[gpui_kit::test]
fn emulators_page_features_retroarch_and_avoids_false_bios_badge(cx: &mut TestAppContext) {
    setup(cx);
    let mut vcx = open_emulators_window(cx, 1400., 1200.);
    let ra_dl = irori_engines::engine("retroarch")
        .unwrap()
        .downloadable_on(irori_engines::Host::current());
    let standalone_id = if ra_dl {
        "engine-duckstation"
    } else {
        "engine-mame"
    };
    for _ in 0..200 {
        vcx.run_until_parked();
        if vcx.debug_bounds("engine-retroarch").is_some()
            && (!ra_dl || vcx.debug_bounds(standalone_id).is_some())
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !ra_dl && vcx.debug_bounds("expand-manual-engines").is_some() {
        click(&mut vcx, "expand-manual-engines");
    }
    for _ in 0..2 {
        vcx.update(|w, cx| _ = w.draw(cx));
        vcx.run_until_parked();
    }

    let retro = vcx.debug_bounds("engine-retroarch").expect("RetroArch");
    let standalone = vcx.debug_bounds(standalone_id).expect("standalone card");
    assert!(
        retro.size.width > standalone.size.width * 1.5,
        "RetroArch spans full width: retro={retro:?}, standalone={standalone:?}"
    );
    assert!(
        standalone.top() >= retro.bottom(),
        "standalone grid starts below RetroArch"
    );
    assert!(vcx.debug_bounds("systems-attention-badge").is_none());
}

#[gpui_kit::test]
fn downloads_view_draws_irori_banner_states_and_clears_restarting_on_failure(
    cx: &mut TestAppContext,
) {
    use irori_services::{
        AppUpdateStatus, InstallKind, Release, SelfInstall, SelfUpdateChannel, SystemPackageManager,
    };

    setup(cx);
    let options = cx.update(|cx| WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(640.), px(720.)),
            cx,
        ))),
        ..Default::default()
    });
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| DownloadsView::new(window, cx))
        })
        .expect("window opens")
    });
    let mut vcx = VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();

    let release = Release {
        version: "9.9.9".into(),
        url: "https://github.com/irori-app/irori/releases/tag/v9.9.9".into(),
    };
    let draw_status = |vcx: &mut VisualTestContext, status: AppUpdateStatus| {
        vcx.update(|window, cx| {
            let app_update = models(cx).app_update.clone();
            app_update.update(cx, |m, cx| {
                m.status = status;
                cx.notify();
            });
            window.refresh();
            let _ = window.draw(cx);
        });
    };

    // 1. Available (opt-in Install update button; header subtitle matches
    // available state rather than contradicting with "Up to date").
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::Tarball),
            available: Some(release.clone()),
            dismissed: false,
            announce: true,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::Idle,
        },
    );
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("irori-install-update").is_some());
    assert!(vcx.debug_bounds("irori-update-progress").is_none());
    let summary = vcx.read(|cx| view.read(cx).summary_for_tests(cx));
    assert!(
        summary.contains("1 update available"),
        "expected Downloads header subtitle to reflect available Irori update, got: {summary}"
    );

    // 2. Downloading (visual progress bar + disabled Install button + header downloading state).
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::AppImage),
            available: Some(release.clone()),
            dismissed: false,
            announce: true,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::Downloading { percent: Some(60) },
        },
    );
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("irori-update-progress").is_some());
    assert!(vcx.debug_bounds("irori-install-update").is_some());
    let summary = vcx.read(|cx| view.read(cx).summary_for_tests(cx));
    assert!(
        summary.contains("Downloading"),
        "expected Downloads header subtitle to show Downloading during Irori self-update, got: {summary}"
    );

    // 3. Failed (long rate-limit error wraps below header row without clipping action buttons).
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::Tarball),
            available: Some(release.clone()),
            dismissed: false,
            announce: true,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::Failed {
                error: "GitHub's rate limit was reached. Try again in about 42 minutes, or download it from the release page.".into(),
            },
        },
    );
    let banner = vcx
        .debug_bounds("irori-self-update-banner")
        .expect("banner drawn");
    let err_bounds = vcx
        .debug_bounds("irori-update-error")
        .expect("error message drawn");
    let notes_btn = vcx
        .debug_bounds("irori-release-notes")
        .expect("release notes button drawn");
    let install_btn = vcx
        .debug_bounds("irori-install-update")
        .expect("install button drawn");
    assert!(
        install_btn.right() <= banner.right()
            && notes_btn.right() <= install_btn.left()
            && err_bounds.right() <= banner.right()
            && err_bounds.top() >= install_btn.bottom(),
        "long error must sit below action buttons inside banner bounds: banner={banner:?} notes={notes_btn:?} install={install_btn:?} err={err_bounds:?}"
    );

    // 4. RolledBack (warning visible alongside retry action).
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::Tarball),
            available: Some(release.clone()),
            dismissed: false,
            announce: true,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::RolledBack {
                version: "9.9.9".into(),
            },
        },
    );
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("irori-rollback-warning").is_some());

    // 5. SystemPackage (manual check shows Copy command, never self-install).
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::SystemPackage(Some(SystemPackageManager::Pacman)),
            self_update_channel: None,
            available: Some(release),
            dismissed: false,
            announce: false,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::Unavailable,
        },
    );
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("irori-copy-command").is_some());
    assert!(vcx.debug_bounds("irori-install-update").is_none());

    // 6. Ready (prefers staged Ready version even when `available` is cleared).
    draw_status(
        &mut vcx,
        AppUpdateStatus {
            current: "1.2.0".into(),
            kind: InstallKind::Manual,
            self_update_channel: Some(SelfUpdateChannel::Tarball),
            available: None,
            dismissed: false,
            announce: false,
            updated: false,
            checking: false,
            checked_at: Some(1_700_000_000),
            install: SelfInstall::Ready {
                version: "9.9.9".into(),
            },
        },
    );
    assert!(vcx.debug_bounds("irori-self-update-banner").is_some());
    assert!(vcx.debug_bounds("irori-restart-update").is_some());

    // Shared restart guard clears loading state when starting the helper fails.
    vcx.update(|window, cx| {
        view.update(cx, |v, cx| v.trigger_restart_irori_for_test(window, cx));
        assert!(models(cx).app_update.read(cx).restarting);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while vcx.read(|cx| models(cx).app_update.read(cx).restarting)
        && std::time::Instant::now() < deadline
    {
        vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !vcx.read(|cx| models(cx).app_update.read(cx).restarting),
        "restarting must clear after a failed restart"
    );
}
