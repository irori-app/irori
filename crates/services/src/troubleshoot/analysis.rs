//! Why a game didn't start, worked out from what Irori can read: the game's
//! log, its program's imports, the winetricks components in its prefix, its
//! anticheat entry and the kernel's memory-map limit. Pure functions of
//! those facts, so every rule is tested without a display or a game.

use std::collections::HashSet;

use irori_extras::anticheat::{AntiCheatInfo, AntiCheatStatus};

use super::pe::PeInfo;

/// Log lines kept as the evidence of one finding.
pub const MAX_EVIDENCE: usize = 3;
/// `vm.max_map_count` below which some engines (Source 2, Unreal 5, DayZ)
/// run out of memory maps.
pub const MAP_COUNT_WANTED: u64 = 1_048_576;
/// The value SteamOS, Fedora and Arch ship, which the explanation suggests.
pub const MAP_COUNT_RECOMMENDED: u64 = 2_147_483_642;

/// A likely cause of a failed start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaunchHint {
    MissingExecutable,
    WrongWineBuild,
    AntiCheat,
    MissingLibrary,
    MissingNativeLibrary,
    DotNet,
    Vulkan,
    BrokenPrefix,
    PermissionDenied,
    /// A program started alongside the game didn't start.
    ProgramFailed,
    /// `vm.max_map_count` is lower than some games need (Linux).
    MapCount,
    /// The game crashed and said so (an unhandled exception, a segfault).
    Crash,
}

impl LaunchHint {
    /// The hints a log line can show, in order.
    const FROM_LOG: [LaunchHint; 11] = [
        LaunchHint::MissingExecutable,
        LaunchHint::WrongWineBuild,
        LaunchHint::AntiCheat,
        LaunchHint::MissingLibrary,
        LaunchHint::MissingNativeLibrary,
        LaunchHint::DotNet,
        LaunchHint::Vulkan,
        LaunchHint::BrokenPrefix,
        LaunchHint::PermissionDenied,
        LaunchHint::ProgramFailed,
        LaunchHint::Crash,
    ];

    /// Whether `lower` (a lower-case log line) shows this cause.
    fn matches(self, lower: &str) -> bool {
        let any = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
        match self {
            LaunchHint::MissingExecutable => {
                any(&[
                    "wine: cannot find",
                    "could not find the program",
                    "shellexecuteex failed: file not found",
                    "application could not be started",
                    ": command not found",
                ]) || (lower.contains("no such file or directory")
                    && (lower.starts_with("sh:")
                        || lower.starts_with("bash:")
                        || lower.contains("exec")))
            }
            LaunchHint::WrongWineBuild => any(&[
                "cannot support 64-bit applications",
                "version mismatch",
                "wrong elf class",
            ]),
            LaunchHint::AntiCheat => any(&["easyanticheat", "battleye", "eac_launcher"]),
            LaunchHint::MissingLibrary => is_missing_dll_line(lower) && !lower.contains("mscoree"),
            LaunchHint::MissingNativeLibrary => any(&["error while loading shared libraries"]),
            LaunchHint::DotNet => any(&["err:mscoree", "mscoree.dll", ".net framework"]),
            LaunchHint::Vulkan => any(&[
                "vk_error_incompatible_driver",
                "vk_error_initialization_failed",
                "failed to load vulkan",
                "no vulkan",
                "dxvk: no adapters",
            ]),
            LaunchHint::BrokenPrefix => any(&["could not load kernel32.dll", "wine client error"]),
            LaunchHint::PermissionDenied => any(&["permission denied"]),
            // Irori's own line when a program alongside fails to start.
            LaunchHint::ProgramFailed => lower.starts_with("could not start "),
            LaunchHint::MapCount => false,
            LaunchHint::Crash => any(&[
                "unhandled exception",
                "unhandled page fault",
                "segmentation fault",
                "core dumped",
                "fatal error",
            ]),
        }
    }
}

/// A line where Wine's loader says a DLL, or a function of one, is missing.
/// `WINEDEBUG=-all,err+module`, Irori's default, keeps these lines.
fn is_missing_dll_line(lower: &str) -> bool {
    lower.contains("err:module:import_dll")
        || lower.contains("err:module:load_dll")
        || (lower.contains("unimplemented function") && lower.contains(".dll"))
}

/// Windows libraries games often lack, with the winetricks verb that
/// installs them. Names are matched as prefixes of the DLL's file name.
const RUNTIME_VERBS: &[(&str, &str)] = &[
    ("vcruntime140", "vcrun2022"),
    ("msvcp140", "vcrun2022"),
    ("concrt140", "vcrun2022"),
    ("vcomp140", "vcrun2022"),
    ("msvcp120", "vcrun2013"),
    ("msvcr120", "vcrun2013"),
    ("msvcp110", "vcrun2012"),
    ("msvcr110", "vcrun2012"),
    ("msvcp100", "vcrun2010"),
    ("msvcr100", "vcrun2010"),
    ("d3dx9_", "d3dx9"),
    ("d3dx10_", "d3dx10"),
    ("d3dx11_", "d3dx11_43"),
    ("d3dcompiler_43", "d3dcompiler_43"),
    ("d3dcompiler_47", "d3dcompiler_47"),
    ("xinput1_3", "xinput"),
    ("xaudio2_7", "xact"),
    ("x3daudio1_7", "xact"),
    ("physxloader", "physx"),
    ("mscoree", "dotnet48"),
];

/// Verbs that also put the Visual C++ 2015–2022 runtime (`*140.dll`) in a
/// prefix.
const VCRUN_140: &[&str] = &["vcrun2015", "vcrun2017", "vcrun2019", "vcrun2022"];

/// The winetricks verb that installs `dll` (a file name, any case).
pub fn verb_for_dll(dll: &str) -> Option<&'static str> {
    let lower = dll.to_ascii_lowercase();
    RUNTIME_VERBS
        .iter()
        .find(|(name, _)| lower.starts_with(name))
        .map(|(_, verb)| *verb)
}

/// The DLL a loader error names: `Library MSVCP140.dll (which is needed by
/// …) not found` or `unimplemented function msvcp140.dll.?_Xbad_alloc…`.
fn dll_named(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for marker in ["library ", "unimplemented function ", "failed to load "] {
        let Some(at) = lower.find(marker) else {
            continue;
        };
        let rest = &lower[at + marker.len()..];
        let rest = rest
            .strip_prefix("l\"")
            .or_else(|| rest.strip_prefix('"'))
            .unwrap_or(rest);
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '\\'))
            .collect();
        let name = name.rsplit('\\').next().unwrap_or_default();
        if let Some(end) = name.find(".dll") {
            let dll = &name[..end + 4];
            if dll.len() > 4 {
                return Some(dll.to_string());
            }
        }
    }
    None
}

/// The DLLs the log says are missing, lower case, in the order first named.
pub fn missing_dlls<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut dlls = Vec::new();
    for line in lines {
        if !is_missing_dll_line(&line.to_ascii_lowercase()) {
            continue;
        }
        if let Some(dll) = dll_named(line)
            && !dlls.contains(&dll)
        {
            dlls.push(dll);
        }
    }
    dlls
}

/// Winetricks verbs that install the Windows libraries the log says are
/// missing, in the order first named.
pub fn runtime_verbs<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let mut verbs = Vec::new();
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if !is_missing_dll_line(&lower) && !lower.contains("err:mscoree") {
            continue;
        }
        // The DLL the line names, not the one needing it; lines naming
        // none (`err:mscoree`) are searched for any known library.
        let found: Vec<&'static str> = match dll_named(line) {
            Some(dll) => verb_for_dll(&dll).into_iter().collect(),
            None => RUNTIME_VERBS
                .iter()
                .filter(|(dll, _)| lower.contains(dll))
                .map(|(_, verb)| *verb)
                .collect(),
        };
        for verb in found {
            if !verbs.contains(&verb) {
                verbs.push(verb);
            }
        }
    }
    verbs
}

/// Index of the first line the game itself wrote: Irori's log header (game,
/// paths, the settings as JSON) ends with the launch command. The header
/// would otherwise feed the hints, e.g. `"battlEyeRuntime":true`.
pub fn game_output_start<'a>(lines: impl IntoIterator<Item = &'a str>) -> usize {
    lines
        .into_iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("Launch command:"))
        .last()
        .map_or(0, |(i, _)| i + 1)
}

/// The likely causes found in `lines`, most telling first.
pub fn launch_hints<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<LaunchHint> {
    let lines: Vec<String> = lines.into_iter().map(str::to_string).collect();
    let facts = Facts {
        lines,
        ..Default::default()
    };
    diagnose(&facts, &HashSet::new())
        .findings
        .into_iter()
        .map(|f| f.hint)
        .collect()
}

/// Parse `/proc/sys/vm/max_map_count`.
pub fn parse_map_count(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// What a finding knows besides its log lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    /// The DLLs the log names as missing, and the verbs that would install
    /// them but are already in the prefix.
    Libraries {
        dlls: Vec<String>,
        installed: Vec<&'static str>,
    },
    /// The program imports these runtime DLLs and the prefix has none of
    /// Microsoft's versions from winetricks.
    ProgramLibraries { dlls: Vec<String> },
    /// The program is a .NET program and the prefix has no .NET Framework.
    DotNetProgram,
    /// The anticheat database's entry: the anticheats and their status.
    AntiCheat {
        names: Vec<String>,
        status: AntiCheatStatus,
    },
    /// `vm.max_map_count`'s value.
    MapCount { current: u64 },
}

/// A one-click fix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fix {
    /// Install these winetricks verbs in the game's prefix, then try again.
    Winetricks(Vec<&'static str>),
    /// Turn on the Easy Anti-Cheat and BattlEye runtimes, then try again.
    AntiCheatRuntimes,
    /// Verify the game's files and download broken ones.
    VerifyFiles,
    /// Start the game's own executable instead of the alternative one set.
    ClearTargetExe,
    /// Run a native game in the Steam Linux Runtime, then try again.
    SteamRuntime,
    /// Open the settings with the programs started alongside.
    ProgramSettings,
    /// Show how to raise `vm.max_map_count`. Irori never runs `sudo`; the
    /// user runs the commands.
    MapCount,
}

impl Fix {
    /// Identifies the fix among those tried this session.
    pub fn key(&self) -> String {
        match self {
            Fix::Winetricks(verbs) => format!("winetricks:{}", verbs.join(",")),
            Fix::AntiCheatRuntimes => "anticheat-runtimes".into(),
            Fix::VerifyFiles => "verify".into(),
            Fix::ClearTargetExe => "target-exe".into(),
            Fix::SteamRuntime => "steam-runtime".into(),
            Fix::ProgramSettings => "programs".into(),
            Fix::MapCount => "map-count".into(),
        }
    }
}

/// The commands that raise `vm.max_map_count`: now, and after restarts.
pub fn map_count_commands() -> [String; 2] {
    [
        format!("sudo sysctl -w vm.max_map_count={MAP_COUNT_RECOMMENDED}"),
        format!(
            "echo 'vm.max_map_count = {MAP_COUNT_RECOMMENDED}' | sudo tee /etc/sysctl.d/80-gamecompatibility.conf"
        ),
    ]
}

/// A likely cause, what shows it and what may fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub hint: LaunchHint,
    /// The log lines that show it, trimmed; empty when the cause wasn't
    /// read from the log.
    pub evidence: Vec<String>,
    pub detail: Option<Detail>,
    /// Shown by the log, the exit code or a check; `false` when it is only
    /// worth checking (the program needs a runtime the prefix may lack).
    pub confirmed: bool,
    pub fixes: Vec<Fix>,
}

impl Finding {
    fn new(hint: LaunchHint) -> Self {
        Self {
            hint,
            evidence: Vec::new(),
            detail: None,
            confirmed: true,
            fixes: Vec::new(),
        }
    }

    /// The first line that shows it.
    pub fn line(&self) -> Option<&str> {
        self.evidence.first().map(String::as_str)
    }
}

/// What the fixes depend on besides the findings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FixContext {
    pub needs_wine: bool,
    /// Irori installed the game and can verify its files.
    pub verifiable: bool,
    /// Irori can turn on the anticheat runtimes here (Linux).
    pub anticheat_runtimes_available: bool,
    /// Both anticheat runtimes are already on.
    pub anticheat_runtimes_on: bool,
    /// An alternative executable is set and doesn't exist.
    pub target_exe_missing: bool,
    /// The game's own program doesn't exist.
    pub exe_missing: bool,
    /// A native Linux build running without the Steam runtime.
    pub native_without_runtime: bool,
}

/// Everything the diagnosis reads.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// What the game wrote (the log without Irori's header).
    pub lines: Vec<String>,
    pub exit_code: Option<i32>,
    /// The headers of the program the launch starts.
    pub program: Option<PeInfo>,
    /// Verbs in the prefix's `winetricks.log`.
    pub installed_verbs: Vec<String>,
    pub anticheat: Option<AntiCheatInfo>,
    /// `vm.max_map_count` (Linux).
    pub map_count: Option<u64>,
    pub context: FixContext,
}

/// A fix offered, and whether it was tried this session already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedFix {
    pub fix: Fix,
    pub tried: bool,
}

/// The findings, most telling first, and every fix they offer, untried
/// ones first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnosis {
    /// The code the game exited with, when known.
    pub exit_code: Option<i32>,
    pub findings: Vec<Finding>,
    pub fixes: Vec<OfferedFix>,
}

/// Lines that suggest the game ran out of memory maps.
fn map_count_evidence(lower: &str) -> bool {
    [
        "out of memory",
        "failed to allocate",
        "could not allocate",
        "bad_alloc",
        "mmap",
        "virtual_alloc",
    ]
    .iter()
    .any(|n| lower.contains(n))
}

fn push_evidence(finding: &mut Finding, line: &str) {
    let line = line.trim();
    if finding.evidence.len() < MAX_EVIDENCE && !finding.evidence.iter().any(|l| l == line) {
        finding.evidence.push(line.to_string());
    }
}

/// Work out why the game didn't start. `tried` holds the [`Fix::key`]s
/// already tried this session; those fixes come last.
pub fn diagnose(facts: &Facts, tried: &HashSet<String>) -> Diagnosis {
    let ctx = facts.context;
    let installed: HashSet<String> = facts
        .installed_verbs
        .iter()
        .map(|v| v.trim().to_ascii_lowercase())
        .collect();
    let has_verb = |verb: &str| installed.contains(verb);

    let mut findings: Vec<Finding> = Vec::new();
    for line in &facts.lines {
        let lower = line.to_ascii_lowercase();
        for hint in LaunchHint::FROM_LOG {
            if !hint.matches(&lower) {
                continue;
            }
            match findings.iter_mut().find(|f| f.hint == hint) {
                Some(f) => push_evidence(f, line),
                None => {
                    let mut f = Finding::new(hint);
                    push_evidence(&mut f, line);
                    findings.push(f);
                }
            }
        }
    }
    let ensure = |findings: &mut Vec<Finding>, hint| {
        if !findings.iter().any(|f| f.hint == hint) {
            findings.push(Finding::new(hint));
        }
    };
    // The shell's "command not found" and "cannot execute".
    match facts.exit_code {
        Some(127) => ensure(&mut findings, LaunchHint::MissingExecutable),
        Some(126) => ensure(&mut findings, LaunchHint::PermissionDenied),
        _ => {}
    }
    if ctx.target_exe_missing || ctx.exe_missing {
        ensure(&mut findings, LaunchHint::MissingExecutable);
    }

    let lines = || facts.lines.iter().map(String::as_str);
    let verbs = runtime_verbs(lines());
    let dlls = missing_dlls(lines());
    // The runtimes Irori can turn on are Easy Anti-Cheat's and BattlEye's.
    let uses_runtime_anticheat = facts.anticheat.as_ref().is_some_and(|ac| {
        ac.anticheats.iter().any(|n| {
            let n = n.to_ascii_lowercase();
            n.contains("easy") || n.contains("battleye")
        })
    });

    for f in &mut findings {
        match f.hint {
            LaunchHint::MissingExecutable => {
                if ctx.target_exe_missing {
                    f.fixes.push(Fix::ClearTargetExe);
                }
                if ctx.verifiable {
                    f.fixes.push(Fix::VerifyFiles);
                }
            }
            LaunchHint::MissingLibrary => {
                let (wanted, present): (Vec<&'static str>, Vec<&'static str>) = verbs
                    .iter()
                    .copied()
                    .filter(|v| *v != "dotnet48")
                    .partition(|v| !has_verb(v));
                f.detail = Some(Detail::Libraries {
                    dlls: dlls
                        .iter()
                        .filter(|d| !d.starts_with("mscoree"))
                        .cloned()
                        .collect(),
                    installed: present,
                });
                if ctx.needs_wine && !wanted.is_empty() {
                    f.fixes.push(Fix::Winetricks(wanted));
                }
            }
            LaunchHint::DotNet => {
                if ctx.needs_wine && !has_verb("dotnet48") && !has_verb("dotnet481") {
                    f.fixes.push(Fix::Winetricks(vec!["dotnet48"]));
                }
            }
            LaunchHint::AntiCheat => {
                if let Some(ac) = &facts.anticheat
                    && matches!(ac.status, AntiCheatStatus::Broken | AntiCheatStatus::Denied)
                {
                    f.detail = Some(Detail::AntiCheat {
                        names: ac.anticheats.clone(),
                        status: ac.status,
                    });
                }
                let blocked = matches!(f.detail, Some(Detail::AntiCheat { .. }));
                if ctx.needs_wine
                    && ctx.anticheat_runtimes_available
                    && !ctx.anticheat_runtimes_on
                    && !blocked
                {
                    f.fixes.push(Fix::AntiCheatRuntimes);
                }
            }
            LaunchHint::MissingNativeLibrary if ctx.native_without_runtime => {
                f.fixes.push(Fix::SteamRuntime);
            }
            LaunchHint::ProgramFailed => f.fixes.push(Fix::ProgramSettings),
            _ => {}
        }
    }

    // The anticheat database knows more than the log: a game whose
    // anticheat refuses Wine fails however it's set up, and one that works
    // with the runtimes fails without them.
    if ctx.needs_wine
        && let Some(ac) = &facts.anticheat
        && ac.online_only.is_none()
        && !findings.iter().any(|f| f.hint == LaunchHint::AntiCheat)
    {
        let blocked = matches!(ac.status, AntiCheatStatus::Broken | AntiCheatStatus::Denied);
        let wants_runtimes = !blocked
            && uses_runtime_anticheat
            && ctx.anticheat_runtimes_available
            && !ctx.anticheat_runtimes_on
            && matches!(
                ac.status,
                AntiCheatStatus::Supported | AntiCheatStatus::Running | AntiCheatStatus::Working
            );
        if blocked || wants_runtimes {
            let mut f = Finding::new(LaunchHint::AntiCheat);
            f.detail = Some(Detail::AntiCheat {
                names: ac.anticheats.clone(),
                status: ac.status,
            });
            f.confirmed = blocked;
            if wants_runtimes {
                f.fixes.push(Fix::AntiCheatRuntimes);
            }
            findings.push(f);
        }
    }

    // Wine's own runtime DLLs stand in for Microsoft's and usually work,
    // so a program's imports are only worth checking when nothing else
    // explains the failure.
    if ctx.needs_wine
        && !findings.iter().any(|f| f.confirmed)
        && let Some(program) = &facts.program
    {
        let has_vcrun = VCRUN_140.iter().any(|v| has_verb(v));
        let mut wanted: Vec<&'static str> = Vec::new();
        let mut needs: Vec<String> = Vec::new();
        for dll in &program.imports {
            let verb = match verb_for_dll(dll) {
                Some("vcrun2022") if !has_vcrun => "vcrun2022",
                Some("d3dcompiler_47") if !has_verb("d3dcompiler_47") => "d3dcompiler_47",
                _ => continue,
            };
            needs.push(dll.clone());
            if !wanted.contains(&verb) {
                wanted.push(verb);
            }
        }
        if !wanted.is_empty() {
            let mut f = Finding::new(LaunchHint::MissingLibrary);
            f.confirmed = false;
            f.detail = Some(Detail::ProgramLibraries { dlls: needs });
            f.fixes.push(Fix::Winetricks(wanted));
            findings.push(f);
        }
        let has_dotnet = installed.iter().any(|v| v.starts_with("dotnet4"));
        if program.dotnet && !has_dotnet && !findings.iter().any(|f| f.hint == LaunchHint::DotNet) {
            let mut f = Finding::new(LaunchHint::DotNet);
            f.confirmed = false;
            f.detail = Some(Detail::DotNetProgram);
            f.fixes.push(Fix::Winetricks(vec!["dotnet48"]));
            findings.push(f);
        }
    }

    if let Some(current) = facts.map_count.filter(|c| *c < MAP_COUNT_WANTED) {
        let mut f = Finding::new(LaunchHint::MapCount);
        for line in &facts.lines {
            if map_count_evidence(&line.to_ascii_lowercase()) {
                push_evidence(&mut f, line);
            }
        }
        f.confirmed = !f.evidence.is_empty();
        f.detail = Some(Detail::MapCount { current });
        f.fixes.push(Fix::MapCount);
        findings.push(f);
    }

    findings.sort_by_key(|f| (!f.confirmed, f.hint));

    let mut fixes: Vec<OfferedFix> = Vec::new();
    for fix in findings.iter().flat_map(|f| &f.fixes) {
        if !fixes.iter().any(|o| &o.fix == fix) {
            fixes.push(OfferedFix {
                fix: fix.clone(),
                tried: tried.contains(&fix.key()),
            });
        }
    }
    fixes.sort_by_key(|o| o.tried);
    Diagnosis {
        exit_code: facts.exit_code,
        findings,
        fixes,
    }
}
