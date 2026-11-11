//! Tests of [`super`]: catalog parsing and matching, downloads checked
//! against the release's digests (from an in-process server), files the
//! user chose, and journaled installs beside ReShade.

use std::sync::atomic::{AtomicUsize, Ordering};

use irori_depot::test_server::{Resp, TestServer};

use super::*;
use crate::pe::tests::build_pe;
use crate::{Api, ReshadeError};

const DL: &str = "https://github.com/clshortfuse/renodx/releases/download/snapshot/";

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn x64_addon() -> Vec<u8> {
    build_pe(0x8664, &["kernel32.dll"], &[])
}

/// RenoDX's catalog, trimmed to the shapes it uses.
fn index() -> String {
    serde_json::json!({
        "games": [
            {
                "id": "hades-ii", "title": "Hades II", "aliases": ["Hades 2"],
                "deploy": {"steam_appid": 1145350, "game_exe": "Hades2.exe"},
                "mods": [{
                    "id": "hades2", "title": "Hades II", "variant": null,
                    "support": null, "category": "game", "status": "beta",
                    "notes": [], "deploy": {"steam_appid": 1145350},
                    "artifacts": [
                        {"name": "renodx-hades2.addon64", "arch": "x64", "url": "./renodx-hades2.addon64"}
                    ]
                }, {
                    "id": "hades2-alt", "title": "Hades II", "variant": "Alternate",
                    "status": "stable",
                    "artifacts": [
                        {"name": "renodx-hades2alt.addon64", "arch": "x64"}
                    ]
                }]
            },
            {
                "id": "alien", "title": "Alien: Isolation™", "aliases": [],
                "deploy": {"architecture": ["x86"]},
                "mods": [{
                    "id": "alienisolation", "title": "Alien: Isolation", "status": "stable",
                    "deploy": {"game_exe": "AI.exe"},
                    "artifacts": [
                        {"name": "renodx-alienisolation.addon32", "arch": "x86"},
                        {"name": "renodx-alienisolation.addon64", "arch": "x86"}
                    ]
                }]
            },
            {
                "id": "ue", "title": "Unreal Engine", "aliases": [],
                "mods": [{
                    "id": "unrealengine", "title": "Generic Unreal Engine",
                    "support": "engine", "category": "engine",
                    "artifacts": [{"name": "renodx-unrealengine.addon64", "arch": "x64"}]
                }]
            },
            {
                "id": "other-game", "title": "Another Game",
                "mods": [{
                    "id": "unrealengine", "title": "Generic Unreal Engine",
                    "artifacts": [{"name": "renodx-unrealengine.addon64", "arch": "x64"}]
                }]
            },
            {
                "id": "missing", "title": "Not Built",
                "mods": [{"id": "notbuilt", "title": "Not Built",
                    "artifacts": [{"name": "renodx-notbuilt.addon64", "arch": "x64"}]}]
            },
            {
                "id": "fork", "title": "Forked",
                "mods": [{"id": "forked", "title": "Forked",
                    "artifacts": [{"name": "renodx-forked.addon64", "arch": "x64"}]}]
            },
            {
                "id": "nodigest", "title": "No Digest",
                "mods": [{"id": "nodigest", "title": "No Digest",
                    "artifacts": [{"name": "renodx-nodigest.addon64", "arch": "x64"}]}]
            },
            {"id": "weird", "title": 42, "mods": "not a list"}
        ],
        "somethingNew": {"a": 1}
    })
    .to_string()
}

fn asset(name: &str, url: &str, digest: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "size": 2600448,
        "digest": digest,
        "updated_at": "2026-11-10T01:56:52Z",
        "browser_download_url": url,
    })
}

/// GitHub's listing of the release, with `files` as `(name, bytes)` served
/// from `downloads`.
fn release(downloads: &str, files: &[(&str, &[u8])]) -> String {
    let mut assets: Vec<serde_json::Value> = files
        .iter()
        .map(|(name, bytes)| {
            asset(
                name,
                &format!("{downloads}{name}"),
                Some(&format!("sha256:{}", sha(bytes))),
            )
        })
        .collect();
    assets.push(asset(
        "renodx-forked.addon64",
        "https://example.com/fork/renodx-forked.addon64",
        Some(&format!("sha256:{}", "a".repeat(64))),
    ));
    assets.push(asset(
        "renodx-nodigest.addon64",
        &format!("{downloads}renodx-nodigest.addon64"),
        None,
    ));
    assets.push(asset(
        "decomp.exe",
        &format!("{downloads}decomp.exe"),
        Some(&format!("sha256:{}", "b".repeat(64))),
    ));
    serde_json::json!({"tag_name": "snapshot", "assets": assets}).to_string()
}

fn official_release() -> String {
    release(
        DL,
        &[
            ("renodx-hades2.addon64", b"hades"),
            ("renodx-hades2alt.addon64", b"alt"),
            ("renodx-alienisolation.addon32", b"alien"),
            ("renodx-alienisolation.addon64", b"alien64"),
            ("renodx-unrealengine.addon64", b"ue"),
        ],
    )
}

fn catalog() -> Catalog {
    Catalog::parse(index().as_bytes(), official_release().as_bytes(), DL).unwrap()
}

#[test]
fn offers_only_mods_the_official_release_holds_with_a_digest() {
    let c = catalog();
    let ids: Vec<&str> = c.mods.iter().map(|m| m.id.as_str()).collect();
    // Sorted by name; not built, forked and digest-less files are left out.
    assert_eq!(
        ids,
        ["alienisolation", "unrealengine", "hades2", "hades2-alt"]
    );

    let hades = c.get("hades2").unwrap();
    assert_eq!(hades.games, ["Hades II", "Hades 2"]);
    assert_eq!(hades.exes, ["Hades2.exe"]);
    assert_eq!(hades.status.as_deref(), Some("beta"));
    assert!(!hades.generic);
    let build = hades.build_for(Machine::X64).unwrap();
    assert_eq!(build.name, "renodx-hades2.addon64");
    assert_eq!(build.sha256, sha(b"hades"));
    assert_eq!(build.built, "2026-11-10");
    assert_eq!(build.size, 2600448);
    assert_eq!(build.url, format!("{DL}renodx-hades2.addon64"));
    assert!(hades.build_for(Machine::X86).is_none());

    assert_eq!(c.get("hades2-alt").unwrap().label(), "Hades II (Alternate)");
    assert_eq!(
        c.get("hades2-alt").unwrap().source(),
        ModSource {
            id: "hades2-alt".into(),
            title: "Hades II (Alternate)".into()
        }
    );

    // An x86 build named .addon64 is not taken for either architecture.
    let alien = c.get("alienisolation").unwrap();
    assert_eq!(alien.builds.len(), 1);
    assert_eq!(alien.builds[0].machine, Machine::X86);
    assert_eq!(alien.exes, ["AI.exe"]);

    // Listed under two games, offered once, for both.
    let ue = c.get("unrealengine").unwrap();
    assert!(ue.generic);
    assert_eq!(ue.games, ["Unreal Engine", "Another Game"]);
}

#[test]
fn damaged_or_empty_lists_are_refused() {
    let release = official_release();
    assert!(Catalog::parse(b"not json", release.as_bytes(), DL).is_err());
    assert!(Catalog::parse(b"{}", release.as_bytes(), DL).is_err());
    assert!(Catalog::parse(index().as_bytes(), b"{\"assets\": []}", DL).is_err());
    assert!(Catalog::parse(index().as_bytes(), b"[]", DL).is_err());
    // Files of another release folder are never taken.
    let elsewhere = release.replace(
        DL,
        "https://github.com/someone/renodx/releases/download/snapshot/",
    );
    assert!(Catalog::parse(index().as_bytes(), elsewhere.as_bytes(), DL).is_err());
}

#[test]
fn suggests_mods_by_title_alias_or_program() {
    let c = catalog();
    let ids = |found: Vec<&RenodxMod>| found.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
    // Stable mods first.
    assert_eq!(ids(c.suggest("HADES II", None)), ["hades2-alt", "hades2"]);
    assert_eq!(ids(c.suggest("Hades 2", None)), ["hades2-alt", "hades2"]);
    assert_eq!(ids(c.suggest("Alien Isolation", None)), ["alienisolation"]);
    assert_eq!(
        ids(c.suggest("Something", Some("ai.EXE"))),
        ["alienisolation"]
    );
    assert!(c.suggest("Hades", None).is_empty());
    assert!(c.suggest("", None).is_empty());
}

#[test]
fn recognises_addon_files() {
    assert!(is_addon_name("renodx-game.addon64"));
    assert!(is_addon_name("Mine.ADDON32"));
    assert!(is_addon_name("x.addon"));
    assert!(!is_addon_name(".addon64"));
    assert!(!is_addon_name("renodx-game.dll"));
    assert!(!is_addon_name("renodx-game.addon64.irori-disabled"));
}

#[test]
fn checks_a_chosen_file() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    let ok = write("renodx-mine.addon64", &x64_addon());
    let (name, bytes) = read_local(&ok, Machine::X64).unwrap();
    assert_eq!(name, "renodx-mine.addon64");
    assert_eq!(bytes, x64_addon());

    let err = read_local(&ok, Machine::X86).unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::AddonArch)
    ));
    let not_pe = write("renodx-text.addon64", b"hello");
    let err = read_local(&not_pe, Machine::X64).unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::NotAddon)
    ));
    let dll = write("renodx-mine.dll", &x64_addon());
    let err = read_local(&dll, Machine::X64).unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::NotAddon)
    ));
}

#[test]
fn finds_another_renodx_addon_in_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(other_addon(dir.path(), "renodx-a.addon64"), None);
    std::fs::write(dir.path().join("renodx-a.addon64"), b"").unwrap();
    std::fs::write(dir.path().join("other.addon64"), b"").unwrap();
    std::fs::write(dir.path().join("renodx-notes.txt"), b"").unwrap();
    assert_eq!(other_addon(dir.path(), "RENODX-A.addon64"), None);
    std::fs::write(dir.path().join("renodx-b.addon64.irori-disabled"), b"").unwrap();
    assert_eq!(
        other_addon(dir.path(), "renodx-a.addon64").as_deref(),
        Some("renodx-b.addon64.irori-disabled")
    );
}

/// A server for the catalog, the release listing and the files under
/// `/dl/`, counting requests.
struct Server {
    server: TestServer,
    hits: Arc<AtomicUsize>,
}

impl Server {
    fn start(files: Vec<(&'static str, Vec<u8>)>, listed: Vec<(&'static str, Vec<u8>)>) -> Self {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let base = Arc::new(std::sync::Mutex::new(String::new()));
        let base_in = base.clone();
        let server = TestServer::start(move |req| {
            counter.fetch_add(1, Ordering::SeqCst);
            let downloads = format!("{}/dl/", base_in.lock().unwrap());
            match req.path.as_str() {
                "/index.json" => Resp {
                    status: 200,
                    body: index().into_bytes(),
                    ..Default::default()
                },
                "/release.json" => {
                    let listed: Vec<(&str, &[u8])> =
                        listed.iter().map(|(n, b)| (*n, b.as_slice())).collect();
                    Resp {
                        status: 200,
                        body: release(&downloads, &listed).into_bytes(),
                        ..Default::default()
                    }
                }
                path => match files.iter().find(|(n, _)| path == format!("/dl/{n}")) {
                    Some((_, bytes)) => Resp {
                        status: 200,
                        body: bytes.clone(),
                        ..Default::default()
                    },
                    None => Resp::status(404),
                },
            }
        });
        *base.lock().unwrap() = server.base();
        Self { server, hits }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            index: self.server.url("/index.json"),
            release: self.server.url("/release.json"),
            downloads: self.server.url("/dl/"),
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn no_progress() -> ProgressSink {
    Arc::new(|_| {})
}

#[tokio::test]
async fn reads_the_catalog_once_a_day_unless_an_install_asks() {
    let addon = x64_addon();
    let server = Server::start(vec![], vec![("renodx-hades2.addon64", addon)]);
    let cache = tempfile::tempdir().unwrap();
    let c = super::catalog(&server.endpoints(), cache.path(), false)
        .await
        .unwrap();
    assert_eq!(c.mods.len(), 1);
    assert_eq!(
        c.mods[0].builds[0].url,
        server.server.url("/dl/renodx-hades2.addon64")
    );
    assert_eq!(server.hits(), 2);
    super::catalog(&server.endpoints(), cache.path(), false)
        .await
        .unwrap();
    assert_eq!(server.hits(), 2, "a fresh copy is not asked again");
    super::catalog(&server.endpoints(), cache.path(), true)
        .await
        .unwrap();
    assert_eq!(server.hits(), 4, "an install asks again");

    // The source gone, the copy stands in.
    let gone = Endpoints {
        index: "http://127.0.0.1:9/index.json".into(),
        release: "http://127.0.0.1:9/release.json".into(),
        downloads: server.endpoints().downloads,
    };
    assert_eq!(super::catalog(&gone, cache.path(), true).await.unwrap(), c);
}

#[tokio::test]
async fn downloads_are_checked_against_the_release_digest() {
    let good = x64_addon();
    let server = Server::start(
        vec![
            ("renodx-hades2.addon64", good.clone()),
            ("renodx-hades2alt.addon64", b"tampered".to_vec()),
            ("renodx-alienisolation.addon32", x64_addon()),
        ],
        vec![
            ("renodx-hades2.addon64", good.clone()),
            ("renodx-hades2alt.addon64", x64_addon()),
            ("renodx-alienisolation.addon32", x64_addon()),
        ],
    );
    let cache = tempfile::tempdir().unwrap();
    let c = super::catalog(&server.endpoints(), cache.path(), false)
        .await
        .unwrap();
    let cancel = CancellationToken::new();

    let build = c.get("hades2").unwrap().build_for(Machine::X64).unwrap();
    let bytes = download(build, cache.path(), &no_progress(), &cancel)
        .await
        .unwrap();
    assert_eq!(bytes, good);
    let before = server.hits();
    download(build, cache.path(), &no_progress(), &cancel)
        .await
        .unwrap();
    assert_eq!(server.hits(), before, "the cached file is used");

    let tampered = c
        .get("hades2-alt")
        .unwrap()
        .build_for(Machine::X64)
        .unwrap();
    let err = download(tampered, cache.path(), &no_progress(), &cancel)
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("not the file RenoDX"),
        "{err:#}"
    );

    // Listed as 32-bit, but the file is a 64-bit DLL.
    let wrong = c
        .get("alienisolation")
        .unwrap()
        .build_for(Machine::X86)
        .unwrap();
    let err = download(wrong, cache.path(), &no_progress(), &cancel)
        .await
        .unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::AddonArch)
    ));
}

fn reshade_manifest(dir: &Path) -> Manifest {
    Manifest {
        kind: GraphicsMod::Reshade,
        version: "6.8.0".into(),
        exe: dir.join("Game.exe"),
        machine: Machine::X64,
        api: Api::Dxgi,
        dll: "dxgi.dll".into(),
        dir: dir.to_path_buf(),
        enabled: true,
        complete: true,
        overrides: Vec::new(),
        files: Vec::new(),
        created_dirs: Vec::new(),
        addon: true,
        source: None,
    }
}

fn steps() -> Arc<dyn Fn(Step) + Send + Sync> {
    Arc::new(|_| {})
}

#[tokio::test]
async fn installs_an_official_build_beside_reshade() {
    let addon = x64_addon();
    let server = Server::start(
        vec![("renodx-hades2.addon64", addon.clone())],
        vec![("renodx-hades2.addon64", addon.clone())],
    );
    let cache = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let journal = state.path().join("renodx/legendary/Hades2.json");
    let c = super::catalog(&server.endpoints(), cache.path(), true)
        .await
        .unwrap();
    let m = c.get("hades2").unwrap();
    let request = AddonRequest {
        reshade: reshade_manifest(game.path()),
        choice: AddonChoice::Official {
            build: m.build_for(Machine::X64).unwrap().clone(),
            source: m.source(),
        },
        journal: journal.clone(),
        cache: cache.path().to_path_buf(),
    };
    let mut installed = install(request, steps(), no_progress(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(installed.kind, GraphicsMod::Renodx);
    assert_eq!(installed.version, "2026-11-10");
    assert_eq!(installed.dll, "renodx-hades2.addon64");
    assert_eq!(installed.source.as_ref().unwrap().id, "hades2");
    assert!(installed.overrides.is_empty());
    assert_eq!(installed.switch_record().unwrap().sha256, sha(&addon));
    assert_eq!(
        std::fs::read(game.path().join("renodx-hades2.addon64")).unwrap(),
        addon
    );
    assert_eq!(journal::load(&journal).unwrap().as_ref(), Some(&installed));

    // Off and on rename the add-on so ReShade doesn't load it.
    journal::set_enabled(&journal, &mut installed, false).unwrap();
    assert!(
        game.path()
            .join("renodx-hades2.addon64.irori-disabled")
            .exists()
    );
    journal::set_enabled(&journal, &mut installed, true).unwrap();
    let report = journal::remove(&journal, &installed, &|_| Ok(())).unwrap();
    assert_eq!(report.removed, 1);
    assert!(!game.path().join("renodx-hades2.addon64").exists());
    assert!(!journal.exists());
}

#[tokio::test]
async fn refuses_what_is_not_irori_s() {
    let game = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let journal = state.path().join("g.json");
    let chosen = state.path().join("renodx-mine.addon64");
    std::fs::write(&chosen, x64_addon()).unwrap();
    let request = |reshade: Manifest| AddonRequest {
        reshade,
        choice: AddonChoice::File(chosen.clone()),
        journal: journal.clone(),
        cache: cache.path().to_path_buf(),
    };
    let run = |req| install(req, steps(), no_progress(), CancellationToken::new());

    // ReShade's standard build can't load add-ons.
    let mut standard = reshade_manifest(game.path());
    standard.addon = false;
    assert!(run(request(standard)).await.is_err());

    // A file of the same name that Irori didn't write.
    std::fs::write(game.path().join("renodx-mine.addon64"), b"user's").unwrap();
    let err = run(request(reshade_manifest(game.path())))
        .await
        .unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::DllTaken { .. })
    ));
    assert_eq!(
        std::fs::read(game.path().join("renodx-mine.addon64")).unwrap(),
        b"user's"
    );
    std::fs::remove_file(game.path().join("renodx-mine.addon64")).unwrap();

    // Another RenoDX mod would hook the game too.
    std::fs::write(game.path().join("renodx-other.addon64"), b"other").unwrap();
    let err = run(request(reshade_manifest(game.path())))
        .await
        .unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::AddonTaken { name }) if name == "renodx-other.addon64"
    ));
    assert!(!journal.exists());
    std::fs::remove_file(game.path().join("renodx-other.addon64")).unwrap();

    // The user's own file, with no version and no source.
    let m = run(request(reshade_manifest(game.path()))).await.unwrap();
    assert_eq!(m.dll, "renodx-mine.addon64");
    assert_eq!(m.version, "");
    assert_eq!(m.source, None);

    // A 32-bit game gets no 64-bit add-on.
    let mut x86 = reshade_manifest(game.path());
    x86.machine = Machine::X86;
    let mut req = request(x86);
    req.journal = state.path().join("other.json");
    let err = run(req).await.unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::AddonArch)
    ));
}
