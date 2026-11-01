//! The sheep in the flock with `sandbox` set, read from the shepherd.

use core::fmt;
use std::path::Path;

use shep_client::{
    Client, dogs,
    shep_core::{
        config::AppConfig,
        protocol::{Request, Response, SANDBOX_SINCE},
    },
};

use super::{FlockEntry, SandboxedSheep};

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
/// The shepherd runs a sheep from its `cwd`, and from its own directory when
/// that is unset or relative, which doctor cannot learn. Such a sheep is
/// [`FlockEntry::Unchecked`] rather than guessed at.
pub fn sandboxed(configs: &[AppConfig]) -> Vec<FlockEntry> {
    configs
        .iter()
        .filter_map(|config| {
            let file = config.sandbox.as_deref()?;
            let cwd = config.cwd.as_deref().map(Path::new);
            Some(match cwd {
                Some(cwd) if cwd.is_absolute() => FlockEntry::Sandboxed(SandboxedSheep {
                    name: config.name.clone(),
                    settings: cwd.join(file),
                    cwd: cwd.to_owned(),
                }),
                _ => unchecked(
                    &config.name,
                    "its `cwd` is unset or relative, so doctor cannot tell where the \
                     shepherd reads `sandbox` from; give the sheep an absolute `cwd`",
                ),
            })
        })
        .collect()
}

fn unchecked(name: &str, reason: &str) -> FlockEntry {
    FlockEntry::Unchecked {
        name: name.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Asks the shepherd for the config of each sheep in the flock and keeps the
/// ones with `sandbox` set.
///
/// # Errors
///
/// [`FlockError::Old`] when the shepherd predates `sandbox`, and
/// [`FlockError::Failed`] when it cannot be reached or lists no flock. A
/// sheep whose own config cannot be read is [`FlockEntry::Unchecked`].
pub async fn read() -> Result<Vec<FlockEntry>, FlockError> {
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
    let mut entries = Vec::new();
    for name in names {
        let config = match client
            .request(Request::SheepConfig { name: name.clone() })
            .await
        {
            Ok(Response::SheepConfig(view)) => view.config,
            Ok(other) => {
                let reason = format!("the shepherd answered {} to SheepConfig", other.name());
                entries.push(unchecked(&name, &reason));
                continue;
            }
            Err(err) => {
                entries.push(unchecked(
                    &name,
                    &format!("its config could not be read: {err}"),
                ));
                continue;
            }
        };
        entries.extend(sandboxed(&[config]));
    }
    Ok(entries)
}
