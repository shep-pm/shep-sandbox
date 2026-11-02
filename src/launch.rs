//! `launch <sheep> -- <command>`: the command, inside a crabpot sandbox.
//!
//! The shepherd spawns a sandboxed sheep through this verb. The command runs
//! under crabpot with the sheep's own `[app.dogs.sandbox]` table, and never
//! without it: every path that cannot reach crabpot ends in a [`Refusal`],
//! which exits 125 as crabpot does for a sandbox it refuses.

use core::fmt;
use std::{ffi::OsString, io, path::Path, process::ExitCode};

use serde_json::{Map, Value};
use shep_client::dogs::{HomeError, ShepherdError};

use crate::{error::Error, settings};

/// The exit code for every refusal, which is crabpot's own for a command line
/// or sandbox it refuses.
const REFUSED: u8 = 125;

/// What `launch` was asked to run, and for which sheep.
#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The sheep whose `[app.dogs.sandbox]` table confines the command.
    pub sheep: String,
    /// The program and its arguments.
    pub command: Vec<OsString>,
}

/// What `launch` hands the sandbox: the settings and the command, both final.
#[derive(Debug)]
pub struct Plan {
    /// The settings document crabpot reads, with the sheep's cwd writable.
    pub settings: Map<String, Value>,
    /// The program and its arguments.
    pub command: Vec<OsString>,
}

/// Why `launch` did not run the command.
#[derive(Debug)]
pub enum Refusal {
    /// No sheep name came before `--`, or it is not UTF-8.
    NoSheep,
    /// Nothing follows `--`.
    NoCommand,
    /// `$SHEP_HOME` is unset, so there is no shepherd to ask.
    NoHome,
    /// `$SHEP_HOME` names no usable directory.
    Home(HomeError),
    /// The shepherd could not be reached, or failed the request.
    Shepherd(ShepherdError),
    /// The shepherd holds no `[app.dogs.sandbox]` table for the sheep.
    NoTable {
        /// The sheep asked about.
        sheep: String,
    },
    /// The sheep's table holds a value crabpot refuses.
    Settings(Error),
    /// The current directory, which is the sheep's cwd, cannot be read as
    /// UTF-8 text.
    Cwd(io::Error),
    /// The settings could not be written to the file crabpot reads them from.
    SettingsFile(io::Error),
    /// The runtime that asks the shepherd could not start.
    Runtime(io::Error),
    /// This platform has no crabpot sandbox.
    Unsupported,
}

impl Refusal {
    /// The process exit code for this cause, which is the same for all.
    pub const fn exit_code(&self) -> u8 {
        REFUSED
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSheep => f.write_str("launch takes a sheep name before `--`"),
            Self::NoCommand => f.write_str("launch takes a command after `--`"),
            Self::NoHome => f.write_str("SHEP_HOME is unset, so there is no shepherd to ask"),
            Self::Home(err) => write!(f, "SHEP_HOME is unusable: {err}"),
            Self::Shepherd(err) => err.fmt(f),
            Self::NoTable { sheep } => {
                write!(
                    f,
                    "the shepherd holds no [app.dogs.sandbox] table for {sheep}"
                )
            }
            Self::Settings(err) => write!(f, "the sheep's table is refused: {err}"),
            Self::Cwd(err) => write!(f, "cannot read the current directory: {err}"),
            Self::SettingsFile(err) => write!(f, "cannot write the settings file: {err}"),
            Self::Runtime(err) => write!(f, "cannot start the runtime: {err}"),
            Self::Unsupported => f.write_str("launch is not supported on this platform"),
        }
    }
}

impl core::error::Error for Refusal {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Home(err) => Some(err),
            Self::Shepherd(err) => Some(err),
            Self::Settings(err) => Some(err),
            Self::Cwd(err) | Self::SettingsFile(err) | Self::Runtime(err) => Some(err),
            Self::NoSheep
            | Self::NoCommand
            | Self::NoHome
            | Self::NoTable { .. }
            | Self::Unsupported => None,
        }
    }
}

/// The sheep and the command in `args`, which follow the word `launch`.
///
/// # Errors
///
/// [`Refusal::NoSheep`] when no UTF-8 name comes before `--`, and
/// [`Refusal::NoCommand`] when nothing follows it.
pub fn parse_args(args: &[OsString]) -> Result<Invocation, Refusal> {
    let (sheep, rest) = args.split_first().ok_or(Refusal::NoSheep)?;
    let sheep = sheep
        .to_str()
        .filter(|name| *name != "--")
        .ok_or(Refusal::NoSheep)?;
    let command = match rest.split_first() {
        Some((dash, command)) if dash == "--" && !command.is_empty() => command,
        _ => return Err(Refusal::NoCommand),
    };
    Ok(Invocation {
        sheep: sheep.to_owned(),
        command: command.to_vec(),
    })
}

/// `table` as crabpot will read it: the five required lists present, `cwd`
/// in `filesystem.allowWrite` once, and the whole checked against crabpot.
///
/// # Errors
///
/// [`Refusal::Cwd`] when `cwd` is not UTF-8, and [`Refusal::Settings`] when
/// crabpot refuses the result.
pub fn effective_settings(
    table: Map<String, Value>,
    cwd: &Path,
) -> Result<Map<String, Value>, Refusal> {
    let cwd = cwd.to_str().ok_or_else(|| {
        Refusal::Cwd(io::Error::new(
            io::ErrorKind::InvalidData,
            "the path is not UTF-8",
        ))
    })?;
    let mut settings = settings::filled(&table);
    if let Some(Value::Object(filesystem)) = settings.get_mut("filesystem")
        && let Some(Value::Array(allow_write)) = filesystem.get_mut("allowWrite")
        && !allow_write.iter().any(|entry| entry == cwd)
    {
        allow_write.push(Value::String(cwd.to_owned()));
    }
    settings::parse(&settings).map_err(Refusal::Settings)?;
    Ok(settings)
}

/// Everything `launch` does short of running the command.
///
/// `fetch` asks for the sheep's table, and `execute` runs the plan. `execute`
/// returns only when it could not, so the value returned is always a refusal.
/// Nothing here runs the command except through `execute`.
pub fn launch(
    args: &[OsString],
    cwd: &Path,
    fetch: impl FnOnce(&str) -> Result<Option<Map<String, Value>>, Refusal>,
    execute: impl FnOnce(Plan) -> Refusal,
) -> Refusal {
    let plan = (|| {
        let Invocation { sheep, command } = parse_args(args)?;
        let table = fetch(&sheep)?.ok_or(Refusal::NoTable { sheep })?;
        let settings = effective_settings(table, cwd)?;
        Ok(Plan { settings, command })
    })();
    match plan {
        Ok(plan) => execute(plan),
        Err(refusal) => refusal,
    }
}

/// Runs `launch` for the arguments after the word `launch`. It returns only
/// with the exit code of a refusal, once it has said why on stderr.
pub fn run(args: &[OsString]) -> ExitCode {
    let refusal = if cfg!(not(unix)) {
        Refusal::Unsupported
    } else {
        match std::env::current_dir() {
            Ok(cwd) => launch(args, &cwd, fetch_table, execute),
            Err(err) => Refusal::Cwd(err),
        }
    };
    eprintln!("shep-sandbox: {refusal}");
    ExitCode::from(refusal.exit_code())
}

/// The sheep's `[app.dogs.sandbox]` table, asked of the shepherd `$SHEP_HOME`
/// names. The runtime that asks is gone when this returns.
fn fetch_table(sheep: &str) -> Result<Option<Map<String, Value>>, Refusal> {
    if std::env::var_os("SHEP_HOME").is_none() {
        return Err(Refusal::NoHome);
    }
    let paths =
        shep_client::dogs::resolve_paths(&|key| std::env::var_os(key)).map_err(Refusal::Home)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Refusal::Runtime)?;
    runtime
        .block_on(async {
            let client = shep_client::Client::connect(&paths.socket)
                .await
                .map_err(ShepherdError::from)?;
            client
                .sheep_dog_table(sheep, crate::DEFAULT_NAME)
                .await
                .map_err(ShepherdError::from)
        })
        .map(|table| table.map(shep_client::shep_core::config::DogTable::into_map))
        .map_err(Refusal::Shepherd)
}

/// Writes the settings to an unlinked file and runs crabpot on it, which
/// exits the process with the command's status.
#[cfg(unix)]
fn execute(plan: Plan) -> Refusal {
    use std::os::fd::AsRawFd;

    let file = match settings_file(&plan.settings) {
        Ok(file) => file,
        Err(err) => return Refusal::SettingsFile(err),
    };
    let mut args: Vec<OsString> = vec![
        "-s".into(),
        format!("/dev/fd/{}", file.as_raw_fd()).into(),
        "--".into(),
    ];
    args.extend(plan.command);
    // `file` stays open until the process exits, because `run_args` never
    // returns.
    crabpot::run_args(args)
}

#[cfg(not(unix))]
fn execute(_plan: Plan) -> Refusal {
    Refusal::Unsupported
}

/// An unlinked file holding `settings`, read from its start.
#[cfg(unix)]
fn settings_file(settings: &Map<String, Value>) -> io::Result<std::fs::File> {
    use std::io::{Seek, Write};

    let mut file = tempfile::tempfile()?;
    file.write_all(&serde_json::to_vec(settings)?)?;
    file.flush()?;
    file.rewind()?;
    Ok(file)
}

#[cfg(test)]
mod tests;
