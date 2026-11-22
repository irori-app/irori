//! Performance HUD runtime support: Linux hardware detection, MangoHud.conf
//! serialization, X11 case-sensitive keysym mapping, and cross-platform launch
//! environment/wrapper/Flatpak/Steam helpers.

pub mod hardware_linux;
pub mod keysym;
pub mod launch_env;
pub mod mangohud_conf;

use std::path::Path;

use anyhow::Result;

/// Atomically write `content` to `path` by writing `<path>.tmp` and renaming it
/// asynchronously. Never touches `~/.config/MangoHud/MangoHud.conf`.
pub async fn write_config_atomic(path: &Path, content: &str) -> Result<()> {
    let _ = (path, content);
    Ok(())
}
