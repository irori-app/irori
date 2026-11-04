use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Once;

use irori_core::process::CancellationToken;
use irori_core::{AppSettings, Paths};
use irori_depot::test_server::{Resp, TestServer};
use serde_json::json;

use crate::deploy::{self, LinkKind, deploy_links, deploy_links_using, undeploy};
use crate::fomod::{self, Choices, FileState, GroupKind, PluginType};
use crate::model::new_id;
use crate::thunderstore::{self, GameKeys};
use crate::{Layer, ModEntry, ModLibrary, ModList, ModSource, conflicts, layout, nexus, overlay};

fn init_paths() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = tempfile::tempdir().expect("tempdir").keep();
        Paths::init(Paths::under(&root));
    });
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Every entry under `dir` (files with their content, folders marked).
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.insert(format!("{rel}/"), String::new());
                walk(root, &path, out);
            } else if meta.file_type().is_symlink() {
                out.insert(rel, "<link>".into());
            } else {
                out.insert(rel, std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn entry(name: &str) -> ModEntry {
    ModEntry {
        id: String::new(),
        name: name.into(),
        version: Some("1.0".into()),
        author: None,
        summary: None,
        source: ModSource::Archive {
            file_name: format!("{name}.zip"),
        },
        enabled: true,
        installed_at: String::new(),
        size: 0,
        files: 0,
        dependencies: Vec::new(),
    }
}

/// A game folder, a library and two mods: `a` adds and replaces files,
/// `b` replaces one of `a`'s.
struct Fixture {
    _tmp: tempfile::TempDir,
    game: PathBuf,
    lib: ModLibrary,
    list: ModList,
}

fn fixture() -> Fixture {
    init_paths();
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("Game");
    write(&game, "Game.exe", "exe");
    write(&game, "readme.txt", "original readme");
    write(&game, "BepInEx/plugins/keep.dll", "game plugin");
    let lib = ModLibrary::new(tmp.path().join("mods"));
    let mut list = ModList::default();
    let app = AppSettings::default();
    for (name, files) in [
        (
            "Mod A",
            vec![
                ("readme.txt", "a readme"),
                ("BepInEx/plugins/a.dll", "a"),
                ("new/deep/a.cfg", "cfg a"),
            ],
        ),
        (
            "Mod B",
            vec![("readme.txt", "b readme"), ("new/deep/b.cfg", "cfg b")],
        ),
    ] {
        let staging = lib.staging().unwrap();
        let tree = staging.path().join("tree");
        for (rel, body) in files {
            write(&tree, rel, body);
        }
        lib.add(&mut list, &tree, entry(name), None, &app).unwrap();
    }
    Fixture {
        _tmp: tmp,
        game,
        lib,
        list,
    }
}

#[test]
fn ids_are_safe_folder_names() {
    for name in ["Mod A", "../../etc", "", "Ünïcødé ✓", "a.b/c\\d"] {
        let id = new_id(name);
        assert!(crate::is_valid_id(&id), "{id}");
    }
    assert!(new_id("SkyUI 5.2").starts_with("skyui-5-2-"));
    assert!(!crate::is_valid_id("../x"));
    assert!(!crate::is_valid_id("mods.json"));
    assert!(!crate::is_valid_id(".written"));
}

#[test]
fn list_order_moves() {
    let mut list = ModList::default();
    for id in ["a", "b", "c"] {
        list.mods.push(ModEntry {
            id: id.into(),
            ..entry(id)
        });
    }
    assert!(list.move_to("a", 2));
    assert!(!list.move_to("a", 9));
    list.reorder(&["c".into(), "zzz".into()]);
    let ids: Vec<&str> = list.mods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["c", "b", "a"]);
}

#[test]
fn library_adds_replaces_and_removes() {
    let mut f = fixture();
    assert_eq!(f.list.mods.len(), 2);
    let a = f.list.mods[0].clone();
    assert_eq!(a.files, 3);
    assert_eq!(a.size, "a readme".len() as u64 + 1 + 5);
    let mut index = f.lib.index(&a.id).unwrap();
    index.sort();
    assert_eq!(
        index,
        ["BepInEx/plugins/a.dll", "new/deep/a.cfg", "readme.txt"]
    );
    // Reloaded from disk.
    assert_eq!(f.lib.load().unwrap(), f.list);

    // An update keeps its place and switch.
    let app = AppSettings::default();
    f.list.mods[0].enabled = false;
    let staging = f.lib.staging().unwrap();
    write(staging.path(), "t/readme.txt", "a2");
    let new = f
        .lib
        .add(
            &mut f.list,
            &staging.path().join("t"),
            entry("Mod A"),
            Some(&a.id),
            &app,
        )
        .unwrap();
    assert_eq!(f.list.mods[0].id, new);
    assert!(!f.list.mods[0].enabled);
    assert!(!f.lib.root().join(&a.id).exists());

    f.lib.remove(&mut f.list, &new, &app).unwrap();
    assert_eq!(f.list.mods.len(), 1);
    assert!(!f.lib.root().join(&new).exists());
    // Staging folders are gone once dropped.
    drop(staging);
    assert_eq!(
        std::fs::read_dir(f.lib.root().join(".staging"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn hand_edited_ids_cannot_leave_the_library() {
    let f = fixture();
    let mut list = f.list.clone();
    list.mods[0].id = "../../escape".into();
    f.lib.save(&list).unwrap();
    assert_eq!(f.lib.load().unwrap().mods.len(), 1);
    assert!(f.lib.files_dir("../x").is_err());
}

fn round_trip(kinds: &[LinkKind]) {
    let f = fixture();
    let before = snapshot(&f.game);
    let layers = f.lib.layers(&f.list).unwrap();
    let manifest = f.lib.manifest_file();
    let d = deploy_links_using(&f.game, &layers, &manifest, kinds).unwrap();
    assert!(deploy::is_deployed(&manifest));
    let read = |rel: &str| std::fs::read_to_string(f.game.join(rel)).unwrap();
    // Mod B is later in the load order and wins.
    assert_eq!(read("readme.txt"), "b readme");
    assert_eq!(read("BepInEx/plugins/a.dll"), "a");
    assert_eq!(read("BepInEx/plugins/keep.dll"), "game plugin");
    assert_eq!(read("new/deep/a.cfg"), "cfg a");
    assert_eq!(read("readme.txt.irori-original"), "original readme");
    assert_eq!(d.dirs, ["new", "new/deep"]);
    assert!(d.files.iter().all(|f| f.how.is_some()));
    // Deploying twice is refused rather than stacking.
    assert!(deploy_links(&f.game, &layers, &manifest).is_err());

    let report = undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(report.restored, 1);
    assert_eq!(report.kept, 0);
    assert_eq!(snapshot(&f.game), before);
    assert!(!manifest.exists());
    // Mods are untouched.
    let a = &f.list.mods[0];
    assert_eq!(
        std::fs::read_to_string(f.lib.files_dir(&a.id).unwrap().join("readme.txt")).unwrap(),
        "a readme"
    );
}

#[test]
fn deploy_and_undeploy_restore_the_game_exactly() {
    round_trip(&[LinkKind::Hard]);
    round_trip(&[LinkKind::Copy]);
    #[cfg(unix)]
    round_trip(&[LinkKind::Symbolic]);
}

#[test]
fn undeploy_recovers_from_an_interrupted_deployment() {
    let f = fixture();
    let before = snapshot(&f.game);
    let layers = f.lib.layers(&f.list).unwrap();
    let manifest = f.lib.manifest_file();
    let mut d = deploy_links(&f.game, &layers, &manifest).unwrap();
    // Pretend it stopped before the readme was replaced: the game file is
    // back in place and no step is recorded as done.
    let readme = f.game.join("readme.txt");
    std::fs::remove_file(&readme).unwrap();
    std::fs::rename(f.game.join("readme.txt.irori-original"), &readme).unwrap();
    for file in &mut d.files {
        file.how = None;
    }
    irori_core::json_store::write(&manifest, &d).unwrap();
    undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(snapshot(&f.game), before);

    // Stopped after setting the readme aside, before linking it.
    let mut d = deploy_links(&f.game, &layers, &manifest).unwrap();
    std::fs::remove_file(&readme).unwrap();
    for file in &mut d.files {
        file.how = None;
    }
    irori_core::json_store::write(&manifest, &d).unwrap();
    undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(snapshot(&f.game), before);

    // Running it again is harmless.
    undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(snapshot(&f.game), before);
}

#[test]
fn files_the_game_writes_are_kept_and_applied_next_time() {
    let mut f = fixture();
    let manifest = f.lib.manifest_file();
    let layers = f.lib.layers(&f.list).unwrap();
    deploy_links_using(&f.game, &layers, &manifest, &[LinkKind::Copy]).unwrap();
    let before_mods = snapshot(f.lib.root());
    // The game changes a mod's file, writes a log in a mod folder, and a
    // save in its own folder.
    write(&f.game, "new/deep/b.cfg", "changed by the game");
    write(&f.game, "new/log.txt", "log");
    write(&f.game, "save.dat", "save");
    let report = undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(report.kept, 2);
    assert!(!f.game.join("new").exists());
    // The save was the game's own business and stays.
    assert_eq!(
        std::fs::read_to_string(f.game.join("save.dat")).unwrap(),
        "save"
    );
    assert_eq!(f.lib.written_files(), 2);
    // The mods themselves are unchanged.
    let after_mods = snapshot(f.lib.root());
    for (k, v) in &before_mods {
        if !k.starts_with(".written") && k != "deployment.json" {
            assert_eq!(after_mods.get(k), Some(v), "{k}");
        }
    }
    // Next time the game's version wins.
    f.list = f.lib.load().unwrap();
    let layers = f.lib.layers(&f.list).unwrap();
    deploy_links(&f.game, &layers, &manifest).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.game.join("new/deep/b.cfg")).unwrap(),
        "changed by the game"
    );
    assert_eq!(
        std::fs::read_to_string(f.game.join("new/log.txt")).unwrap(),
        "log"
    );
    undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
}

#[test]
fn a_replaced_hard_link_is_kept_not_lost() {
    let f = fixture();
    let manifest = f.lib.manifest_file();
    let layers = f.lib.layers(&f.list).unwrap();
    deploy_links_using(&f.game, &layers, &manifest, &[LinkKind::Hard]).unwrap();
    // Games often save by writing a new file and renaming it over the old.
    let cfg = f.game.join("new/deep/a.cfg");
    std::fs::remove_file(&cfg).unwrap();
    std::fs::write(&cfg, "rewritten").unwrap();
    let report = undeploy(&manifest, &f.lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(report.kept, 1);
    assert_eq!(
        std::fs::read_to_string(f.lib.written_dir().join("new/deep/a.cfg")).unwrap(),
        "rewritten"
    );
}

#[test]
fn a_leftover_backup_stops_the_deployment() {
    let f = fixture();
    write(&f.game, "readme.txt.irori-original", "stale");
    let layers = f.lib.layers(&f.list).unwrap();
    let manifest = f.lib.manifest_file();
    assert!(deploy_links(&f.game, &layers, &manifest).is_err());
    assert!(!manifest.exists());
    assert_eq!(
        std::fs::read_to_string(f.game.join("readme.txt")).unwrap(),
        "original readme"
    );
}

#[test]
fn deployment_follows_the_game_folders_letter_case() {
    // Holds on case-sensitive and case-insensitive file systems alike: the
    // spelling comes from the game folder's listing, not from what opens.
    let f = fixture();
    let lib = &f.lib;
    write(&f.game, "Docs/Readme.TXT", "game docs");
    let before = snapshot(&f.game);
    let mut list = ModList::default();
    let staging = lib.staging().unwrap();
    write(staging.path(), "t/bepinex/Plugins/case.dll", "x");
    write(staging.path(), "t/docs/readme.txt", "mod docs");
    lib.add(
        &mut list,
        &staging.path().join("t"),
        entry("Case"),
        None,
        &AppSettings::default(),
    )
    .unwrap();
    let layers = lib.layers(&list).unwrap();
    let plan = deploy::plan(&f.game, &layers).unwrap();
    assert!(plan.contains_key("BepInEx/plugins/case.dll"), "{plan:?}");
    assert!(plan.contains_key("Docs/Readme.TXT"), "{plan:?}");
    assert_eq!(
        crate::align_case(&f.game, "bepinex/PLUGINS/x/y"),
        "BepInEx/plugins/x/y"
    );

    // Putting the replaced file back keeps the game's own spelling.
    let manifest = lib.manifest_file();
    deploy_links(&f.game, &layers, &manifest).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.game.join("Docs/Readme.TXT")).unwrap(),
        "mod docs"
    );
    undeploy(&manifest, &lib.written_dir(), &AppSettings::default()).unwrap();
    assert_eq!(snapshot(&f.game), before);
}

#[test]
fn conflicts_name_the_winner_by_load_order() {
    let a: Vec<String> = vec!["readme.txt".into(), "a.dll".into()];
    let b: Vec<String> = vec!["README.TXT".into(), "b.dll".into()];
    let c: Vec<String> = vec!["b.dll".into()];
    let mods: Vec<(&str, &[String])> = vec![("a", &a), ("b", &b), ("c", &c)];
    let found = conflicts::find(&mods, |p| p.eq_ignore_ascii_case("readme.txt"));
    assert_eq!(found.files.len(), 2);
    let readme = found
        .files
        .iter()
        .find(|f| f.path.eq_ignore_ascii_case("readme.txt"))
        .unwrap();
    assert_eq!(readme.mods, ["a", "b"]);
    assert_eq!(readme.winner(), "b");
    let b = found.of("b").unwrap();
    assert_eq!(b.wins_over.get("a"), Some(&1));
    assert_eq!(b.loses_to.get("c"), Some(&1));
    assert_eq!(b.game_files, 1);
    assert!(!b.hidden);
    assert!(found.of("c").unwrap().wins_over.contains_key("b"));
    // A mod whose every file is overridden does nothing.
    let solo: Vec<String> = vec!["b.dll".into()];
    let mods: Vec<(&str, &[String])> = vec![("x", &solo), ("c", &c)];
    assert!(conflicts::find(&mods, |_| false).of("x").unwrap().hidden);
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            if name.ends_with('/') {
                zip.add_directory(*name, opts).unwrap();
            } else {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(body).unwrap();
            }
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

#[tokio::test]
async fn zips_unpack_inside_the_destination_only() {
    let tmp = tempfile::tempdir().unwrap();
    let good = tmp.path().join("good.zip");
    std::fs::write(
        &good,
        zip_of(&[
            ("Mod/", b""),
            ("Mod\\Data\\a.esp", b"esp"),
            ("Mod/readme.txt", b"hi"),
        ]),
    )
    .unwrap();
    let out = tmp.path().join("out");
    crate::archive::extract(&good, &out, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(std::fs::read(out.join("Mod/Data/a.esp")).unwrap(), b"esp");

    for evil in ["../escape.txt", "/abs.txt", "Mod/../../escape.txt"] {
        let bad = tmp.path().join("bad.zip");
        std::fs::write(&bad, zip_of(&[("ok.txt", b"ok"), (evil, b"evil")])).unwrap();
        let out = tmp.path().join(format!("bad-out-{}", evil.len()));
        let result = crate::archive::extract(&bad, &out, &CancellationToken::new()).await;
        assert!(result.is_err(), "{evil}");
        assert!(!tmp.path().join("escape.txt").exists());
    }
}

#[test]
fn archives_are_recognised_by_content() {
    let tmp = tempfile::tempdir().unwrap();
    let z = tmp.path().join("mod.bin");
    std::fs::write(&z, zip_of(&[("a", b"a")])).unwrap();
    assert_eq!(crate::archive::kind(&z), Some(crate::archive::Kind::Zip));
    let r = tmp.path().join("mod.dat");
    std::fs::write(&r, b"Rar!\x1a\x07\x01\x00rest").unwrap();
    assert_eq!(crate::archive::kind(&r), Some(crate::archive::Kind::Rar));
    let s = tmp.path().join("x");
    std::fs::write(&s, b"7z\xbc\xaf\x27\x1c\x00\x04").unwrap();
    assert_eq!(
        crate::archive::kind(&s),
        Some(crate::archive::Kind::SevenZip)
    );
    let t = tmp.path().join("notes.txt");
    std::fs::write(&t, b"hello").unwrap();
    assert_eq!(crate::archive::kind(&t), None);
}

#[test]
fn rar_listings_are_read() {
    let seven = "Path = Mod/a.dll\nSize = 3\n\nPath = Mod\\..\\x\nSize = 1\n";
    assert_eq!(
        crate::archive::rar_names_for_test(true, seven),
        ["Mod/a.dll", "Mod\\..\\x"]
    );
    assert_eq!(
        crate::archive::rar_names_for_test(false, "a.txt\nb/c.txt\n\n"),
        ["a.txt", "b/c.txt"]
    );
}

#[test]
fn layout_finds_where_a_mod_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("game");
    write(&game, "Data/Skyrim.esm", "");
    write(&game, "SkyrimSE.exe", "");
    let x = tmp.path().join("x");
    write(&x, "Cool Mod v2/Cool Mod/meshes/a.nif", "");
    write(&x, "__MACOSX/._junk", "");
    let root = layout::content_root(&x, &game);
    assert!(root.ends_with("Cool Mod v2/Cool Mod"), "{}", root.display());
    assert_eq!(layout::data_dir(&game).as_deref(), Some("Data"));
    assert_eq!(layout::target_prefix(&root, &game), "Data");

    // A mod already laid out like the game stays at the root.
    let y = tmp.path().join("y");
    write(&y, "wrapper/data/b.esp", "");
    let root = layout::content_root(&y, &game);
    assert!(root.ends_with("wrapper"));
    assert_eq!(layout::target_prefix(&root, &game), "");

    // Games without a data folder take files at their root.
    let plain = tmp.path().join("plain");
    write(&plain, "Game.exe", "");
    assert_eq!(layout::target_prefix(&root, &plain), "");

    let tree = tmp.path().join("tree");
    let n = layout::import(&root, &tree, &game, |rel| Some(layout::prefixed("", rel))).unwrap();
    assert_eq!(n, 1);
    // `data` is spelled as the game spells it.
    assert!(tree.join("Data/b.esp").is_file());
}

const FOMOD_XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<config xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="http://qconsulting.ca/fo3/ModConfig5.0.xsd">
  <moduleName> Better Things </moduleName>
  <moduleImage path="fomod\images\cover.png"/>
  <requiredInstallFiles>
    <folder source="Core" destination=""/>
  </requiredInstallFiles>
  <installSteps order="Explicit">
    <installStep name="Textures">
      <optionalFileGroups order="Explicit">
        <group name="Resolution" type="SelectExactlyOne">
          <plugins order="Explicit">
            <plugin name="2K">
              <description>Sharper.</description>
              <image path="fomod\images\2k.png"/>
              <files><folder source="Textures\2K" destination="textures" priority="0"/></files>
              <conditionFlags><flag name="res">2k</flag></conditionFlags>
              <typeDescriptor><type name="Recommended"/></typeDescriptor>
            </plugin>
            <plugin name="1K">
              <description>Faster.</description>
              <files><folder source="textures\1k" destination="textures"/></files>
              <conditionFlags><flag name="res">1k</flag></conditionFlags>
              <typeDescriptor><type name="Optional"/></typeDescriptor>
            </plugin>
          </plugins>
        </group>
        <group name="Extras" type="SelectAny">
          <plugins order="Ascending">
            <plugin name="Zeta patch">
              <description/>
              <files><file source="Patches\zeta.esp" destination="zeta.esp"/></files>
              <typeDescriptor>
                <dependencyType>
                  <defaultType name="NotUsable"/>
                  <patterns>
                    <pattern>
                      <dependencies operator="And"><fileDependency file="Zeta.esm" state="Active"/></dependencies>
                      <type name="Recommended"/>
                    </pattern>
                  </patterns>
                </dependencyType>
              </typeDescriptor>
            </plugin>
            <plugin name="Alpha patch">
              <description>Always useful.</description>
              <files><file source="Patches\alpha.esp" destination="patches\" alwaysInstall="false"/></files>
              <typeDescriptor><type name="Optional"/></typeDescriptor>
            </plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
    <installStep name="Only for 2K">
      <visible><flagDependency flag="res" value="2k"/></visible>
      <optionalFileGroups>
        <group name="Normals" type="SelectAll">
          <plugins>
            <plugin name="Normal maps">
              <description>Required with 2K.</description>
              <files><file source="Textures\2K_n.dds" destination="textures\n.dds"/></files>
              <typeDescriptor><type name="Required"/></typeDescriptor>
            </plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
  </installSteps>
  <conditionalFileInstalls>
    <patterns>
      <pattern>
        <dependencies operator="Or">
          <flagDependency flag="res" value="1k"/>
          <flagDependency flag="never" value="on"/>
        </dependencies>
        <files><file source="Extra\low.ini" destination="low.ini" priority="5"/></files>
      </pattern>
    </patterns>
  </conditionalFileInstalls>
</config>"#;

fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[test]
fn fomod_parses_steps_groups_and_conditions() {
    let config = fomod::parse(&utf16le_with_bom(FOMOD_XML)).unwrap();
    assert_eq!(config.name, "Better Things");
    assert_eq!(config.image.as_deref(), Some("fomod\\images\\cover.png"));
    assert_eq!(config.required.len(), 1);
    assert_eq!(config.steps.len(), 2);
    let group = &config.steps[0].groups[0];
    assert_eq!(group.kind, GroupKind::ExactlyOne);
    assert_eq!(group.plugins[0].name, "2K");
    assert_eq!(
        group.plugins[0].image.as_deref(),
        Some("fomod\\images\\2k.png")
    );
    // Ascending order sorts by name.
    assert_eq!(config.steps[0].groups[1].plugins[0].name, "Alpha patch");
    assert_eq!(
        config.steps[0].groups[1].plugins[0].files[0].destination,
        "patches/alpha.esp"
    );

    let no_files = |_: &str| FileState::Missing;
    let zeta = |p: &str| {
        if p.eq_ignore_ascii_case("zeta.esm") {
            FileState::Active
        } else {
            FileState::Missing
        }
    };
    let patch = &config.steps[0].groups[1].plugins[1];
    let flags = Default::default();
    assert_eq!(
        fomod::ModuleConfig::plugin_type(patch, &flags, &no_files),
        PluginType::NotUsable
    );
    assert_eq!(
        fomod::ModuleConfig::plugin_type(patch, &flags, &zeta),
        PluginType::Recommended
    );

    // Defaults: the recommended 2K, then the 2K-only step with its
    // required option.
    let choices = config.default_choices(&no_files);
    assert_eq!(choices.0[0], vec![vec![true, false], vec![false, false]]);
    assert!(config.step_visible(1, &choices, &no_files));
    assert_eq!(choices.0[1], vec![vec![true]]);
    assert!(config.group_valid(0, 0, &choices));
    let files = config.files(&choices, &no_files);
    let sources: Vec<&str> = files.iter().map(|f| f.source.as_str()).collect();
    assert_eq!(sources, ["Core", "Textures/2K", "Textures/2K_n.dds"]);

    // Picking 1K hides the 2K step and adds the conditional file last.
    let mut one_k = choices.clone();
    one_k.0[0][0] = vec![false, true];
    assert!(!config.step_visible(1, &one_k, &no_files));
    let files = config.files(&one_k, &no_files);
    let sources: Vec<&str> = files.iter().map(|f| f.source.as_str()).collect();
    assert_eq!(sources, ["Core", "textures/1k", "Extra/low.ini"]);

    let none = Choices(vec![vec![vec![false, false], vec![]]]);
    assert!(!config.group_valid(0, 0, &none));
}

#[test]
fn fomod_installs_chosen_files_case_insensitively() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pkg");
    write(&root, "fomod/ModuleConfig.xml", FOMOD_XML);
    write(&root, "core/base.esp", "base");
    write(&root, "textures/2k/rock.dds", "2k rock");
    write(&root, "textures/1k/rock.dds", "1k rock");
    write(&root, "Textures/2K_n.dds", "normals");
    write(&root, "extra/low.ini", "low");
    let (found_root, xml) = fomod::find(tmp.path()).unwrap();
    assert_eq!(found_root, root);
    let config = fomod::parse(&std::fs::read(xml).unwrap()).unwrap();
    let no_files = |_: &str| FileState::Missing;
    let choices = config.default_choices(&no_files);
    let tree = tmp.path().join("tree");
    let n = fomod::install(&root, &config.files(&choices, &no_files), &tree).unwrap();
    assert_eq!(n, 3);
    assert_eq!(
        std::fs::read_to_string(tree.join("base.esp")).unwrap(),
        "base"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("textures/rock.dds")).unwrap(),
        "2k rock"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("textures/n.dds")).unwrap(),
        "normals"
    );
}

#[test]
fn fomod_refuses_unsafe_paths_and_bad_xml() {
    let evil = r#"<config><moduleName>x</moduleName><requiredInstallFiles>
        <file source="a.esp" destination="..\..\evil.esp"/></requiredInstallFiles></config>"#;
    assert!(fomod::parse(evil.as_bytes()).is_err());
    // A leading separator stays inside the installer's folder.
    let rooted = r#"<config><requiredInstallFiles><file source="\etc\passwd"/></requiredInstallFiles></config>"#;
    assert_eq!(
        fomod::parse(rooted.as_bytes()).unwrap().required[0].source,
        "etc/passwd"
    );
    assert!(fomod::parse(b"<config><unclosed></config>").is_err());
    assert!(fomod::parse(b"<notaconfig/>").is_err());
    // Entities from a DTD are not expanded.
    let dtd =
        r#"<!DOCTYPE config [<!ENTITY x "boom">]><config><moduleName>&x;</moduleName></config>"#;
    assert!(fomod::parse(dtd.as_bytes()).is_err());
}

fn ecosystem_json() -> serde_json::Value {
    json!({
        "games": {
            "lethal-company": {
                "r2modman": [{
                    "meta": { "displayName": "Lethal Company" },
                    "distributions": [{ "platform": "steam", "identifier": "1966720" }],
                    "exeNames": ["Lethal Company.exe"],
                    "additionalSearchStrings": ["lc"],
                    "packageLoader": "bepinex",
                    "installRules": [
                        { "route": "BepInEx/plugins", "defaultFileExtensions": [".dll"], "trackingMethod": "subdir", "isDefaultLocation": true },
                        { "route": "BepInEx/monomod", "defaultFileExtensions": [".mm.dll"], "trackingMethod": "subdir", "isDefaultLocation": false },
                        { "route": "BepInEx/config", "defaultFileExtensions": [], "trackingMethod": "none", "isDefaultLocation": false }
                    ]
                }]
            },
            "riskofrain2": {
                "r2modman": [{
                    "meta": { "displayName": "Risk of Rain 2" },
                    "distributions": [{ "platform": "epic-games-store", "identifier": "4b3dcc5723454a47a9112d8fe8fd5f5c" }],
                    "exeNames": ["Risk of Rain 2.exe"],
                    "packageLoader": "bepinex",
                    "installRules": [{ "route": "BepInEx/plugins", "defaultFileExtensions": [".dll"], "trackingMethod": "subdir", "isDefaultLocation": true }]
                }]
            },
            "northstar-game": {
                "r2modman": [{ "meta": { "displayName": "Titanfall 2" }, "packageLoader": "northstar", "installRules": [] }]
            },
            "no-r2": { "meta": {} }
        },
        "modloaderPackages": [
            { "packageId": "BepInEx-BepInExPack", "rootFolder": "BepInExPack", "loader": "bepinex" }
        ]
    })
}

#[test]
fn thunderstore_ecosystem_finds_games() {
    let eco = thunderstore::parse_ecosystem(&ecosystem_json());
    assert_eq!(eco.communities.len(), 3);
    let lc = eco.community("lethal-company").unwrap();
    assert!(lc.supported());
    assert!(!eco.community("northstar-game").unwrap().supported());
    let by_steam = GameKeys {
        steam_id: Some("1966720"),
        title: "something else",
        ..Default::default()
    };
    assert_eq!(eco.find(&by_steam).unwrap().id, "lethal-company");
    let by_epic = GameKeys {
        epic_ids: vec!["4B3DCC5723454A47A9112D8FE8FD5F5C"],
        title: "x",
        ..Default::default()
    };
    assert_eq!(eco.find(&by_epic).unwrap().id, "riskofrain2");
    let by_title = GameKeys {
        title: "Risk of Rain™ 2",
        ..Default::default()
    };
    assert_eq!(eco.find(&by_title).unwrap().id, "riskofrain2");
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "lethal company.EXE", "");
    let by_exe = GameKeys {
        title: "My copy",
        game_dir: Some(tmp.path()),
        ..Default::default()
    };
    assert_eq!(eco.find(&by_exe).unwrap().id, "lethal-company");
    assert!(
        eco.find(&GameKeys {
            title: "Hades",
            ..Default::default()
        })
        .is_none()
    );
}

#[test]
fn thunderstore_packages_follow_install_rules() {
    let eco = thunderstore::parse_ecosystem(&ecosystem_json());
    let lc = eco.community("lethal-company").unwrap();
    let r = |rel: &str| thunderstore::route(lc, None, "Team-Mod", rel);
    assert_eq!(
        r("Mod.dll").as_deref(),
        Some("BepInEx/plugins/Team-Mod/Mod.dll")
    );
    assert_eq!(
        r("Hook.mm.dll").as_deref(),
        Some("BepInEx/monomod/Team-Mod/Hook.mm.dll")
    );
    assert_eq!(
        r("plugins/sub/a.dll").as_deref(),
        Some("BepInEx/plugins/Team-Mod/sub/a.dll")
    );
    assert_eq!(
        r("BepInEx/config/x.cfg").as_deref(),
        Some("BepInEx/config/x.cfg")
    );
    assert_eq!(
        r("assets/x.bundle").as_deref(),
        Some("BepInEx/plugins/Team-Mod/assets/x.bundle")
    );
    assert_eq!(r("manifest.json"), None);
    assert_eq!(r("README.md"), None);
    let loader = eco.loader_package(lc, "bepinex-bepinexpack").unwrap();
    let l = |rel: &str| thunderstore::route(lc, Some(loader), "BepInEx-BepInExPack", rel);
    assert_eq!(l("BepInExPack/winhttp.dll").as_deref(), Some("winhttp.dll"));
    assert_eq!(
        l("BepInExPack/BepInEx/core/BepInEx.dll").as_deref(),
        Some("BepInEx/core/BepInEx.dll")
    );
    assert_eq!(l("icon.png"), None);
}

#[test]
fn thunderstore_answers_are_parsed() {
    assert_eq!(
        thunderstore::split_dependency("BepInEx-BepInExPack-5.4.2100"),
        Some(("BepInEx".into(), "BepInExPack".into()))
    );
    assert_eq!(
        thunderstore::split_dependency("Team-Mod_With-Dash-1.0.0"),
        Some(("Team".into(), "Mod_With-Dash".into()))
    );
    assert_eq!(thunderstore::split_dependency("nonsense"), None);
    let page = thunderstore::parse_search(&json!({
        "count": 2, "next": "https://thunderstore.io/…page=2",
        "results": [
            { "namespace": "a", "name": "More_Company", "download_count": 5, "description": "d" },
            { "namespace": "b", "name": "Nsfw", "is_nsfw": true }
        ]
    }));
    assert_eq!(page.total, 2);
    assert!(page.has_more);
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].display_name(), "More Company");
    let foreign = json!({ "namespace": "a", "name": "b", "latest": { "download_url": "https://evil.example/x.zip" } });
    assert!(thunderstore::parse_package(&foreign).is_none());
}

#[tokio::test]
async fn thunderstore_resolves_dependencies_first() {
    let server = TestServer::start(|req| {
        let pkg = |ns: &str, name: &str, deps: &[&str]| {
            Resp::json(json!({
                "namespace": ns, "name": name,
                "latest": {
                    "version_number": "1.0.0",
                    "dependencies": deps,
                    "download_url": format!("https://thunderstore.io/package/download/{ns}/{name}/1.0.0/"),
                }
            }))
        };
        match req.path.as_str() {
            "/api/experimental/package/T/Top/" => {
                pkg("T", "Top", &["L-Lib-1.0.0", "BepInEx-BepInExPack-5.4.0"])
            }
            "/api/experimental/package/L/Lib/" => {
                pkg("L", "Lib", &["BepInEx-BepInExPack-5.4.0", "T-Top-1.0.0"])
            }
            "/api/experimental/package/BepInEx/BepInExPack/" => pkg("BepInEx", "BepInExPack", &[]),
            _ => Resp::status(404),
        }
    });
    let base = server.base();
    let order = thunderstore::resolve(&base, "T", "Top", &HashSet::new())
        .await
        .unwrap();
    let names: Vec<String> = order.iter().map(|p| p.full_name()).collect();
    assert_eq!(names, ["BepInEx-BepInExPack", "L-Lib", "T-Top"]);
    let installed: HashSet<String> = ["bepinex-bepinexpack".to_string()].into();
    let order = thunderstore::resolve(&base, "T", "Top", &installed)
        .await
        .unwrap();
    assert_eq!(order.len(), 2);
    assert!(
        thunderstore::resolve(&base, "No", "Such", &HashSet::new())
            .await
            .is_err()
    );
}

#[test]
fn nexus_links_parse_and_hide_their_key() {
    let link = nexus::NxmLink::parse(
        "nxm://SkyrimSpecialEdition/mods/266/files/1234?key=SECRETKEY&expires=1700000000&user_id=1",
    )
    .unwrap();
    assert_eq!(link.domain, "skyrimspecialedition");
    assert_eq!((link.mod_id, link.file_id), (266, 1234));
    assert_eq!(link.expires, Some(1_700_000_000));
    assert!(!format!("{link:?}").contains("SECRETKEY"));
    assert!(!format!("{:?}", nexus::ApiKey("SECRET".into())).contains("SECRET"));
    assert!(nexus::NxmLink::parse("https://x/mods/1/files/2").is_none());
    assert!(nexus::NxmLink::parse("nxm://game/mods/x/files/2").is_none());
    assert!(nexus::NxmLink::parse("nxm://ga_me/mods/1/files/2").is_none());
    assert_eq!(
        nexus::parse_mod_url("https://www.nexusmods.com/stardewvalley/mods/2400?tab=files"),
        Some(("stardewvalley".into(), 2400))
    );
    assert_eq!(
        nexus::parse_mod_url("https://evil.com/stardewvalley/mods/1"),
        None
    );
}

#[tokio::test]
async fn nexus_client_sends_the_key_and_explains_refusals() {
    let server = TestServer::start(|req| {
        if req.header("apikey") != Some("k3y") {
            return Resp::status(401);
        }
        match req.path.as_str() {
            "/v1/users/validate.json" => Resp::json(json!({ "name": "Ana", "is_premium": false })),
            "/v1/games/g/mods/1/files.json" => Resp::json(json!({ "files": [
                { "file_id": 1, "name": "Old", "category_name": "OLD_VERSION", "uploaded_timestamp": 9 },
                { "file_id": 2, "name": "Main", "category_name": "MAIN", "size_in_bytes": 10, "uploaded_timestamp": 5 },
                { "file_id": 3, "name": "Newer", "category_name": "OPTIONAL", "uploaded_timestamp": 7 }
            ]})),
            p if p.starts_with("/v1/games/g/mods/1/files/2/download_link.json?key=abc") => {
                Resp::json(json!([{ "URI": "https://cdn.example/f.zip" }]))
            }
            p if p.starts_with("/v1/games/g/mods/1/files/2/download_link.json") => {
                Resp::status(403)
            }
            _ => Resp::status(404),
        }
    });
    let client = nexus::Client::with_base(&server.base(), nexus::ApiKey("k3y".into()));
    let user = client.validate().await.unwrap();
    assert_eq!(user.name, "Ana");
    assert!(!user.is_premium);
    let files = client.files("g", 1).await.unwrap();
    let ids: Vec<u64> = files.iter().map(|f| f.file_id).collect();
    assert_eq!(ids, [3, 2, 1]);
    let mut link = nexus::NxmLink::parse("nxm://g/mods/1/files/2").unwrap();
    let err = client.download_url(&link).await.unwrap_err();
    assert!(format!("{err:#}").contains("Premium"));
    link.key = Some("abc".into());
    link.expires = Some(1);
    assert_eq!(
        client.download_url(&link).await.unwrap(),
        "https://cdn.example/f.zip"
    );
    let bad = nexus::Client::with_base(&server.base(), nexus::ApiKey("nope".into()));
    assert!(format!("{:#}", bad.validate().await.unwrap_err()).contains("API key"));
}

#[test]
fn overlay_options_and_mount_table() {
    let layers = vec![
        Layer {
            id: "a".into(),
            dir: "/m/a/files".into(),
        },
        Layer {
            id: "b".into(),
            dir: "/m/b/files".into(),
        },
    ];
    assert_eq!(
        overlay::mount_options(
            Path::new("/g"),
            &layers,
            Path::new("/m/.written"),
            Path::new("/m/.work")
        ),
        "lowerdir=/m/b/files:/m/a/files:/g,upperdir=/m/.written,workdir=/m/.work"
    );
    assert!(!overlay::paths_supported(&[Path::new("/games/a:b")]));
    assert!(overlay::paths_supported(&[Path::new("/games/My Game")]));
    let info = "36 35 0:31 / /games/My\\040Game rw,nosuid - fuse.fuse-overlayfs fuse-overlayfs rw\n\
                37 35 0:32 / /other rw - ext4 /dev/sda1 rw\n";
    assert!(overlay::mounted_in(info, Path::new("/games/My Game")));
    assert!(!overlay::mounted_in(info, Path::new("/other")));
}

#[test]
fn overlay_settles_game_writes_into_the_game_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let game = tmp.path().join("game");
    write(&game, "saves/old.sav", "old");
    write(&game, "settings.ini", "default");
    let upper = tmp.path().join("upper");
    write(&upper, "saves/new.sav", "new");
    write(&upper, "settings.ini", "changed");
    write(&upper, "BepInEx/config/mod.cfg", "mod config");
    write(&upper, "BepInEx/LogOutput.log", "log");
    write(&upper, "saves/.wh.old.sav", "");
    let mod_files: HashSet<String> = ["bepinex/plugins/m/m.dll".to_string()].into();
    let moved = overlay::settle(&game, &upper, &mod_files).unwrap();
    assert_eq!(moved, 2);
    assert_eq!(
        std::fs::read_to_string(game.join("saves/new.sav")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(game.join("settings.ini")).unwrap(),
        "changed"
    );
    // Writes inside the mods' own folders stay with the mods.
    assert!(upper.join("BepInEx/config/mod.cfg").exists());
    assert!(upper.join("BepInEx/LogOutput.log").exists());
    // The game deleting its file does not hide it next time.
    assert!(!upper.join("saves/.wh.old.sav").exists());
    assert!(game.join("saves/old.sav").exists());
}

/// Mounts a real overlay; needs `fuse-overlayfs` and `/dev/fuse`.
#[tokio::test]
#[ignore]
async fn overlay_mounts_over_the_game_folder() {
    let Some(tool) = overlay::tool() else {
        panic!("fuse-overlayfs is not installed");
    };
    let f = fixture();
    let before = snapshot(&f.game);
    let layers = f.lib.layers(&f.list).unwrap();
    let lower: Vec<Layer> = layers[..layers.len() - 1].to_vec();
    overlay::mount(
        &tool,
        &f.game,
        &lower,
        &f.lib.written_dir(),
        &f.lib.work_dir(),
    )
    .await
    .unwrap();
    assert!(overlay::is_mounted(&f.game));
    assert_eq!(
        std::fs::read_to_string(f.game.join("readme.txt")).unwrap(),
        "b readme"
    );
    std::fs::write(f.game.join("save.dat"), "save").unwrap();
    overlay::unmount(&f.game).await.unwrap();
    assert!(!overlay::is_mounted(&f.game));
    let mut expected = before.clone();
    assert_eq!(snapshot(&f.game), expected);
    overlay::settle(&f.game, &f.lib.written_dir(), &HashSet::new()).unwrap();
    expected.insert("save.dat".into(), "save".into());
    assert_eq!(snapshot(&f.game), expected);
}
