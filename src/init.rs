//! `shep sandbox init`: write a commented crabpot settings file to start from.

use core::fmt;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// The file written when no path is given.
const DEFAULT_PATH: &str = "sandbox.json";

/// crabpot reads plain JSON and drops the keys it does not know, so the
/// comments are `"//"` keys rather than JSONC.
const SKELETON: &str = include_str!("init/skeleton.json");

/// Why `shep sandbox init` wrote nothing.
#[derive(Debug)]
pub enum Error {
    /// More than one argument was given, or one started with `-`.
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage { arg } => {
                write!(f, "init takes at most one path, not `{arg}`")
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
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Write { source, .. } => Some(source),
            Self::Usage { .. } | Self::Exists(_) => None,
        }
    }
}

/// Write the settings skeleton to the one path in `args`, or to `sandbox.json`
/// when there is none. A relative path is read against `dir`. Returns the
/// path written.
///
/// # Errors
///
/// [`Error::Usage`] for a second argument or one that starts with `-`,
/// [`Error::Exists`] when a file is already at the path, and [`Error::Write`]
/// when it cannot be created.
pub fn run(dir: &Path, args: &[String]) -> Result<PathBuf, Error> {
    let unexpected = args
        .iter()
        .enumerate()
        .find(|(at, arg)| *at > 0 || arg.starts_with('-'));
    if let Some((_, arg)) = unexpected {
        return Err(Error::Usage { arg: arg.clone() });
    }
    let path = dir.join(args.first().map_or(DEFAULT_PATH, String::as_str));
    create(&path, |file| file.write_all(SKELETON.as_bytes()))?;
    Ok(path)
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
