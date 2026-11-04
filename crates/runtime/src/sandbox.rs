//! Running a game in a bubblewrap (`bwrap`) sandbox on Linux.
//!
//! The sandbox is one more wrapper in front of the game: gamescope and
//! gamemoderun stay outside it (they are the user's tools and talk to the
//! compositor and D-Bus), everything after them runs inside. Inside, the game
//! sees an empty root holding only what it needs: read-only system folders,
//! its install folder and Wine prefix, the Wine or Proton build, the GPU,
//! controllers, sound and display sockets, and a private `/tmp`, `/run` and
//! `/proc`. "No home folder" replaces the home folder with one of the game's
//! own and hides every other drive; "No network" gives it a network namespace
//! with nothing but loopback.
//!
//! The same outer sandbox serves every runner. umu runs Proton in
//! pressure-vessel, which is itself bubblewrap; it nests inside ours (its
//! user namespace is a child of ours) and only ever sees what ours exposes.
//! pressure-vessel's own options are not a sandbox: it shares the home folder,
//! D-Bus and sockets by design and doesn't apply to direct Proton, plain Wine
//! or native games, so it is the wrong layer to restrict a game with.
//!
//! [`bwrap_args`] is pure: it turns a [`HostLayout`] (what this computer has)
//! and a [`SandboxPlan`] (what this game needs) into the wrapper's argv, so
//! tests cover every mode and runner without a sandbox. [`wrapper`] gathers
//! both and checks that bubblewrap can run at all.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::process::CommandSpec;
use irori_core::{GameId, Paths, SandboxProblem, SandboxSettings, SandboxUnavailable};

use crate::locations;

/// Top-level host folders bound read-only: the system itself.
const SYSTEM_DIRS: [&str; 10] = [
    "usr", "etc", "opt", "bin", "sbin", "lib", "lib32", "lib64", "libx32", "nix",
];

/// Top-level host folders the sandbox builds itself or never shows: kernel
/// filesystems, private scratch space, the home folders and the boot files.
const SPECIAL_DIRS: [&str; 11] = [
    "proc",
    "dev",
    "sys",
    "tmp",
    "run",
    "var",
    "home",
    "root",
    "boot",
    "efi",
    "lost+found",
];

/// Device nodes the game may use, when the host has them: the GPU, sound and
/// controllers. `/dev/uinput` is left out on purpose: it injects input into
/// every other program.
const DEVICE_DIRS: [&str; 3] = ["/dev/dri", "/dev/input", "/dev/snd"];

/// Folders under `/run` that games read: udev's device database (controller
/// hot-plug) and the GPU drivers NixOS links there.
const RUN_DIRS: [&str; 3] = ["/run/udev", "/run/opengl-driver", "/run/opengl-driver-32"];

/// One entry at the root of the host filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootEntry {
    Dir(String),
    /// A symlink and where it points (`bin` → `usr/bin`).
    Symlink(String, String),
}

/// What this computer offers a sandbox. [`HostLayout::detect`] reads it;
/// tests write it by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostLayout {
    pub root: Vec<RootEntry>,
    pub home: PathBuf,
    /// `$XDG_RUNTIME_DIR`.
    pub runtime_dir: Option<PathBuf>,
    /// Wayland, PipeWire and PulseAudio sockets, by absolute path.
    pub sockets: Vec<PathBuf>,
    /// `/tmp/.X11-unix` exists.
    pub x11: bool,
    /// The X cookie file.
    pub xauthority: Option<PathBuf>,
    /// Device nodes and folders to pass through (`/dev/dri`, `/dev/nvidia0`…).
    pub devices: Vec<PathBuf>,
    /// Folders holding the DNS configuration `/etc/resolv.conf` points to
    /// (`/run/systemd/resolve`).
    pub resolver_dirs: Vec<PathBuf>,
    /// bubblewrap knows `--overlay-src`/`--tmp-overlay` (0.10 and later).
    pub overlay: bool,
    /// bubblewrap knows `--disable-userns` (0.8 and later).
    pub disable_userns: bool,
}

/// How the game is started inside, which decides how the sandbox ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxKind {
    /// A native program.
    Native,
    /// Plain Wine. The sandbox's processes all end with its first one, so it
    /// waits for the prefix's wineserver, as a launcher that starts the game
    /// and exits would otherwise take the game with it.
    Wine { wineserver: Option<PathBuf> },
    /// Proton run directly; `waitforexitandrun` already waits.
    Proton,
    /// umu (Proton) or umu's Steam runtime for native games: pressure-vessel
    /// nests its own bubblewrap, so the sandbox must allow user namespaces.
    PressureVessel,
}

/// What one game needs from the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    pub settings: SandboxSettings,
    pub kind: SandboxKind,
    /// Writable: the install folder, the Wine prefix.
    pub game_dirs: Vec<PathBuf>,
    /// Read-only: the Wine or Proton build, umu-run, anticheat runtimes, an
    /// installer the user picked (files work too).
    pub tool_dirs: Vec<PathBuf>,
    /// Read-only with a throwaway writable layer (umu's Steam runtime, which
    /// umu and pressure-vessel write lock and state files into). Without
    /// overlay support they are writable.
    pub overlay_dirs: Vec<PathBuf>,
    /// The game's own home folder, shown in place of the user's with
    /// `no_home`.
    pub home: PathBuf,
    /// gamescope runs outside and opens its own Wayland socket for the game.
    pub gamescope: bool,
}

/// One bind mount, sorted so parents are mounted before their children.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    flag: &'static str,
    src: PathBuf,
    dest: PathBuf,
}

impl Mount {
    fn new(flag: &'static str, src: impl Into<PathBuf>, dest: impl Into<PathBuf>) -> Self {
        Self {
            flag,
            src: src.into(),
            dest: dest.into(),
        }
    }

    fn same(flag: &'static str, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self::new(flag, path.clone(), path)
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Shell run inside the sandbox for plain Wine: run the command, then wait
/// for the prefix's wineserver, then end with the command's status.
const WAIT_FOR_WINESERVER: &str = r#"w=$1; shift; "$@"; s=$?; "$w" -w; exit $s"#;

/// The wrapper argv: `bwrap`, its options, `--`, and for plain Wine the shell
/// that waits for the wineserver. The game's command follows it.
pub fn bwrap_args(bwrap: &str, host: &HostLayout, plan: &SandboxPlan) -> Vec<String> {
    let mut a: Vec<String> = vec![bwrap.to_string()];
    let mut push = |args: &[&str]| a.extend(args.iter().map(|x| x.to_string()));
    let no_home = plan.settings.no_home;
    let no_network = plan.settings.no_network;

    push(&["--die-with-parent", "--new-session", "--unshare-pid"]);
    push(&["--unshare-uts", "--unshare-cgroup-try"]);
    if no_network {
        push(&["--unshare-net"]);
    }
    // A game that can't create namespaces of its own can't reach the
    // kernel's namespace code; pressure-vessel needs them.
    if host.disable_userns && plan.kind != SandboxKind::PressureVessel {
        push(&["--unshare-user", "--disable-userns"]);
    }
    // The session bus isn't in the sandbox (it could start programs outside
    // it); a variable still naming it makes pressure-vessel try to bind it.
    push(&["--unsetenv", "DBUS_SESSION_BUS_ADDRESS"]);
    push(&["--unsetenv", "DBUS_SYSTEM_BUS_ADDRESS"]);
    push(&["--setenv", "IRORI_SANDBOX", "1"]);
    if plan.kind == SandboxKind::PressureVessel {
        // Updates would land in the throwaway layer; Irori's own launches
        // keep the runtime current.
        push(&["--setenv", "UMU_RUNTIME_UPDATE", "0"]);
    }

    let mut user_dirs: Vec<String> = Vec::new();
    for entry in &host.root {
        match entry {
            RootEntry::Symlink(name, target) => {
                a.extend(["--symlink".into(), target.clone(), format!("/{name}")]);
            }
            RootEntry::Dir(name) if SYSTEM_DIRS.contains(&name.as_str()) => {
                let p = format!("/{name}");
                a.extend(["--ro-bind".into(), p.clone(), p]);
            }
            RootEntry::Dir(name) if SPECIAL_DIRS.contains(&name.as_str()) => {}
            // `/mnt`, `/media`, `/srv`, a games drive at `/games`.
            RootEntry::Dir(name) => user_dirs.push(format!("/{name}")),
        }
    }

    let mut push = |args: &[&str]| a.extend(args.iter().map(|x| x.to_string()));
    push(&[
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--ro-bind",
        "/sys",
        "/sys",
    ]);
    for dev in &host.devices {
        let d = s(dev);
        push(&["--dev-bind-try", &d, &d]);
    }
    push(&["--tmpfs", "/tmp", "--tmpfs", "/var/tmp"]);
    push(&["--symlink", "../run", "/var/run"]);
    push(&[
        "--ro-bind-try",
        "/var/cache/fontconfig",
        "/var/cache/fontconfig",
    ]);
    push(&["--ro-bind-try", "/var/lib/dbus", "/var/lib/dbus"]);
    push(&["--tmpfs", "/run"]);
    for dir in RUN_DIRS {
        push(&["--ro-bind-try", dir, dir]);
    }
    if !no_network {
        for dir in &host.resolver_dirs {
            let d = s(dir);
            push(&["--ro-bind-try", &d, &d]);
        }
    }
    if let Some(runtime) = &host.runtime_dir {
        let r = s(runtime);
        push(&["--perms", "0700", "--dir", &r]);
        let mut sockets = host.sockets.clone();
        if plan.gamescope {
            // gamescope opens it just before starting the sandbox.
            sockets.push(runtime.join("gamescope-0"));
        }
        for socket in sockets {
            let p = s(&socket);
            push(&["--ro-bind-try", &p, &p]);
        }
    }
    if host.x11 {
        // In a network namespace of their own, X11 clients can't reach the
        // abstract socket and fall back to these.
        push(&["--ro-bind", "/tmp/.X11-unix", "/tmp/.X11-unix"]);
    }

    let mut mounts: Vec<Mount> = Vec::new();
    // Often in the home folder, so mounted after it.
    if let Some(auth) = &host.xauthority {
        mounts.push(Mount::same("--ro-bind-try", auth));
    }
    if no_home {
        mounts.push(Mount::new("--bind", &plan.home, &host.home));
    } else {
        mounts.push(Mount::same("--bind", &host.home));
        mounts.push(Mount::same("--bind-try", "/run/media"));
        for dir in user_dirs {
            mounts.push(Mount::same("--bind", dir));
        }
    }
    for dir in &plan.tool_dirs {
        mounts.push(Mount::same("--ro-bind", dir));
    }
    for dir in &plan.overlay_dirs {
        if !no_home && dir.starts_with(&host.home) {
            continue;
        }
        mounts.push(Mount::same(
            if host.overlay { "--overlay" } else { "--bind" },
            dir,
        ));
    }
    for dir in &plan.game_dirs {
        mounts.push(Mount::same("--bind", dir));
    }
    // A shallower mount first, so a deeper one isn't hidden under it.
    mounts.sort_by_key(|m| m.dest.components().count());
    let mut seen = std::collections::HashSet::new();
    mounts.retain(|m| seen.insert(m.dest.clone()));
    for m in mounts {
        let (src, dest) = (s(&m.src), s(&m.dest));
        if m.flag == "--overlay" {
            a.extend(["--overlay-src".into(), src, "--tmp-overlay".into(), dest]);
        } else {
            a.extend([m.flag.into(), src, dest]);
        }
    }

    a.push("--".into());
    if let SandboxKind::Wine {
        wineserver: Some(server),
    } = &plan.kind
    {
        a.extend([
            "/bin/sh".into(),
            "-c".into(),
            WAIT_FOR_WINESERVER.into(),
            "irori-sandbox".into(),
            s(server),
        ]);
    }
    a
}

impl HostLayout {
    /// Read the host: the root folder, display and sound sockets, devices,
    /// DNS configuration and what this bubblewrap supports.
    pub fn detect(bwrap_version: Option<(u32, u32)>) -> Self {
        let home = locations::home();
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let mut sockets = Vec::new();
        if let Some(r) = &runtime_dir {
            let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
            sockets.push(r.join(wayland));
            sockets.push(r.join("pipewire-0"));
            sockets.push(r.join("pulse/native"));
        }
        if let Some(pulse) = std::env::var("PULSE_SERVER")
            .ok()
            .and_then(|v| v.strip_prefix("unix:").map(PathBuf::from))
        {
            sockets.push(pulse);
        }
        sockets.retain(|p| p.exists());
        sockets.dedup();
        let xauthority = std::env::var_os("XAUTHORITY")
            .map(PathBuf::from)
            .or_else(|| Some(home.join(".Xauthority")))
            .filter(|p| p.is_file());
        let (major, minor) = bwrap_version.unwrap_or((0, 0));
        Self {
            root: read_root(Path::new("/")),
            home,
            runtime_dir,
            sockets,
            x11: Path::new("/tmp/.X11-unix").is_dir(),
            xauthority,
            devices: devices(Path::new("/dev")),
            resolver_dirs: resolver_dirs(Path::new("/etc/resolv.conf")),
            overlay: (major, minor) >= (0, 10),
            disable_userns: (major, minor) >= (0, 8),
        }
    }
}

fn read_root(root: &Path) -> Vec<RootEntry> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<RootEntry> = dir
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let meta = std::fs::symlink_metadata(e.path()).ok()?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(e.path()).ok()?;
                Some(RootEntry::Symlink(name, target.to_str()?.to_string()))
            } else {
                meta.is_dir().then_some(RootEntry::Dir(name))
            }
        })
        .collect();
    out.sort_by(|a, b| root_name(a).cmp(root_name(b)));
    out
}

fn root_name(e: &RootEntry) -> &str {
    match e {
        RootEntry::Dir(n) | RootEntry::Symlink(n, _) => n,
    }
}

/// The GPU, controller and sound devices this computer has. NVIDIA's are
/// single nodes (`nvidia0`, `nvidiactl`, `nvidia-uvm`) plus `nvidia-caps`.
fn devices(dev: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = DEVICE_DIRS
        .iter()
        .map(|d| dev.join(Path::new(d).strip_prefix("/dev").unwrap_or(Path::new(d))))
        .filter(|p| p.exists())
        .collect();
    if let Ok(dir) = std::fs::read_dir(dev) {
        let mut nodes: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("nvidia") || n.starts_with("hidraw"))
            })
            .map(|e| e.path())
            .collect();
        nodes.sort();
        out.extend(nodes);
    }
    out
}

/// Where the DNS configuration really is when `/etc/resolv.conf` links out
/// of `/etc` (systemd-resolved, NetworkManager).
fn resolver_dirs(resolv: &Path) -> Vec<PathBuf> {
    std::fs::canonicalize(resolv)
        .ok()
        .filter(|p| !p.starts_with("/etc"))
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .into_iter()
        .collect()
}

/// `bubblewrap 0.11.0` → `(0, 11)`.
pub fn parse_version(output: &str) -> Option<(u32, u32)> {
    let version = output.split_whitespace().nth(1)?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Whether games of `runner` can run sandboxed: Irori starts them itself
/// (not Steam or another launcher), and not through an emulator, which may
/// be a Flatpak that can't start inside the sandbox.
pub fn applies_to(runner: irori_core::Runner) -> bool {
    cfg!(target_os = "linux")
        && runner.manages_installs()
        && runner != irori_core::Runner::Emulation
}

/// The game's own home folder: `<data>/sandbox/<runner>/<app>`. Its saves
/// and settings stay there while "No home folder" is on.
pub fn sandbox_home(id: &GameId) -> PathBuf {
    Paths::get()
        .data
        .join("sandbox")
        .join(id.runner.id())
        .join(irori_core::paths::sanitize(&id.app_name))
}

/// A path and its real location, when they differ, so the game finds its
/// folders under either name.
pub fn with_real_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        if p.as_os_str().is_empty() || !p.is_absolute() {
            continue;
        }
        let real = std::fs::canonicalize(&p).ok();
        out.push(p);
        out.extend(real);
    }
    out.sort();
    out.dedup();
    out
}

/// Build the sandbox wrapper for a launch of `id` that needs `plan`'s
/// folders: checks bubblewrap, creates the game's own home folder, reads the
/// host and returns the argv to put in front of the game.
pub async fn wrapper(id: &GameId, mut plan: SandboxPlan) -> Result<Vec<String>> {
    let (bwrap, version) = check().await?;
    if plan.settings.no_home {
        tokio::fs::create_dir_all(&plan.home)
            .await
            .with_context(|| format!("Could not create {}", plan.home.display()))?;
    }
    let host = tokio::task::spawn_blocking(move || {
        plan.game_dirs = with_real_paths(std::mem::take(&mut plan.game_dirs));
        plan.tool_dirs = with_real_paths(std::mem::take(&mut plan.tool_dirs));
        plan.overlay_dirs = with_real_paths(std::mem::take(&mut plan.overlay_dirs));
        plan.tool_dirs
            .retain(|d| d.exists() && !d.starts_with("/usr") && !d.starts_with("/opt"));
        plan.overlay_dirs.retain(|d| d.is_dir());
        plan.game_dirs.retain(|d| d.is_dir());
        (HostLayout::detect(version), plan)
    })
    .await
    .context("Could not look at this system for the sandbox")?;
    let (host, plan) = host;
    if plan.settings.no_home && !plan.overlay_dirs.is_empty() && !host.overlay {
        tracing::warn!(
            "bubblewrap {version:?} has no overlays; {id} can write to the Steam runtime"
        );
    }
    Ok(bwrap_args(&bwrap.to_string_lossy(), &host, &plan))
}

/// Find bubblewrap and check that it can make a sandbox here. Refuses with a
/// typed [`SandboxUnavailable`] instead of letting the game run without it.
pub async fn check() -> Result<(PathBuf, Option<(u32, u32)>)> {
    let unavailable = |problem| anyhow::Error::from(SandboxUnavailable { problem });
    if !cfg!(target_os = "linux") {
        return Err(unavailable(SandboxProblem::Unsupported));
    }
    let flatpak = locations::is_flatpak();
    let Ok(bwrap) = which::which("bwrap") else {
        return Err(unavailable(if flatpak {
            SandboxProblem::Flatpak
        } else {
            SandboxProblem::Missing
        }));
    };
    let version = crate::discovery::run_with_timeout(
        &CommandSpec::new(&bwrap).arg("--version"),
        std::time::Duration::from_secs(5),
    )
    .await
    .and_then(|r| parse_version(&r.stdout));
    let probe = CommandSpec::new(&bwrap).args([
        "--unshare-pid",
        "--unshare-net",
        "--die-with-parent",
        "--ro-bind",
        "/",
        "/",
        "--proc",
        "/proc",
        "--",
        "/bin/true",
    ]);
    match crate::discovery::run_with_timeout(&probe, std::time::Duration::from_secs(10)).await {
        Some(r) if r.success() => Ok((bwrap, version)),
        _ if flatpak => Err(unavailable(SandboxProblem::Flatpak)),
        Some(r) => Err(unavailable(SandboxProblem::Refused {
            detail: r
                .stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("bwrap failed")
                .trim()
                .to_string(),
        })),
        None => Err(unavailable(SandboxProblem::Refused {
            detail: "bwrap did not answer".into(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostLayout {
        HostLayout {
            root: vec![
                RootEntry::Symlink("bin".into(), "usr/bin".into()),
                RootEntry::Dir("boot".into()),
                RootEntry::Dir("dev".into()),
                RootEntry::Dir("etc".into()),
                RootEntry::Dir("games".into()),
                RootEntry::Dir("home".into()),
                RootEntry::Symlink("lib64".into(), "usr/lib64".into()),
                RootEntry::Dir("mnt".into()),
                RootEntry::Dir("proc".into()),
                RootEntry::Dir("tmp".into()),
                RootEntry::Dir("usr".into()),
                RootEntry::Dir("var".into()),
            ],
            home: "/home/u".into(),
            runtime_dir: Some("/run/user/1000".into()),
            sockets: vec![
                "/run/user/1000/wayland-0".into(),
                "/run/user/1000/pipewire-0".into(),
            ],
            x11: true,
            xauthority: Some("/run/user/1000/xauth_x".into()),
            devices: vec![
                "/dev/dri".into(),
                "/dev/input".into(),
                "/dev/nvidia0".into(),
                "/dev/nvidiactl".into(),
                "/dev/hidraw0".into(),
            ],
            resolver_dirs: vec!["/run/systemd/resolve".into()],
            overlay: true,
            disable_userns: true,
        }
    }

    fn plan(no_network: bool, no_home: bool, kind: SandboxKind) -> SandboxPlan {
        SandboxPlan {
            settings: SandboxSettings {
                no_network,
                no_home,
            },
            kind,
            game_dirs: vec!["/games/Hades".into(), "/home/u/Games/Prefixes/Hades".into()],
            tool_dirs: vec!["/home/u/.local/share/irori/tools/proton/GE-Proton10".into()],
            overlay_dirs: Vec::new(),
            home: "/home/u/.local/share/irori/sandbox/gog/hades".into(),
            gamescope: false,
        }
    }

    fn args(host: &HostLayout, plan: &SandboxPlan) -> Vec<String> {
        bwrap_args("/usr/bin/bwrap", host, plan)
    }

    /// Whether `seq` appears in `args` as consecutive items.
    fn has(args: &[String], seq: &[&str]) -> bool {
        args.windows(seq.len())
            .any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
    }

    fn pos(args: &[String], seq: &[&str]) -> usize {
        args.windows(seq.len())
            .position(|w| w.iter().zip(seq).all(|(a, b)| a == b))
            .unwrap_or_else(|| panic!("{seq:?} not in {args:?}"))
    }

    #[test]
    fn no_network_keeps_the_files_and_drops_the_network() {
        let a = args(&host(), &plan(true, false, SandboxKind::Native));
        assert_eq!(a[0], "/usr/bin/bwrap");
        assert!(has(&a, &["--unshare-net"]));
        assert!(has(&a, &["--bind", "/home/u", "/home/u"]));
        assert!(has(&a, &["--bind", "/games", "/games"]));
        assert!(has(&a, &["--bind", "/mnt", "/mnt"]));
        assert!(has(&a, &["--bind-try", "/run/media", "/run/media"]));
        assert!(
            !has(&a, &["--ro-bind-try", "/run/systemd/resolve"]),
            "no DNS without a network"
        );
        assert_eq!(a.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn no_home_shows_only_the_games_own_folders() {
        let a = args(&host(), &plan(false, true, SandboxKind::Proton));
        assert!(!a.iter().any(|x| x == "--unshare-net"));
        assert!(has(
            &a,
            &[
                "--bind",
                "/home/u/.local/share/irori/sandbox/gog/hades",
                "/home/u"
            ]
        ));
        assert!(!has(&a, &["--bind", "/home/u", "/home/u"]));
        assert!(!has(&a, &["--bind", "/games", "/games"]));
        assert!(!a.iter().any(|x| x == "/mnt" || x == "/boot"));
        assert!(has(&a, &["--bind", "/games/Hades", "/games/Hades"]));
        assert!(has(
            &a,
            &[
                "--ro-bind",
                "/home/u/.local/share/irori/tools/proton/GE-Proton10",
                "/home/u/.local/share/irori/tools/proton/GE-Proton10"
            ]
        ));
        assert!(has(
            &a,
            &[
                "--ro-bind-try",
                "/run/systemd/resolve",
                "/run/systemd/resolve"
            ]
        ));
        // The private home goes first, then what lies inside it.
        let home = pos(
            &a,
            &["--bind", "/home/u/.local/share/irori/sandbox/gog/hades"],
        );
        let prefix = pos(&a, &["--bind", "/home/u/Games/Prefixes/Hades"]);
        assert!(home < prefix);
    }

    #[test]
    fn both_restrictions_together() {
        let a = args(&host(), &plan(true, true, SandboxKind::Native));
        assert!(has(&a, &["--unshare-net"]));
        assert!(has(
            &a,
            &["--bind", "/home/u/.local/share/irori/sandbox/gog/hades"]
        ));
        assert!(!has(&a, &["--bind", "/home/u", "/home/u"]));
    }

    #[test]
    fn the_system_is_read_only_and_scratch_space_private() {
        let a = args(&host(), &plan(true, true, SandboxKind::Native));
        assert!(has(&a, &["--symlink", "usr/bin", "/bin"]));
        assert!(has(&a, &["--symlink", "usr/lib64", "/lib64"]));
        assert!(has(&a, &["--ro-bind", "/usr", "/usr"]));
        assert!(has(&a, &["--ro-bind", "/etc", "/etc"]));
        assert!(has(&a, &["--ro-bind", "/sys", "/sys"]));
        assert!(has(&a, &["--proc", "/proc"]));
        assert!(has(&a, &["--dev", "/dev"]));
        assert!(has(&a, &["--tmpfs", "/tmp"]));
        assert!(has(&a, &["--tmpfs", "/run"]));
        assert!(has(
            &a,
            &["--die-with-parent", "--new-session", "--unshare-pid"]
        ));
        assert!(has(&a, &["--unsetenv", "DBUS_SESSION_BUS_ADDRESS"]));
        assert!(!a.iter().any(|x| x.contains("/run/user/1000/bus")));
        assert!(!a.iter().any(|x| x.contains("uinput")));
        assert!(!a.iter().any(|x| x == "--unshare-ipc"), "X11 shared memory");
    }

    #[test]
    fn devices_display_and_sound_pass_through() {
        let a = args(&host(), &plan(true, true, SandboxKind::Native));
        for dev in [
            "/dev/dri",
            "/dev/input",
            "/dev/nvidia0",
            "/dev/nvidiactl",
            "/dev/hidraw0",
        ] {
            assert!(has(&a, &["--dev-bind-try", dev, dev]), "{dev}");
        }
        assert!(has(&a, &["--perms", "0700", "--dir", "/run/user/1000"]));
        for socket in ["/run/user/1000/wayland-0", "/run/user/1000/pipewire-0"] {
            assert!(has(&a, &["--ro-bind-try", socket, socket]), "{socket}");
        }
        assert!(has(&a, &["--ro-bind", "/tmp/.X11-unix", "/tmp/.X11-unix"]));
        assert!(has(&a, &["--ro-bind-try", "/run/user/1000/xauth_x"]));
        // The runtime folder exists before the sockets in it.
        assert!(
            pos(&a, &["--dir", "/run/user/1000"])
                < pos(&a, &["--ro-bind-try", "/run/user/1000/wayland-0"])
        );
        assert!(pos(&a, &["--tmpfs", "/tmp"]) < pos(&a, &["--ro-bind", "/tmp/.X11-unix"]));
    }

    #[test]
    fn an_x_cookie_in_the_home_folder_stays_visible() {
        let h = HostLayout {
            xauthority: Some("/home/u/.Xauthority".into()),
            ..host()
        };
        let a = args(&h, &plan(true, true, SandboxKind::Native));
        let home = pos(
            &a,
            &[
                "--bind",
                "/home/u/.local/share/irori/sandbox/gog/hades",
                "/home/u",
            ],
        );
        let cookie = pos(
            &a,
            &[
                "--ro-bind-try",
                "/home/u/.Xauthority",
                "/home/u/.Xauthority",
            ],
        );
        assert!(home < cookie);
    }

    #[test]
    fn gamescope_gets_its_socket() {
        let mut p = plan(true, true, SandboxKind::Native);
        p.gamescope = true;
        let a = args(&host(), &p);
        assert!(has(
            &a,
            &[
                "--ro-bind-try",
                "/run/user/1000/gamescope-0",
                "/run/user/1000/gamescope-0"
            ]
        ));
    }

    #[test]
    fn plain_wine_waits_for_its_wineserver() {
        let a = args(
            &host(),
            &plan(
                true,
                true,
                SandboxKind::Wine {
                    wineserver: Some("/usr/bin/wineserver".into()),
                },
            ),
        );
        let dashes = pos(&a, &["--"]);
        assert_eq!(
            &a[dashes + 1..],
            [
                "/bin/sh",
                "-c",
                WAIT_FOR_WINESERVER,
                "irori-sandbox",
                "/usr/bin/wineserver"
            ]
        );
        assert!(has(&a, &["--unshare-user", "--disable-userns"]));
    }

    #[test]
    fn proton_and_native_games_end_at_the_separator() {
        for kind in [
            SandboxKind::Proton,
            SandboxKind::Native,
            SandboxKind::Wine { wineserver: None },
        ] {
            let a = args(&host(), &plan(true, true, kind.clone()));
            assert_eq!(a.last().map(String::as_str), Some("--"), "{kind:?}");
            assert!(has(&a, &["--disable-userns"]), "{kind:?}");
        }
    }

    #[test]
    fn umu_nests_pressure_vessel() {
        let mut p = plan(true, true, SandboxKind::PressureVessel);
        p.overlay_dirs = vec!["/home/u/.local/share/umu".into()];
        let a = args(&host(), &p);
        assert!(!a.iter().any(|x| x == "--disable-userns"));
        assert!(has(&a, &["--setenv", "UMU_RUNTIME_UPDATE", "0"]));
        assert!(has(
            &a,
            &[
                "--overlay-src",
                "/home/u/.local/share/umu",
                "--tmp-overlay",
                "/home/u/.local/share/umu"
            ]
        ));
        assert_eq!(a.last().map(String::as_str), Some("--"));

        // Older bubblewrap: the runtime stays writable rather than broken.
        let old = HostLayout {
            overlay: false,
            disable_userns: false,
            ..host()
        };
        let a = args(&old, &p);
        assert!(has(
            &a,
            &[
                "--bind",
                "/home/u/.local/share/umu",
                "/home/u/.local/share/umu"
            ]
        ));
        assert!(!a.iter().any(|x| x == "--overlay-src"));

        // With the home folder shown, the runtime is reachable anyway.
        let p = SandboxPlan {
            settings: SandboxSettings {
                no_network: true,
                no_home: false,
            },
            ..p
        };
        let a = args(&host(), &p);
        assert!(!a.iter().any(|x| x == "--overlay-src"));
    }

    #[test]
    fn old_bubblewrap_keeps_user_namespaces() {
        let old = HostLayout {
            disable_userns: false,
            ..host()
        };
        let a = args(&old, &plan(true, true, SandboxKind::Proton));
        assert!(!a.iter().any(|x| x == "--disable-userns"));
    }

    #[test]
    fn duplicate_folders_are_bound_once() {
        let mut p = plan(true, true, SandboxKind::Native);
        p.game_dirs.push("/games/Hades".into());
        let a = args(&host(), &p);
        assert_eq!(a.iter().filter(|x| *x == "/games/Hades").count(), 2);
    }

    #[test]
    fn reads_bubblewrap_versions() {
        assert_eq!(parse_version("bubblewrap 0.11.0\n"), Some((0, 11)));
        assert_eq!(parse_version("bubblewrap 0.8.0"), Some((0, 8)));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[cfg(unix)]
    #[test]
    fn detects_root_entries_and_devices() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("usr")).unwrap();
        std::os::unix::fs::symlink("usr/bin", dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
        assert_eq!(
            read_root(dir.path()),
            vec![
                RootEntry::Symlink("bin".into(), "usr/bin".into()),
                RootEntry::Dir("usr".into()),
            ]
        );

        let dev = tempfile::tempdir().unwrap();
        for node in ["nvidia0", "nvidiactl", "hidraw3", "uinput", "sda"] {
            std::fs::write(dev.path().join(node), "").unwrap();
        }
        std::fs::create_dir(dev.path().join("dri")).unwrap();
        let found = devices(dev.path());
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["dri", "hidraw3", "nvidia0", "nvidiactl"]);
    }

    #[cfg(unix)]
    #[test]
    fn real_paths_are_added() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let paths = with_real_paths([link.clone(), PathBuf::new(), "relative".into()]);
        assert!(paths.contains(&link));
        assert!(paths.contains(&std::fs::canonicalize(&real).unwrap()));
        assert_eq!(paths.len(), 2);
    }

    /// Runs `argv` and returns its status and output.
    async fn run(argv: Vec<String>, env: &[(&str, &str)]) -> (bool, String) {
        let mut argv = argv.into_iter();
        let spec = CommandSpec::new(argv.next().unwrap())
            .args(argv)
            .envs(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let r = irori_core::process::run(&spec, Default::default())
            .await
            .unwrap();
        (r.success(), format!("{}{}", r.stdout, r.stderr))
    }

    fn smoke() -> bool {
        std::env::var_os("IRORI_SANDBOX_SMOKE").is_some()
    }

    #[tokio::test]
    #[ignore = "starts a real sandbox: IRORI_SANDBOX_SMOKE=1"]
    async fn a_native_program_runs_sandboxed() {
        assert!(smoke());
        let game = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let plan = SandboxPlan {
            settings: SandboxSettings {
                no_network: true,
                no_home: true,
            },
            kind: SandboxKind::Native,
            game_dirs: vec![game.path().to_path_buf()],
            tool_dirs: Vec::new(),
            overlay_dirs: Vec::new(),
            home: own.path().join("home"),
            gamescope: false,
        };
        let id = GameId::new(irori_core::Runner::Sideload, "smoke");
        let mut argv = wrapper(&id, plan).await.unwrap();
        let script = r#"
            echo saved > "$GAME/save.txt" || exit 2
            echo mine > "$HOME/own.txt" || exit 3
            test -e "$REPO/Cargo.toml" && exit 4
            test "$(grep -c : /proc/net/dev)" = 1 || exit 5
            test "$IRORI_SANDBOX" = 1 || exit 6
            echo ok"#;
        argv.extend(["/bin/sh".into(), "-c".into(), script.into()]);
        let game_dir = game.path().to_string_lossy().into_owned();
        let (ok, out) = run(
            argv,
            &[("GAME", &game_dir), ("REPO", env!("CARGO_MANIFEST_DIR"))],
        )
        .await;
        assert!(ok, "{out}");
        assert_eq!(out.trim(), "ok");
        assert!(game.path().join("save.txt").is_file());
        assert!(own.path().join("home/own.txt").is_file());
    }

    #[tokio::test]
    #[ignore = "starts Wine in a real sandbox: IRORI_SANDBOX_SMOKE=1"]
    async fn a_wine_program_runs_sandboxed() {
        assert!(smoke());
        let Ok(wine) = which::which("wine") else {
            return;
        };
        let pfx = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let plan = SandboxPlan {
            settings: SandboxSettings {
                no_network: true,
                no_home: true,
            },
            kind: SandboxKind::Wine {
                wineserver: which::which("wineserver").ok(),
            },
            game_dirs: vec![pfx.path().to_path_buf()],
            tool_dirs: Vec::new(),
            overlay_dirs: Vec::new(),
            home: own.path().join("home"),
            gamescope: false,
        };
        let id = GameId::new(irori_core::Runner::Sideload, "smoke-wine");
        let mut argv = wrapper(&id, plan).await.unwrap();
        argv.extend([
            wine.to_string_lossy().into_owned(),
            "cmd".into(),
            "/c".into(),
            "echo hello from wine".into(),
        ]);
        let prefix = pfx.path().to_string_lossy().into_owned();
        let (ok, out) = run(argv, &[("WINEPREFIX", &prefix), ("WINEDEBUG", "-all")]).await;
        assert!(ok, "{out}");
        assert!(out.contains("hello from wine"), "{out}");
        assert!(pfx.path().join("system.reg").is_file());
    }

    #[tokio::test]
    #[ignore = "starts umu in a real sandbox: IRORI_SANDBOX_SMOKE=1 IRORI_SANDBOX_PROTON=<Proton folder>"]
    async fn umu_runs_sandboxed() {
        assert!(smoke());
        let (Some(proton), Ok(umu)) = (
            std::env::var_os("IRORI_SANDBOX_PROTON").map(PathBuf::from),
            which::which("umu-run"),
        ) else {
            return;
        };
        let pfx = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let plan = SandboxPlan {
            settings: SandboxSettings {
                no_network: true,
                no_home: true,
            },
            kind: SandboxKind::PressureVessel,
            game_dirs: vec![pfx.path().to_path_buf()],
            tool_dirs: vec![proton.clone()],
            overlay_dirs: dirs::data_dir()
                .map(|d| d.join("umu"))
                .into_iter()
                .collect(),
            home: own.path().join("home"),
            gamescope: false,
        };
        let id = GameId::new(irori_core::Runner::Sideload, "smoke-umu");
        let mut argv = wrapper(&id, plan).await.unwrap();
        argv.extend([
            umu.to_string_lossy().into_owned(),
            "cmd".into(),
            "/c".into(),
            "echo hello from umu".into(),
        ]);
        let prefix = pfx.path().to_string_lossy().into_owned();
        let proton = proton.to_string_lossy().into_owned();
        let (ok, out) = run(
            argv,
            &[
                ("WINEPREFIX", &prefix),
                ("PROTONPATH", &proton),
                ("GAMEID", "umu-0"),
            ],
        )
        .await;
        assert!(ok, "{out}");
        assert!(out.contains("hello from umu"), "{out}");
    }
}
