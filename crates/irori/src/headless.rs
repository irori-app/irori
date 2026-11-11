//! Command-line operations that run without a window: `irori <command>`.
//! Useful for scripts, SSH sessions and troubleshooting.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::util::{format_bytes, format_duration};
use irori_core::{
    GameId, GameInfo, InstallInfoQuery, InstallPlatform, InstallRequest, RemoveArgs, Runner,
};
use irori_services::{Backend, DownloadState};

pub const COMMANDS: &[&str] = &[
    "import-heroic",
    "import-lutris",
    "import-bottles",
    "import-faugus",
    "import-playnite",
    "list",
    "refresh",
    "info",
    "playtime",
    "install",
    "uninstall",
    "accounts",
    "sunshine-sync",
    "steam-shortcuts",
];

pub const HELP: &str = "Commands (run without opening a window):
  irori accounts                      Show which stores are logged in
  irori import-heroic                 Import accounts, installs and settings from Heroic
  irori import-lutris                 Import games, categories and playtime from Lutris
  irori import-bottles                Import the programs of every Bottles bottle
  irori import-faugus                 Import the games of Faugus Launcher
  irori import-playnite               Import the games and playtime of Playnite (Windows)
  irori refresh                       Refresh every store's library
  irori list [--installed]            List the library
  irori list --json                   Installed games as JSON: folders, programs,
                                      Wine prefixes and builds (for other tools)
  irori info <runner>/<app> [--platform windows|linux]
                                      Show download size, languages, DLCs and builds
  irori playtime <runner>/<app>       Show recorded playtime (and GOG's server total)
  irori install <runner>/<app> [--path DIR] [--platform windows|linux]
                                      Install a game (Ctrl+C stops; run again to resume)
  irori uninstall <runner>/<app> [--keep-files]
                                      Uninstall a game, or discard a stopped install
  irori sunshine-sync [--remove]      Bring Irori's games in Sunshine up to date
                                      (--remove: take them out of Sunshine)
  irori steam-shortcuts [--remove] [<runner>/<app>...]
                                      Bring Irori's Steam shortcuts up to date for every
                                      installed game, or the games named (Steam must be
                                      closed; --remove: take them out of Steam)";

/// Run a headless command. Returns the process exit code.
pub fn run(args: &[String]) -> Result<i32> {
    let backend = Backend::start_headless(crate::stores::build)?;
    let rt = backend.runtime();
    rt.block_on(async {
        wait_for_stores(&backend).await;
        match args[0].as_str() {
            "accounts" => accounts(&backend),
            "import-heroic" => import_heroic(&backend).await,
            "import-lutris" => import_lutris(&backend).await,
            "import-bottles" => {
                let s =
                    crate::lutris::import_bottles(&backend, crate::stores::SIDELOAD.get().cloned())
                        .await?;
                print_launcher_summary("Bottles", &s);
                Ok(0)
            }
            "import-faugus" => {
                let s =
                    crate::lutris::import_faugus(&backend, crate::stores::SIDELOAD.get().cloned())
                        .await?;
                print_launcher_summary("Faugus Launcher", &s);
                Ok(0)
            }
            "import-playnite" => {
                let s = crate::playnite::import(&backend, crate::stores::SIDELOAD.get().cloned())
                    .await?;
                print_launcher_summary("Playnite", &s);
                Ok(0)
            }
            "refresh" => {
                backend.refresh_all(false).await;
                println!("{} games", backend.library().len());
                Ok(0)
            }
            "list" if args.iter().any(|a| a == "--json") => {
                let export = crate::export::collect(&backend).await;
                println!("{}", serde_json::to_string_pretty(&export)?);
                Ok(0)
            }
            "list" => list(&backend, args.iter().any(|a| a == "--installed")),
            "sunshine-sync" => {
                let report = if args.iter().any(|a| a == "--remove") {
                    backend.remove_from_sunshine().await?
                } else {
                    backend.sync_sunshine().await?
                };
                println!(
                    "Sunshine: {} added, {} updated, {} removed",
                    report.added, report.updated, report.removed
                );
                for line in &report.failed {
                    eprintln!("  {line}");
                }
                Ok(i32::from(!report.failed.is_empty()))
            }
            "steam-shortcuts" => steam_shortcuts(&backend, &args[1..]).await,
            "info" => info(&backend, &args[1..]).await,
            "playtime" => playtime(&backend, &args[1..]).await,
            "install" => install(&backend, &args[1..]).await,
            "uninstall" => uninstall(&backend, &args[1..]).await,
            other => bail!("unknown command {other}"),
        }
    })
}

/// Stores initialise in the background at startup; wait until the first
/// library build happened.
async fn wait_for_stores(backend: &Backend) {
    for _ in 0..200 {
        if backend.library().revision > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn parse_id(s: Option<&String>) -> Result<GameId> {
    let s = s.context("missing <runner>/<app>")?;
    let (r, app) = s.split_once('/').context("expected <runner>/<app>")?;
    let runner = Runner::from_id(r).with_context(|| format!("unknown runner {r}"))?;
    Ok(GameId::new(runner, app))
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
}

/// `--remove` and the games named, from `steam-shortcuts`' arguments.
fn steam_shortcut_args(args: &[String]) -> Result<(bool, Vec<GameId>)> {
    let mut remove = false;
    let mut games = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--remove" => remove = true,
            a if a.starts_with('-') => bail!("unknown option {a}"),
            _ => games.push(parse_id(Some(arg))?),
        }
    }
    Ok((remove, games))
}

/// The games a full sync gives a Steam shortcut: installed ones that Steam
/// doesn't list itself, without DLCs and without hidden games while they
/// are locked.
fn steam_shortcut_games<'a>(
    games: impl IntoIterator<Item = &'a Arc<GameInfo>>,
    concealed: impl Fn(&GameId) -> bool,
) -> Vec<Arc<GameInfo>> {
    games
        .into_iter()
        .filter(|g| g.is_installed && !g.is_dlc && g.runner() != Runner::Steam)
        .filter(|g| !concealed(&g.id()))
        .cloned()
        .collect()
}

/// `steam-shortcuts`: bring Irori's Steam shortcuts up to date for every
/// installed game (removing those of games that left the library), or for
/// the games named; with `--remove`, take them out of Steam.
async fn steam_shortcuts(backend: &Backend, args: &[String]) -> Result<i32> {
    use irori_extras::shortcuts;
    let (remove, picked) = steam_shortcut_args(args)?;
    let app = backend.settings.app();
    let report = if remove {
        let ids: Option<HashSet<GameId>> =
            (!picked.is_empty()).then(|| picked.into_iter().collect());
        shortcuts::remove_from_steam(ids.as_ref(), &app).await?
    } else if picked.is_empty() {
        let library = backend.library();
        let games = steam_shortcut_games(library.all_games(), |id| backend.concealed(id));
        let known: HashSet<GameId> = library.all_games().map(|g| g.id()).collect();
        // A store that is signed out lists fewer games; its shortcuts stay.
        let listing: HashSet<Runner> = backend
            .stores()
            .filter(|s| !s.runner().has_account() || s.user().is_some())
            .map(|s| s.runner())
            .collect();
        let gone = |id: &GameId| !known.contains(id) && listing.contains(&id.runner);
        shortcuts::sync_steam(&games, Some(&gone), &app).await?
    } else {
        let games = picked
            .iter()
            .map(|id| {
                backend.game(id).with_context(|| {
                    format!("{}/{} isn't in your library", id.runner.id(), id.app_name)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        shortcuts::sync_steam(&games, None, &app).await?
    };
    println!(
        "Steam: {} added, {} updated, {} removed",
        report.added, report.updated, report.removed
    );
    for line in &report.errors {
        eprintln!("  {line}");
    }
    Ok(i32::from(!report.errors.is_empty()))
}

fn accounts(backend: &Backend) -> Result<i32> {
    for store in backend.stores() {
        let name = store.runner().store_name();
        let accounts = store.accounts();
        if accounts.is_empty() || !store.runner().has_account() {
            let state = match store.user() {
                Some(u) => format!("logged in as {}", u.display_name),
                None => "not logged in".into(),
            };
            let count = backend.library().count(store.runner());
            println!("{name:<16} {state} ({count} games)");
            continue;
        }
        for account in accounts {
            let state = if account.expired.is_some() {
                format!("{} must log in again", account.display_name)
            } else if account.signed_in {
                format!("logged in as {}", account.display_name)
            } else {
                format!("{} logged out", account.display_name)
            };
            let count = if account.signed_in {
                format!("{} games", account.games)
            } else {
                format!("{} installed", account.installed)
            };
            println!("{name:<16} {state} ({count})");
        }
    }
    Ok(0)
}

async fn import_heroic(backend: &Backend) -> Result<i32> {
    let dir =
        irori_core::paths::heroic_config_dir().context("No Heroic configuration was found")?;
    println!("Importing from {}", dir.display());
    let imports = crate::stores::HEROIC_IMPORTS;
    for (name, import) in imports {
        let d = dir.clone();
        match tokio::task::spawn_blocking(move || import(&d)).await? {
            Ok(()) => println!("✓ {name}"),
            Err(e) => println!("✗ {name}: {e:#}"),
        }
    }
    for store in backend.stores() {
        store.init().await?;
    }
    let mut summary = irori_ui::hooks::ImportSummary::default();
    crate::import::apply_launcher_data(backend, &dir, &mut summary).await?;
    println!(
        "Settings for {} games, {} favourites, {} categories",
        summary.games_with_settings, summary.favourites, summary.categories
    );
    backend.refresh_all(false).await;
    accounts(backend)
}

/// What a Bottles, Faugus Launcher or Playnite import did, for the terminal.
fn print_launcher_summary(launcher: &str, s: &irori_ui::hooks::LutrisSummary) {
    println!("{} games in {launcher}: {} imported", s.found, s.imported);
    if s.matched > 0 {
        println!("{} already in Irori", s.matched);
    }
    if s.favourites + s.categories + s.playtime_games > 0 {
        println!(
            "{} favourites, {} category entries, playtime for {} games",
            s.favourites, s.categories, s.playtime_games
        );
    }
    for (title, store) in &s.store_games {
        println!("→ {title}: log in to {store} in Irori to see it");
    }
    for (title, build) in &s.wine_missing {
        println!("! {title}: Wine build {build} isn't installed; it uses your default build");
    }
    for skip in &s.skipped {
        match &skip.detail {
            Some(d) => println!("✗ {}: {} ({d})", skip.title, skip.reason),
            None => println!("✗ {}: {}", skip.title, skip.reason),
        }
    }
    for w in &s.warnings {
        println!("! {w}");
    }
}

async fn import_lutris(backend: &Backend) -> Result<i32> {
    let s = crate::lutris::import(
        backend,
        crate::stores::SIDELOAD.get().cloned(),
        crate::stores::EMULATION.get().cloned(),
    )
    .await?;
    println!(
        "{} games in Lutris: {} imported, {} already in Irori",
        s.found, s.imported, s.matched
    );
    println!(
        "{} favourites, {} category entries, playtime for {} games",
        s.favourites, s.categories, s.playtime_games
    );
    for (title, store) in &s.store_games {
        println!("→ {title}: log in to {store} in Irori to see it");
    }
    for (title, build) in &s.wine_missing {
        println!("! {title}: Wine build {build} isn't installed; it uses your default build");
    }
    for skip in &s.skipped {
        match &skip.detail {
            Some(d) => println!("✗ {}: {} ({d})", skip.title, skip.reason),
            None => println!("✗ {}: {}", skip.title, skip.reason),
        }
    }
    for w in &s.warnings {
        println!("! {w}");
    }
    Ok(0)
}

fn list(backend: &Backend, installed_only: bool) -> Result<i32> {
    for g in backend
        .library()
        .games
        .iter()
        .filter(|g| !installed_only || g.is_installed)
        // Hidden games behind the PIN stay hidden here too.
        .filter(|g| !backend.concealed(&g.id()))
    {
        let state = match &g.install {
            Some(i) if g.is_installed => format!("installed {} {}", i.platform, i.install_size),
            _ => String::new(),
        };
        println!(
            "{:<10} {:<28} {:<40} {state}",
            g.runner().short_name(),
            g.app_name,
            g.title
        );
    }
    Ok(0)
}

async fn info(backend: &Backend, args: &[String]) -> Result<i32> {
    let id = parse_id(args.first())?;
    let store = backend.store(id.runner).context("store not available")?;
    let platform = flag_value(args, "--platform")
        .and_then(|p| InstallPlatform::parse(p))
        .unwrap_or_default();
    let i = store
        .install_info(&id.app_name, platform, &InstallInfoQuery::default())
        .await?;
    println!("{} ({platform})", i.title);
    println!("  download   {}", format_bytes(i.download_size));
    println!("  on disk    {}", format_bytes(i.disk_size));
    println!("  version    {}", i.version.unwrap_or_default());
    println!("  languages  {}", i.languages.join(", "));
    println!("  native     {}", i.is_native);
    for d in &i.dlcs {
        println!("  dlc        {} ({})", d.title, d.app_name);
    }
    for b in i.builds.iter().take(5) {
        println!("  build      {} {} {}", b.build_id, b.version_name, b.date);
    }
    if !i.branches.is_empty() {
        println!("  branches   {}", i.branches.join(", "));
    }
    if !i.dependencies.is_empty() {
        println!("  requires   {}", i.dependencies.join(", "));
    }
    Ok(0)
}

async fn playtime(backend: &Backend, args: &[String]) -> Result<i32> {
    let id = parse_id(args.first())?;
    let user = backend.userdata.snapshot();
    let rec = user.playtime.get(&id.key()).cloned().unwrap_or_default();
    println!(
        "Irori:  {} min (last played {})",
        rec.total_minutes,
        rec.last_played.as_deref().unwrap_or("never")
    );
    if id.runner == Runner::Gog
        && let Some(gog) = crate::stores::GOG.get()
    {
        match gog.store_for(&id.app_name).gog_playtime(&id.app_name).await {
            Some(m) => println!("GOG:    {m} min"),
            None => println!("GOG:    unavailable"),
        }
    }
    Ok(0)
}

async fn install(backend: &Backend, args: &[String]) -> Result<i32> {
    let id = parse_id(args.first())?;
    let game = backend
        .game(&id)
        .with_context(|| format!("{id} is not in the library"))?;
    if game.is_installed {
        bail!("{} is already installed", game.title);
    }
    let store = backend.store(id.runner).context("store not available")?;
    let platform = flag_value(args, "--platform")
        .and_then(|p| InstallPlatform::parse(p))
        .unwrap_or_else(|| {
            if game.platforms.contains(&InstallPlatform::Linux) {
                InstallPlatform::Linux
            } else {
                InstallPlatform::Windows
            }
        });
    let path = flag_value(args, "--path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&backend.settings.app().default_install_path));
    let info = store
        .install_info(&id.app_name, platform, &InstallInfoQuery::default())
        .await?;
    println!(
        "Installing {} ({platform}) to {} — download {}, on disk {}",
        game.title,
        path.display(),
        format_bytes(info.download_size),
        format_bytes(info.disk_size)
    );
    let request = InstallRequest {
        path,
        platform,
        language: preferred_language(&info.languages, &backend.settings.app().ui_language),
        dependencies: info.dependencies.clone(),
        ..Default::default()
    };
    backend.enqueue_install(id.clone(), game.title.clone(), request, info.download_size);

    let stop = tokio::signal::ctrl_c();
    tokio::pin!(stop);
    let mut stopped = false;
    loop {
        tokio::select! {
            _ = &mut stop, if !stopped => {
                println!("\nStopping (run the same command to resume)…");
                backend.cancel_download(&id, true);
                stopped = true;
            }
            _ = tokio::time::sleep(Duration::from_millis(1000)) => {}
        }
        if let Some(s) = backend.statuses.get(&id)
            && let Some(p) = s.progress
        {
            print!(
                "\r{} {:>5.1}%  {} / {}  {}/s  eta {}      ",
                s.kind.label(),
                p.percent.unwrap_or(0.0),
                format_bytes(p.bytes_done.unwrap_or(0)),
                format_bytes(p.bytes_total.unwrap_or(info.download_size)),
                format_bytes(p.download_speed.unwrap_or(0)),
                format_duration(p.eta_secs.unwrap_or(0)),
            );
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        if !backend.is_queued(&id) {
            break;
        }
    }
    let finished = backend
        .downloads()
        .finished
        .into_iter()
        .find(|i| i.id == id);
    println!();
    match finished.map(|i| (i.state, i.error)) {
        Some((DownloadState::Done, _)) => {
            println!("Installed.");
            Ok(0)
        }
        Some((DownloadState::Canceled, _)) => {
            println!("Stopped. Partial files kept.");
            Ok(130)
        }
        Some((_, err)) => {
            println!("Failed: {}", err.unwrap_or_default());
            Ok(1)
        }
        None => Ok(1),
    }
}

async fn uninstall(backend: &Backend, args: &[String]) -> Result<i32> {
    let id = parse_id(args.first())?;
    let keep = args.iter().any(|a| a == "--keep-files");
    let installed = backend.game(&id).is_some_and(|g| g.is_installed);
    let remove = RemoveArgs {
        delete_files: !keep,
        ..Default::default()
    };
    if installed {
        backend.uninstall(&id, remove).await?;
    } else {
        // A stopped install: discard its partial download.
        let store = backend.store(id.runner).context("store not available")?;
        store.uninstall(&id.app_name, remove).await?;
    }
    println!("Done.");
    Ok(0)
}

/// The game language matching the interface language (`de` → `de-DE`),
/// else English, else the store's first.
fn preferred_language(languages: &[String], ui: &str) -> Option<String> {
    let base = |s: &str| s.split(['-', '_']).next().unwrap_or(s).to_ascii_lowercase();
    let want = base(ui);
    languages
        .iter()
        .find(|l| base(l) == want)
        .or_else(|| languages.iter().find(|l| l.eq_ignore_ascii_case("en-US")))
        .or_else(|| languages.iter().find(|l| base(l) == "en"))
        .or_else(|| languages.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steam_shortcut_arguments() {
        let args = |list: &[&str]| -> Vec<String> { list.iter().map(|s| s.to_string()).collect() };
        assert_eq!(steam_shortcut_args(&[]).unwrap(), (false, Vec::new()));
        assert_eq!(
            steam_shortcut_args(&args(&["gog/1", "--remove", "epic/Fortnite"])).unwrap(),
            (
                true,
                vec![
                    GameId::new(Runner::Gog, "1"),
                    GameId::new(Runner::Legendary, "Fortnite")
                ]
            )
        );
        assert!(steam_shortcut_args(&args(&["--nope"])).is_err());
        assert!(steam_shortcut_args(&args(&["gog"])).is_err());
    }

    #[test]
    fn syncs_installed_games_steam_doesnt_list() {
        let game = |runner, app: &str, installed, dlc| {
            Arc::new(GameInfo {
                runner: Some(runner),
                app_name: app.into(),
                title: app.into(),
                is_installed: installed,
                is_dlc: dlc,
                ..Default::default()
            })
        };
        let games = [
            game(Runner::Gog, "installed", true, false),
            game(Runner::Gog, "not-installed", false, false),
            game(Runner::Gog, "dlc", true, true),
            game(Runner::Steam, "620", true, false),
            game(Runner::Sideload, "locked", true, false),
        ];
        let picked = steam_shortcut_games(&games, |id| id.app_name.as_ref() == "locked");
        let names: Vec<&str> = picked.iter().map(|g| g.app_name.as_str()).collect();
        assert_eq!(names, ["installed"]);
    }

    #[test]
    fn picks_ui_language_then_english() {
        let langs: Vec<String> = ["ja-JP", "de-DE", "en-US"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(preferred_language(&langs, "de").as_deref(), Some("de-DE"));
        assert_eq!(preferred_language(&langs, "fr").as_deref(), Some("en-US"));
        assert_eq!(preferred_language(&[], "fr"), None);
    }
}
