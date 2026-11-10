//! The Xbox store (`Runner::Xbox`, opt-in through `iroriXbox`): Microsoft
//! Store and PC Game Pass games, played on Linux through Proton with
//! `xgameruntime.dll` (the Nexus Runtime from the Wine manager). Everything
//! else is Irori's own, ported from Xodus and Nexus Launcher:
//!
//! * Accounts: Microsoft's own sign-in page in the sign-in window's
//!   CloudExperienceHost mode ([`irori_core::LoginFlow::MicrosoftAccount`]);
//!   the page's `DAProperty` becomes Passport tokens through RST2 with a
//!   registered device. The Store account (`main`) owns games and licences;
//!   a separate Xbox player account (`player`) may sign in for play and
//!   social. Credentials: `credentials.json` through `irori_secrets`.
//! * Library: the Store account's entitlements, held by purchase or through
//!   a Game Pass plan, described by the display catalog ([`library`]).
//! * Install, update, repair: the package's MSIXVC file streamed into the
//!   game folder with its content key from the licence ([`install`]).
//! * Launch: the licence (or a stored offline one), the encrypted programs
//!   decrypted into a private launch view ([`launch_view`]), the account
//!   service the runtime talks to ([`service`]), then Proton.

// The streaming installer ported from Xodus walks fixed-size pages with
// `chunks_exact`; `as_chunks` would change every loop for no behaviour change.
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod config;
pub mod credentials;
pub mod install;
#[cfg(unix)]
pub mod launch_view;
pub mod library;
pub mod locale;
pub mod packages;
pub mod records;
#[cfg(unix)]
pub mod service;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use irori_core::process::CommandSpec;
use irori_core::{
    Event, EventBus, ExtraInfo, GameInfo, InstallInfo, InstallInfoQuery, InstallPlatform,
    InstallRequest, LaunchPlan, LaunchRequest, LoginApproval, LoginFlow, MAIN_ACCOUNT, OpContext,
    Outcome, Paths, RemoveArgs, Runner, SessionExpired, SettingsFn, Store, StoreAccount,
    UpdateRequest, UserInfo, XboxProblem, XboxUnavailable,
};
use irori_msstore_api::catalog::Product;
use irori_msstore_api::http::StoreApi;
use irori_msstore_api::models::live::{DAProperty, ExchangeUserTokenOutcome};
use irori_msstore_api::models::secrets::{LegacyToken, Token, User};
use irori_msstore_api::models::soap;
use irori_msstore_api::tokens::{AccountRole, TokenManager};
use parking_lot::{Mutex, RwLock};

use library::{Access, Entry};
use records::{InstalledRecord, Layout, LibraryCache};

const TURNED_OFF: &str = "The Xbox store is turned off in the settings";
/// The Xbox app's Microsoft account client, as Xodus signs in with it.
const CLIENT_ID: &str = "000000004424da1f";
const USER_AUTH_SCOPE: &str = "scope=service::user.auth.xboxlive.com::MBI_SSL&api-version=2.0";
/// The slot of the separate Xbox player account.
pub const PLAYER_ACCOUNT: &str = "player";
const PLAYFAB_RELYING_PARTY: &str = "http://playfab.xboxlive.com/";
/// Microsoft's page for unlinking devices (licensing code 501).
pub const DEVICES_URL: &str = "https://account.microsoft.com/devices/content";

/// Microsoft's sign-in page, as Windows opens it in CloudExperienceHost.
pub fn login_url() -> String {
    format!(
        "https://login.live.com/ppsecure/InlineLogin.srf?id=80604&scid=3&mkt=en-US&Platform=Windows10&clientid={CLIENT_ID}&hosted=1"
    )
}

/// Heroic has no Xbox store, so there is nothing to import.
pub fn import_from_heroic(_heroic_config_dir: &Path) -> Result<()> {
    Ok(())
}

/// The account has no licence for the game (any more).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct NotEntitled {
    pub message: String,
}

impl NotEntitled {
    /// This error behind `error`, if that is what it is.
    pub fn of(error: &anyhow::Error) -> Option<&NotEntitled> {
        error.chain().find_map(|e| e.downcast_ref::<NotEntitled>())
    }
}

fn unavailable(problem: XboxProblem) -> anyhow::Error {
    anyhow::Error::new(XboxUnavailable::new(problem))
}

/// A Microsoft sign-in waiting for its approval page.
struct PendingLogin {
    role: AccountRole,
    attempt: irori_msstore_api::tokens::LoginAttempt,
    device: LegacyToken,
}

#[derive(Default)]
struct State {
    library: LibraryCache,
    installed: BTreeMap<String, InstalledRecord>,
    games: Vec<Arc<GameInfo>>,
    by_app: HashMap<String, Arc<GameInfo>>,
    /// Who signed in, read from the credentials at start and after changes.
    store_user: Option<User>,
    player_user: Option<User>,
    expired: bool,
}

impl State {
    fn recompose(&mut self) {
        let mut games: Vec<Arc<GameInfo>> = Vec::new();
        for (app, entry) in &self.library.entries {
            let mut info = library::game_info(entry);
            if let Some(record) = self.installed.get(app) {
                info.is_installed = true;
                info.install = Some(record.to_installed_info());
            }
            games.push(Arc::new(info));
        }
        for (app, record) in &self.installed {
            if !self.library.entries.contains_key(app) {
                games.push(Arc::new(
                    GameInfo {
                        runner: Some(Runner::Xbox),
                        app_name: app.clone(),
                        title: record.title.clone(),
                        is_installed: true,
                        install: Some(record.to_installed_info()),
                        installable: true,
                        platforms: vec![InstallPlatform::Windows],
                        ..Default::default()
                    }
                    .finalize(),
                ));
            }
        }
        self.by_app = games
            .iter()
            .map(|g| (g.app_name.clone(), g.clone()))
            .collect();
        self.games = games;
    }
}

/// What a running game holds until it exits.
#[cfg(unix)]
struct GameSession {
    _view: launch_view::LaunchView,
    _service: Option<service::Session>,
    _guard: Option<std::fs::File>,
}

/// The Xbox store.
pub struct XboxStore {
    bus: EventBus,
    settings: SettingsFn,
    layout: Layout,
    tokens: TokenManager,
    client: reqwest::Client,
    api: StoreApi,
    state: RwLock<State>,
    pending: Mutex<Option<PendingLogin>>,
    #[cfg(unix)]
    sessions: Mutex<HashMap<String, GameSession>>,
    files_lock: tokio::sync::Mutex<()>,
}

impl XboxStore {
    pub fn new(bus: EventBus, settings: SettingsFn) -> Arc<Self> {
        let paths = Paths::get();
        Self::with_layout(
            bus,
            settings,
            Layout {
                config: paths.store_config_dir(Runner::Xbox),
                cache: paths.store_cache_dir(Runner::Xbox),
            },
        )
    }

    /// A store keeping its files in `layout`.
    pub fn with_layout(bus: EventBus, settings: SettingsFn, layout: Layout) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap_or_default();
        let tokens = credentials::token_manager(layout.credentials_file());
        Arc::new(Self {
            bus,
            settings,
            api: StoreApi::new(client.clone()),
            client,
            tokens,
            layout,
            state: RwLock::new(State::default()),
            pending: Mutex::new(None),
            #[cfg(unix)]
            sessions: Mutex::new(HashMap::new()),
            files_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn enabled(&self) -> bool {
        cfg!(target_os = "linux") && (self.settings)().xbox
    }

    /// The Store market: the setting, else the interface language's region,
    /// else `US`.
    pub fn market(&self) -> String {
        let app = (self.settings)();
        let chosen = app.xbox_market.trim().to_ascii_uppercase();
        if chosen.len() == 2 && chosen.bytes().all(|b| b.is_ascii_uppercase()) {
            return chosen;
        }
        [app.ui_language.clone()]
            .into_iter()
            .chain(
                ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .into_iter()
                    .filter_map(|k| std::env::var(k).ok()),
            )
            .find_map(|locale| region_of(&locale))
            .unwrap_or_else(|| "US".to_string())
    }

    /// Whether the Store account is signed in and its store turned on.
    pub fn signed_in(&self) -> bool {
        self.enabled() && self.state.read().store_user.is_some()
    }

    /// The display catalog and Store browsing, for the Store screen.
    pub fn api(&self) -> &StoreApi {
        &self.api
    }

    /// The account token store (the account service and social use it).
    pub fn tokens(&self) -> &TokenManager {
        &self.tokens
    }

    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        tokio::task::spawn_blocking(f)
            .await
            .context("a background task failed")
    }

    fn read_users(&self) {
        let store = self.tokens.get_license_user().ok();
        let player = self.tokens.get_player_user().ok().flatten();
        let mut state = self.state.write();
        state.store_user = store;
        state.player_user =
            player.filter(|p| state.store_user.as_ref().is_none_or(|s| s.puid != p.puid));
    }

    fn require_record(&self, app: &str) -> Result<InstalledRecord> {
        self.state
            .read()
            .installed
            .get(app)
            .cloned()
            .with_context(|| format!("{app} is not installed"))
    }

    async fn update_installed(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, InstalledRecord>),
    ) -> Result<()> {
        let _guard = self.files_lock.lock().await;
        let mut installed = self.state.read().installed.clone();
        change(&mut installed);
        let (layout, snapshot) = (self.layout.clone(), installed.clone());
        Self::blocking(move || records::save_installed(&layout, &snapshot)).await??;
        {
            let mut state = self.state.write();
            state.installed = installed;
            state.recompose();
        }
        self.bus.emit(Event::LibraryChanged(Runner::Xbox));
        Ok(())
    }

    /// Register this device with Microsoft, or renew its sign-in.
    async fn ensure_device(&self) -> Result<()> {
        irori_msstore_api::tokens::device::ensure_device_credentials(&self.client, &self.tokens)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("Could not register this device with Microsoft")
    }

    /// Exchange the page's tokens for the account's, as Xodus' login does.
    async fn exchange(
        &self,
        prop: &DAProperty,
        device: LegacyToken,
        finish: bool,
    ) -> Result<ExchangeUserTokenOutcome> {
        let user_token = LegacyToken {
            key_name: None,
            token: prop.da_token.clone(),
            binary_secret: None,
            tpm_key: None,
            lifetime: soap::Timestamp {
                id: None,
                created: prop.da_start_time.clone(),
                expires: prop.da_expires.clone(),
            },
        };
        let mut scopes = vec![(
            USER_AUTH_SCOPE.to_string(),
            Some(soap::PolicyReference::token_broker()),
        )];
        if finish {
            scopes.push(("http://Passport.NET/tb".to_string(), None));
        }
        irori_msstore_api::api::live::exchange_user_token(
            &self.client,
            user_token,
            prop.username.clone(),
            device,
            Some(prop.sts_inline_flow_token.clone()),
            None,
            CLIENT_ID.to_string(),
            &scopes,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("Microsoft did not accept the sign-in")
    }

    /// Sign in the account of `role` with what the sign-in window captured.
    async fn sign_in(&self, role: AccountRole, input: &str) -> Result<UserInfo> {
        if !self.enabled() {
            bail!(TURNED_OFF);
        }
        let prop = parse_da_property(input)?;
        let pending = self.pending.lock().take().filter(|p| p.role == role);
        let (attempt, device, finish) = match pending {
            Some(p) => (p.attempt, p.device, true),
            None => {
                self.ensure_device().await?;
                let (attempt, device) = self
                    .tokens
                    .begin_role_login(role)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let Token::Legacy(device) = device else {
                    bail!("The device sign-in is not usable. Try again.");
                };
                (attempt, device, false)
            }
        };
        match self.exchange(&prop, device.clone(), finish).await? {
            ExchangeUserTokenOutcome::Fault(pp) => {
                let url = pp
                    .and_then(|pp| pp.inline_auth_url)
                    .filter(|u| !finish && !u.is_empty());
                let Some(url) = url else {
                    bail!("Microsoft refused the sign-in. Try again.");
                };
                *self.pending.lock() = Some(PendingLogin {
                    role,
                    attempt,
                    device,
                });
                Err(LoginApproval::new(url).into())
            }
            ExchangeUserTokenOutcome::Issued(body) => {
                let issued = match body {
                    soap::BodyContent::RequestSecurityTokenResponseCollection(c) => {
                        c.security_tokens
                    }
                    soap::BodyContent::RequestSecurityTokenResponse(t) => vec![*t],
                    _ => bail!("Microsoft returned an unsupported sign-in response"),
                };
                let mut tokens = HashMap::new();
                for token in issued {
                    let address = token.applies_to.endpoint_reference.address.clone();
                    let token = Token::try_from(token).map_err(|e| anyhow::anyhow!("{e}"))?;
                    let address = match &token {
                        Token::Legacy(legacy) => legacy.key_name.clone().unwrap_or(address),
                        _ => address,
                    };
                    tokens.insert(address, token);
                }
                if tokens.is_empty() {
                    bail!("Microsoft returned no tokens");
                }
                let user = User {
                    puid: prop.puid.clone(),
                    username: prop.username.clone(),
                };
                let tokens_store = self.tokens.clone();
                Self::blocking(move || tokens_store.complete_role_login(&attempt, user, tokens))
                    .await?
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                self.read_users();
                self.state.write().expired = false;
                self.bus.emit(Event::AccountChanged(Runner::Xbox));
                Ok(UserInfo {
                    account_id: prop.puid,
                    display_name: prop.username,
                })
            }
        }
    }

    /// Entitlements, resolved and described.
    async fn fetch_library(&self, market: &str) -> Result<BTreeMap<String, Entry>> {
        self.ensure_device().await?;
        let answer = irori_msstore_api::ops::library::entitlements(&self.tokens, market)
            .await
            .map_err(|stage| session_error(stage, "Could not load the Microsoft Store library"))?;
        let now = irori_core::util::unix_now() as i64;
        let mut access = library::resolve(&answer, now);
        // With PC Game Pass (or Ultimate) every PC Game Pass game can be
        // played, not only those the account has claimed.
        let store_user = self.state.read().store_user.clone();
        if let Some(user) = store_user {
            let membership =
                irori_msstore_api::ops::subscription::lookup(&self.tokens, &user.username).await;
            if library::has_pc_game_pass(&membership) {
                match self
                    .api
                    .game_pass(irori_msstore_api::catalog::GamePassList::AllPc, market)
                    .await
                {
                    Ok(ids) => {
                        for id in ids {
                            access.entry(id).or_insert(Access::GamePass);
                        }
                    }
                    Err(e) => tracing::warn!("PC Game Pass catalog unavailable: {e}"),
                }
            }
        }
        let ids: Vec<String> = access.keys().cloned().collect();
        let mut products = Vec::new();
        for chunk in ids.chunks(400) {
            products.extend(
                self.api
                    .products(chunk, market)
                    .await
                    .context("Could not read the Microsoft Store catalog")?,
            );
        }
        Ok(products
            .into_iter()
            .filter(library::is_listed)
            .filter_map(|product| {
                let access = *access.get(&product.product_id)?;
                Some((product.product_id.clone(), Entry { product, access }))
            })
            .collect())
    }

    /// The Xbox title of a game: its package's, else the catalog's.
    fn title_id(&self, app: &str) -> Option<u32> {
        title_id_in(&self.state.read(), app)
    }

    fn product(&self, app: &str) -> Option<Product> {
        self.state
            .read()
            .library
            .entries
            .get(app)
            .map(|e| e.product.clone())
    }

    /// The content ID to install for a product: the PC base game when the
    /// choice is clear, `selected` when the user picked one.
    pub async fn package_plan(
        &self,
        product_id: &str,
    ) -> Result<irori_msstore_api::ops::package_plan::Plan> {
        irori_msstore_api::ops::package_plan::resolve(&self.client, product_id, &self.market())
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The download address of a content ID's package.
    async fn package_url(&self, content_id: &str) -> Result<(String, String)> {
        let package = irori_msstore_api::ops::package::get_packages(
            &self.client,
            &self.tokens,
            content_id.to_string(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let file = package
            .package_files
            .iter()
            .find(|p| p.file_name.ends_with(".msixvc"))
            .context("Microsoft offers no PC package for this game")?;
        let root = file
            .cdn_root_paths
            .first()
            .context("Microsoft returned no package download location")?;
        Ok((format!("{root}{}", file.relative_url), package.version))
    }

    /// The licence of a package and its content key.
    async fn content_key(
        &self,
        content_id: &str,
        offline: bool,
    ) -> Result<([u8; 32], irori_msstore_api::ops::license::Grant)> {
        let grant = irori_msstore_api::ops::license::get_license(
            &self.client,
            &self.tokens,
            content_id.to_string(),
            self.market(),
            Some(&self.layout.licenses_dir()),
            offline,
        )
        .await
        .map_err(license_error)?;
        let mut keys = grant.license.content_keys.values();
        let (Some(key), None) = (keys.next(), keys.next()) else {
            bail!(
                "Unexpected number of content keys ({})",
                grant.license.content_keys.len()
            );
        };
        let key = key.unpack(&grant.device_key).map_err(|_| {
            anyhow::anyhow!("The content key could not be authenticated. Sign in again and retry.")
        })?;
        Ok((*key, grant))
    }

    /// Install `content_id` into `dir` (or update and repair what is there).
    async fn install_into(
        &self,
        app: &str,
        title: &str,
        content_id: &str,
        dir: PathBuf,
        ctx: &OpContext,
    ) -> Result<Outcome> {
        (ctx.step)("Preparing");
        // The package streams share one limiter; the setting applies to the
        // next bytes read, so a changed limit takes effect on the next install.
        irori_msixvc::bandwidth::set_limit_kib((self.settings)().max_download_speed);
        let (url, version) = self.package_url(content_id).await?;
        let download = irori_msixvc::http_client()?;
        let remote = match install::Remote::open(&download, &url, &dir).await {
            Ok(remote) => remote,
            Err(e) => return Ok(Outcome::Error(e.to_string())),
        };
        let (key, _) = self
            .content_key(&remote.content_id().to_string(), false)
            .await?;
        let size = remote.size();
        (ctx.step)("Downloading");
        let parallel = ((self.settings)().max_workers as usize).clamp(1, 16);
        match remote
            .install(key, parallel, &ctx.progress, &ctx.cancel)
            .await
        {
            Ok(()) => {}
            Err(install::InstallError::Cancelled) => return Ok(Outcome::Abort),
            Err(e) => return Ok(Outcome::Error(e.to_string())),
        }
        let config = Self::blocking({
            let dir = dir.clone();
            move || config::GameConfig::read(&dir)
        })
        .await?
        .unwrap_or_default();
        let record = InstalledRecord {
            title: title.to_string(),
            install_path: dir.to_string_lossy().into_owned(),
            content_id: content_id.to_string(),
            version: config.version.clone().unwrap_or(version),
            executable: config.executable().unwrap_or_default().to_string(),
            title_id: config.title_id_number(),
            size,
            ..self
                .state
                .read()
                .installed
                .get(app)
                .cloned()
                .unwrap_or_default()
        };
        self.update_installed(|m| {
            m.insert(app.to_string(), record);
        })
        .await?;
        Ok(Outcome::Done)
    }

    fn install_dir(&self, request: &InstallRequest, title: &str) -> PathBuf {
        let base = if request.path.as_os_str().is_empty() {
            irori_core::paths::expand_home(&(self.settings)().default_install_path)
        } else {
            irori_core::paths::expand_home(&request.path.to_string_lossy())
        };
        base.join(irori_core::paths::sanitize(title.trim()))
    }

    /// Read or change whether this device is the account's offline PC.
    /// Changing it is limited by Microsoft: confirm with the user first.
    pub async fn offline_pc(&self, set: Option<bool>) -> Result<serde_json::Value> {
        self.ensure_device().await?;
        irori_msstore_api::ops::offline_pc::designation(
            &self.client,
            &self.tokens,
            set,
            Some(&self.layout.licenses_dir()),
        )
        .await
        .map_err(|e| {
            if e.status == 401 || e.status == 403 {
                anyhow::Error::new(e).context(SessionExpired::new(Runner::Xbox, "offline PC"))
            } else {
                anyhow::Error::new(e)
            }
        })
    }

    /// The player's (or the Store account's) Xbox profile with Game Pass
    /// membership: schema 1 of `ops::profile::profile_with_membership`.
    pub async fn profile(&self) -> Result<serde_json::Value> {
        let license = self.state.read().player_user.is_none();
        irori_msstore_api::ops::profile::profile_with_membership(&self.tokens, license)
            .await
            .map_err(|stage| session_error(stage, "Could not read the Xbox profile"))
    }

    /// An XSTS `Authorization` value for `relying_party`, of the player
    /// account when one is signed in, else the Store account.
    pub async fn xsts(&self, relying_party: &str) -> Result<String> {
        self.ensure_device().await?;
        let Token::Legacy(device) = self
            .tokens
            .get_device_sts_token()
            .map_err(|e| anyhow::anyhow!("{e}"))?
        else {
            bail!("The device sign-in is not usable");
        };
        let snapshot = self
            .tokens
            .user_token_snapshot_role(AccountRole::Xbox)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let Token::Legacy(user) = snapshot.token() else {
            bail!("Sign in to Xbox first");
        };
        let xsts = irori_msstore_api::api::xbox::run(&self.client, device, user, relying_party)
            .await
            .map_err(|e| match e {
                irori_msstore_api::api::xbox::SessionError::Refused => {
                    anyhow::Error::new(SessionExpired::new(Runner::Xbox, "Xbox sign-in refused"))
                }
                other => anyhow::anyhow!("{other}"),
            })?;
        irori_msstore_api::api::xbox::get_xsts_auth_header(xsts).map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Whether a Wine build can play Microsoft Store games.
    pub fn runtime_of(
        wine: &irori_core::WineInstallation,
    ) -> Option<irori_runtime::xbox::XboxRuntime> {
        irori_runtime::xbox::XboxRuntime::inspect(wine)
    }
}

/// The page's `DAProperty`, from the JSON the sign-in window printed (or
/// the bare object).
fn parse_da_property(input: &str) -> Result<DAProperty> {
    let value: serde_json::Value = serde_json::from_str(input.trim())
        .context("The sign-in window returned something unexpected. Try again.")?;
    let value = match value.get("text").and_then(|t| t.as_str()) {
        Some(text) => serde_json::from_str(text)
            .context("The sign-in window returned something unexpected. Try again.")?,
        None => value,
    };
    let prop: DAProperty =
        serde_json::from_value(value).context("The sign-in did not finish. Try again.")?;
    if prop.da_token.is_empty() || prop.username.is_empty() || prop.puid.is_empty() {
        bail!("The sign-in did not finish. Try again.");
    }
    Ok(prop)
}

/// A refused account stage becomes [`SessionExpired`].
fn session_error(stage: &'static str, what: &str) -> anyhow::Error {
    let refused = stage.contains("http") || stage.contains("refused") || stage == "stored-account";
    let error = anyhow::anyhow!("{what} ({stage})");
    if refused {
        error.context(SessionExpired::new(Runner::Xbox, stage))
    } else {
        error
    }
}

/// Licensing failures that the UI explains get their types.
fn license_error(message: String) -> anyhow::Error {
    if message.contains("501") || message.to_ascii_lowercase().contains("device limit") {
        unavailable(XboxProblem::DeviceLimit)
    } else if message.contains("not entitled") || message.contains("refused") {
        anyhow::Error::new(NotEntitled { message })
    } else {
        anyhow::anyhow!(message)
    }
}

#[async_trait]
impl Store for XboxStore {
    fn runner(&self) -> Runner {
        Runner::Xbox
    }

    async fn init(&self) -> Result<()> {
        let layout = self.layout.clone();
        let (library, installed) = Self::blocking(move || {
            #[cfg(unix)]
            for record in records::load_installed(&layout).values() {
                if let Some(parent) = record.root().parent() {
                    launch_view::sweep(parent);
                }
            }
            (
                records::load_library(&layout),
                records::load_installed(&layout),
            )
        })
        .await?;
        {
            let mut state = self.state.write();
            state.library = library;
            state.installed = installed;
            state.recompose();
        }
        // Reading the credentials may wait for the keyring: off the start.
        let tokens = self.tokens.clone();
        let users = Self::blocking(move || {
            (
                tokens.get_license_user().ok(),
                tokens.get_player_user().ok().flatten(),
            )
        })
        .await?;
        let mut state = self.state.write();
        state.store_user = users.0;
        state.player_user = users.1;
        Ok(())
    }

    fn user(&self) -> Option<UserInfo> {
        if !self.enabled() {
            return None;
        }
        self.state.read().store_user.as_ref().map(|u| UserInfo {
            account_id: u.puid.clone(),
            display_name: u.username.clone(),
        })
    }

    fn login_flow(&self) -> LoginFlow {
        LoginFlow::MicrosoftAccount { url: login_url() }
    }

    async fn login(&self, input: &str) -> Result<UserInfo> {
        self.sign_in(AccountRole::License, input).await
    }

    /// Signs both accounts out and forgets the device registration and the
    /// offline licences; installed games stay.
    async fn logout(&self) -> Result<()> {
        let tokens = self.tokens.clone();
        let licenses = self.layout.licenses_dir();
        Self::blocking(move || -> Result<()> {
            tokens
                .remove_all_accounts_and_device()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            irori_msstore_api::ops::subscription::clear();
            match std::fs::remove_dir_all(&licenses) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).context("Could not remove the offline licences"),
            }
        })
        .await??;
        {
            let mut state = self.state.write();
            state.store_user = None;
            state.player_user = None;
            state.library = LibraryCache::default();
            state.recompose();
        }
        let layout = self.layout.clone();
        let _ = Self::blocking(move || std::fs::remove_file(layout.library_file())).await;
        self.bus.emit(Event::AccountChanged(Runner::Xbox));
        self.bus.emit(Event::LibraryChanged(Runner::Xbox));
        Ok(())
    }

    fn accounts(&self) -> Vec<StoreAccount> {
        if !self.enabled() {
            return Vec::new();
        }
        let state = self.state.read();
        let games = state.games.iter().filter(|g| !g.is_dlc);
        let installed = games.clone().filter(|g| g.is_installed).count();
        let total = games.count();
        let expired = state
            .expired
            .then(|| "Microsoft no longer accepts the sign-in".to_string());
        let mut out = Vec::new();
        if let Some(user) = &state.store_user {
            out.push(StoreAccount {
                slot: MAIN_ACCOUNT.to_string(),
                account_id: user.puid.clone(),
                display_name: user.username.clone(),
                signed_in: true,
                games: total,
                installed,
                expired: expired.clone(),
            });
        }
        if let Some(user) = &state.player_user {
            out.push(StoreAccount {
                slot: PLAYER_ACCOUNT.to_string(),
                account_id: user.puid.clone(),
                display_name: user.username.clone(),
                signed_in: true,
                games: 0,
                installed: 0,
                expired,
            });
        }
        out
    }

    /// A separate Xbox player account can join the Store account.
    fn can_add_account(&self) -> bool {
        let state = self.state.read();
        self.enabled() && state.store_user.is_some() && state.player_user.is_none()
    }

    async fn account_login(&self, slot: Option<&str>, input: &str) -> Result<UserInfo> {
        let player = match slot {
            Some(PLAYER_ACCOUNT) => true,
            Some(_) => false,
            None => self.state.read().store_user.is_some(),
        };
        let role = if player {
            AccountRole::Xbox
        } else {
            AccountRole::License
        };
        self.sign_in(role, input).await
    }

    async fn account_logout(&self, slot: &str) -> Result<()> {
        if slot != PLAYER_ACCOUNT {
            return self.logout().await;
        }
        let tokens = self.tokens.clone();
        Self::blocking(move || tokens.remove_player().map_err(|e| anyhow::anyhow!("{e}")))
            .await??;
        self.state.write().player_user = None;
        self.bus.emit(Event::AccountChanged(Runner::Xbox));
        Ok(())
    }

    async fn refresh(&self, _full: bool) -> Result<()> {
        if !self.enabled() || self.state.read().store_user.is_none() {
            return Ok(());
        }
        let market = self.market();
        let entries = match self.fetch_library(&market).await {
            Ok(entries) => entries,
            Err(e) => {
                if SessionExpired::of(&e).is_some() {
                    self.state.write().expired = true;
                }
                return Err(e);
            }
        };
        let cache = LibraryCache { market, entries };
        let (layout, saved) = (self.layout.clone(), cache.clone());
        Self::blocking(move || records::save_library(&layout, &saved)).await??;
        {
            let mut state = self.state.write();
            state.library = cache;
            state.expired = false;
            state.recompose();
        }
        self.bus.emit(Event::LibraryChanged(Runner::Xbox));
        Ok(())
    }

    fn games(&self) -> Vec<Arc<GameInfo>> {
        if !self.enabled() {
            return Vec::new();
        }
        self.state.read().games.clone()
    }

    fn game(&self, app_name: &str) -> Option<Arc<GameInfo>> {
        if !self.enabled() {
            return None;
        }
        self.state.read().by_app.get(app_name).cloned()
    }

    async fn extra_info(&self, app_name: &str) -> Result<ExtraInfo> {
        let product = match self.api.product(app_name, &self.market()).await {
            Ok(Some(product)) => product,
            _ => self
                .product(app_name)
                .with_context(|| format!("{app_name} is not in your Xbox library"))?,
        };
        let game = library::game_info(&Entry {
            product,
            access: Access::Owned,
        });
        Ok(ExtraInfo {
            short_description: game.description.clone(),
            store_url: game.store_url.clone(),
            release_date: game.release_date.clone(),
            genres: game.genres.clone(),
            age_ratings: game.age_ratings.clone(),
            ..Default::default()
        })
    }

    async fn install_info(
        &self,
        app_name: &str,
        platform: InstallPlatform,
        query: &InstallInfoQuery,
    ) -> Result<InstallInfo> {
        let plan = self.package_plan(app_name).await?;
        let choice = packages::choose(&plan, query.build.as_deref());
        anyhow::ensure!(
            choice.is_some(),
            "The Microsoft Store offers no package of this game that plays on PC"
        );
        let title = self
            .game(app_name)
            .map(|g| g.title.clone())
            .or_else(|| choice.map(|c| c.title.clone()))
            .unwrap_or_default();
        Ok(InstallInfo {
            app_name: app_name.to_string(),
            title,
            platform,
            download_size: choice.and_then(|c| c.download_bytes).unwrap_or(0),
            disk_size: choice
                .and_then(|c| c.install_bytes.or(c.download_bytes))
                .unwrap_or(0),
            builds: packages::builds(&plan),
            is_native: false,
            ..Default::default()
        })
    }

    async fn updatable_games(&self) -> Result<Vec<String>> {
        if !self.enabled() || self.state.read().store_user.is_none() {
            return Ok(Vec::new());
        }
        let installed: Vec<(String, InstalledRecord)> = self
            .state
            .read()
            .installed
            .iter()
            .filter(|(_, r)| !r.pinned_version && !r.content_id.is_empty())
            .map(|(a, r)| (a.clone(), r.clone()))
            .collect();
        let mut out = Vec::new();
        for (app, record) in installed {
            let incomplete = install::state::incomplete(record.root()).unwrap_or(false);
            match self.package_url(&record.content_id).await {
                Ok((_, version)) if incomplete || newer(&version, &record.version) => out.push(app),
                Ok(_) => {}
                Err(e) => tracing::warn!("checking {} for updates: {e:#}", record.title),
            }
        }
        Ok(out)
    }

    async fn install(
        &self,
        app_name: &str,
        request: &InstallRequest,
        ctx: OpContext,
    ) -> Result<Outcome> {
        if !self.enabled() {
            bail!(TURNED_OFF);
        }
        let game = self
            .game(app_name)
            .with_context(|| format!("{app_name} is not in your Xbox library"))?;
        let plan = self.package_plan(app_name).await?;
        // A package the user chose in the install dialog comes as a build.
        let content = packages::choose(&plan, request.build.as_deref())
            .map(|c| c.content_id.clone())
            .context("The chosen package is not one this game offers for PC")?;
        let dir = self.install_dir(request, &game.title);
        self.install_into(app_name, &game.title, &content, dir, &ctx)
            .await
    }

    async fn update(
        &self,
        app_name: &str,
        _request: &UpdateRequest,
        ctx: OpContext,
    ) -> Result<Outcome> {
        let record = self.require_record(app_name)?;
        self.install_into(
            app_name,
            &record.title,
            &record.content_id,
            PathBuf::from(&record.install_path),
            &ctx,
        )
        .await
    }

    /// Checks every file against the package and writes what differs.
    async fn repair(&self, app_name: &str, ctx: OpContext) -> Result<Outcome> {
        self.update(app_name, &UpdateRequest::default(), ctx).await
    }

    async fn uninstall(&self, app_name: &str, args: RemoveArgs) -> Result<()> {
        let record = self.require_record(app_name)?;
        if args.delete_files {
            let dir = PathBuf::from(&record.install_path);
            let app = (self.settings)();
            Self::blocking(move || -> Result<()> {
                if !dir.exists() {
                    return Ok(());
                }
                irori_core::fs_guard::ensure_deletable(&dir, &app)?;
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("Could not delete {}", dir.display()))
            })
            .await??;
        }
        self.update_installed(|m| {
            m.remove(app_name);
        })
        .await
    }

    /// Adopts a folder installed by Irori or by Nexus Launcher (Xodus): its
    /// package metadata names the content.
    async fn import(&self, app_name: &str, path: &Path, _platform: InstallPlatform) -> Result<()> {
        let dir = path.to_path_buf();
        let (package, config) = Self::blocking({
            let dir = dir.clone();
            move || -> Result<(PathBuf, config::GameConfig)> {
                let ours = dir.join(install::PACKAGE_FILE);
                let xodus = dir.join(install::XODUS_PACKAGE_FILE);
                if !ours.is_file() && xodus.is_file() {
                    std::fs::copy(&xodus, &ours)
                        .context("Could not adopt the package metadata of Nexus Launcher")?;
                }
                anyhow::ensure!(
                    ours.is_file(),
                    "{} holds no Microsoft Store package installed by Irori or Nexus Launcher",
                    dir.display()
                );
                Ok((ours, config::GameConfig::read(&dir).unwrap_or_default()))
            }
        })
        .await??;
        let mut file = tokio::fs::File::open(&package).await?;
        let xvd = irori_msixvc::xvd::XvdFile::parse(&mut file)
            .await
            .map_err(|e| anyhow::anyhow!("The package metadata is unreadable: {e}"))?;
        let title = self
            .game(app_name)
            .map(|g| g.title.clone())
            .or(config.display_name.clone())
            .unwrap_or_else(|| app_name.to_string());
        let size = Self::blocking({
            let dir = dir.clone();
            move || irori_runtime::download::dir_size(&dir)
        })
        .await?;
        let record = InstalledRecord {
            title,
            install_path: dir.to_string_lossy().into_owned(),
            content_id: xvd.content_id().to_string(),
            version: config.version.clone().unwrap_or_default(),
            executable: config.executable().unwrap_or_default().to_string(),
            title_id: config.title_id_number(),
            size,
            ..Default::default()
        };
        self.update_installed(|m| {
            m.insert(app_name.to_string(), record);
        })
        .await
    }

    async fn set_install_path(&self, app_name: &str, path: &Path) -> Result<()> {
        self.require_record(app_name)?;
        let p = path.to_string_lossy().into_owned();
        self.update_installed(|m| {
            if let Some(r) = m.get_mut(app_name) {
                r.install_path = p;
            }
        })
        .await
    }

    async fn set_version_pinned(&self, app_name: &str, pinned: bool) -> Result<()> {
        self.require_record(app_name)?;
        self.update_installed(|m| {
            if let Some(r) = m.get_mut(app_name) {
                r.pinned_version = pinned;
            }
        })
        .await
    }

    async fn is_available(&self, app_name: &str) -> bool {
        self.state
            .read()
            .installed
            .get(app_name)
            .is_some_and(|r| r.root().join(install::PACKAGE_FILE).is_file())
    }

    async fn launch(
        &self,
        app_name: &str,
        request: &LaunchRequest,
        ctx: OpContext,
    ) -> Result<LaunchPlan> {
        #[cfg(unix)]
        {
            self.launch_unix(app_name, request, ctx).await
        }
        #[cfg(not(unix))]
        {
            let _ = (app_name, request, ctx);
            bail!("Microsoft Store games run through the Xbox app on this system")
        }
    }

    async fn after_exit(&self, app_name: &str, _played_secs: u64) -> Result<()> {
        #[cfg(unix)]
        self.sessions.lock().remove(app_name);
        let _ = app_name;
        Ok(())
    }

    /// Minutes the player account (else the Store account) played the
    /// installed games, as Xbox counts them; one request per game.
    async fn remote_playtime(&self) -> Result<HashMap<String, u64>> {
        if !self.enabled() || self.state.read().store_user.is_none() {
            return Ok(HashMap::new());
        }
        let titles: Vec<(String, u32)> = {
            // One guard: a second read while a writer waits would deadlock.
            let state = self.state.read();
            state
                .installed
                .keys()
                .filter_map(|app| Some((app.clone(), title_id_in(&state, app)?)))
                .collect()
        };
        let mut out = HashMap::new();
        for (app, title_id) in titles {
            match irori_msstore_api::ops::game_activity::activity(&self.tokens, title_id).await {
                Ok(value) => {
                    if let Some(minutes) = value["minutesPlayed"].as_f64() {
                        out.insert(app, minutes.round() as u64);
                    }
                }
                Err(stage) => tracing::debug!("Xbox activity of {app} unavailable: {stage}"),
            }
        }
        Ok(out)
    }

    async fn achievements(
        &self,
        app_name: &str,
        language: &str,
    ) -> Result<Vec<irori_core::Achievement>> {
        let _ = language;
        let Some(title_id) = self.title_id(app_name) else {
            return Ok(Vec::new());
        };
        let value = irori_msstore_api::ops::achievements::achievements(&self.tokens, title_id)
            .await
            .map_err(|stage| session_error(stage, "Could not read the Xbox achievements"))?;
        Ok(value["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(achievement)
            .collect())
    }
}

/// One achievement of `ops::achievements` as Irori shows it.
fn achievement(item: &serde_json::Value) -> Option<irori_core::Achievement> {
    let text = |key: &str| item[key].as_str().map(str::to_string);
    let unlocked = item["state"].as_str() == Some("unlocked");
    let icon = text("iconUrl").unwrap_or_default();
    Some(irori_core::Achievement {
        id: text("id")?,
        key: text("id")?,
        visible: unlocked || item["secret"].as_bool() != Some(true),
        name: text("name").unwrap_or_default(),
        description: text("description").unwrap_or_default(),
        image_unlocked: icon.clone(),
        image_locked: icon,
        unlocked_at: if unlocked {
            Some(text("unlockedAt").unwrap_or_default())
        } else {
            None
        },
        points: item["gamerscore"].as_u64().map_or(0, |p| p as u32),
        progress: item["progress"].as_f64().map(|p| (p / 100.0) as f32),
        ..Default::default()
    })
}

/// Whether package version `latest` is newer than `installed` (four
/// numbers; anything unreadable never counts as newer).
fn newer(latest: &str, installed: &str) -> bool {
    fn parse(v: &str) -> Option<[u32; 4]> {
        let numeric = v.split('.').take(4).collect::<Vec<_>>();
        if numeric.len() != 4 {
            return None;
        }
        let mut out = [0; 4];
        for (slot, part) in out.iter_mut().zip(numeric) {
            *slot = part.parse().ok()?;
        }
        Some(out)
    }
    match (parse(latest), parse(installed)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => !installed.is_empty() && latest != installed,
        _ => false,
    }
}

#[cfg(unix)]
impl XboxStore {
    async fn launch_unix(
        &self,
        app_name: &str,
        request: &LaunchRequest,
        ctx: OpContext,
    ) -> Result<LaunchPlan> {
        let record = self.require_record(app_name)?;
        let root = record.root().to_path_buf();
        let title = &record.title;
        anyhow::ensure!(
            root.is_dir(),
            "The files of {title} appear to be deleted (expected at {})",
            root.display()
        );
        let env = &request.environment;
        let wine = env
            .wine
            .as_ref()
            .ok_or_else(|| unavailable(XboxProblem::RuntimeMissing))?;
        let runtime = wine
            .build_bin
            .parent()
            .and_then(irori_runtime::xbox::XboxRuntime::at)
            .ok_or_else(|| unavailable(XboxProblem::RuntimeMissing))?;
        let guard = Self::blocking({
            let root = root.clone();
            move || install::state::launch_guard(&root)
        })
        .await?
        .map_err(|e| anyhow::anyhow!("{e}"))?;

        (ctx.step)("Checking the licence");
        let mut package = tokio::fs::File::open(root.join(install::PACKAGE_FILE))
            .await
            .context("The package metadata is missing. Verify the game's files.")?;
        let xvd = irori_msixvc::xvd::XvdFile::parse(&mut package)
            .await
            .map_err(|e| anyhow::anyhow!("The package metadata is unreadable: {e}"))?;
        let files = install::launch_files_of(&mut package)
            .await
            .map_err(|e| anyhow::anyhow!("The package metadata is unreadable: {e}"))?;
        let offline = request.offline;
        if !offline {
            let _ = self.ensure_device().await;
        }
        let (key, grant) = self
            .content_key(&xvd.content_id().to_string(), offline)
            .await?;

        let exe = match request.settings.target_exe.trim() {
            "" => record.executable.clone(),
            chosen => chosen.to_string(),
        };
        let exe = if exe.is_empty() {
            let root = root.clone();
            Self::blocking(move || config::GameConfig::read(&root))
                .await?
                .and_then(|c| c.executable().map(str::to_string))
                .context(
                    "The package names no program to start. Choose one in the game settings.",
                )?
        } else {
            exe
        };
        (ctx.step)("Preparing the game");
        let view = launch_view::prepare(&root, &xvd, &files, key, &exe).await?;

        let mut vars: BTreeMap<String, String> = env.env.clone();
        vars.extend(runtime.launch_env(&wine.prefix));
        for stale in [
            "XODUS_OFFLINE_IDENTITY",
            "XODUS_FORCE_OFFLINE",
            "XODUS_ACCOUNT_HASH",
            "XODUS_XSTS_PLAYFAB",
            "XODUS_PACKAGE_LOCALE",
            "WINEUSERLOCALE",
        ] {
            vars.remove(stale);
        }
        vars.extend(locale::environment(
            &request.settings.language,
            &(self.settings)().ui_language,
        ));
        let offline_used = offline || grant.offline_used;
        if offline_used {
            vars.insert("XODUS_FORCE_OFFLINE".into(), "1".into());
        }
        if let Some(mode) = irori_msstore_api::ops::license::offline_identity_mode(
            grant.offline_used,
            grant.lease_stored,
        ) {
            vars.insert("XODUS_OFFLINE_IDENTITY".into(), mode.into());
        }
        let service = if runtime.has_service_bridge() && !offline_used {
            match service::Session::start(self.tokens.clone(), &self.layout.service_dir()).await {
                Ok(session) => {
                    vars.extend(session.env());
                    Some(session)
                }
                Err(e) => {
                    tracing::warn!("Xbox account service unavailable: {e:#}");
                    None
                }
            }
        } else {
            None
        };
        if !offline_used && let Ok(header) = self.xsts(PLAYFAB_RELYING_PARTY).await {
            vars.insert("XODUS_XSTS_PLAYFAB".into(), header);
        }
        if let Some(user) = &self.state.read().store_user {
            vars.insert("XODUS_ACCOUNT_HASH".into(), account_hash(&user.puid));
        }
        let content_roots: Vec<String> = record
            .content
            .values()
            .map(|dir| irori_runtime::xbox::XboxRuntime::windows_path(Path::new(dir)))
            .collect();
        if !content_roots.is_empty() {
            vars.insert("XODUS_CONTENT_ROOTS".into(), content_roots.join(";"));
        }

        let mut prefix = wine.exec_prefix();
        let program = prefix.remove(0);
        let entry = view.entry().to_path_buf();
        let cwd = entry
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| view.path().to_path_buf());
        let mut args: Vec<String> =
            irori_core::process::split_program_args(true, &request.settings.launcher_args);
        args.extend(request.extra_args.iter().cloned());
        let command = CommandSpec::new(program)
            .args(prefix)
            .arg(irori_runtime::xbox::XboxRuntime::windows_path(&entry))
            .args(args)
            .cwd(cwd)
            .env(
                "STEAM_COMPAT_DATA_PATH",
                wine.prefix.to_string_lossy().into_owned(),
            )
            .envs(vars)
            .env("HEROIC_APP_SOURCE", "xbox")
            .wrapped(&env.wrappers);
        self.sessions.lock().insert(
            app_name.to_string(),
            GameSession {
                _view: view,
                _service: service,
                _guard: guard,
            },
        );
        Ok(LaunchPlan {
            command,
            track_wineserver: true,
            watch: None,
        })
    }
}

/// The country of a locale (`pt_BR` → `BR`, `en_US.UTF-8` → `US`); `None`
/// for a bare language, which is not a Store market.
fn region_of(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next()?;
    let mut parts = base.split(['_', '-']);
    parts.next()?;
    let region = parts.next_back()?;
    (region.len() == 2 && region.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| region.to_ascii_uppercase())
}

fn title_id_in(state: &State, app: &str) -> Option<u32> {
    state
        .installed
        .get(app)
        .and_then(|r| r.title_id)
        .or_else(|| {
            state
                .library
                .entries
                .get(app)?
                .product
                .xbox_title_id
                .as_ref()?
                .parse()
                .ok()
        })
}

/// An opaque key naming the account for the runtime's offline identity cache
/// (never the account itself).
fn account_hash(puid: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "irori-xbox-account".hash(&mut hasher);
    puid.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markets_are_countries_not_languages() {
        assert_eq!(region_of("pt_BR").as_deref(), Some("BR"));
        assert_eq!(region_of("en_US.UTF-8").as_deref(), Some("US"));
        assert_eq!(region_of("sr_RS.UTF-8@latin").as_deref(), Some("RS"));
        assert_eq!(region_of("zh-Hant-TW").as_deref(), Some("TW"));
        assert_eq!(region_of("en"), None);
        assert_eq!(region_of("zh_Hans"), None);
        assert_eq!(region_of("C.UTF-8"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("1.2.10.0", "1.2.9.0"));
        assert!(!newer("1.2.9.0", "1.2.10.0"));
        assert!(!newer("1.0.0.0", "1.0.0.0"));
        assert!(!newer("garbage", "1.0.0.0"));
        assert!(newer("1.0.0.1", "old"));
        assert!(!newer("1.0.0.1", ""));
    }

    #[test]
    fn da_property_from_the_window_or_bare() {
        let prop = r#"{"sDAToken":"t","sDASessionKey":"k","sDAStartTime":"s","sDAExpires":"e","sSTSInlineFlowToken":"f","sSigninName":"a@b.c","K":"0123"}"#;
        let wrapped = serde_json::json!({ "text": prop }).to_string();
        assert_eq!(parse_da_property(&wrapped).unwrap().username, "a@b.c");
        assert_eq!(parse_da_property(prop).unwrap().puid, "0123");
        assert!(parse_da_property("{}").is_err());
        assert!(parse_da_property("not json").is_err());
    }

    #[test]
    fn licence_failures_get_their_types() {
        let limit = license_error("Licensing failed (code 501)".into());
        assert_eq!(
            XboxUnavailable::of(&limit).map(|e| e.problem),
            Some(XboxProblem::DeviceLimit)
        );
        assert!(NotEntitled::of(&license_error("Microsoft refused the licence".into())).is_some());
        assert!(XboxUnavailable::of(&license_error("timeout".into())).is_none());
    }

    #[test]
    fn account_hash_is_stable_and_hides_the_account() {
        assert_eq!(account_hash("abc"), account_hash("abc"));
        assert_ne!(account_hash("abc"), account_hash("abd"));
        assert!(!account_hash("abc").contains("abc"));
    }
}
