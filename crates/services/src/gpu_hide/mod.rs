//! Hiding a graphics card from a game while it runs (`iroriHideGpu`,
//! Windows only): for games that pick the wrong card and offer no choice.
//!
//! Windows has no per-program way to hide a card, so the card is disabled in
//! Device Manager's sense for the session and enabled again when it ends.
//! That changes the system, so the rules are strict:
//!
//! * Only a card the user named for this game, and never one driving a
//!   display, nor one Windows can't say that of, nor the last one enabled
//!   ([`plan`]). A card the user disabled themselves is never touched.
//! * Disabling needs an administrator: Windows' own permission prompt
//!   starts a small elevated copy of Irori (`irori --gpu-helper …`) that
//!   disables the card, waits, and enables it again. No service, no stored
//!   credentials. The card is disabled until the next restart at most, never
//!   for good.
//! * The card comes back when the game exits, when its launch fails or is
//!   cancelled ([`HiddenGpu`] enables it when dropped), and when Irori itself
//!   goes away: the helper also waits on Irori's process. Should the helper
//!   be killed too, `state/gpu-hidden.json` records every card before it is
//!   disabled, and the next start enables what is still disabled
//!   ([`recover`]), asking for permission again.
//!
//! The device calls sit behind [`GpuDevices`], so every rule is tested on any
//! system; only Windows has real ones (`windows.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{GameId, Paths, json_store, util};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::helper_main;

/// The argument that makes Irori run as the elevated helper (Windows).
pub const HELPER_FLAG: &str = "--gpu-helper";

/// A display adapter as Windows lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    /// PnP instance id, what `iroriHideGpu` stores.
    pub instance_id: String,
    /// The driver's description ("NVIDIA GeForce RTX 3060").
    pub name: String,
    /// Disabled in Device Manager (problem code 22).
    pub disabled: bool,
    /// Shows a display of the desktop; `None` when Windows didn't say.
    pub drives_display: Option<bool>,
}

/// Why a card isn't hidden at a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No adapter has the id (removed, or another computer's setting).
    NotFound,
    /// The user disabled it themselves; Irori leaves it as it is.
    AlreadyDisabled,
    /// It shows a display: hiding it would turn a screen off.
    DrivesDisplay,
    /// Windows didn't say which cards show the displays.
    DisplaysUnknown,
    /// No other card would be left enabled.
    LastAdapter,
    /// The id holds characters no PnP instance id has.
    InvalidId,
}

impl Refusal {
    /// For the game's log, in English.
    pub fn explain(self) -> &'static str {
        match self {
            Refusal::NotFound => "no graphics card with that id is installed",
            Refusal::AlreadyDisabled => "it is already disabled, so Irori leaves it alone",
            Refusal::DrivesDisplay => "it shows a display, which would go dark",
            Refusal::DisplaysUnknown => "Windows didn't say which cards show the displays",
            Refusal::LastAdapter => "no other graphics card would be left",
            Refusal::InvalidId => "the setting doesn't hold a device id",
        }
    }
}

/// What a launch does about `iroriHideGpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The setting is empty.
    Nothing,
    Hide(Adapter),
    Refuse(Refusal),
}

/// Whether `id` can be a PnP instance id: the characters Windows uses in
/// them, and nothing a command line would read as a quote or a separator,
/// since it is handed to the elevated helper.
pub fn valid_instance_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id.contains('\\')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "\\&_-.{}#".contains(c))
}

/// What to do about the card `setting` names, given the adapters Windows
/// lists. Instance ids compare without letter case, as Windows does.
pub fn plan(setting: &str, adapters: &[Adapter]) -> Plan {
    let wanted = setting.trim();
    if wanted.is_empty() {
        return Plan::Nothing;
    }
    if !valid_instance_id(wanted) {
        return Plan::Refuse(Refusal::InvalidId);
    }
    let Some(card) = adapters
        .iter()
        .find(|a| a.instance_id.eq_ignore_ascii_case(wanted))
    else {
        return Plan::Refuse(Refusal::NotFound);
    };
    if card.disabled {
        return Plan::Refuse(Refusal::AlreadyDisabled);
    }
    match card.drives_display {
        Some(true) => return Plan::Refuse(Refusal::DrivesDisplay),
        None => return Plan::Refuse(Refusal::DisplaysUnknown),
        Some(false) => {}
    }
    let others = adapters
        .iter()
        .filter(|a| !a.disabled && !a.instance_id.eq_ignore_ascii_case(wanted))
        .count();
    if others == 0 {
        return Plan::Refuse(Refusal::LastAdapter);
    }
    Plan::Hide(card.clone())
}

/// Which displays' adapters a hardware id from `EnumDisplayDevices`
/// (`PCI\VEN_10DE&DEV_2484&SUBSYS_…&REV_A1`) covers: the instance ids that
/// start with it. Two identical cards share one, so both count as showing
/// the display, which keeps the rule on the safe side.
pub fn shows_display(instance_id: &str, display_hardware_ids: &[String]) -> bool {
    let id = instance_id.to_ascii_uppercase();
    display_hardware_ids.iter().any(|hw| {
        let hw = hw.trim().to_ascii_uppercase();
        !hw.is_empty() && (id == hw || id.starts_with(&format!("{hw}\\")))
    })
}

/// The system's graphics cards and the power to disable one for a while.
pub trait GpuDevices: Send + Sync {
    /// Every present display adapter.
    fn adapters(&self) -> Result<Vec<Adapter>>;
    /// Disable `instance_id` until the returned hold is released. Asks
    /// Windows for permission; blocks until the card is disabled.
    fn hide(&self, instance_id: &str) -> Result<Box<dyn Held>>;
    /// Enable a card a session left disabled. Asks for permission.
    fn enable(&self, instance_id: &str) -> Result<()>;
}

/// A card disabled for a session.
pub trait Held: Send {
    /// Enable the card again; blocks until it is.
    fn release(self: Box<Self>) -> Result<()>;
}

/// The system's devices: Windows only.
pub fn system_devices() -> Option<Arc<dyn GpuDevices>> {
    #[cfg(windows)]
    {
        Some(Arc::new(windows::SystemDevices))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Cards disabled for a session and not yet known to be enabled again:
/// `state/gpu-hidden.json`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Journal {
    pub pending: Vec<Pending>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub instance_id: String,
    /// The game's key, for the log.
    #[serde(default)]
    pub game: String,
    /// Unix seconds.
    #[serde(default)]
    pub since: u64,
}

fn journal_path() -> PathBuf {
    Paths::get().state_dir().join("gpu-hidden.json")
}

/// One writer at a time, so a session ending while another starts never
/// loses an entry.
static JOURNAL: Mutex<()> = Mutex::new(());

fn update_journal(path: &Path, f: impl FnOnce(&mut Journal)) -> Result<()> {
    let _lock = JOURNAL.lock();
    let mut journal: Journal = json_store::read_or_default(path);
    f(&mut journal);
    json_store::write(path, &journal).context("Could not save the hidden graphics card")
}

fn forget(path: &Path, instance_id: &str) -> Result<()> {
    update_journal(path, |j| {
        j.pending
            .retain(|p| !p.instance_id.eq_ignore_ascii_case(instance_id))
    })
}

/// A card hidden for a session. Enabled again by [`HiddenGpu::end`] or,
/// for every path that leaves the launch early, when dropped.
pub struct HiddenGpu {
    held: Option<Box<dyn Held>>,
    instance_id: String,
    journal: PathBuf,
}

impl HiddenGpu {
    /// Enable the card again. Blocking. The journal keeps the card when
    /// that fails, so the next start tries again.
    pub fn end(mut self) -> Result<()> {
        self.release()
    }

    fn release(&mut self) -> Result<()> {
        let Some(held) = self.held.take() else {
            return Ok(());
        };
        held.release()
            .with_context(|| format!("Could not enable {} again", self.instance_id))?;
        forget(&self.journal, &self.instance_id)
    }
}

impl Drop for HiddenGpu {
    fn drop(&mut self) {
        if self.held.is_none() {
            return;
        }
        // Dropped on an async path that left the launch: the release waits
        // on the helper, so it runs off the runtime.
        let mut this = HiddenGpu {
            held: self.held.take(),
            instance_id: std::mem::take(&mut self.instance_id),
            journal: std::mem::take(&mut self.journal),
        };
        std::thread::spawn(move || {
            if let Err(e) = this.release() {
                tracing::warn!("hidden graphics card: {e:#}");
            }
        });
    }
}

/// The result of [`hide`]: the hold, if any, and a line for the game's log.
pub struct Hidden {
    pub held: Option<HiddenGpu>,
    pub note: Option<String>,
}

/// Hide the card `setting` names for `game`'s session, when [`plan`] allows.
/// Blocking: it waits for the user to answer Windows' prompt. Never fails the
/// launch; what went wrong goes to the log.
pub fn hide(devices: &dyn GpuDevices, journal: &Path, game: &GameId, setting: &str) -> Hidden {
    let none = |note: String| Hidden {
        held: None,
        note: Some(note),
    };
    let adapters = match devices.adapters() {
        Ok(adapters) => adapters,
        Err(e) => return none(format!("Graphics card not hidden: {e:#}\n")),
    };
    let card = match plan(setting, &adapters) {
        Plan::Nothing => {
            return Hidden {
                held: None,
                note: None,
            };
        }
        Plan::Refuse(why) => {
            return none(format!(
                "Graphics card {} not hidden: {}\n",
                setting.trim(),
                why.explain()
            ));
        }
        Plan::Hide(card) => card,
    };
    // Written before the card is touched, so a crash at any later point
    // still brings it back at the next start.
    if let Err(e) = update_journal(journal, |j| {
        j.pending
            .retain(|p| !p.instance_id.eq_ignore_ascii_case(&card.instance_id));
        j.pending.push(Pending {
            instance_id: card.instance_id.clone(),
            game: game.key(),
            since: util::unix_now(),
        });
    }) {
        return none(format!("Graphics card not hidden: {e:#}\n"));
    }
    match devices.hide(&card.instance_id) {
        Ok(held) => Hidden {
            held: Some(HiddenGpu {
                held: Some(held),
                instance_id: card.instance_id.clone(),
                journal: journal.to_path_buf(),
            }),
            note: Some(format!(
                "Graphics card hidden from the game while it runs: {} ({})\n",
                card.name, card.instance_id
            )),
        },
        Err(e) => {
            // The helper may have stopped after disabling it: keep the entry
            // only while the card is still disabled.
            let still_disabled = devices.adapters().is_ok_and(|all| {
                all.iter()
                    .any(|a| a.instance_id.eq_ignore_ascii_case(&card.instance_id) && a.disabled)
            });
            if !still_disabled && let Err(e) = forget(journal, &card.instance_id) {
                tracing::warn!("hidden graphics card: {e:#}");
            }
            none(format!("Graphics card {} not hidden: {e:#}\n", card.name))
        }
    }
}

/// At start: enable the cards a session left disabled. Cards enabled since,
/// or gone, are only forgotten. Returns the ids that are still disabled.
pub fn recover(devices: &dyn GpuDevices, journal: &Path) -> Vec<String> {
    let pending = {
        let _lock = JOURNAL.lock();
        json_store::read_or_default::<Journal>(journal).pending
    };
    if pending.is_empty() {
        return Vec::new();
    }
    let adapters = match devices.adapters() {
        Ok(adapters) => adapters,
        Err(e) => {
            tracing::warn!("hidden graphics cards: {e:#}");
            return pending.into_iter().map(|p| p.instance_id).collect();
        }
    };
    let mut left = Vec::new();
    for entry in pending {
        let disabled = adapters
            .iter()
            .any(|a| a.instance_id.eq_ignore_ascii_case(&entry.instance_id) && a.disabled);
        if disabled {
            tracing::info!(
                "enabling {} again, which a session of {} left disabled",
                entry.instance_id,
                entry.game
            );
            if let Err(e) = devices.enable(&entry.instance_id) {
                tracing::warn!("enabling {} again: {e:#}", entry.instance_id);
                left.push(entry.instance_id);
                continue;
            }
        }
        if let Err(e) = forget(journal, &entry.instance_id) {
            tracing::warn!("hidden graphics cards: {e:#}");
        }
    }
    left
}

impl crate::Backend {
    /// The graphics cards a game can hide, for its settings. Empty outside
    /// Windows.
    pub async fn hideable_gpus(&self) -> Result<Vec<Adapter>> {
        let Some(devices) = system_devices() else {
            return Ok(Vec::new());
        };
        self.spawn_blocking(move || devices.adapters()).await?
    }

    /// Before a launch: hide the card the game's settings name.
    pub(crate) async fn hide_gpu_for_launch(&self, id: &GameId, setting: &str) -> Option<Hidden> {
        let devices = system_devices()?;
        if setting.trim().is_empty() {
            return None;
        }
        let (id, setting) = (id.clone(), setting.to_string());
        self.spawn_blocking(move || hide(&*devices, &journal_path(), &id, &setting))
            .await
            .ok()
    }

    /// At start: enable what a session left disabled, and say so when it
    /// can't.
    pub(crate) async fn recover_hidden_gpus(&self) {
        let Some(devices) = system_devices() else {
            return;
        };
        let journal = journal_path();
        if !journal.exists() {
            return;
        }
        let left = self
            .spawn_blocking(move || recover(&*devices, &journal))
            .await
            .unwrap_or_default();
        if !left.is_empty() {
            let cards = left.join(", ");
            self.bus.startup_notice(
                irori_core::Notice::new(
                    irori_core::NoticeLevel::Warning,
                    "A graphics card is still turned off",
                )
                .message(format!(
                    "Irori hid {cards} from a game and couldn’t turn it back on. Turn it on in \
                     Device Manager, or restart Windows."
                ))
                .keys(
                    "shell.notice.gpu_still_hidden",
                    Some("shell.notice.gpu_still_hidden_message"),
                    &[("cards", &cards)],
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn card(id: &str, disabled: bool, display: Option<bool>) -> Adapter {
        Adapter {
            instance_id: id.into(),
            name: format!("card {id}"),
            disabled,
            drives_display: display,
        }
    }

    const IGPU: &str = r"PCI\VEN_8086&DEV_3E92&SUBSYS_86941043&REV_00\3&11583659&0&10";
    const DGPU: &str = r"PCI\VEN_10DE&DEV_2484&SUBSYS_146B10DE&REV_A1\4&2A3F5C1&0&0008";

    #[test]
    fn plans_keep_to_the_safe_cards() {
        let both = [
            card(IGPU, false, Some(false)),
            card(DGPU, false, Some(true)),
        ];
        assert_eq!(plan("", &both), Plan::Nothing);
        assert_eq!(plan("  ", &both), Plan::Nothing);
        assert_eq!(plan(IGPU, &both), Plan::Hide(both[0].clone()));
        // Letter case doesn't matter to Windows.
        assert_eq!(
            plan(&IGPU.to_lowercase(), &both),
            Plan::Hide(both[0].clone())
        );
        assert_eq!(plan(DGPU, &both), Plan::Refuse(Refusal::DrivesDisplay));
        assert_eq!(
            plan(r"PCI\VEN_1002&DEV_73BF\1", &both),
            Plan::Refuse(Refusal::NotFound)
        );
        assert_eq!(
            plan(&format!("{IGPU}\" --other"), &both),
            Plan::Refuse(Refusal::InvalidId)
        );
        let user_disabled = [card(IGPU, true, Some(false)), card(DGPU, false, Some(true))];
        assert_eq!(
            plan(IGPU, &user_disabled),
            Plan::Refuse(Refusal::AlreadyDisabled)
        );
        let unknown = [card(IGPU, false, None), card(DGPU, false, Some(true))];
        assert_eq!(plan(IGPU, &unknown), Plan::Refuse(Refusal::DisplaysUnknown));
        let alone = [
            card(IGPU, false, Some(false)),
            card(DGPU, true, Some(false)),
        ];
        assert_eq!(plan(IGPU, &alone), Plan::Refuse(Refusal::LastAdapter));
    }

    #[test]
    fn displays_are_matched_by_hardware_id() {
        let shown = vec![r"PCI\VEN_10DE&DEV_2484&SUBSYS_146B10DE&REV_A1".to_string()];
        assert!(shows_display(DGPU, &shown));
        assert!(shows_display(&DGPU.to_lowercase(), &shown));
        assert!(!shows_display(IGPU, &shown));
        // A prefix of another id isn't a match.
        assert!(!shows_display(
            r"PCI\VEN_10DE&DEV_2484&SUBSYS_146B10DE&REV_A10\1",
            &shown
        ));
        assert!(!shows_display(IGPU, &[String::new()]));
    }

    #[test]
    fn instance_ids_are_checked_before_reaching_a_command_line() {
        assert!(valid_instance_id(IGPU));
        assert!(valid_instance_id(r"SWD\REMOTEDISPLAYENUM\{GUID}#1"));
        assert!(!valid_instance_id("no-backslash"));
        assert!(!valid_instance_id(r"PCI\VEN_1 & calc"));
        assert!(!valid_instance_id(r#"PCI\VEN"1"#));
        assert!(!valid_instance_id(""));
    }

    /// Devices in memory: what was disabled and enabled, and whether
    /// Windows' prompt was declined.
    #[derive(Default)]
    struct Fake {
        adapters: Mutex<Vec<Adapter>>,
        declined: bool,
        broken_enable: bool,
        log: Arc<Mutex<Vec<String>>>,
    }

    struct FakeHold {
        id: String,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Held for FakeHold {
        fn release(self: Box<Self>) -> Result<()> {
            self.log.lock().push(format!("enable {}", self.id));
            Ok(())
        }
    }

    impl Fake {
        fn with(adapters: Vec<Adapter>) -> Self {
            Self {
                adapters: Mutex::new(adapters),
                ..Self::default()
            }
        }

        fn set_disabled(&self, id: &str, disabled: bool) {
            for a in self.adapters.lock().iter_mut() {
                if a.instance_id == id {
                    a.disabled = disabled;
                }
            }
        }
    }

    impl GpuDevices for Fake {
        fn adapters(&self) -> Result<Vec<Adapter>> {
            Ok(self.adapters.lock().clone())
        }
        fn hide(&self, instance_id: &str) -> Result<Box<dyn Held>> {
            if self.declined {
                anyhow::bail!("Windows' permission prompt was declined");
            }
            self.log.lock().push(format!("disable {instance_id}"));
            self.set_disabled(instance_id, true);
            Ok(Box::new(FakeHold {
                id: instance_id.into(),
                log: self.log.clone(),
            }))
        }
        fn enable(&self, instance_id: &str) -> Result<()> {
            if self.broken_enable {
                anyhow::bail!("refused");
            }
            self.log.lock().push(format!("enable {instance_id}"));
            self.set_disabled(instance_id, false);
            Ok(())
        }
    }

    fn game() -> GameId {
        GameId::new(Runner::Gog, "1207658924")
    }

    fn journal(dir: &Path) -> Journal {
        json_store::read_or_default(&dir.join("gpu-hidden.json"))
    }

    #[test]
    fn a_session_records_the_card_and_forgets_it_once_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-hidden.json");
        let fake = Fake::with(vec![
            card(IGPU, false, Some(false)),
            card(DGPU, false, Some(true)),
        ]);
        let hidden = hide(&fake, &path, &game(), IGPU);
        assert!(hidden.note.unwrap().contains("hidden"));
        assert_eq!(journal(dir.path()).pending.len(), 1);
        assert_eq!(journal(dir.path()).pending[0].game, game().key());
        hidden.held.unwrap().end().unwrap();
        assert_eq!(
            *fake.log.lock(),
            vec![format!("disable {IGPU}"), format!("enable {IGPU}")]
        );
        assert!(journal(dir.path()).pending.is_empty());
    }

    #[test]
    fn a_dropped_session_enables_the_card_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-hidden.json");
        let fake = Fake::with(vec![
            card(IGPU, false, Some(false)),
            card(DGPU, false, Some(true)),
        ]);
        let hidden = hide(&fake, &path, &game(), IGPU);
        drop(hidden.held);
        // The release runs on a thread of its own.
        for _ in 0..200 {
            if fake.log.lock().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(fake.log.lock().last().unwrap(), &format!("enable {IGPU}"));
    }

    #[test]
    fn refusals_and_a_declined_prompt_touch_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-hidden.json");
        let fake = Fake::with(vec![
            card(IGPU, false, Some(false)),
            card(DGPU, false, Some(true)),
        ]);
        let refused = hide(&fake, &path, &game(), DGPU);
        assert!(refused.held.is_none());
        assert!(refused.note.unwrap().contains("display"));
        let empty = hide(&fake, &path, &game(), "");
        assert!(empty.held.is_none() && empty.note.is_none());
        assert!(fake.log.lock().is_empty());

        let declined = Fake {
            declined: true,
            ..Fake::with(vec![
                card(IGPU, false, Some(false)),
                card(DGPU, false, Some(true)),
            ])
        };
        let hidden = hide(&declined, &path, &game(), IGPU);
        assert!(hidden.held.is_none());
        assert!(hidden.note.unwrap().contains("declined"));
        assert!(journal(dir.path()).pending.is_empty());
    }

    #[test]
    fn the_next_start_enables_what_a_killed_session_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-hidden.json");
        let fake = Fake::with(vec![
            card(IGPU, false, Some(false)),
            card(DGPU, false, Some(true)),
        ]);
        let hidden = hide(&fake, &path, &game(), IGPU);
        // Irori and its helper are killed: the hold is never released.
        std::mem::forget(hidden.held);
        assert_eq!(journal(dir.path()).pending.len(), 1);
        assert!(recover(&fake, &path).is_empty());
        assert_eq!(fake.log.lock().last().unwrap(), &format!("enable {IGPU}"));
        assert!(!fake.adapters.lock()[0].disabled);
        assert!(journal(dir.path()).pending.is_empty());
        // Nothing left: nothing asked.
        let asked = fake.log.lock().len();
        assert!(recover(&fake, &path).is_empty());
        assert_eq!(fake.log.lock().len(), asked);
    }

    #[test]
    fn recovery_forgets_enabled_cards_and_keeps_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-hidden.json");
        let pending = |id: &str| Pending {
            instance_id: id.into(),
            game: game().key(),
            since: 1,
        };
        json_store::write(
            &path,
            &Journal {
                pending: vec![pending(IGPU), pending(r"PCI\GONE\1")],
            },
        )
        .unwrap();
        // The helper already enabled it when Irori went away.
        let fake = Fake::with(vec![card(IGPU, false, Some(false))]);
        assert!(recover(&fake, &path).is_empty());
        assert!(fake.log.lock().is_empty());
        assert!(journal(dir.path()).pending.is_empty());

        json_store::write(
            &path,
            &Journal {
                pending: vec![pending(IGPU)],
            },
        )
        .unwrap();
        let broken = Fake {
            broken_enable: true,
            ..Fake::with(vec![card(IGPU, true, Some(false))])
        };
        assert_eq!(recover(&broken, &path), vec![IGPU.to_string()]);
        assert_eq!(journal(dir.path()).pending.len(), 1);
    }

    #[test]
    fn journals_from_newer_versions_still_read() {
        let journal: Journal = serde_json::from_str(
            r#"{"pending": [{"instanceId": "PCI\\A\\1", "later": true}], "more": 1}"#,
        )
        .unwrap();
        assert_eq!(journal.pending[0].instance_id, r"PCI\A\1");
    }
}
