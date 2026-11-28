use super::options::{NEEDS_AUTO_EXTRACT, NEEDS_BITTORRENT};
use super::types::{
    FieldKind::{Text, Toggle},
    FieldSection::General,
    FieldSpec,
    Label::Verbatim,
    TextKind, app,
};

pub(super) const FIELDS: &[FieldSpec] = &[
    app(
        "maxWorkers",
        "settings.field.max_workers.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number { min: 0, max: 64 }),
    )
    .help("settings.field.max_workers.help")
    .tooltip("settings.field.max_workers.tip")
    .keywords(&["workers", "threads", "parallel", "download"]),
    app(
        "maxDownloadSpeed",
        "settings.field.max_download_speed.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number {
            min: 0,
            max: 10_000_000,
        }),
    )
    .help("settings.field.max_download_speed.help")
    .tooltip("settings.field.max_download_speed.tip")
    .keywords(&["bandwidth", "speed", "limit", "throttle", "download", "kib"]),
    app(
        "downloadNoHttps",
        "settings.field.download_no_https.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.download_no_https.help")
    .tooltip("settings.field.download_no_https.tip")
    .keywords(&["https", "lancache", "cdn", "cache"]),
    app(
        "iroriWebDownloads.defaultFolder",
        "settings.field.web_default_folder.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Directory),
    )
    .help("settings.field.web_default_folder.help")
    .tooltip("settings.field.web_default_folder.tip")
    .keywords(&[
        "download",
        "folder",
        "directory",
        "links",
        "torrent",
        "path",
    ]),
    app(
        "iroriWebDownloads.maxConcurrent",
        "settings.field.web_max_concurrent.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number { min: 1, max: 5 }),
    )
    .help("settings.field.web_max_concurrent.help")
    .tooltip("settings.field.web_max_concurrent.tip")
    .keywords(&[
        "concurrent",
        "simultaneous",
        "parallel",
        "links",
        "download",
    ]),
    app(
        "iroriWebDownloads.autoExtract",
        "settings.field.web_auto_extract.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .help("settings.field.web_auto_extract.help")
    .tooltip("settings.field.web_auto_extract.tip")
    .keywords(&["extract", "unpack", "archive", "zip", "7z", "tar", "rar"]),
    app(
        "iroriWebDownloads.deleteArchiveAfterExtract",
        "settings.field.web_delete_archive.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .help("settings.field.web_delete_archive.help")
    .tooltip("settings.field.web_delete_archive.tip")
    .keywords(&["trash", "delete", "cleanup", "archive", "extract"])
    .requires(NEEDS_AUTO_EXTRACT),
    app(
        "iroriWebDownloads.autoAddToLibrary",
        "settings.field.web_auto_add.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .help("settings.field.web_auto_add.help")
    .tooltip("settings.field.web_auto_add.tip")
    .keywords(&["library", "sideload", "add", "portable", "game", "play"]),
    app(
        "iroriWebDownloads.bittorrent",
        "settings.field.web_bittorrent.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .help("settings.field.web_bittorrent.help")
    .tooltip("settings.field.web_bittorrent.tip")
    .keywords(&["bittorrent", "torrent", "magnet", "aria2", "p2p"]),
    app(
        "iroriWebDownloads.aria2Path",
        "settings.field.web_aria2_path.label",
        General,
        "settings.group.downloads",
        Text(TextKind::File),
    )
    .help("settings.field.web_aria2_path.help")
    .tooltip("settings.field.web_aria2_path.tip")
    .keywords(&["aria2", "aria2c", "binary", "path", "torrent"])
    .requires(NEEDS_BITTORRENT),
];
