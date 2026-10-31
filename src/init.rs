//! `shep sandbox init`: write a commented crabpot settings file to start from.
//!
//! With `--from <sheep>` the file is filled from what the running sheep
//! listens on, dials and writes, instead of left plain.

use core::fmt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use shep_client::{
    Client, RequestError,
    dogs::ShepherdError,
    shep_core::{
        protocol::{ProcessInfo, Request, Response, SelectorSpec},
        status::ProcStatus,
    },
};

use observe::{Observed, Socket};

mod observe;

/// The file written when no path is given.
const DEFAULT_PATH: &str = "sandbox.json";

/// crabpot reads plain JSON and drops the keys it does not know, so the
/// comments are `"//"` keys rather than JSONC.
const SKELETON: &str = include_str!("init/skeleton.json");

/// The flag that names the sheep to take the settings from.
const FROM_FLAG: &str = "--from";

/// Why `shep sandbox init` wrote nothing.
#[derive(Debug)]
pub enum Error {
    /// An argument the verb does not take: a second path, a flag besides
    /// `--from`, a repeated `--from`, or a `--from` with no sheep after it.
    Usage {
        /// The first argument the verb does not take.
        arg: String,
    },
    /// A file already exists at the path, which init never overwrites.
    Exists(PathBuf),
    /// The file could not be created or written, such as when its folder is
    /// missing.
    Write {
        /// The path that was to be written.
        path: PathBuf,
        /// What the filesystem refused.
        source: io::Error,
    },
    /// The shepherd holds no sheep of the name given to `--from`.
    UnknownSheep {
        /// The name that was asked for.
        sheep: String,
    },
    /// The sheep is registered, but no instance of it has a process.
    NotRunning {
        /// The name that was asked for.
        sheep: String,
    },
    /// The shepherd could not be reached, or failed the request.
    Shepherd(ShepherdError),
    /// The sheep's sockets could not be read.
    Observe(observe::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage { arg } => {
                write!(
                    f,
                    "init takes a path and `{FROM_FLAG} <sheep>`, each at most once, not `{arg}`"
                )
            }
            Self::Exists(path) => {
                write!(
                    f,
                    "{} already exists, and init does not overwrite",
                    path.display()
                )
            }
            Self::Write { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
            Self::UnknownSheep { sheep } => write!(f, "the shepherd holds no sheep named {sheep}"),
            Self::NotRunning { sheep } => {
                write!(f, "{sheep} is not running, so there are no sockets to read")
            }
            Self::Shepherd(err) => err.fmt(f),
            Self::Observe(err) => err.fmt(f),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Write { source, .. } => Some(source),
            Self::Shepherd(err) => Some(err),
            Self::Observe(err) => Some(err),
            Self::Usage { .. }
            | Self::Exists(_)
            | Self::UnknownSheep { .. }
            | Self::NotRunning { .. } => None,
        }
    }
}

impl Error {
    /// A failed read of `sheep`'s sockets. A process that exited since the
    /// shepherd listed it is a sheep that is not running.
    fn observed(err: observe::Error, sheep: &str) -> Self {
        match err {
            observe::Error::Gone => Self::NotRunning {
                sheep: sheep.to_owned(),
            },
            err => Self::Observe(err),
        }
    }
}

/// What `shep sandbox init` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    path: Option<String>,
    from: Option<String>,
}

impl Args {
    /// The sheep to take the settings from, when `--from` named one.
    pub fn from(&self) -> Option<&str> {
        self.from.as_deref()
    }
}

/// Read the arguments after `init`.
///
/// # Errors
///
/// [`Error::Usage`] for a second path, a flag besides `--from`, a repeated
/// `--from` or one with no sheep after it.
pub fn parse(args: &[String]) -> Result<Args, Error> {
    let usage = |arg: &str| Error::Usage {
        arg: arg.to_owned(),
    };
    let mut parsed = Args {
        path: None,
        from: None,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == FROM_FLAG {
            let sheep = rest.next().ok_or_else(|| usage(arg))?;
            if parsed.from.replace(sheep.clone()).is_some() {
                return Err(usage(arg));
            }
        } else if arg.starts_with('-') || parsed.path.replace(arg.clone()).is_some() {
            return Err(usage(arg));
        }
    }
    Ok(parsed)
}

/// A running sheep as it was when `init` looked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    sheep: String,
    pid: u32,
    cwd: Option<String>,
    observed: Observed,
}

impl Taken {
    /// The sheep `sheep` at `pid`, with the TCP `sockets` it and its children
    /// hold.
    pub fn new(sheep: &str, pid: u32, cwd: Option<String>, sockets: &[Socket]) -> Self {
        Self {
            sheep: sheep.to_owned(),
            pid,
            cwd,
            observed: Observed::from_sockets(sockets),
        }
    }
}

/// The row of the running sheep `name`, or the first of its instances that
/// is.
///
/// # Errors
///
/// [`Error::UnknownSheep`] when `flock` has no sheep of that name, and
/// [`Error::NotRunning`] when none of its rows has a process.
fn running<'a>(name: &str, flock: &'a [ProcessInfo]) -> Result<&'a ProcessInfo, Error> {
    let mut named = flock
        .iter()
        .filter(|row| row.name == name && row.dog.is_none())
        .peekable();
    if named.peek().is_none() {
        return Err(Error::UnknownSheep {
            sheep: name.to_owned(),
        });
    }
    named
        .find(|row| {
            row.pid.is_some() && matches!(row.status, ProcStatus::Online | ProcStatus::Starting)
        })
        .ok_or_else(|| Error::NotRunning {
            sheep: name.to_owned(),
        })
}

/// Ask the shepherd for the sheep `name` and read the sockets it and its
/// children hold.
///
/// # Errors
///
/// [`Error::UnknownSheep`] and [`Error::NotRunning`] as for [`running`],
/// [`Error::Shepherd`] when a request fails, and [`Error::Observe`] when the
/// sockets cannot be read.
pub async fn take(client: &Client, name: &str) -> Result<Taken, Error> {
    let flock = client.list_flock().await.map_err(request)?;
    let row = running(name, &flock)?;
    let pid = row.pid.unwrap_or_default();

    let described = match client
        .request(Request::Describe {
            selector: SelectorSpec::Id(row.id),
        })
        .await
        .map_err(request)?
    {
        Response::Described(rows) => rows,
        other => return Err(unexpected("Describe", &other)),
    };
    let children = described
        .iter()
        .find(|described| described.id == row.id)
        .and_then(|described| described.lambs.as_deref())
        .unwrap_or_default();
    let pids: Vec<u32> = core::iter::once(pid)
        .chain(children.iter().map(|lamb| lamb.pid))
        .collect();

    let cwd = match client
        .request(Request::SheepConfig {
            name: name.to_owned(),
        })
        .await
        .map_err(request)?
    {
        Response::SheepConfig(view) => view.config.cwd,
        other => return Err(unexpected("SheepConfig", &other)),
    };

    let sockets = observe::sockets(&pids).map_err(|err| Error::observed(err, name))?;
    Ok(Taken::new(name, pid, cwd, &sockets))
}

fn request(err: RequestError) -> Error {
    Error::Shepherd(ShepherdError::Request(err))
}

fn unexpected(asked: &'static str, answer: &Response) -> Error {
    request(RequestError::UnexpectedReply {
        asked,
        answered: answer.name(),
    })
}

/// Write the settings file `args` asks for, in `dir` when its path is
/// relative, filled from `taken` when there is one. Returns the path
/// written.
///
/// # Errors
///
/// [`Error::Exists`] when a file is already at the path, and [`Error::Write`]
/// when it cannot be created.
pub fn run(dir: &Path, args: &Args, taken: Option<&Taken>) -> Result<PathBuf, Error> {
    let path = dir.join(args.path.as_deref().unwrap_or(DEFAULT_PATH));
    let text = taken.map_or_else(|| SKELETON.to_owned(), |taken| render(taken, Applies::HOST));
    create(&path, |file| file.write_all(text.as_bytes()))?;
    Ok(path)
}

/// Which network keys the host's crabpot applies. It refuses a non-empty list
/// for a key it does not, so init writes what it saw for such a key into a
/// comment instead.
#[derive(Debug, Clone, Copy)]
struct Applies {
    publish: bool,
    allow_tcp: bool,
}

impl Applies {
    /// crabpot v0.10.0 applies `publish` on macOS and Linux and `allowTcp` on
    /// Linux only. `init` describes the host it runs on, so this is fixed at
    /// compile time.
    const HOST: Self = Self {
        publish: cfg!(any(target_os = "linux", target_os = "macos")),
        allow_tcp: cfg!(target_os = "linux"),
    };
}

/// A `"//"` key holding `intro` and then `lines`, after a comma, or nothing
/// when there are no `lines`.
fn comment<'a>(
    key: &str,
    intro: &str,
    lines: impl IntoIterator<Item = &'a core::net::SocketAddr>,
) -> String {
    let mut lines = lines.into_iter().peekable();
    if lines.peek().is_none() {
        return String::new();
    }
    let lines: Vec<String> = core::iter::once(json(intro))
        .chain(lines.map(|addr| json(&addr.to_string())))
        .collect();
    format!(
        ",\n    {}: [\n      {}\n    ]",
        json(key),
        lines.join(",\n      ")
    )
}

/// The skeleton with the settings `taken` shows filled in. Each is a
/// one-line array in the skeleton, so each is found by its exact text.
fn render(taken: &Taken, applies: Applies) -> String {
    let Taken {
        sheep,
        pid,
        cwd,
        observed,
    } = taken;
    let mut text = SKELETON.to_owned();
    let mut fill = |from: &str, to: String| {
        debug_assert!(text.contains(from), "{from}");
        text = text.replace(from, &to);
    };

    let source = format!(
        "Taken from the running sheep {sheep} (pid {pid}) when init ran: what it held open at that moment. \
         A port it opens later is not here."
    );
    fill(
        "    \"Delete a comment",
        format!("    {},\n    \"Delete a comment", json(&source)),
    );
    if let Some(cwd) = cwd {
        fill(
            "\"allowWrite\": [\".\"],",
            format!("\"allowWrite\": [{}],", json(cwd)),
        );
    }
    let (published, listening) = if applies.publish {
        (addresses(&observed.listening), String::new())
    } else {
        let listening = comment(
            "// listening",
            "The sheep listens on these addresses. crabpot does not apply publish on this platform and refuses a non-empty list, so they are listed here.",
            &observed.listening,
        );
        (String::new(), listening)
    };
    fill(
        "\"publish\": []",
        format!("\"publish\": [{published}]{listening}"),
    );
    let (dialed, direct) = if applies.allow_tcp {
        (addresses(&observed.dials), String::new())
    } else {
        let direct = comment(
            "// dialed directly",
            "The sheep holds TCP connections to these addresses. crabpot applies allowTcp on Linux only and refuses a non-empty list elsewhere, so they are listed here.",
            &observed.dials,
        );
        (String::new(), direct)
    };
    let https = comment(
        "// dialed over HTTPS",
        "The sheep holds connections to these addresses on port 443. allowedDomains covers HTTPS by name, \
         so name the hosts there; an address cannot be named.",
        &observed.https,
    );
    fill(
        "\"allowTcp\": []",
        format!("\"allowTcp\": [{dialed}]{direct}{https}"),
    );
    text
}

fn addresses<'a>(set: impl IntoIterator<Item = &'a core::net::SocketAddr>) -> String {
    set.into_iter()
        .map(|addr| json(&addr.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// Create the file at `path`, which must not exist, and `fill` it. A file
/// that `fill` fails to write is removed, so a retry is not refused for a
/// half-written one.
fn create(path: &Path, fill: impl FnOnce(&mut File) -> io::Result<()>) -> Result<(), Error> {
    let write = |source| Error::Write {
        path: path.to_owned(),
        source,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| match source.kind() {
            io::ErrorKind::AlreadyExists => Error::Exists(path.to_owned()),
            _ => write(source),
        })?;
    fill(&mut file).map_err(|source| {
        // Best effort: the write error is the one worth reporting.
        let _ = fs::remove_file(path);
        write(source)
    })
}

#[cfg(test)]
mod tests;
