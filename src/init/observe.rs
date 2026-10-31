//! What a running sheep's sockets say about its network needs.
//!
//! The parsers take text and are tested from fixtures. Only [`sockets`]
//! reads the machine.

use core::fmt;
use std::{collections::BTreeSet, io, net::SocketAddr};

pub mod linux;
pub mod macos;

/// The port whose dials `allowedDomains` covers by name.
const HTTPS_PORT: u16 = 443;

/// One TCP socket a sheep holds, in a state that says what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    /// Waiting for connections on `0`.
    Listening(SocketAddr),
    /// An open connection, whichever end the sheep is.
    Connected {
        /// The sheep's end.
        local: SocketAddr,
        /// The other end.
        remote: SocketAddr,
    },
}

/// What a sheep listens on and dials.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// Where it accepts connections.
    pub listening: BTreeSet<SocketAddr>,
    /// Where it dials, besides HTTPS.
    pub dials: BTreeSet<SocketAddr>,
    /// Where it dials on the HTTPS port.
    pub https: BTreeSet<SocketAddr>,
}

impl Observed {
    /// Sort `sockets` into listeners and dials.
    ///
    /// A connection whose local end a listener serves was accepted, not
    /// dialed. A dial to a loopback address that one of the sheep's own
    /// listeners serves is the sheep talking to itself.
    pub fn from_sockets(sockets: &[Socket]) -> Self {
        let listening: BTreeSet<SocketAddr> = sockets
            .iter()
            .filter_map(|socket| match socket {
                Socket::Listening(addr) => Some(*addr),
                Socket::Connected { .. } => None,
            })
            .collect();
        let served = |addr: &SocketAddr| listening.iter().any(|own| serves(own, addr));

        let mut observed = Self::default();
        for socket in sockets {
            let Socket::Connected { local, remote } = socket else {
                continue;
            };
            if served(local) || (remote.ip().is_loopback() && served(remote)) {
                continue;
            }
            if remote.port() == HTTPS_PORT {
                observed.https.insert(*remote);
            } else {
                observed.dials.insert(*remote);
            }
        }
        observed.listening = listening;
        observed
    }
}

/// Whether a connection to or from `addr` is one the listener `own` takes.
///
/// A wildcard listener takes every address of its family. `[::]` takes both
/// families, as a dual-stack socket does.
fn serves(own: &SocketAddr, addr: &SocketAddr) -> bool {
    own.port() == addr.port()
        && (own.ip() == addr.ip()
            || (own.ip().is_unspecified() && (own.is_ipv6() || addr.is_ipv4())))
}

/// Why a sheep's sockets could not be read.
#[derive(Debug)]
pub enum Error {
    /// A file under `/proc` could not be read, such as another user's
    /// `fd` folder.
    Proc {
        /// The path that was read.
        path: String,
        /// What the filesystem refused.
        source: io::Error,
    },
    /// `lsof` could not be run or exited with an error.
    Lsof(String),
    /// The sheep's own process exited before its sockets were read.
    Gone,
    /// Neither `/proc` nor `lsof` is how this platform reports sockets.
    Unsupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proc { path, source } => write!(f, "could not read {path}: {source}"),
            Self::Lsof(why) => write!(f, "lsof failed: {why}"),
            Self::Gone => f.write_str("the process exited before its sockets were read"),
            Self::Unsupported => f.write_str("reading a sheep's sockets needs Linux or macOS"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Proc { source, .. } => Some(source),
            Self::Lsof(_) | Self::Gone | Self::Unsupported => None,
        }
    }
}

/// The TCP sockets held by `pids`, which are a sheep and its children.
///
/// # Errors
///
/// [`Error`] when the platform's socket listing cannot be read.
pub fn sockets(pids: &[u32]) -> Result<Vec<Socket>, Error> {
    if cfg!(target_os = "linux") {
        linux::read(pids)
    } else if cfg!(target_os = "macos") {
        macos::read(pids)
    } else {
        Err(Error::Unsupported)
    }
}
