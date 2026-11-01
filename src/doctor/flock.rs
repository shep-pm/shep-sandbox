//! The sheep in the flock with `sandbox` set, read from the shepherd.

use core::fmt;
use std::path::{Path, PathBuf};

use shep_client::{
    Client, dogs,
    shep_core::{
        config::AppConfig,
        protocol::{Request, Response, SANDBOX_SINCE},
    },
};

use super::SandboxedSheep;

/// Why the flock could not be read for sandboxed sheep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlockError {
    /// The shepherd speaks a protocol older than [`SANDBOX_SINCE`], so it
    /// ignores `sandbox` and no sheep in it is sandboxed.
    Old {
        /// The protocol the shepherd answered with.
        protocol: u32,
    },
    /// No shepherd was reached, or a request to it failed, with the reason.
    Failed(String),
}

impl fmt::Display for FlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Old { protocol } => write!(
                f,
                "the shepherd speaks protocol {protocol} and runs a sandbox from protocol \
                 {SANDBOX_SINCE}, so no sheep in it is sandboxed"
            ),
            Self::Failed(reason) => write!(f, "could not read the flock: {reason}"),
        }
    }
}

impl core::error::Error for FlockError {}

fn failed(err: impl fmt::Display) -> FlockError {
    FlockError::Failed(err.to_string())
}

/// The sheep among `configs` with `sandbox` set.
///
/// A `cwd` that is unset or relative is made absolute against `base`, and a
/// relative `sandbox` file is read from the `cwd`, as the shepherd does.
pub fn sandboxed(configs: &[AppConfig], base: &Path) -> Vec<SandboxedSheep> {
    configs
        .iter()
        .filter_map(|config| {
            let file = config.sandbox.as_deref()?;
            let cwd: PathBuf = base.join(config.cwd.as_deref().unwrap_or_default());
            Some(SandboxedSheep {
                name: config.name.clone(),
                settings: cwd.join(file),
                cwd,
            })
        })
        .collect()
}

/// Asks the shepherd for the config of each sheep in the flock and keeps the
/// ones with `sandbox` set.
///
/// # Errors
///
/// [`FlockError::Old`] when the shepherd predates `sandbox`, and
/// [`FlockError::Failed`] when it cannot be reached or a request fails.
pub async fn read() -> Result<Vec<SandboxedSheep>, FlockError> {
    let base = std::env::current_dir().map_err(failed)?;
    let paths = dogs::resolve_paths(&|key| std::env::var_os(key)).map_err(failed)?;
    let client = Client::connect(&paths.socket).await.map_err(failed)?;
    let protocol = client.daemon().protocol;
    if protocol < SANDBOX_SINCE {
        return Err(FlockError::Old { protocol });
    }

    let mut names: Vec<String> = Vec::new();
    for row in client.list_flock().await.map_err(failed)? {
        if row.dog.is_none() && !names.contains(&row.name) {
            names.push(row.name);
        }
    }
    let mut configs = Vec::with_capacity(names.len());
    for name in names {
        match client
            .request(Request::SheepConfig { name })
            .await
            .map_err(failed)?
        {
            Response::SheepConfig(view) => configs.push(view.config),
            other => return Err(failed(format!("answered {} to SheepConfig", other.name()))),
        }
    }
    Ok(sandboxed(&configs, &base))
}
