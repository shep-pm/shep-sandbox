//! Run a shep sheep inside a crabpot sandbox.
//!
//! Adopted, the dog has no work of its own yet: it reads its `[sandbox]`
//! section and idles until the shepherd stops it. `shep sandbox init` and
//! `shep sandbox doctor` reach the same binary as a first argument. `init`
//! writes a settings file; `doctor` checks the host.
//!
//! `shep adopt` asks `--version`, `--schema` and `--shep-manifest` of a
//! candidate it has not registered. [`Probe::answer`] replies on the first
//! line of [`main`], before this process opens a socket or reads `$SHEP_HOME`.

mod config;
mod doctor;
mod error;
mod init;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

use shep_client::{
    Client,
    dogs::{
        self, DogAction, DogIdentity, DogRuntime, Probe, ShepherdError, Stop, Subcommand, Verb,
    },
};

use shep_client::shep_core::dogs::{DOG_LINK_FD_VAR, DOG_LINK_PIPE_VAR};

use crate::{config::Section, error::Error};

/// The `[<name>]` section to read when `$SHEP_DOG_NAME` is unset, which
/// means nothing adopted this process. It matches the name `shep adopt`
/// picks for this binary.
const DEFAULT_NAME: &str = "sandbox";

/// The verbs `shep sandbox` forwards to this binary.
const VERBS: [(&str, &str); 2] = [
    ("init", "Set up the sandbox for a sheep"),
    ("doctor", "Check that the sandbox is set up"),
];

/// Exit status for a verb given an argument it does not take, which is the
/// code shep uses for bad arguments.
const USAGE: u8 = shep_client::shep_core::exit::USAGE;

fn verb() -> Verb {
    let mut verb = Verb::new("Run a shep sheep inside a crabpot sandbox");
    verb.subcommands = VERBS
        .iter()
        .map(|(name, about)| Subcommand::new(*name, *about))
        .collect();
    verb
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    Probe::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .verb(verb())
        .answer::<Section>();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("doctor") => return doctor_verb(&args[1..]).await,
        Some("init") => return init_verb(&args[1..]).await,
        _ => {}
    }

    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("shep-sandbox: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

/// `shep sandbox doctor`: print every check and exit 1 if any failed.
async fn doctor_verb(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("shep-sandbox: `doctor` takes no arguments");
        return ExitCode::from(USAGE);
    }
    let report = doctor::run(&doctor::System::new(doctor::read_flock().await));
    print!("{report}");
    ExitCode::from(report.exit_code())
}

/// Write the settings file `shep sandbox init` asks for, in the current
/// directory unless the path says otherwise.
async fn init_verb(args: &[String]) -> ExitCode {
    match init_file(args).await {
        Ok(path) => {
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("shep-sandbox: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

/// Look up the sheep `--from` names, when it does, then write the file.
async fn init_file(args: &[String]) -> Result<PathBuf, Error> {
    let args = init::parse(args)?;
    let taken = match args.from() {
        Some(sheep) => {
            let paths = dogs::resolve_paths(&|key| std::env::var_os(key))?;
            let client = Client::connect(&paths.socket)
                .await
                .map_err(|err| init::Error::Shepherd(ShepherdError::Connect(err)))?;
            Some(init::take(&client, sheep).await?)
        }
        None => None,
    };
    Ok(init::run(Path::new("."), &args, taken.as_ref())?)
}

/// Whether the shepherd handed this process a connection, which it does only
/// to a dog it supervises. `shep <name>` sets `$SHEP_DOG_NAME` and no link.
fn is_supervised(var: &dyn Fn(&str) -> Option<OsString>) -> bool {
    var(DOG_LINK_FD_VAR).is_some() || var(DOG_LINK_PIPE_VAR).is_some()
}

/// Idle as an adopted dog until the shepherd stops it.
async fn run(args: &[String]) -> Result<(), Error> {
    let identity = DogIdentity::from_env(&|key| std::env::var(key).ok(), DEFAULT_NAME);
    let section = identity.section();
    match dogs::parse_args(section, args.iter().map(String::as_str))? {
        DogAction::Run => {}
        DogAction::PrintConfig => {
            println!("# The [{section}] section has no keys yet.");
            return Ok(());
        }
    }

    let named = identity.handshake().is_some();
    if named && !is_supervised(&|key| std::env::var_os(key)) {
        return Err(Error::Bare {
            verbs: VERBS.map(|(name, _)| name),
        });
    }

    let mut stop = Stop::on_stop_signals();
    let paths = dogs::resolve_paths(&|key| std::env::var_os(key))?;
    let runtime = DogRuntime::start(identity, paths).await?;
    let _: Section = runtime.config()?;

    stop.wait().await;
    Ok(())
}
