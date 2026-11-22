//! Tests for [`super`]: types, release catalog parsing, ETag/TTL caching,
//! mandatory SHA-256 verification, streaming `.7z` and `.zip` extraction with
//! `archive_guard`, surgical INI patching, and end-to-end journaled installs.

use std::io::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};

use irori_depot::test_server::{Resp, TestServer};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::pe::tests::build_pe;

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn x64_pe(imports: &[&str]) -> Vec<u8> {
    build_pe(0x8664, imports, &[])
}

fn x86_pe(imports: &[&str]) -> Vec<u8> {
    build_pe(0x014c, imports, &[])
}

fn no_progress() -> ProgressSink {
    Arc::new(|_| {})
}

fn no_steps() -> Arc<dyn Fn(Step) + Send + Sync> {
    Arc::new(|_| {})
}

/// Standard IEEE CRC-32 used by the 7-Zip header format.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn push_7z_u64(buf: &mut Vec<u8>, v: u64) {
    if v < 0x80 {
        buf.push(v as u8);
    } else {
        buf.push(0xFF);
        buf.extend_from_slice(&v.to_le_bytes());
    }
}

/// Build a valid uncompressed (Copy coder) `.7z` archive in pure Rust for tests.
/// Every entry must have non-empty data.
fn build_7z(files: &[(&str, &[u8])]) -> Vec<u8> {
    assert!(!files.is_empty());
    let mut packed = Vec::new();
    for (_, data) in files {
        assert!(
            !data.is_empty(),
            "build_7z test helper expects non-empty files"
        );
        packed.extend_from_slice(data);
    }
    let total_packed = packed.len() as u64;

    let mut header = Vec::new();
    header.push(0x01); // kHeader
    header.push(0x04); // kMainStreamsInfo

    // kPackInfo
    header.push(0x06);
    push_7z_u64(&mut header, 0); // pack_pos
    push_7z_u64(&mut header, 1); // num_pack_streams
    header.push(0x09); // kSize
    push_7z_u64(&mut header, total_packed);
    header.push(0x00); // kEnd

    // kUnpackInfo
    header.push(0x07);
    header.push(0x0B); // kFolder
    push_7z_u64(&mut header, 1); // num_folders
    header.push(0x00); // external = false
    push_7z_u64(&mut header, 1); // num_coders
    header.push(0x01); // coder flags: id size = 1, simple coder, no props
    header.push(0x00); // Copy codec id
    header.push(0x0C); // kCodersUnpackSize
    push_7z_u64(&mut header, total_packed);
    header.push(0x00); // kEnd

    // kSubStreamsInfo
    header.push(0x08);
    header.push(0x0D); // kNumUnpackStream
    push_7z_u64(&mut header, files.len() as u64);
    if files.len() > 1 {
        header.push(0x09); // kSize
        for (_, data) in &files[..files.len() - 1] {
            push_7z_u64(&mut header, data.len() as u64);
        }
    }
    header.push(0x0A); // kCRC
    header.push(0x01); // all_are_defined = true
    for (_, data) in files {
        header.extend_from_slice(&crc32(data).to_le_bytes());
    }
    header.push(0x00); // kEnd (SubStreamsInfo)
    header.push(0x00); // kEnd (MainStreamsInfo)

    // kFilesInfo
    header.push(0x05);
    push_7z_u64(&mut header, files.len() as u64);
    let mut names = Vec::new();
    names.push(0x00); // external = false
    for (name, _) in files {
        for unit in name.encode_utf16() {
            names.extend_from_slice(&unit.to_le_bytes());
        }
        names.extend_from_slice(&[0x00, 0x00]);
    }
    header.push(0x11); // kName
    push_7z_u64(&mut header, names.len() as u64);
    header.extend_from_slice(&names);
    header.push(0x00); // kEnd (FilesInfo)
    header.push(0x00); // kEnd (Header)

    let mut start_header = Vec::with_capacity(20);
    start_header.extend_from_slice(&total_packed.to_le_bytes());
    start_header.extend_from_slice(&(header.len() as u64).to_le_bytes());
    start_header.extend_from_slice(&crc32(&header).to_le_bytes());

    let mut out = Vec::with_capacity(32 + packed.len() + header.len());
    out.extend_from_slice(b"7z\xBC\xAF\x27\x1C");
    out.extend_from_slice(&[0x00, 0x04]); // version 0.4
    out.extend_from_slice(&crc32(&start_header).to_le_bytes());
    out.extend_from_slice(&start_header);
    out.extend_from_slice(&packed);
    out.extend_from_slice(&header);
    out
}

fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn github_release_json(tag: &str, assets: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "published_at": "2026-11-20T12:00:00Z",
        "assets": assets,
    })
    .to_string()
}

fn github_asset(name: &str, url: &str, digest: Option<&str>, size: u64) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "size": size,
        "digest": digest,
        "updated_at": "2026-11-21T08:30:00Z",
        "browser_download_url": url,
    })
}

#[test]
fn proxy_dll_names_overrides_and_serde_round_trip() {
    let expected = [
        (ProxyDll::Dxgi, "dxgi.dll", Some("dxgi")),
        (ProxyDll::Version, "version.dll", Some("version")),
        (ProxyDll::Dbghelp, "dbghelp.dll", Some("dbghelp")),
        (ProxyDll::D3d12, "d3d12.dll", Some("d3d12")),
        (ProxyDll::Wininet, "wininet.dll", Some("wininet")),
        (ProxyDll::Winhttp, "winhttp.dll", Some("winhttp")),
        (ProxyDll::Winmm, "winmm.dll", Some("winmm")),
        (ProxyDll::OptiScalerAsi, "OptiScaler.asi", None),
    ];
    assert_eq!(ProxyDll::ALL.len(), expected.len());
    for (variant, name, stem) in expected {
        assert_eq!(variant.file_name(), name);
        assert_eq!(variant.override_stem(), stem);
        assert_eq!(ProxyDll::from_file_name(name), Some(variant));
        assert_eq!(
            ProxyDll::from_file_name(&name.to_ascii_uppercase()),
            Some(variant)
        );
    }

    let cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Nightly,
        gpu_driver: GpuDriverMode::Nvidia,
        proxy_dll: ProxyDll::OptiScalerAsi,
        enable_optipatcher: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    assert_eq!(
        serde_json::from_str::<OptiscalerConfig>(&json).unwrap(),
        cfg
    );
}

#[test]
fn compares_stable_and_nightly_release_versions() {
    assert!(is_newer_release(
        OptiscalerChannel::Stable,
        "v0.7.7",
        "v0.7.8"
    ));
    assert!(is_newer_release(
        OptiscalerChannel::Stable,
        "0.7.7-pre9",
        "0.7.7"
    ));
    assert!(!is_newer_release(
        OptiscalerChannel::Stable,
        "v0.7.7",
        "0.7.7"
    ));
    assert!(!is_newer_release(
        OptiscalerChannel::Stable,
        "v0.7.8",
        "v0.7.7"
    ));

    assert!(is_newer_release(
        OptiscalerChannel::Nightly,
        "nightly-2026-11-20",
        "nightly-2026-11-22"
    ));
    assert!(!is_newer_release(
        OptiscalerChannel::Nightly,
        "nightly-2026-11-22",
        "nightly-2026-11-22"
    ));
}

#[test]
fn parses_official_releases_and_requires_sha256_and_allowed_host() {
    let ep = OptiscalerEndpoints::official();
    let digest = format!("sha256:{}", "a".repeat(64));

    // Stable matches case-insensitive optiscaler_*.7z and skips other assets.
    let stable_body = github_release_json(
        "v0.7.7",
        vec![
            github_asset(
                "SourceCode.7z",
                "https://github.com/optiscaler/OptiScaler/releases/download/v0.7.7/SourceCode.7z",
                Some(&digest),
                100,
            ),
            github_asset(
                "OPTISCALER_0.7.7.7Z",
                "https://github.com/optiscaler/OptiScaler/releases/download/v0.7.7/OPTISCALER_0.7.7.7Z",
                Some(&digest),
                5000,
            ),
        ],
    );
    let stable = parse_channel_release(OptiscalerChannel::Stable, &stable_body, &ep).unwrap();
    assert_eq!(stable.version, "v0.7.7");
    assert_eq!(stable.asset_name, "OPTISCALER_0.7.7.7Z");
    assert_eq!(stable.sha256, "a".repeat(64));
    assert_eq!(stable.published_at, "2026-11-21");

    // Missing SHA-256 digest must be refused.
    let no_digest = github_release_json(
        "v0.7.7",
        vec![github_asset(
            "OptiScaler_0.7.7.7z",
            "https://github.com/optiscaler/OptiScaler/releases/download/v0.7.7/OptiScaler_0.7.7.7z",
            None,
            5000,
        )],
    );
    assert!(parse_channel_release(OptiscalerChannel::Stable, &no_digest, &ep).is_err());

    // Untrusted download host must be refused.
    let evil_host = github_release_json(
        "v0.7.7",
        vec![github_asset(
            "OptiScaler_0.7.7.7z",
            "https://evil.example.com/OptiScaler_0.7.7.7z",
            Some(&digest),
            5000,
        )],
    );
    assert!(parse_channel_release(OptiscalerChannel::Stable, &evil_host, &ep).is_err());

    // Nightly supports JSON array of releases (skipping drafts).
    let nightly_body = serde_json::json!([
        {
            "tag_name": "draft-1",
            "draft": true,
            "assets": [github_asset(
                "OptiScaler_draft.7z",
                "https://github.com/optiscaler/OptiScaler-nightly/releases/download/draft-1/OptiScaler_draft.7z",
                Some(&digest),
                100
            )]
        },
        {
            "tag_name": "nightly-20261122",
            "draft": false,
            "prerelease": true,
            "assets": [github_asset(
                "OptiScaler_0.7.8_pre.7z",
                "https://github.com/optiscaler/OptiScaler-nightly/releases/download/nightly-20261122/OptiScaler_0.7.8_pre.7z",
                Some(&digest),
                2048
            )]
        }
    ])
    .to_string();
    let nightly = parse_channel_release(OptiscalerChannel::Nightly, &nightly_body, &ep).unwrap();
    assert_eq!(nightly.version, "nightly-20261122");
    assert_eq!(nightly.asset_name, "OptiScaler_0.7.8_pre.7z");

    // FakeNVAPI accepts .zip or .7z.
    let fake_body = github_release_json(
        "v1.3.0",
        vec![github_asset(
            "fakenvapi-v1.3.0.zip",
            "https://github.com/optiscaler/fakenvapi/releases/download/v1.3.0/fakenvapi-v1.3.0.zip",
            Some(&digest),
            1024,
        )],
    );
    let fake = parse_fakenvapi_release(&fake_body, &ep).unwrap();
    assert_eq!(fake.asset_name, "fakenvapi-v1.3.0.zip");

    // OptiPatcher accepts standalone .asi.
    let patcher_body = github_release_json(
        "v0.2.0",
        vec![github_asset(
            "OptiPatcher.asi",
            "https://github.com/optiscaler/OptiPatcher/releases/download/v0.2.0/OptiPatcher.asi",
            Some(&digest),
            4096,
        )],
    );
    let patcher = parse_optipatcher_release(&patcher_body, &ep).unwrap();
    assert_eq!(patcher.asset_name, "OptiPatcher.asi");
}

#[test]
fn surgical_ini_patcher_preserves_comments_formatting_and_unknown_keys() {
    let original = concat!(
        "\u{feff}; Top header comment\r\n",
        "# Another comment\r\n",
        "[Upscalers]\r\n",
        "Dx12Upscaler = auto\r\n",
        "\r\n",
        "[Plugins]\r\n",
        "; Enable ASI plugin loading\r\n",
        "; LoadAsiPlugins=false\r\n",
        "  LoadAsiPlugins = auto ; keep this inline comment\r\n",
        "Path = plugins\r\n",
        "\r\n",
        "[FakeNVAPI]\r\n",
        "; Comment about fakenvapi\r\n",
        "UseFakenvapi=auto\r\n",
        "CustomSetting = 42\r\n",
    );

    let cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Stable,
        gpu_driver: GpuDriverMode::MesaAuto,
        proxy_dll: ProxyDll::Dxgi,
        enable_optipatcher: true,
        ..Default::default()
    };
    let patched = patch_optiscaler_ini(original, &cfg);
    assert!(patched.starts_with("\u{feff}; Top header comment\r\n"));
    assert!(patched.contains("; LoadAsiPlugins=false\r\n"));
    assert!(patched.contains("  LoadAsiPlugins = true ; keep this inline comment\r\n"));
    assert!(patched.contains("Path = plugins\r\n"));
    assert!(patched.contains("UseFakenvapi=true\r\n"));
    assert!(patched.contains("CustomSetting = 42\r\n"));
    assert!(patched.contains("Dx12Upscaler = auto\r\n"));

    // Nvidia + OptiPatcher disabled flips both values cleanly.
    let cfg_nv = OptiscalerConfig {
        channel: OptiscalerChannel::Stable,
        gpu_driver: GpuDriverMode::Nvidia,
        proxy_dll: ProxyDll::Winmm,
        enable_optipatcher: false,
        ..Default::default()
    };
    let patched_nv = patch_optiscaler_ini(&patched, &cfg_nv);
    assert!(patched_nv.contains("  LoadAsiPlugins = false ; keep this inline comment\r\n"));
    assert!(patched_nv.contains("UseFakenvapi=false\r\n"));
}

#[test]
fn surgical_ini_patcher_inserts_missing_keys_and_appends_missing_sections() {
    let input = "[Plugins]\nPath=plugins\n\n[Other]\nKey=1\n";
    let cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Stable,
        gpu_driver: GpuDriverMode::MesaAuto,
        proxy_dll: ProxyDll::Dxgi,
        enable_optipatcher: true,
        ..Default::default()
    };
    let out = patch_optiscaler_ini(input, &cfg);
    assert_eq!(
        out,
        "[Plugins]\nPath=plugins\nLoadAsiPlugins=true\n\n[Other]\nKey=1\n\n[fakenvapi]\nUseFakenvapi=true\n"
    );

    // Patching fakenvapi.ini with no overrides returns the file unchanged.
    let fake_in = "; fakenvapi\n[fakenvapi]\nenable_logs=0\n";
    assert_eq!(patch_fakenvapi_ini(fake_in, &[]), fake_in);
    assert_eq!(
        patch_fakenvapi_ini(fake_in, &[("fakenvapi", "enable_logs", "1")]),
        "; fakenvapi\n[fakenvapi]\nenable_logs=1\n"
    );
}

#[test]
fn extract_7z_and_zip_guard_against_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();

    // Valid 7z extraction.
    let good_7z = dir.path().join("good.7z");
    std::fs::write(
        &good_7z,
        build_7z(&[
            ("OptiScaler.dll", b"hello-dll"),
            ("D3D12_Optiscaler/D3D12Core.dll", b"core-dll"),
        ]),
    )
    .unwrap();
    let out_good = dir.path().join("out_good");
    extract_7z(&good_7z, &out_good, &cancel).unwrap();
    assert_eq!(
        std::fs::read(out_good.join("OptiScaler.dll")).unwrap(),
        b"hello-dll"
    );
    assert_eq!(
        std::fs::read(out_good.join("D3D12_Optiscaler/D3D12Core.dll")).unwrap(),
        b"core-dll"
    );

    // Path traversal inside 7z must fail and never touch parent directory.
    let evil_7z = dir.path().join("evil.7z");
    std::fs::write(&evil_7z, build_7z(&[("../escape.txt", b"pwned")])).unwrap();
    let out_evil = dir.path().join("out_evil");
    assert!(extract_7z(&evil_7z, &out_evil, &cancel).is_err());
    assert!(!dir.path().join("escape.txt").exists());

    // Windows backslash traversal inside 7z must fail.
    let evil_win_7z = dir.path().join("evil_win.7z");
    std::fs::write(&evil_win_7z, build_7z(&[("..\\escape2.txt", b"pwned")])).unwrap();
    let out_evil_win = dir.path().join("out_evil_win");
    assert!(extract_7z(&evil_win_7z, &out_evil_win, &cancel).is_err());
    assert!(!dir.path().join("escape2.txt").exists());

    // Absolute path and Windows drive prefix inside 7z must fail.
    let abs_7z = dir.path().join("abs.7z");
    std::fs::write(&abs_7z, build_7z(&[("/etc/passwd", b"pwned")])).unwrap();
    assert!(extract_7z(&abs_7z, &dir.path().join("out_abs"), &cancel).is_err());

    let drive_7z = dir.path().join("drive.7z");
    std::fs::write(&drive_7z, build_7z(&[("C:\\evil.dll", b"pwned")])).unwrap();
    assert!(extract_7z(&drive_7z, &dir.path().join("out_drive"), &cancel).is_err());

    // Path traversal inside zip must fail.
    let evil_zip = dir.path().join("evil.zip");
    std::fs::write(&evil_zip, build_zip(&[("../escape_zip.txt", b"pwned")])).unwrap();
    let out_zip = dir.path().join("out_zip");
    assert!(extract_zip(&evil_zip, &out_zip, &cancel).is_err());
    assert!(!dir.path().join("escape_zip.txt").exists());
}

#[tokio::test]
async fn fetch_catalog_loads_all_four_official_components() {
    let mock = MockOptiscalerServer::start(false);
    let cache = tempfile::tempdir().unwrap();
    let catalog = fetch_catalog(&mock.endpoints(), cache.path(), false)
        .await
        .unwrap();
    assert_eq!(
        catalog
            .release_for(OptiscalerChannel::Stable)
            .unwrap()
            .version,
        "v0.7.7"
    );
    assert_eq!(
        catalog
            .release_for(OptiscalerChannel::Nightly)
            .unwrap()
            .version,
        "nightly-2026-11-22"
    );
    assert_eq!(catalog.fakenvapi.as_ref().unwrap().version, "v1.3.0");
    assert_eq!(catalog.optipatcher.as_ref().unwrap().version, "v0.2.0");
}

#[tokio::test]
async fn catalog_respects_ttl_and_uses_etag_conditional_requests() {
    let hits = Arc::new(AtomicUsize::new(0));
    let conditional_hits = Arc::new(AtomicUsize::new(0));
    let hits_in = hits.clone();
    let cond_in = conditional_hits.clone();
    let base = Arc::new(std::sync::Mutex::new(String::new()));
    let base_in = base.clone();

    let digest = format!("sha256:{}", "c".repeat(64));
    let server = TestServer::start(move |req| {
        hits_in.fetch_add(1, Ordering::SeqCst);
        if req.header("if-none-match") == Some("\"etag-v1\"") {
            cond_in.fetch_add(1, Ordering::SeqCst);
            return Resp {
                status: 304,
                headers: vec![("ETag".into(), "\"etag-v1\"".into())],
                ..Default::default()
            };
        }
        let dl = format!("{}/dl/OptiScaler_0.7.7.7z", base_in.lock().unwrap());
        let body = github_release_json(
            "v0.7.7",
            vec![github_asset(
                "OptiScaler_0.7.7.7z",
                &dl,
                Some(&digest),
                1234,
            )],
        );
        Resp {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("ETag".into(), "\"etag-v1\"".into()),
            ],
            body: body.into_bytes(),
            ..Default::default()
        }
    });
    *base.lock().unwrap() = server.base();

    let ep = OptiscalerEndpoints {
        stable: server.url("/stable"),
        nightly: server.url("/nightly"),
        fakenvapi: server.url("/fakenvapi"),
        optipatcher: server.url("/optipatcher"),
        allowed_download_prefixes: vec![server.base()],
    };
    let cache = tempfile::tempdir().unwrap();

    // First fetch hits server and caches response + ETag.
    let r1 = fetch_channel_release(OptiscalerChannel::Stable, &ep, cache.path(), false)
        .await
        .unwrap();
    assert_eq!(r1.version, "v0.7.7");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Second fetch within 15-minute TTL does zero network requests.
    let r2 = fetch_channel_release(OptiscalerChannel::Stable, &ep, cache.path(), false)
        .await
        .unwrap();
    assert_eq!(r2, r1);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Forced revalidation sends If-None-Match and handles 304 Not Modified.
    let r3 = fetch_channel_release(OptiscalerChannel::Stable, &ep, cache.path(), true)
        .await
        .unwrap();
    assert_eq!(r3, r1);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(conditional_hits.load(Ordering::SeqCst), 1);

    // Offline fallback uses the cached response when the server is unreachable.
    let offline = OptiscalerEndpoints {
        stable: "http://127.0.0.1:9/stable".into(),
        nightly: "http://127.0.0.1:9/nightly".into(),
        fakenvapi: "http://127.0.0.1:9/fakenvapi".into(),
        optipatcher: "http://127.0.0.1:9/optipatcher".into(),
        allowed_download_prefixes: vec![server.base()],
    };
    let r4 = fetch_channel_release(OptiscalerChannel::Stable, &offline, cache.path(), true)
        .await
        .unwrap();
    assert_eq!(r4, r1);
}

struct MockOptiscalerServer {
    server: TestServer,
    opti_bytes: Vec<u8>,
    fake_bytes: Vec<u8>,
    patcher_bytes: Vec<u8>,
}

impl MockOptiscalerServer {
    fn start(tamper_optiscaler: bool) -> Self {
        let dll = x64_pe(&["kernel32.dll", "dxgi.dll"]);
        let ini = b"; OptiScaler config\r\n[Plugins]\r\nLoadAsiPlugins=auto\r\n\r\n[fakenvapi]\r\nUseFakenvapi=auto\r\n";
        let extra = x64_pe(&["kernel32.dll"]);
        let opti_archive = build_7z(&[
            ("OptiScaler_0.7.7/OptiScaler.dll", &dll),
            ("OptiScaler_0.7.7/OptiScaler.ini", ini),
            ("OptiScaler_0.7.7/D3D12_Optiscaler/D3D12Core.dll", &extra),
            ("OptiScaler_0.7.7/setup_linux.sh", b"#!/bin/sh\necho skip\n"),
            ("OptiScaler_0.7.7/OptiScaler Setup.bat", b"@echo off\r\n"),
        ]);

        let nvapi_dll = x64_pe(&["kernel32.dll"]);
        let fake_ini = b"[fakenvapi]\r\nenable_logs=0\r\n";
        let fake_archive = build_zip(&[("nvapi64.dll", &nvapi_dll), ("fakenvapi.ini", fake_ini)]);

        let patcher_asi = x64_pe(&["kernel32.dll"]);

        let opti_sha = sha(&opti_archive);
        let fake_sha = sha(&fake_archive);
        let patcher_sha = sha(&patcher_asi);
        let opti_len = opti_archive.len() as u64;
        let fake_len = fake_archive.len() as u64;
        let patcher_len = patcher_asi.len() as u64;

        let served_opti = if tamper_optiscaler {
            b"corrupted-archive-bytes".to_vec()
        } else {
            opti_archive
        };
        let served_fake = fake_archive;
        let served_patcher = patcher_asi.clone();

        let base = Arc::new(std::sync::Mutex::new(String::new()));
        let base_in = base.clone();
        let server = TestServer::start(move |req| {
            let b = base_in.lock().unwrap().clone();
            match req.path.as_str() {
                "/stable" => Resp::json(serde_json::json!({
                    "tag_name": "v0.7.7",
                    "draft": false,
                    "assets": [github_asset(
                        "OptiScaler_0.7.7.7z",
                        &format!("{b}/dl/OptiScaler_0.7.7.7z"),
                        Some(&format!("sha256:{opti_sha}")),
                        opti_len,
                    )]
                })),
                "/nightly" => Resp::json(serde_json::json!([{
                    "tag_name": "nightly-2026-11-22",
                    "draft": false,
                    "prerelease": true,
                    "assets": [github_asset(
                        "OptiScaler_nightly.7z",
                        &format!("{b}/dl/OptiScaler_nightly.7z"),
                        Some(&format!("sha256:{opti_sha}")),
                        opti_len,
                    )]
                }])),
                "/fakenvapi" => Resp::json(serde_json::json!({
                    "tag_name": "v1.3.0",
                    "draft": false,
                    "assets": [github_asset(
                        "fakenvapi-v1.3.0.zip",
                        &format!("{b}/dl/fakenvapi-v1.3.0.zip"),
                        Some(&format!("sha256:{fake_sha}")),
                        fake_len,
                    )]
                })),
                "/optipatcher" => Resp::json(serde_json::json!({
                    "tag_name": "v0.2.0",
                    "draft": false,
                    "assets": [github_asset(
                        "OptiPatcher.asi",
                        &format!("{b}/dl/OptiPatcher.asi"),
                        Some(&format!("sha256:{patcher_sha}")),
                        patcher_len,
                    )]
                })),
                "/dl/OptiScaler_0.7.7.7z" | "/dl/OptiScaler_nightly.7z" => Resp {
                    status: 200,
                    body: served_opti.clone(),
                    ..Default::default()
                },
                "/dl/fakenvapi-v1.3.0.zip" => Resp {
                    status: 200,
                    body: served_fake.clone(),
                    ..Default::default()
                },
                "/dl/OptiPatcher.asi" => Resp {
                    status: 200,
                    body: served_patcher.clone(),
                    ..Default::default()
                },
                _ => Resp::status(404),
            }
        });
        *base.lock().unwrap() = server.base();
        Self {
            server,
            opti_bytes: dll,
            fake_bytes: nvapi_dll,
            patcher_bytes: patcher_asi,
        }
    }

    fn endpoints(&self) -> OptiscalerEndpoints {
        OptiscalerEndpoints {
            stable: self.server.url("/stable"),
            nightly: self.server.url("/nightly"),
            fakenvapi: self.server.url("/fakenvapi"),
            optipatcher: self.server.url("/optipatcher"),
            allowed_download_prefixes: vec![self.server.base()],
        }
    }
}

#[tokio::test]
async fn installs_mesa_auto_with_fakenvapi_and_optipatcher_and_removes_cleanly() {
    let mock = MockOptiscalerServer::start(false);
    let game = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let journal_path = state.path().join("optiscaler/legendary/Game.json");

    let exe = game.path().join("Game.exe");
    std::fs::write(&exe, x64_pe(&["d3d12.dll"])).unwrap();

    let req = OptiscalerInstallRequest {
        game_root: game.path().to_path_buf(),
        configured_exe: Some(exe.clone()),
        chosen_exe: None,
        config: OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::MesaAuto,
            proxy_dll: ProxyDll::Dxgi,
            enable_optipatcher: true,
            ..Default::default()
        },
        journal: journal_path.clone(),
        cache: cache.path().to_path_buf(),
        endpoints: mock.endpoints(),
        previous: None,
    };

    let result = install(req, no_steps(), no_progress(), CancellationToken::new())
        .await
        .unwrap();
    let mut manifest = result.manifest;
    assert_eq!(manifest.kind, GraphicsMod::Optiscaler);
    assert_eq!(manifest.version, "v0.7.7");
    assert_eq!(manifest.dll, "dxgi.dll");
    assert_eq!(manifest.overrides, vec!["dxgi", "nvapi64"]);
    assert!(result.fakenvapi.is_some());
    assert!(result.optipatcher.is_some());

    // Verify deployed binaries and patched INI.
    assert_eq!(
        std::fs::read(game.path().join("dxgi.dll")).unwrap(),
        mock.opti_bytes
    );
    assert_eq!(
        std::fs::read(game.path().join("nvapi64.dll")).unwrap(),
        mock.fake_bytes
    );
    assert_eq!(
        std::fs::read(game.path().join(OPTIPATCHER_REL_PATH)).unwrap(),
        mock.patcher_bytes
    );
    assert!(game.path().join("D3D12_Optiscaler/D3D12Core.dll").is_file());
    // Setup scripts from the archive must not be copied into the game folder.
    assert!(!game.path().join("setup_linux.sh").exists());
    assert!(!game.path().join("OptiScaler Setup.bat").exists());

    let ini = std::fs::read_to_string(game.path().join("OptiScaler.ini")).unwrap();
    assert!(ini.contains("LoadAsiPlugins=true"));
    assert!(ini.contains("UseFakenvapi=true"));

    // Toggle off and on via journal.
    journal::set_enabled(&journal_path, &mut manifest, false).unwrap();
    assert!(!game.path().join("dxgi.dll").exists());
    assert!(game.path().join("dxgi.dll.irori-disabled").exists());
    journal::set_enabled(&journal_path, &mut manifest, true).unwrap();
    assert!(game.path().join("dxgi.dll").exists());

    // User modifies OptiScaler.ini: removal must preserve it and delete untouched files.
    std::fs::write(
        game.path().join("OptiScaler.ini"),
        format!("{ini}; user tweak\r\n"),
    )
    .unwrap();
    let report = journal::remove(&journal_path, &manifest, &|_| Ok(())).unwrap();
    assert_eq!(report.kept, vec![PathBuf::from("OptiScaler.ini")]);
    assert!(game.path().join("OptiScaler.ini").exists());
    assert!(!game.path().join("dxgi.dll").exists());
    assert!(!game.path().join("nvapi64.dll").exists());
    assert!(!game.path().join("fakenvapi.ini").exists());
    assert!(!game.path().join(OPTIPATCHER_REL_PATH).exists());
    assert!(!game.path().join("OptiScaler").exists());
    assert!(!game.path().join("D3D12_Optiscaler").exists());
    assert!(!journal_path.exists());
}

#[tokio::test]
async fn installs_nvidia_mode_without_fakenvapi_and_supports_asi_proxy() {
    let mock = MockOptiscalerServer::start(false);
    let game = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let exe = game.path().join("Game.exe");
    std::fs::write(&exe, x64_pe(&["vulkan-1.dll"])).unwrap();

    // Case 1: NVIDIA + winmm.dll + no OptiPatcher.
    let journal_nv = state.path().join("optiscaler/nv.json");
    let res = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe.clone()),
            chosen_exe: None,
            config: OptiscalerConfig {
                channel: OptiscalerChannel::Nightly,
                gpu_driver: GpuDriverMode::Nvidia,
                proxy_dll: ProxyDll::Winmm,
                enable_optipatcher: false,
                ..Default::default()
            },
            journal: journal_nv.clone(),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(res.manifest.version, "nightly-2026-11-22");
    assert_eq!(res.manifest.api, Api::Vulkan);
    assert_eq!(res.manifest.dll, "winmm.dll");
    assert_eq!(res.manifest.overrides, vec!["winmm"]);
    assert!(res.fakenvapi.is_none());
    assert!(res.optipatcher.is_none());
    assert!(!game.path().join("nvapi64.dll").exists());
    assert!(!game.path().join("fakenvapi.ini").exists());
    assert!(!game.path().join(OPTIPATCHER_REL_PATH).exists());
    let ini = std::fs::read_to_string(game.path().join("OptiScaler.ini")).unwrap();
    assert!(ini.contains("LoadAsiPlugins=false"));
    assert!(ini.contains("UseFakenvapi=false"));
    journal::remove(&journal_nv, &res.manifest, &|_| Ok(())).unwrap();

    // Case 2: OptiScaler.asi + MesaAuto records only nvapi64 override, never OptiScaler.asi.
    let journal_asi = state.path().join("optiscaler/asi.json");
    let res_asi = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe),
            chosen_exe: None,
            config: OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::MesaAuto,
                proxy_dll: ProxyDll::OptiScalerAsi,
                enable_optipatcher: false,
                ..Default::default()
            },
            journal: journal_asi,
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res_asi.manifest.dll, "OptiScaler.asi");
    assert_eq!(res_asi.manifest.overrides, vec!["nvapi64"]);
    assert!(game.path().join("OptiScaler.asi").is_file());
    assert!(game.path().join("nvapi64.dll").is_file());
}

#[tokio::test]
async fn refuses_untracked_proxy_dll_tampered_downloads_and_32_bit_games() {
    let game = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let exe64 = game.path().join("Game64.exe");
    std::fs::write(&exe64, x64_pe(&["d3d11.dll"])).unwrap();

    // Untracked dxgi.dll (e.g. ReShade) must be refused with DllTaken before writing anything.
    std::fs::write(game.path().join("dxgi.dll"), b"reshade-dll").unwrap();
    let mock = MockOptiscalerServer::start(false);
    let err = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe64.clone()),
            chosen_exe: None,
            config: OptiscalerConfig::default(),
            journal: state.path().join("taken.json"),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::DllTaken { name }) if name == "dxgi.dll"
    ));
    assert_eq!(
        std::fs::read(game.path().join("dxgi.dll")).unwrap(),
        b"reshade-dll"
    );
    assert!(!game.path().join("OptiScaler.ini").exists());
    std::fs::remove_file(game.path().join("dxgi.dll")).unwrap();

    // Tampered archive fails SHA-256 verification and writes nothing.
    let tampered = MockOptiscalerServer::start(true);
    let err = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe64),
            chosen_exe: None,
            config: OptiscalerConfig::default(),
            journal: state.path().join("tampered.json"),
            cache: cache.path().to_path_buf(),
            endpoints: tampered.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("SHA-256 mismatch"), "{err:#}");
    assert!(!game.path().join("dxgi.dll").exists());

    // 32-bit game is rejected with UnsupportedArch.
    let game32 = tempfile::tempdir().unwrap();
    let exe32 = game32.path().join("OldGame.exe");
    std::fs::write(&exe32, x86_pe(&["d3d9.dll"])).unwrap();
    let err = detect_target(game32.path(), Some(&exe32), None).unwrap_err();
    assert!(matches!(
        ReshadeError::of(&err),
        Some(ReshadeError::UnsupportedArch)
    ));
}

#[tokio::test]
async fn reconfigure_renames_proxy_without_network_and_preserves_custom_ini() {
    let mock = MockOptiscalerServer::start(false);
    let game = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let journal_path = state.path().join("optiscaler/sideload/reconf.json");

    let exe = game.path().join("Game.exe");
    std::fs::write(&exe, x64_pe(&["d3d12.dll"])).unwrap();

    let res = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe),
            chosen_exe: None,
            config: OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::Nvidia,
                proxy_dll: ProxyDll::Dxgi,
                enable_optipatcher: false,
                ..Default::default()
            },
            journal: journal_path.clone(),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    // Simulate a user ShortcutKey edit and an older installed version marker.
    let ini_path = game.path().join("OptiScaler.ini");
    let custom_ini = format!(
        "{}\r\n[Menu]\r\nShortcutKey=0x2E\r\n",
        std::fs::read_to_string(&ini_path).unwrap()
    );
    std::fs::write(&ini_path, custom_ini).unwrap();

    let mut installed = res.manifest;
    installed.version = "v0.7.5".into();
    journal::save_manifest(&journal_path, &installed).unwrap();

    // Use unreachable/invalid endpoints when renaming only the proxy DLL:
    // reconfigure must make zero network requests and preserve version v0.7.5.
    let offline_endpoints = OptiscalerEndpoints {
        stable: "http://127.0.0.1:1/stable".into(),
        nightly: "http://127.0.0.1:1/nightly".into(),
        fakenvapi: "http://127.0.0.1:1/fakenvapi".into(),
        optipatcher: "http://127.0.0.1:1/optipatcher".into(),
        allowed_download_prefixes: vec!["http://127.0.0.1:".into()],
    };
    let renamed = reconfigure(
        journal_path.clone(),
        installed,
        OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::Nvidia,
            proxy_dll: ProxyDll::Version,
            enable_optipatcher: false,
            ..Default::default()
        },
        cache.path().to_path_buf(),
        offline_endpoints,
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(renamed.version, "v0.7.5");
    assert_eq!(renamed.dll, "version.dll");
    assert_eq!(renamed.overrides, vec!["version"]);
    assert!(!game.path().join("dxgi.dll").exists());
    assert!(game.path().join("version.dll").is_file());

    // Now toggle MesaAuto + OptiPatcher: fetches companions only, preserves
    // version v0.7.5 and ShortcutKey=0x2E in OptiScaler.ini.
    let updated_cfg = reconfigure(
        journal_path.clone(),
        renamed,
        OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::MesaAuto,
            proxy_dll: ProxyDll::Version,
            enable_optipatcher: true,
            ..Default::default()
        },
        cache.path().to_path_buf(),
        mock.endpoints(),
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(updated_cfg.version, "v0.7.5");
    assert!(game.path().join("nvapi64.dll").is_file());
    assert!(game.path().join(OPTIPATCHER_REL_PATH).is_file());
    let patched = std::fs::read_to_string(&ini_path).unwrap();
    assert!(patched.contains("ShortcutKey=0x2E"));
    assert!(patched.contains("UseFakenvapi=true"));
    assert!(patched.contains("LoadAsiPlugins=true"));
    assert!(patched.contains("Path=plugins"));
}

#[tokio::test]
async fn update_skips_identical_files_preserves_custom_ini_and_prunes_old_cache() {
    let mock = MockOptiscalerServer::start(false);
    let game = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let journal_path = state.path().join("optiscaler/sideload/update.json");

    let exe = game.path().join("Game.exe");
    std::fs::write(&exe, x64_pe(&["d3d12.dll"])).unwrap();

    let cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Stable,
        gpu_driver: GpuDriverMode::MesaAuto,
        proxy_dll: ProxyDll::Dxgi,
        enable_optipatcher: false,
        ..Default::default()
    };
    let first = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe.clone()),
            chosen_exe: None,
            config: cfg.clone(),
            journal: journal_path.clone(),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    // Plant a stale archive from an unused older release in cache/assets.
    let stale_cached = cache
        .path()
        .join("assets/deadbeef00112233-Optiscaler_0.7.4.7z");
    std::fs::write(&stale_cached, b"old archive").unwrap();
    assert!(stale_cached.is_file());

    // Edit OptiScaler.ini on disk and record mtime of an unchanged file.
    let ini_path = game.path().join("OptiScaler.ini");
    let custom_ini = format!(
        "{}\r\n[Menu]\r\nShortcutKey=0x2E\r\n",
        std::fs::read_to_string(&ini_path).unwrap()
    );
    std::fs::write(&ini_path, custom_ini).unwrap();
    let unchanged_file = game.path().join("D3D12_Optiscaler/D3D12Core.dll");
    let mtime_before = std::fs::metadata(&unchanged_file)
        .unwrap()
        .modified()
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));

    let second = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe.clone()),
            chosen_exe: None,
            config: cfg.clone(),
            journal: journal_path.clone(),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: Some(first.manifest),
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let mtime_after = std::fs::metadata(&unchanged_file)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime_before, mtime_after);
    assert!(
        std::fs::read_to_string(&ini_path)
            .unwrap()
            .contains("ShortcutKey=0x2E")
    );
    assert!(second.manifest.complete);
    assert!(!stale_cached.exists());

    // A second update switching to Nightly (and enabling OptiPatcher) must
    // still preserve the user's ShortcutKey=0x2E and surgically patch [Plugins].
    let nightly_cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Nightly,
        gpu_driver: GpuDriverMode::MesaAuto,
        proxy_dll: ProxyDll::Dxgi,
        enable_optipatcher: true,
        ..Default::default()
    };
    let third = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe.clone()),
            chosen_exe: None,
            config: nightly_cfg,
            journal: journal_path.clone(),
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: Some(second.manifest),
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let nightly_ini = std::fs::read_to_string(&ini_path).unwrap();
    assert!(nightly_ini.contains("ShortcutKey=0x2E"));
    assert!(nightly_ini.contains("LoadAsiPlugins=true"));

    // Removing after multiple updates must still recognize OptiScaler.ini as
    // user-modified and keep it on disk.
    let report = journal::remove(&journal_path, &third.manifest, &|_| Ok(())).unwrap();
    assert!(report.kept.contains(&PathBuf::from("OptiScaler.ini")));
    assert!(
        std::fs::read_to_string(&ini_path)
            .unwrap()
            .contains("ShortcutKey=0x2E")
    );

    // A fresh install in the same folder where OptiScaler.ini was kept must
    // preserve ShortcutKey=0x2E while patching Irori-managed keys.
    let fourth = install(
        OptiscalerInstallRequest {
            game_root: game.path().to_path_buf(),
            configured_exe: Some(exe),
            chosen_exe: None,
            config: cfg,
            journal: journal_path,
            cache: cache.path().to_path_buf(),
            endpoints: mock.endpoints(),
            previous: None,
        },
        no_steps(),
        no_progress(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(fourth.manifest.complete);
    let kept_ini = std::fs::read_to_string(&ini_path).unwrap();
    assert!(kept_ini.contains("ShortcutKey=0x2E"));
    assert!(kept_ini.contains("LoadAsiPlugins=false"));
}

#[test]
fn patch_ini_applies_shortcut_key_and_preserves_comments() {
    let opti_in = concat!(
        "[Menu]\r\n",
        "; default is Insert\r\n",
        "ShortcutKey=auto\r\n",
        "\r\n",
        "[Plugins]\r\n",
        "LoadAsiPlugins=false\r\n",
        "\r\n",
        "[fakenvapi]\r\n",
        "UseFakenvapi=true\r\n",
    );
    let cfg = OptiscalerConfig {
        channel: OptiscalerChannel::Stable,
        gpu_driver: GpuDriverMode::MesaAuto,
        proxy_dll: ProxyDll::Dxgi,
        enable_optipatcher: false,
        shortcut_key: Some("0x2e".into()),
    };
    let out = patch_optiscaler_ini(opti_in, &cfg);
    assert!(out.contains("ShortcutKey=0x2E\r\n"));
    assert!(out.contains("; default is Insert\r\n"));

    // Leaving shortcut_key as None keeps existing ShortcutKey untouched.
    let untouched = patch_optiscaler_ini(
        &out,
        &OptiscalerConfig {
            shortcut_key: None,
            ..cfg
        },
    );
    assert!(untouched.contains("ShortcutKey=0x2E\r\n"));
}

#[test]
fn sync_wine_user_reg_overrides_adds_updates_and_removes_entries_surgically() {
    let dir = tempfile::tempdir().unwrap();
    let reg = dir.path().join("user.reg");
    let initial = concat!(
        "WINE REGISTRY Version 2\n\n",
        "[Software\\\\Wine\\\\DllOverrides] 1784681205\n",
        "#time=1dd197391f92142\n",
        "\"atiadlxx\"=\"disabled\"\n",
        "\"atl100\"=\"native,builtin\"\n\n",
        "[Software\\\\Wine\\\\Fonts] 1784681205\n",
        "\"LogPixels\"=dword:00000060\n",
    );
    std::fs::write(&reg, initial).unwrap();

    // Add winmm.dll and dxgi.dll (normalizes .dll suffix and ignores .asi).
    assert!(sync_wine_user_reg_overrides(&reg, &["winmm.dll", "OptiScaler.asi"], &[]).unwrap());
    let after_add = std::fs::read_to_string(&reg).unwrap();
    assert!(after_add.contains("\"winmm\"=\"native,builtin\"\n"));
    assert!(!after_add.contains("OptiScaler.asi"));
    assert!(after_add.contains("\"atiadlxx\"=\"disabled\"\n"));
    assert!(after_add.contains("[Software\\\\Wine\\\\Fonts]"));

    // Idempotent second call performs zero disk writes.
    assert!(!sync_wine_user_reg_overrides(&reg, &["winmm"], &[]).unwrap());

    // Switch winmm -> version and remove winmm cleanly.
    assert!(sync_wine_user_reg_overrides(&reg, &["version"], &["winmm"]).unwrap());
    let after_swap = std::fs::read_to_string(&reg).unwrap();
    assert!(!after_swap.contains("\"winmm\""));
    assert!(after_swap.contains("\"version\"=\"native,builtin\"\n"));

    // Removing version leaves untouched Wine overrides intact.
    assert!(sync_wine_user_reg_overrides(&reg, &[], &["version"]).unwrap());
    let after_remove = std::fs::read_to_string(&reg).unwrap();
    assert!(!after_remove.contains("\"version\""));
    assert!(after_remove.contains("\"atl100\"=\"native,builtin\"\n"));
}
