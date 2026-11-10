//! A whole install: check the compatibility list, find the program and its
//! API, download what is missing, then write it all through [`journal`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;

use crate::detect::{self, Detection};
use crate::journal::{self, Existing, Manifest, Planned};
use crate::layer::GraphicsMod;
use crate::{Api, ReshadeError, download};

/// Where an install is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Checking,
    DownloadingReshade,
    DownloadingShaders,
    DownloadingCompiler,
    /// Downloading an add-on (a RenoDX mod).
    DownloadingAddon,
    Writing,
}

/// What to install for.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// The game's folder.
    pub game_root: PathBuf,
    /// The store's or the user's executable setting.
    pub configured_exe: Option<PathBuf>,
    /// A program the user picked instead.
    pub chosen_exe: Option<PathBuf>,
    /// The game runs through Wine: install `d3dcompiler_47.dll` too and
    /// record the DLL overrides the launch needs.
    pub wine: bool,
    /// The game's manifest file ([`crate::journal_path`]).
    pub journal: PathBuf,
    /// Install the build with full add-on support (for RenoDX).
    pub addon: bool,
}

/// Find the program and API, refusing what ReShade or Irori can't do.
/// Blocking.
pub fn check(
    root: &Path,
    configured: Option<&Path>,
    chosen: Option<&Path>,
    compat: &crate::CompatList,
) -> Result<(Detection, Api, String)> {
    let found = detect::detect(root, configured, chosen, compat)?;
    if let Some(exe) = &found.banned {
        bail!(ReshadeError::Banned { exe: exe.clone() });
    }
    let api = found.api.ok_or(ReshadeError::NoApi)?;
    let dll = found.dll.clone().ok_or(ReshadeError::Unsupported(api))?;
    if !matches!(found.machine, crate::Machine::X86 | crate::Machine::X64) {
        bail!(ReshadeError::UnsupportedArch);
    }
    Ok((found, api, dll))
}

/// Install ReShade. `step` reports where it is, `progress` the downloads.
pub async fn install(
    req: InstallRequest,
    step: Arc<dyn Fn(Step) + Send + Sync>,
    progress: ProgressSink,
    cancel: CancellationToken,
) -> Result<Manifest> {
    step(Step::Checking);
    let compat = download::compatibility().await?;
    let (found, api, dll) = {
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            check(
                &req.game_root,
                req.configured_exe.as_deref(),
                req.chosen_exe.as_deref(),
                &compat,
            )
        })
        .await??
    };
    let version = download::latest().await?;

    step(Step::DownloadingReshade);
    let reshade =
        download::reshade_dll(&version, found.machine, req.addon, &progress, &cancel).await?;
    step(Step::DownloadingShaders);
    let shaders = download::shaders(&progress, &cancel).await?;
    // OpenGL effects compile to GLSL; Direct3D ones need the HLSL compiler,
    // which Wine lacks.
    let needs_compiler = req.wine && api != Api::OpenGl;
    let compiler = if needs_compiler {
        step(Step::DownloadingCompiler);
        Some(download::d3dcompiler(found.machine, &progress, &cancel).await?)
    } else {
        None
    };
    if cancel.is_cancelled() {
        bail!(irori_runtime::Cancelled);
    }

    step(Step::Writing);
    let mut files = vec![Planned::new(&dll, reshade, Existing::Refuse)];
    if let Some(compiler) = compiler {
        files.push(Planned::new("d3dcompiler_47.dll", compiler, Existing::Keep));
    }
    files.push(Planned::new(
        "ReShade.ini",
        download::default_ini(),
        Existing::Keep,
    ));
    files.extend(
        shaders
            .into_iter()
            .map(|(path, bytes)| Planned::new(path, bytes, Existing::Keep)),
    );
    let stem = dll.trim_end_matches(".dll").to_string();
    let base = Manifest {
        kind: GraphicsMod::Reshade,
        version,
        exe: found.exe.clone(),
        machine: found.machine,
        api,
        dll,
        dir: found.dir.clone(),
        enabled: true,
        complete: false,
        overrides: match (req.wine, needs_compiler) {
            (false, _) => Vec::new(),
            (true, false) => vec![stem],
            (true, true) => vec![stem, "d3dcompiler_47".into()],
        },
        files: Vec::new(),
        created_dirs: Vec::new(),
        addon: req.addon,
        source: None,
    };
    if !found.dir.is_dir() {
        bail!(
            "The folder ReShade goes to, {}, is not in the game folder",
            found.dir.display()
        );
    }
    let journal = req.journal.clone();
    tokio::task::spawn_blocking(move || {
        journal::apply(&journal, base, files, &|| cancel.is_cancelled())
    })
    .await
    .context("The ReShade install stopped")?
}
