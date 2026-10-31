//! Why the dog stopped.

use core::fmt;

use shep_client::dogs::{HomeError, SectionError, ShepherdError, UsageError};

use crate::init;

/// What stopped the dog, each carrying the exit code shep itself would use
/// for the same cause.
#[derive(Debug)]
pub enum Error {
    /// The command line held an argument the dog does not take.
    Usage(UsageError),
    /// No shep home could be found to reach the shepherd through.
    Home(HomeError),
    /// The shepherd could not be reached, or failed the dog once reached.
    Shepherd(ShepherdError),
    /// The `[sandbox]` section of `dogs.toml` has a key the dog does not have.
    Config(SectionError),
    /// `shep sandbox init` wrote nothing.
    Init(init::Error),
    /// `shep <name>` ran the dog with no subcommand: named by the shepherd,
    /// but holding no connection to it, so not the supervised dog.
    Bare {
        /// The subcommands the dog has, for the usage line.
        verbs: [&'static str; 2],
    },
}

impl Error {
    /// The process exit code for this cause.
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(err) => err.exit_code(),
            Self::Home(err) => err.exit_code(),
            Self::Shepherd(err) => err.exit_code(),
            Self::Config(err) => err.exit_code(),
            Self::Init(init::Error::Usage { .. }) | Self::Bare { .. } => {
                shep_client::shep_core::exit::USAGE
            }
            Self::Init(init::Error::UnknownSheep { .. }) => shep_client::shep_core::exit::NOT_FOUND,
            Self::Init(init::Error::Shepherd(err)) => err.exit_code(),
            Self::Init(_) => shep_client::shep_core::exit::FAILURE,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(err) => err.fmt(f),
            Self::Home(err) => err.fmt(f),
            Self::Shepherd(err) => err.fmt(f),
            Self::Config(err) => err.fmt(f),
            Self::Init(err) => err.fmt(f),
            Self::Bare { verbs } => write!(f, "takes a subcommand: {}", verbs.join(", ")),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Usage(err) => Some(err),
            Self::Home(err) => Some(err),
            Self::Shepherd(err) => Some(err),
            Self::Config(err) => Some(err),
            Self::Init(err) => Some(err),
            Self::Bare { .. } => None,
        }
    }
}

impl From<UsageError> for Error {
    fn from(err: UsageError) -> Self {
        Self::Usage(err)
    }
}

impl From<HomeError> for Error {
    fn from(err: HomeError) -> Self {
        Self::Home(err)
    }
}

impl From<ShepherdError> for Error {
    fn from(err: ShepherdError) -> Self {
        Self::Shepherd(err)
    }
}

impl From<SectionError> for Error {
    fn from(err: SectionError) -> Self {
        Self::Config(err)
    }
}

impl From<init::Error> for Error {
    fn from(err: init::Error) -> Self {
        Self::Init(err)
    }
}
