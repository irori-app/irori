//! macOS: System Events through `osascript`, as the Apple menu does it.
//! macOS asks once whether Irori may control System Events.

use super::*;
use irori_core::process::{self, CommandSpec, RunOptions};

pub(super) async fn permissions() -> Vec<(PowerAction, Permission)> {
    PowerAction::ALL
        .into_iter()
        .map(|action| (action, Permission::Yes))
        .collect()
}

pub(super) async fn perform(action: PowerAction) -> Result<()> {
    let spec = CommandSpec::new("/usr/bin/osascript")
        .arg("-e")
        .arg(apple_script(action));
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!(
            "macOS refused (Irori may need permission to control System Events in System Settings → Privacy & Security → Automation): {}",
            out.error_summary()
        );
    }
    Ok(())
}
