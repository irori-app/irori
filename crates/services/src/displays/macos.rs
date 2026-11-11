//! macOS's displays, from `system_profiler SPDisplaysDataType -json`.

use std::time::Duration;

use irori_core::process::CommandSpec;
use irori_runtime::discovery::run_with_timeout;

use super::{DisplayReport, parse};

pub(super) async fn probe() -> DisplayReport {
    let spec = CommandSpec::new("/usr/sbin/system_profiler").args(["SPDisplaysDataType", "-json"]);
    // It asks every graphics processor, which can take a few seconds.
    let displays = match run_with_timeout(&spec, Duration::from_secs(15)).await {
        Some(out) if out.success() => parse::system_profiler(&out.stdout).unwrap_or_default(),
        Some(out) => {
            tracing::debug!("system_profiler: {}", out.error_summary());
            Vec::new()
        }
        None => Vec::new(),
    };
    DisplayReport::new("system_profiler", displays, false)
}
