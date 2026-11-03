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
use shep_client::{
    ReconnectingClient,
    dogs::{DogIdentity, DogRuntime, HomeError, ShepherdError},
    shep_core::{config::DogTable, dogs::DOG_LINK_FD_VAR, paths::ShepPaths},
};

use crate::{error::Error, follow::Held, settings};

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

/// What `launch` hands the sandbox: the settings and the command, both final,
/// and the connection to the shepherd, which the follower takes over.
#[derive(Debug)]
pub struct Plan<C> {
    /// The sheep the command runs as.
    pub sheep: String,
    /// The sheep's cwd, which `settings` makes writable.
    pub cwd: PathBuf,
    /// The settings document crabpot reads, with the sheep's cwd writable.
    pub settings: Map<String, Value>,
    /// The shepherd's `$SHEP_HOME`.
    pub shep_home: PathBuf,
    /// The connection the table came over.
    pub connection: C,
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
    /// The shepherd's link could not be kept from the command.
    Link(io::Error),
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
            Self::Link(err) => write!(f, "cannot keep the shepherd's link from the command: {err}"),
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
            | Self::Link(err)
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
pub struct Fetched<C> {
    /// The shepherd's `$SHEP_HOME`, which no sheep may read.
    pub shep_home: PathBuf,
    /// The sheep's `[app.dogs.sandbox]` table, or `None` when it has none.
    pub table: Option<Map<String, Value>>,
    /// The connection the table came over, which stays open for the follower.
    pub connection: C,
}

/// Everything `launch` does short of running the command.
///
/// `fetch` resolves `$SHEP_HOME` and asks for the sheep's table, and only
/// once the arguments parse. `execute` runs the plan. `execute`
/// returns only when it could not, so the value returned is always a refusal.
/// Nothing here runs the command except through `execute`.
pub fn launch<C>(
    args: &[OsString],
    cwd: &Path,
    fetch: impl FnOnce(&str) -> Result<Fetched<C>, Refusal>,
    execute: impl FnOnce(Plan<C>) -> Refusal,
) -> Refusal {
    let plan = (|| {
        let Invocation { sheep, command } = parse_args(args)?;
        let Fetched {
            shep_home,
            table,
            connection,
        } = fetch(&sheep)?;
        let table = table.ok_or_else(|| Refusal::NoTable {
            sheep: sheep.clone(),
        })?;
        let settings = effective_settings(table, cwd, &shep_home)?;
        Ok(Plan {
            sheep,
            cwd: cwd.to_owned(),
            settings,
            shep_home,
            connection,
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

/// How `launch` reaches the shepherd.
trait Reach {
    /// A connection to the shepherd.
    type Conn;

    /// Takes the link the shepherd spawned this process holding.
    async fn over_link(&mut self) -> Result<Self::Conn, ShepherdError>;

    /// Dials the shepherd's socket.
    async fn over_socket(&mut self) -> Result<Self::Conn, ShepherdError>;

    /// The table of `sheep`, asked over `conn`.
    async fn table(
        &mut self,
        conn: &Self::Conn,
        sheep: &str,
    ) -> Result<Option<Map<String, Value>>, ShepherdError>;
}

/// The sheep's table over one connection, which is the link when `has_link`
/// and the socket otherwise. A link that fails is refused and never retried on
/// the socket, because an inherited link can be taken once.
async fn fetch_over<R: Reach>(
    reach: &mut R,
    has_link: bool,
    shep_home: PathBuf,
    sheep: &str,
) -> Result<Fetched<R::Conn>, Refusal> {
    let connection = if has_link {
        reach.over_link().await
    } else {
        reach.over_socket().await
    }
    .map_err(Refusal::Shepherd)?;
    let table = reach
        .table(&connection, sheep)
        .await
        .map_err(Refusal::Shepherd)?;
    Ok(Fetched {
        shep_home,
        table,
        connection,
    })
}

/// The shepherd `$SHEP_HOME` names, reached with the dog connection of
/// `shep-client`.
struct Wire {
    paths: ShepPaths,
    identity: DogIdentity,
}

impl Reach for Wire {
    type Conn = ReconnectingClient;

    async fn over_link(&mut self) -> Result<ReconnectingClient, ShepherdError> {
        DogRuntime::start(self.identity.clone(), self.paths.clone())
            .await
            .map(DogRuntime::into_client)
    }

    async fn over_socket(&mut self) -> Result<ReconnectingClient, ShepherdError> {
        Ok(ReconnectingClient::connect_as(&self.paths.socket, &self.identity).await?)
    }

    async fn table(
        &mut self,
        conn: &ReconnectingClient,
        sheep: &str,
    ) -> Result<Option<Map<String, Value>>, ShepherdError> {
        let table = conn.sheep_dog_table(sheep, crate::DEFAULT_NAME).await?;
        Ok(table.map(DogTable::into_map))
    }
}

/// The sheep's `[app.dogs.sandbox]` table, asked of the shepherd `$SHEP_HOME`
/// names. The connection and the runtime that drives it stay open in the
/// result, for the follower.
fn fetch_table(sheep: &str) -> Result<Fetched<Held>, Refusal> {
    let paths = shep_paths()?;
    let has_link = std::env::var_os(DOG_LINK_FD_VAR).is_some();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Refusal::Runtime)?;
    let identity = DogIdentity::from_env(&|key| std::env::var(key).ok(), crate::DEFAULT_NAME);
    let mut wire = Wire {
        paths: paths.clone(),
        identity,
    };
    let Fetched {
        shep_home,
        table,
        connection,
    } = runtime.block_on(fetch_over(&mut wire, has_link, paths.home, sheep))?;
    // An inherited link cannot be dialled again, so the follower redials the
    // socket only for a launch that started without one.
    let socket = (!has_link).then_some(paths.socket);
    Ok(Fetched {
        shep_home,
        table,
        connection: Held::new(runtime, connection, socket),
    })
}

/// Keeps the shepherd's link from the command. The descriptor `var` names is
/// closed on exec and `var` leaves the environment crabpot hands the command.
/// A launch that holds no link has nothing to withhold.
///
/// Call it before any other thread runs, as the environment is changed.
///
/// # Errors
///
/// Whatever `fcntl` fails with, and `InvalidData` when `var` is not a number.
#[cfg(unix)]
fn withhold_link(var: &str) -> io::Result<()> {
    let Some(value) = std::env::var_os(var) else {
        return Ok(());
    };
    let fd = value
        .to_str()
        .and_then(|value| value.parse::<i32>().ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{var} is not a number"))
        })?;
    crate::inherit::close_on_exec(fd)?;
    crate::inherit::unset_var(var);
    Ok(())
}

/// Writes the settings to an unlinked file and runs crabpot on it, which
/// exits the process with the command's status. A thread follows the sheep's
/// table meanwhile, and writes the network lists to crabpot's control pipe.
#[cfg(unix)]
fn execute(plan: Plan<Held>) -> Refusal {
    use std::os::fd::AsRawFd;

    if let Err(err) = withhold_link(DOG_LINK_FD_VAR) {
        return Refusal::Link(err);
    }
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
    follow_table(
        plan.connection,
        plan.sheep,
        plan.cwd,
        plan.shep_home,
        plan.settings,
        writer,
    );
    // `file` and `control` stay open until the process exits, because
    // `run_args` never returns.
    crabpot::run_args(args)
}

/// Starts the thread that follows the sheep's table. The sandbox runs without
/// it when it cannot start: the settings it starts with are final.
#[cfg(unix)]
fn follow_table(
    connection: Held,
    sheep: String,
    cwd: PathBuf,
    shep_home: PathBuf,
    settings: Map<String, Value>,
    control: std::io::PipeWriter,
) {
    if let Err(err) =
        crate::follow::spawn(connection, sheep.clone(), cwd, shep_home, settings, control)
    {
        eprintln!("shep-sandbox: {sheep}: edits to the table will not reach the sandbox: {err}");
    }
}

#[cfg(not(unix))]
fn execute(_plan: Plan<Held>) -> Refusal {
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
