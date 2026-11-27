//! Store construction and the cross-store Heroic import.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _};
use irori_core::{DynStore, EventBus, SettingsFn, Store};
use irori_services::{MultiAccountStore, SlotDirs};
use irori_ui::hooks::{Hooks, ImportSummary};

/// A store crate's constructor for one more account (`for_account`).
type ForAccount<S> = fn(EventBus, SettingsFn, PathBuf, PathBuf) -> Arc<S>;

/// `main` plus the store's other accounts.
fn with_accounts<S: Store>(
    main: Arc<S>,
    bus: &EventBus,
    settings: &SettingsFn,
    for_account: ForAccount<S>,
) -> Arc<MultiAccountStore<S>> {
    let (slot_bus, settings) = (bus.clone(), settings.clone());
    MultiAccountStore::new(
        main,
        bus.clone(),
        Arc::new(move |dirs: &SlotDirs| {
            for_account(
                slot_bus.clone(),
                settings.clone(),
                dirs.config.clone(),
                dirs.cache.clone(),
            )
        }),
    )
}

/// Every store backend, in sidebar order. Stores with accounts take several.
pub fn build(bus: EventBus, settings: SettingsFn) -> Vec<DynStore> {
    let mut stores: Vec<DynStore> = vec![
        {
            let epic = with_accounts(
                irori_epic::EpicStore::new(bus.clone(), settings.clone()),
                &bus,
                &settings,
                irori_epic::EpicStore::for_account,
            );
            let _ = EPIC.set(epic.clone());
            epic
        },
        {
            let gog = with_accounts(
                irori_gog::GogStore::new(bus.clone(), settings.clone()),
                &bus,
                &settings,
                irori_gog::GogStore::for_account,
            );
            let _ = GOG.set(gog.clone());
            gog
        },
        with_accounts(
            irori_amazon::AmazonStore::new(bus.clone(), settings.clone()),
            &bus,
            &settings,
            irori_amazon::AmazonStore::for_account,
        ),
    ];
    // Always present so it can be turned on from its account card; it
    // shows nothing while `zoom_platform` is off.
    stores.push(with_accounts(
        irori_zoom::ZoomStore::new(bus.clone(), settings.clone()),
        &bus,
        &settings,
        irori_zoom::ZoomStore::for_account,
    ));
    // Always present so it can be turned on from its account card; it
    // shows nothing while `itch_io` is off.
    stores.push(with_accounts(
        irori_itch::ItchStore::new(bus.clone(), settings.clone()),
        &bus,
        &settings,
        irori_itch::ItchStore::for_account,
    ));
    // Microsoft Store games play only on Linux here; Windows has the Xbox
    // app (listed by the launchers store) and macOS has no runtime for them.
    #[cfg(target_os = "linux")]
    {
        let xbox = irori_xbox::XboxStore::new(bus.clone(), settings.clone());
        let _ = XBOX.set(xbox.clone());
        stores.push(xbox);
    }
    let steam = irori_steam::SteamStore::new(bus.clone(), settings.clone());
    let _ = STEAM.set(steam.clone());
    stores.push(steam);
    // Always present so its switch on the Stores page can turn it on; it
    // lists nothing while `third_party_launchers` is off.
    let launchers = irori_launchers::LaunchersStore::new(bus.clone(), settings.clone());
    let _ = LAUNCHERS.set(launchers.clone());
    stores.push(launchers);
    let emulation = irori_emulation::EmulationStore::new(bus.clone(), settings.clone());
    let _ = EMULATION.set(emulation.clone());
    stores.push(emulation);
    let sideload = irori_sideload::SideloadStore::new(bus, settings.clone());
    let _ = SIDELOAD.set(sideload.clone());
    let _ = SETTINGS.set(settings);
    stores.push(sideload);
    stores
}

/// The concrete Epic store (EOS overlay, EGL sync use its main account).
pub static EPIC: once_cell::sync::OnceCell<Arc<MultiAccountStore<irori_epic::EpicStore>>> =
    once_cell::sync::OnceCell::new();

/// The concrete GOG store (private-branch passwords, REDmod).
pub static GOG: once_cell::sync::OnceCell<Arc<MultiAccountStore<irori_gog::GogStore>>> =
    once_cell::sync::OnceCell::new();

/// The concrete Xbox store (Linux): offline PC, Store browsing, social.
#[cfg(target_os = "linux")]
pub static XBOX: once_cell::sync::OnceCell<std::sync::Arc<irori_xbox::XboxStore>> =
    once_cell::sync::OnceCell::new();

/// The concrete Steam store (the compatibility tool choice of its games).
pub static STEAM: once_cell::sync::OnceCell<Arc<irori_steam::SteamStore>> =
    once_cell::sync::OnceCell::new();

/// Hooks of the game feature that need concrete store crates.
pub fn game_hooks() -> irori_game::GameHooks {
    let mut hooks = irori_game::GameHooks::new();
    if let Some(gog) = GOG.get().cloned() {
        hooks = hooks.with_branch_password(Arc::new(move |id, password, cx| {
            let gog = gog.store_for(&id.app_name);
            irori_ui::run(cx, async move {
                gog.set_private_branch_password(&id.app_name, &password)
                    .await
            })
        }));
    }
    hooks
}

/// The third-party launchers store (setup and scanning on the Stores page).
pub static LAUNCHERS: once_cell::sync::OnceCell<Arc<irori_launchers::LaunchersStore>> =
    once_cell::sync::OnceCell::new();

/// The ROM library store (scan state for the setup guide).
pub static EMULATION: once_cell::sync::OnceCell<Arc<irori_emulation::EmulationStore>> =
    once_cell::sync::OnceCell::new();

/// The concrete sideload store (the library's add/edit app dialogs need its
/// own API, which `dyn Store` does not expose).
pub static SIDELOAD: once_cell::sync::OnceCell<Arc<irori_sideload::SideloadStore>> =
    once_cell::sync::OnceCell::new();

static SETTINGS: once_cell::sync::OnceCell<SettingsFn> = once_cell::sync::OnceCell::new();

pub fn hooks() -> Hooks {
    Hooks {
        import_from_heroic: Some(Arc::new(import_from_heroic)),
        open_emulation_setup: Some(Arc::new(irori_manage::open_emulation_setup)),
        import_from_lutris: Some(Arc::new(|cx| {
            let backend = irori_ui::backend(cx);
            irori_ui::run(cx, async move {
                crate::lutris::import(&backend, SIDELOAD.get().cloned(), EMULATION.get().cloned())
                    .await
            })
        })),
        import_from_bottles: Some(Arc::new(|cx| {
            let backend = irori_ui::backend(cx);
            irori_ui::run(cx, async move {
                crate::lutris::import_bottles(&backend, SIDELOAD.get().cloned()).await
            })
        })),
        import_from_faugus: Some(Arc::new(|cx| {
            let backend = irori_ui::backend(cx);
            irori_ui::run(cx, async move {
                crate::lutris::import_faugus(&backend, SIDELOAD.get().cloned()).await
            })
        })),
        import_from_playnite: Some(Arc::new(|cx| {
            let backend = irori_ui::backend(cx);
            irori_ui::run(cx, async move {
                crate::playnite::import(&backend, SIDELOAD.get().cloned()).await
            })
        })),
        game_panel: Some(Arc::new(crate::routing::game_panel)),
        game_settings: Some(Arc::new(crate::routing::game_settings)),
        open_store_page: Some(Arc::new(|runner, url, window, cx| {
            irori_manage::open_store_window(runner, Some(url), window, cx)
        })),
        steam_library_hint: Some(irori_ui::hooks::SteamLibraryHint {
            shown: Arc::new(irori_manage::suggest_steam_client_library),
            open: Arc::new(irori_manage::open_steam_library_dialog),
            dismiss: Arc::new(irori_manage::dismiss_steam_client_library_hint),
        }),
        show_unlocks: Some(Arc::new(irori_bigpicture::overlay::show_unlocks)),
        uninstall: Some(Arc::new(irori_game::open_uninstall_dialog)),
        game_setup: Some(Arc::new(irori_settings::setup_card)),
        found_sources: Some(Arc::new(irori_manage::found_sources_view)),
        accounts_view: Some(Arc::new(|window, cx| {
            cx.new(|cx| irori_manage::AccountsView::new(window, cx))
                .into()
        })),
        wine_manager_view: Some(Arc::new(|window, cx| {
            cx.new(|cx| irori_manage::WineManagerView::new(window, cx))
                .into()
        })),
        emulators_view: Some(Arc::new(|window, cx| {
            cx.new(|cx| irori_manage::EmulatorsView::new(window, cx))
                .into()
        })),
        play: Some(Arc::new(|id, window, cx| {
            irori_game::ops::play(&id, None, window, cx).detach()
        })),
        install: Some(Arc::new(|id, window, cx| {
            irori_game::ops::install(&id, window, cx)
        })),
        add_to_library: Some(Arc::new(irori_library::open_sideload_dialog_for_path)),
    }
}

fn canonical_key(path: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.trim_start_matches(r"\\?\").to_ascii_lowercase()
    } else {
        text
    }
}

struct SideloadBridgeImpl {
    store: Arc<irori_sideload::SideloadStore>,
    settings: Option<SettingsFn>,
}

#[irori_core::async_trait]
impl irori_services::SideloadBridge for SideloadBridgeImpl {
    async fn scan_extracted(
        &self,
        root: &std::path::Path,
    ) -> Option<irori_services::SideloadCandidate> {
        let root = root.to_path_buf();
        let found =
            tokio::task::spawn_blocking(move || irori_sideload::scan::scan_extracted_game(&root))
                .await
                .ok()??;
        Some(irori_services::SideloadCandidate {
            title: found.title,
            executable: found.executable,
            install_folder: found.install_folder,
            platform: found.platform.install_platform(),
            launcher_args: found.args,
            cover_art: found.icon.map(|p| p.display().to_string()),
            background_art: None,
        })
    }

    async fn register_game(
        &self,
        mut candidate: irori_services::SideloadCandidate,
    ) -> anyhow::Result<irori_core::GameId> {
        let want_exe = canonical_key(&candidate.executable);
        let want_dir = candidate.install_folder.as_deref().map(canonical_key);
        if let Some(existing) = self.store.games().into_iter().find(|g| {
            let Some(inst) = &g.install else {
                return false;
            };
            if !inst.executable.is_empty()
                && canonical_key(std::path::Path::new(&inst.executable)) == want_exe
            {
                return true;
            }
            if let Some(dir) = &want_dir
                && !inst.install_path.is_empty()
                && canonical_key(std::path::Path::new(&inst.install_path)) == *dir
            {
                return true;
            }
            false
        }) {
            return Ok(existing.id());
        }
        let allow_network = self
            .settings
            .as_ref()
            .is_none_or(|f| f().network_mode != irori_core::net::NetworkMode::Offline);
        if candidate.cover_art.is_none()
            && allow_network
            && let Some((cover, hero)) =
                irori_extras::wiki::steam_art_for_title(&candidate.title).await
        {
            candidate.cover_art = Some(cover);
            candidate.background_art = Some(hero);
        }
        let platform = irori_sideload::SideloadPlatform::from_install_platform(candidate.platform)
            .unwrap_or_default();
        let app = irori_sideload::SideloadApp {
            title: candidate.title,
            executable: Some(candidate.executable),
            platform,
            cover_art: candidate.cover_art,
            background_art: candidate.background_art,
            install_folder: candidate.install_folder,
            launcher_args: candidate.launcher_args,
            ..Default::default()
        };
        self.store.add_app(app).await
    }
}

/// Wire the sideload bridge into `backend` so completed web downloads can scan
/// extracted folders and auto-register games in the Library.
pub fn install_sideload_bridge(backend: &irori_services::Backend) {
    if let Some(store) = SIDELOAD.get().cloned() {
        backend.set_sideload_bridge(Arc::new(SideloadBridgeImpl {
            store,
            settings: SETTINGS.get().cloned(),
        }));
    }
}

/// A store crate's Heroic import entry point.
pub type ImportFn = fn(&std::path::Path) -> anyhow::Result<()>;

/// Every store's Heroic import, with its display name.
pub const HEROIC_IMPORTS: [(&str, ImportFn); 5] = [
    ("Epic Games", irori_epic::import_from_heroic),
    ("GOG", irori_gog::import_from_heroic),
    ("Amazon Games", irori_amazon::import_from_heroic),
    ("ZOOM Platform", irori_zoom::import_from_heroic),
    ("Added manually", irori_sideload::import_from_heroic),
];

/// Copy credentials and installs from each store, then launcher data.
fn import_from_heroic(cx: &mut App) -> gpui_kit::Task<anyhow::Result<ImportSummary>> {
    let backend = irori_ui::backend(cx);
    irori_ui::run(cx, async move {
        let dir = irori_core::paths::heroic_config_dir()
            .ok_or_else(|| anyhow::anyhow!("No Heroic configuration was found"))?;
        tracing::info!("importing from Heroic's folder {}", dir.display());
        let mut summary = ImportSummary::default();
        let imports = HEROIC_IMPORTS;
        for (name, import) in imports {
            let d = dir.clone();
            match tokio::task::spawn_blocking(move || import(&d)).await? {
                Ok(()) => summary.stores.push(name.to_string()),
                Err(e) => tracing::warn!("importing {name} from Heroic: {e:#}"),
            }
        }
        for store in backend.stores() {
            if let Err(e) = store.init().await {
                tracing::warn!("re-initialising {}: {e:#}", store.runner().store_name());
            }
        }
        crate::import::apply_launcher_data(&backend, &dir, &mut summary).await?;
        backend.refresh_all(false).await;
        Ok(summary)
    })
}
