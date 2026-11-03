//! `launch <sheep> -- <command>`: the command, inside a crabpot sandbox.
//!
//! The shepherd spawns a sandboxed sheep through this verb. The command runs
//! under crabpot with the sheep's own `[app.dogs.sandbox]` table, and never
//! without it: every path that cannot reach crabpot ends in a [`Refusal`],
//! which exits 125 as crabpot does for a sandbox it refuses.

use core::fmt;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

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
    /// The sheep the command runs as.
    pub sheep: String,
    /// The sheep's cwd, which `settings` makes writable.
    pub cwd: PathBuf,
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
    /// The pipe that carries later edits to crabpot could not be made.
    ControlPipe(io::Error),
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
            Self::ControlPipe(err) => write!(f, "cannot open the control pipe: {err}"),
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
            Self::Cwd(err)
            | Self::SettingsFile(err)
            | Self::ControlPipe(err)
            | Self::Runtime(err) => Some(err),
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
/// in `filesystem.allowWrite` and, as a literal path, in
/// `filesystem.allowRead`, `~` and `shep_home` in `filesystem.denyRead`, each
/// once, and the whole checked against crabpot.
///
/// A sheep reads nothing under home but its cwd until its own `allowRead`
/// re-opens a path. `shep_home` stays hidden even then, because crabpot keeps
/// a `denyRead` entry that is more specific than the `allowRead` around it.
///
/// # Errors
///
/// [`Refusal::Cwd`] when `cwd` is not UTF-8, [`Refusal::Home`] when
/// `shep_home` is not, and [`Refusal::Settings`] when crabpot refuses the
/// result.
pub fn effective_settings(
    table: Map<String, Value>,
    cwd: &Path,
    shep_home: &Path,
) -> Result<Map<String, Value>, Refusal> {
    let cwd = cwd.to_str().ok_or_else(|| {
        Refusal::Cwd(io::Error::new(
            io::ErrorKind::InvalidData,
            "the path is not UTF-8",
        ))
    })?;
    let shep_home = shep_home
        .to_str()
        .ok_or(Refusal::Home(HomeError::NotUtf8))?;
    let mut settings = settings::filled(&table);
    if let Some(Value::Object(filesystem)) = settings.get_mut("filesystem") {
        add_once(filesystem, "allowWrite", Value::from(cwd));
        add_once(filesystem, "allowRead", literal(cwd));
        add_once(filesystem, "denyRead", Value::from("~"));
        add_once(filesystem, "denyRead", Value::from(shep_home));
    }
    settings::parse(&settings).map_err(Refusal::Settings)?;
    Ok(settings)
}

/// Appends `entry` to the list `key` of `filesystem` unless it is there. A
/// missing list is made, and a `key` that is not a list is left for crabpot to
/// refuse.
fn add_once(filesystem: &mut Map<String, Value>, key: &str, entry: Value) {
    let list = filesystem
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(list) = list
        && !list.contains(&entry)
    {
        list.push(entry);
    }
}

/// `path` as an entry crabpot reads only as that name. A plain entry with
/// `*`, `?`, `[` or `]` is also a pattern, which for an allow would open
/// paths beside the cwd.
fn literal(path: &str) -> Value {
    serde_json::json!({"path": path, "literal": true})
}

/// What the shepherd answered a request for a sheep's table with.
#[derive(Debug)]
pub struct Fetched {
    /// The shepherd's `$SHEP_HOME`, which no sheep may read.
    pub shep_home: PathBuf,
    /// The sheep's `[app.dogs.sandbox]` table, or `None` when it has none.
    pub table: Option<Map<String, Value>>,
}

/// Everything `launch` does short of running the command.
///
/// `fetch` resolves `$SHEP_HOME` and asks for the sheep's table, and only
/// once the arguments parse. `execute` runs the plan. `execute`
/// returns only when it could not, so the value returned is always a refusal.
/// Nothing here runs the command except through `execute`.
pub fn launch(
    args: &[OsString],
    cwd: &Path,
    fetch: impl FnOnce(&str) -> Result<Fetched, Refusal>,
    execute: impl FnOnce(Plan) -> Refusal,
) -> Refusal {
    let plan = (|| {
        let Invocation { sheep, command } = parse_args(args)?;
        let Fetched { shep_home, table } = fetch(&sheep)?;
        let table = table.ok_or_else(|| Refusal::NoTable {
            sheep: sheep.clone(),
        })?;
        let settings = effective_settings(table, cwd, &shep_home)?;
        Ok(Plan {
            sheep,
            cwd: cwd.to_owned(),
            settings,
            command,
        })
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

/// The paths `$SHEP_HOME` names.
///
/// # Errors
///
/// [`Refusal::NoHome`] when it is unset, and [`Refusal::Home`] when it names no
/// usable directory.
fn shep_paths() -> Result<shep_client::shep_core::paths::ShepPaths, Refusal> {
    if std::env::var_os("SHEP_HOME").is_none() {
        return Err(Refusal::NoHome);
    }
    shep_client::dogs::resolve_paths(&|key| std::env::var_os(key)).map_err(Refusal::Home)
}

/// The sheep's `[app.dogs.sandbox]` table, asked of the shepherd `$SHEP_HOME`
/// names. The runtime that asks is gone when this returns.
fn fetch_table(sheep: &str) -> Result<Fetched, Refusal> {
    let paths = shep_paths()?;
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
        .map(|table| Fetched {
            shep_home: paths.home,
            table: table.map(shep_client::shep_core::config::DogTable::into_map),
        })
        .map_err(Refusal::Shepherd)
}

/// Writes the settings to an unlinked file and runs crabpot on it, which
/// exits the process with the command's status. A thread follows the sheep's
/// table meanwhile, and writes the network lists to crabpot's control pipe.
#[cfg(unix)]
fn execute(plan: Plan) -> Refusal {
    use std::os::fd::AsRawFd;

    let file = match settings_file(&plan.settings) {
        Ok(file) => file,
        Err(err) => return Refusal::SettingsFile(err),
    };
    let (control, writer) = match std::io::pipe() {
        Ok(pipe) => pipe,
        Err(err) => return Refusal::ControlPipe(err),
    };
    let mut args: Vec<OsString> = vec![
        "-s".into(),
        format!("/dev/fd/{}", file.as_raw_fd()).into(),
        "--control-fd".into(),
        control.as_raw_fd().to_string().into(),
        "--".into(),
    ];
    args.extend(plan.command);
    follow_table(plan.sheep, plan.cwd, plan.settings, writer);
    // `file` and `control` stay open until the process exits, because
    // `run_args` never returns.
    crabpot::run_args(args)
}

/// Starts the thread that follows the sheep's table. The sandbox runs without
/// it when it cannot start: the settings it starts with are final.
#[cfg(unix)]
fn follow_table(
    sheep: String,
    cwd: PathBuf,
    settings: Map<String, Value>,
    control: std::io::PipeWriter,
) {
    let started = match shep_paths() {
        Ok(paths) => crate::follow::spawn(
            paths.socket,
            paths.home,
            sheep.clone(),
            cwd,
            settings,
            control,
        )
        .map_err(|err| err.to_string()),
        Err(err) => Err(err.to_string()),
    };
    if let Err(err) = started {
        eprintln!("shep-sandbox: {sheep}: edits to the table will not reach the sandbox: {err}");
    }
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
