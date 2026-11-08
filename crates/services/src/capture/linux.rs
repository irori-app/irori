//! Linux: the desktop portal, else grim (see the parent module).

use super::*;
use anyhow::{Context as _, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

async fn portal() -> Result<PathBuf> {
    use ashpd::desktop::screenshot::Screenshot;
    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await?
        .response()?;
    file_uri_path(response.uri().as_str()).context("The desktop saved the screenshot elsewhere")
}

pub(super) async fn capture(target: &Path) -> Result<PathBuf> {
    let portal_error = match portal().await {
        Ok(path) => {
            let target = target.to_path_buf();
            return tokio::task::spawn_blocking(move || settle(&path, &target))
                .await
                .context("Could not save the screenshot");
        }
        Err(e) => e,
    };
    let Some(grim) = irori_core::host::find_program("grim").await else {
        bail!("The desktop took no screenshot: {portal_error:#}");
    };
    if let Some(dir) = target.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let spec = irori_core::host::command(CommandSpec::new(grim).arg(target));
    process::run(&spec, RunOptions::default()).await?.check()?;
    Ok(target.to_path_buf())
}
