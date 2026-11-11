//! Windows' own graphics preference per program (`iroriWindowsGpu`): the
//! value Settings › System › Display › Graphics writes under
//! `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`, named after the
//! program's full path and holding `;`-separated pairs such as
//! `GpuPreference=2;SwapEffectUpgradeEnable=1;`.
//!
//! Irori only ever changes the `GpuPreference` pair of the game's own
//! program and keeps every other pair Windows or the user put there. The
//! first time it touches a program it notes the pair that was there before
//! in `state/gpu-preferences.json`; clearing the game's choice puts that
//! pair back (or removes the value when Irori created it and nothing else
//! is left in it). A pair the user changed afterwards in Windows' settings
//! is theirs: Irori leaves it and forgets the program.
//!
//! The preference is written at launch, because only the launch plan says
//! which program the game starts (stores pick one per launch option, and an
//! update can rename it), and kept afterwards, so starting the program
//! another way gets the same card. Changing or clearing the setting applies
//! at once to the programs already written ([`Backend::reconcile_gpu_preferences`]),
//! so nobody has to start the game to undo the choice; a program that is
//! gone (an uninstalled game) is restored the same way.
//!
//! The registry sits behind [`GpuPreferenceRegistry`], so every rule above is
//! tested on any system; only Windows has a real one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use irori_core::process::CommandSpec;
use irori_core::{GameId, GameSettings, Paths, Runner, json_store};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::Backend;

/// The key Windows reads per-program graphics preferences from.
pub const USER_GPU_PREFERENCES: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
/// The pair Irori owns in a program's value.
const GPU_PREFERENCE: &str = "GpuPreference";

/// What `iroriWindowsGpu` asks Windows for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsGpuPreference {
    /// "Let Windows decide" (`GpuPreference=0`).
    Auto,
    /// The power-saving card, usually the integrated one (`1`).
    PowerSaving,
    /// The high-performance card, usually the discrete one (`2`).
    HighPerformance,
}

impl WindowsGpuPreference {
    /// The setting's value; empty or unknown values leave Windows alone.
    pub fn from_setting(value: &str) -> Option<Self> {
        match value.trim() {
            "auto" => Some(Self::Auto),
            "powerSaving" => Some(Self::PowerSaving),
            "highPerformance" => Some(Self::HighPerformance),
            _ => None,
        }
    }

    /// The number Windows stores after `GpuPreference=`.
    pub fn number(self) -> &'static str {
        match self {
            Self::Auto => "0",
            Self::PowerSaving => "1",
            Self::HighPerformance => "2",
        }
    }
}

/// The `GpuPreference` number in a registry value, if it has one.
pub fn preference_in(value: &str) -> Option<&str> {
    pairs(value).find_map(|(k, v)| k.eq_ignore_ascii_case(GPU_PREFERENCE).then_some(v?))
}

/// `value` with its `GpuPreference` pair set to `number`, or taken out for
/// `None`. Every other pair keeps its place and spelling.
pub fn with_preference(value: &str, number: Option<&str>) -> String {
    let mut out = String::new();
    let mut placed = false;
    for (key, v) in pairs(value) {
        if key.eq_ignore_ascii_case(GPU_PREFERENCE) {
            if let (Some(n), false) = (number, placed) {
                out.push_str(&format!("{GPU_PREFERENCE}={n};"));
                placed = true;
            }
            continue;
        }
        match v {
            Some(v) => out.push_str(&format!("{key}={v};")),
            None => out.push_str(&format!("{key};")),
        }
    }
    if let (Some(n), false) = (number, placed) {
        // Windows' own settings put it first.
        out.insert_str(0, &format!("{GPU_PREFERENCE}={n};"));
    }
    out
}

/// The `key=value` pairs of a value (a bare `key` has no value), skipping
/// empty ones (`;;`).
fn pairs(value: &str) -> impl Iterator<Item = (&str, Option<&str>)> {
    value
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (p, None),
        })
}

/// Where per-program graphics preferences are kept: the registry on
/// Windows, a map in tests.
pub trait GpuPreferenceRegistry: Send + Sync {
    /// The value named after `program`, `None` when there is none.
    fn read(&self, program: &str) -> Result<Option<String>>;
    fn write(&self, program: &str, value: &str) -> Result<()>;
    fn remove(&self, program: &str) -> Result<()>;
}

/// The registry of this computer; `None` where Windows' preference doesn't
/// exist.
pub fn system_registry() -> Option<Arc<dyn GpuPreferenceRegistry>> {
    #[cfg(windows)]
    {
        Some(Arc::new(windows::UserGpuPreferences))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// What Irori wrote, per program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

/// One program whose preference Irori set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub runner: Runner,
    pub app_name: String,
    /// Full path, the registry value's name.
    pub program: String,
    /// The `GpuPreference` number before Irori; `None` when there was none.
    #[serde(default)]
    pub previous: Option<String>,
    /// The number Irori wrote.
    pub written: String,
}

impl Entry {
    fn game(&self) -> GameId {
        GameId::new(self.runner, self.app_name.as_str())
    }

    fn is(&self, program: &str) -> bool {
        same_path(&self.program, program)
    }
}

/// Windows paths ignore letter case.
fn same_path(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

impl Journal {
    /// Set `preference` for `program`, which `game` starts. Other programs
    /// Irori set for the game before (an older executable) are restored.
    pub fn apply(
        &mut self,
        registry: &dyn GpuPreferenceRegistry,
        game: &GameId,
        program: &str,
        preference: WindowsGpuPreference,
    ) -> Result<()> {
        self.restore_where(registry, |e| e.game() == *game && !e.is(program))?;
        let current = registry.read(program)?;
        let number = preference.number();
        let wanted = with_preference(current.as_deref().unwrap_or(""), Some(number));
        if current.as_deref() != Some(wanted.as_str()) {
            registry.write(program, &wanted)?;
        }
        match self.entries.iter_mut().find(|e| e.is(program)) {
            // The pair from before Irori stays what clearing puts back.
            Some(entry) => {
                entry.written = number.to_string();
                entry.runner = game.runner;
                entry.app_name = game.app_name.to_string();
            }
            None => self.entries.push(Entry {
                runner: game.runner,
                app_name: game.app_name.to_string(),
                program: program.to_string(),
                previous: current
                    .as_deref()
                    .and_then(preference_in)
                    .map(str::to_string),
                written: number.to_string(),
            }),
        }
        Ok(())
    }

    /// Restore every program of `game`.
    pub fn restore_game(
        &mut self,
        registry: &dyn GpuPreferenceRegistry,
        game: &GameId,
    ) -> Result<()> {
        self.restore_where(registry, |e| e.game() == *game)
    }

    /// Bring the registry in line with the settings: programs whose game no
    /// longer asks for a preference (`wanted` says `None`) or that are gone
    /// (`exists` says no) are restored; a changed preference is written.
    pub fn reconcile(
        &mut self,
        registry: &dyn GpuPreferenceRegistry,
        wanted: impl Fn(&GameId) -> Option<WindowsGpuPreference>,
        exists: impl Fn(&str) -> bool,
    ) -> Result<()> {
        let mut first_error = None;
        let mut i = 0;
        while i < self.entries.len() {
            let entry = self.entries[i].clone();
            let result = match wanted(&entry.game()).filter(|_| exists(&entry.program)) {
                None => restore(registry, &entry).map(|()| None),
                Some(p) if p.number() == entry.written => Ok(Some(entry)),
                Some(p) => rewrite(registry, &entry, p).map(Some),
            };
            match result {
                Ok(Some(kept)) => {
                    self.entries[i] = kept;
                    i += 1;
                }
                Ok(None) => {
                    self.entries.remove(i);
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                    i += 1;
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Restore the entries matching `which` and forget them; one that
    /// couldn't be restored stays for the next try.
    fn restore_where(
        &mut self,
        registry: &dyn GpuPreferenceRegistry,
        which: impl Fn(&Entry) -> bool,
    ) -> Result<()> {
        let mut first_error = None;
        self.entries.retain(|e| {
            if !which(e) {
                return true;
            }
            match restore(registry, e) {
                Ok(()) => false,
                Err(err) => {
                    first_error.get_or_insert(err);
                    true
                }
            }
        });
        first_error.map_or(Ok(()), Err)
    }
}

/// Put back the pair from before Irori, unless the user changed it since.
fn restore(registry: &dyn GpuPreferenceRegistry, entry: &Entry) -> Result<()> {
    let Some(current) = registry.read(&entry.program)? else {
        return Ok(());
    };
    if preference_in(&current) != Some(entry.written.as_str()) {
        return Ok(());
    }
    let restored = with_preference(&current, entry.previous.as_deref());
    if restored.is_empty() {
        registry.remove(&entry.program)
    } else {
        registry.write(&entry.program, &restored)
    }
}

/// Write another preference for a program Irori set before.
fn rewrite(
    registry: &dyn GpuPreferenceRegistry,
    entry: &Entry,
    preference: WindowsGpuPreference,
) -> Result<Entry> {
    let current = registry.read(&entry.program)?.unwrap_or_default();
    // The user chose something else in Windows meanwhile: theirs wins until
    // the game is started again.
    if preference_in(&current).is_some_and(|n| n != entry.written) {
        return Ok(entry.clone());
    }
    registry.write(
        &entry.program,
        &with_preference(&current, Some(preference.number())),
    )?;
    Ok(Entry {
        written: preference.number().to_string(),
        ..entry.clone()
    })
}

/// The game's own program in a launch command: the command's program, else
/// the first argument (after wrappers) that is an `.exe` inside the install
/// folder. A program outside it (a store client, `cmd`, a wrapper) is never
/// the game's to set.
pub fn game_program(command: &CommandSpec, install_path: &str) -> Option<String> {
    let folder = normalize(install_path);
    let folder = folder.trim_end_matches('\\');
    if folder.is_empty() {
        return None;
    }
    let candidates =
        std::iter::once(command.program.clone()).chain(command.args.iter().map(PathBuf::from));
    candidates
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        })
        .map(|p| absolute(&p, command.cwd.as_deref(), install_path))
        .find(|p| {
            normalize(p)
                .strip_prefix(folder)
                .is_some_and(|rest| rest.starts_with('\\'))
        })
}

/// A relative program is relative to the command's folder, else the game's.
fn absolute(path: &Path, cwd: Option<&Path>, install_path: &str) -> String {
    let text = path.to_string_lossy();
    let rooted = text.starts_with(['\\', '/']) || text.get(1..2) == Some(":");
    if rooted {
        return text.into_owned();
    }
    let base = cwd
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| install_path.to_string());
    format!("{}\\{}", base.trim_end_matches(['\\', '/']), text)
}

/// Lower case with Windows' separators, for comparing paths.
fn normalize(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// The journal file; one writer at a time.
static JOURNAL: Mutex<()> = Mutex::new(());

fn journal_path() -> PathBuf {
    Paths::get().state_dir().join("gpu-preferences.json")
}

/// Read the journal, change it and save it if it changed. Blocking.
fn with_journal(f: impl FnOnce(&mut Journal) -> Result<()>) -> Result<()> {
    let _lock = JOURNAL.lock();
    let path = journal_path();
    let mut journal: Journal = json_store::read_or_default(&path);
    let before = journal.clone();
    let result = f(&mut journal);
    if journal != before {
        json_store::write(&path, &journal)?;
    }
    result
}

impl Backend {
    /// Before a launch: set Windows' graphics preference of the program the
    /// game starts, or restore what Irori set when the game no longer asks
    /// for one. A failure is logged and never stops the game.
    pub(crate) async fn apply_gpu_preference(
        &self,
        id: &GameId,
        install_path: &str,
        settings: &GameSettings,
        command: &CommandSpec,
    ) -> Option<String> {
        let registry = system_registry()?;
        let preference = WindowsGpuPreference::from_setting(&settings.windows_gpu);
        let program = game_program(command, install_path);
        let id = id.clone();
        let result = self
            .spawn_blocking(move || {
                with_journal(|journal| match (preference, &program) {
                    (Some(p), Some(program)) => journal.apply(&*registry, &id, program, p),
                    (Some(_), None) => Ok(()),
                    (None, _) => journal.restore_game(&*registry, &id),
                })
                .map(|()| preference.zip(program))
            })
            .await;
        match result {
            Ok(Ok(Some((p, program)))) => Some(format!(
                "Windows graphics preference: GpuPreference={} for {program}\n",
                p.number()
            )),
            Ok(Ok(None)) if preference.is_some() => {
                Some("Windows graphics preference: the game's program wasn't found in its install folder; left to Windows\n".into())
            }
            Ok(Ok(None)) => None,
            Ok(Err(e)) => {
                tracing::warn!("Windows graphics preference: {e:#}");
                Some(format!("Windows graphics preference not set: {e:#}\n"))
            }
            Err(e) => {
                tracing::warn!("Windows graphics preference: {e}");
                None
            }
        }
    }

    /// After the settings changed: write the new preference to the programs
    /// Irori set before, and restore those whose game no longer asks for one
    /// or whose program is gone.
    pub(crate) async fn reconcile_gpu_preferences(&self) {
        let Some(registry) = system_registry() else {
            return;
        };
        let settings = self.settings.clone();
        let result = self
            .spawn_blocking(move || {
                with_journal(|journal| {
                    if journal.entries.is_empty() {
                        return Ok(());
                    }
                    journal.reconcile(
                        &*registry,
                        |id| WindowsGpuPreference::from_setting(&settings.game(id).windows_gpu),
                        |program| Path::new(program).is_file(),
                    )
                })
            })
            .await;
        match result {
            Ok(Err(e)) => tracing::warn!("Windows graphics preferences: {e:#}"),
            Err(e) => tracing::warn!("Windows graphics preferences: {e}"),
            Ok(Ok(())) => {}
        }
    }
}

#[cfg(windows)]
mod windows {
    //! The real registry, through `windows-registry`.

    use anyhow::{Result, anyhow};
    use windows_registry::CURRENT_USER;

    use super::{GpuPreferenceRegistry, USER_GPU_PREFERENCES};

    /// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`: no such key or value.
    const NOT_FOUND: u32 = 0x8007_0002;

    pub struct UserGpuPreferences;

    impl GpuPreferenceRegistry for UserGpuPreferences {
        fn read(&self, program: &str) -> Result<Option<String>> {
            let key = match CURRENT_USER.open(USER_GPU_PREFERENCES) {
                Ok(key) => key,
                Err(e) if e.code().0 as u32 == NOT_FOUND => return Ok(None),
                Err(e) => return Err(anyhow!("opening the graphics preferences: {e}")),
            };
            match key.get_string(program) {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.code().0 as u32 == NOT_FOUND => Ok(None),
                Err(e) => Err(anyhow!("reading the graphics preference of {program}: {e}")),
            }
        }

        fn write(&self, program: &str, value: &str) -> Result<()> {
            CURRENT_USER
                .create(USER_GPU_PREFERENCES)
                .and_then(|key| key.set_string(program, value))
                .map_err(|e| anyhow!("writing the graphics preference of {program}: {e}"))
        }

        fn remove(&self, program: &str) -> Result<()> {
            let key = match CURRENT_USER.open(USER_GPU_PREFERENCES) {
                Ok(key) => key,
                Err(e) if e.code().0 as u32 == NOT_FOUND => return Ok(()),
                Err(e) => return Err(anyhow!("opening the graphics preferences: {e}")),
            };
            match key.remove_value(program) {
                Ok(()) => Ok(()),
                Err(e) if e.code().0 as u32 == NOT_FOUND => Ok(()),
                Err(e) => Err(anyhow!(
                    "removing the graphics preference of {program}: {e}"
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A registry key in memory, names compared without case like Windows.
    #[derive(Default)]
    struct Memory {
        values: Mutex<BTreeMap<String, String>>,
        fail: Mutex<bool>,
    }

    impl Memory {
        fn with(values: &[(&str, &str)]) -> Self {
            let m = Self::default();
            for (k, v) in values {
                m.values.lock().insert(k.to_lowercase(), v.to_string());
            }
            m
        }
        fn get(&self, program: &str) -> Option<String> {
            self.values.lock().get(&program.to_lowercase()).cloned()
        }
        fn set(&self, program: &str, value: &str) {
            self.values
                .lock()
                .insert(program.to_lowercase(), value.to_string());
        }
    }

    impl GpuPreferenceRegistry for Memory {
        fn read(&self, program: &str) -> Result<Option<String>> {
            if *self.fail.lock() {
                anyhow::bail!("access denied");
            }
            Ok(self.get(program))
        }
        fn write(&self, program: &str, value: &str) -> Result<()> {
            if *self.fail.lock() {
                anyhow::bail!("access denied");
            }
            self.set(program, value);
            Ok(())
        }
        fn remove(&self, program: &str) -> Result<()> {
            self.values.lock().remove(&program.to_lowercase());
            Ok(())
        }
    }

    const HADES: &str = r"C:\Games\Hades\x64\Hades.exe";

    fn hades() -> GameId {
        GameId::new(Runner::Legendary, "Min")
    }

    #[test]
    fn values_keep_the_other_pairs() {
        assert_eq!(preference_in("GpuPreference=2;"), Some("2"));
        assert_eq!(
            preference_in("SwapEffectUpgradeEnable=1;gpupreference=1;"),
            Some("1")
        );
        assert_eq!(preference_in("SwapEffectUpgradeEnable=1;"), None);
        assert_eq!(with_preference("", Some("2")), "GpuPreference=2;");
        assert_eq!(
            with_preference("SwapEffectUpgradeEnable=1;AutoHDREnable=2097;", Some("1")),
            "GpuPreference=1;SwapEffectUpgradeEnable=1;AutoHDREnable=2097;"
        );
        assert_eq!(
            with_preference(
                "VRROptimizeEnable=0;GpuPreference=1;SwapEffectUpgradeEnable=1;",
                Some("2")
            ),
            "VRROptimizeEnable=0;GpuPreference=2;SwapEffectUpgradeEnable=1;"
        );
        assert_eq!(
            with_preference("GpuPreference=2;SwapEffectUpgradeEnable=1;", None),
            "SwapEffectUpgradeEnable=1;"
        );
        assert_eq!(with_preference("GpuPreference=2;", None), "");
        // Without the closing `;` Windows also accepts.
        assert_eq!(with_preference("GpuPreference=2;;Odd", None), "Odd;");
    }

    #[test]
    fn settings_values() {
        assert_eq!(
            WindowsGpuPreference::from_setting("highPerformance"),
            Some(WindowsGpuPreference::HighPerformance)
        );
        assert_eq!(
            WindowsGpuPreference::from_setting("auto").unwrap().number(),
            "0"
        );
        assert_eq!(
            WindowsGpuPreference::from_setting("powerSaving")
                .unwrap()
                .number(),
            "1"
        );
        // Empty, or written by a newer Irori: Windows is left alone.
        assert_eq!(WindowsGpuPreference::from_setting(""), None);
        assert_eq!(WindowsGpuPreference::from_setting("npu"), None);
    }

    #[test]
    fn a_value_irori_created_is_removed_when_cleared() {
        let reg = Memory::default();
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::HighPerformance)
            .unwrap();
        assert_eq!(reg.get(HADES).as_deref(), Some("GpuPreference=2;"));
        assert_eq!(journal.entries[0].previous, None);
        journal.restore_game(&reg, &hades()).unwrap();
        assert_eq!(reg.get(HADES), None);
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn the_previous_choice_comes_back() {
        let reg = Memory::with(&[(HADES, "GpuPreference=1;SwapEffectUpgradeEnable=1;")]);
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::HighPerformance)
            .unwrap();
        assert_eq!(
            reg.get(HADES).as_deref(),
            Some("GpuPreference=2;SwapEffectUpgradeEnable=1;")
        );
        // A second launch with another choice keeps what was there first.
        journal
            .apply(
                &reg,
                &hades(),
                &HADES.to_uppercase(),
                WindowsGpuPreference::Auto,
            )
            .unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].previous.as_deref(), Some("1"));
        assert_eq!(journal.entries[0].written, "0");
        // Windows added a pair meanwhile; it stays.
        reg.set(
            HADES,
            "GpuPreference=0;SwapEffectUpgradeEnable=1;VRROptimizeEnable=1;",
        );
        journal.restore_game(&reg, &hades()).unwrap();
        assert_eq!(
            reg.get(HADES).as_deref(),
            Some("GpuPreference=1;SwapEffectUpgradeEnable=1;VRROptimizeEnable=1;")
        );
    }

    #[test]
    fn other_pairs_survive_removal() {
        let reg = Memory::with(&[(HADES, "SwapEffectUpgradeEnable=1;")]);
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::PowerSaving)
            .unwrap();
        assert_eq!(
            reg.get(HADES).as_deref(),
            Some("GpuPreference=1;SwapEffectUpgradeEnable=1;")
        );
        journal.restore_game(&reg, &hades()).unwrap();
        assert_eq!(
            reg.get(HADES).as_deref(),
            Some("SwapEffectUpgradeEnable=1;")
        );
    }

    #[test]
    fn the_users_own_change_wins() {
        let reg = Memory::default();
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::HighPerformance)
            .unwrap();
        reg.set(HADES, "GpuPreference=1;");
        journal.restore_game(&reg, &hades()).unwrap();
        assert_eq!(reg.get(HADES).as_deref(), Some("GpuPreference=1;"));
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn only_the_games_programs_are_touched() {
        let other = r"C:\Games\Other\Other.exe";
        let reg = Memory::with(&[(other, "GpuPreference=2;")]);
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::PowerSaving)
            .unwrap();
        // The game now starts another program: the old one is restored.
        let new = r"C:\Games\Hades\x86\Hades.exe";
        journal
            .apply(&reg, &hades(), new, WindowsGpuPreference::PowerSaving)
            .unwrap();
        assert_eq!(reg.get(HADES), None);
        assert_eq!(reg.get(new).as_deref(), Some("GpuPreference=1;"));
        assert_eq!(reg.get(other).as_deref(), Some("GpuPreference=2;"));
        assert_eq!(journal.entries.len(), 1);
        journal
            .restore_game(&reg, &GameId::new(Runner::Gog, "1"))
            .unwrap();
        assert_eq!(journal.entries.len(), 1);
    }

    #[test]
    fn reconcile_follows_the_settings() {
        let reg = Memory::default();
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::HighPerformance)
            .unwrap();
        let celeste = GameId::new(Runner::Gog, "1");
        let celeste_exe = r"D:\Celeste\Celeste.exe";
        journal
            .apply(
                &reg,
                &celeste,
                celeste_exe,
                WindowsGpuPreference::PowerSaving,
            )
            .unwrap();

        // Hades changed to "auto", Celeste cleared.
        journal
            .reconcile(
                &reg,
                |id| (*id == hades()).then_some(WindowsGpuPreference::Auto),
                |_| true,
            )
            .unwrap();
        assert_eq!(reg.get(HADES).as_deref(), Some("GpuPreference=0;"));
        assert_eq!(reg.get(celeste_exe), None);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].written, "0");

        // Uninstalled: the program is gone.
        journal
            .reconcile(&reg, |_| Some(WindowsGpuPreference::Auto), |_| false)
            .unwrap();
        assert_eq!(reg.get(HADES), None);
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn a_failed_restore_is_tried_again() {
        let reg = Memory::default();
        let mut journal = Journal::default();
        journal
            .apply(&reg, &hades(), HADES, WindowsGpuPreference::HighPerformance)
            .unwrap();
        *reg.fail.lock() = true;
        assert!(journal.restore_game(&reg, &hades()).is_err());
        assert_eq!(journal.entries.len(), 1);
        *reg.fail.lock() = false;
        journal.reconcile(&reg, |_| None, |_| true).unwrap();
        assert_eq!(reg.get(HADES), None);
        assert!(journal.entries.is_empty());
    }

    #[test]
    fn journal_reads_files_from_newer_versions() {
        let journal: Journal = serde_json::from_str(
            r#"{"entries": [{"runner": "legendary", "appName": "Min",
                "program": "C:\\Games\\Hades.exe", "written": "2", "extra": 1}],
                "future": true}"#,
        )
        .unwrap();
        assert_eq!(journal.entries[0].previous, None);
        assert_eq!(journal.entries[0].game(), hades());
        let back: Journal =
            serde_json::from_value(serde_json::to_value(&journal).unwrap()).unwrap();
        assert_eq!(back, journal);
    }

    #[test]
    fn the_program_comes_from_the_launch_command() {
        let folder = r"C:\Games\Hades";
        let direct = CommandSpec::new(HADES);
        assert_eq!(game_program(&direct, folder).as_deref(), Some(HADES));
        // Folder spelled differently, with a trailing separator.
        assert_eq!(
            game_program(&direct, r"c:/games/hades/").as_deref(),
            Some(HADES)
        );
        // A wrapper in front of the game.
        let wrapped = CommandSpec::new(r"C:\Tools\wrapper.exe").args([HADES, "-dx12"]);
        assert_eq!(game_program(&wrapped, folder).as_deref(), Some(HADES));
        // Relative to the command's folder.
        let relative = CommandSpec::new(r"x64\Hades.exe").cwd(folder);
        assert_eq!(
            game_program(&relative, folder).as_deref(),
            Some(r"C:\Games\Hades\x64\Hades.exe")
        );
        // A store client or a sibling folder is never the game.
        assert_eq!(
            game_program(&CommandSpec::new(r"C:\Steam\steam.exe"), folder),
            None
        );
        assert_eq!(
            game_program(&CommandSpec::new(r"C:\Games\Hades2\Hades.exe"), folder),
            None
        );
        assert_eq!(
            game_program(&CommandSpec::new(r"C:\Games\Hades\start.bat"), folder),
            None
        );
        assert_eq!(game_program(&direct, ""), None);
    }
}
