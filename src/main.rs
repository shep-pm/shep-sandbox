//! Run a shep sheep inside a crabpot sandbox.
//!
//! Adopted, the dog has no work of its own yet: it reads its `[sandbox]`
//! section and idles until the shepherd stops it. It declares no verb.
//!
//! `shep adopt` asks `--version`, `--schema` and `--shep-manifest` of a
//! candidate it has not registered. [`Probe::answer`] replies on the first
//! line of [`main`], before this process opens a socket or reads `$SHEP_HOME`.

mod config;
mod error;

use std::{ffi::OsString, process::ExitCode};

use shep_client::dogs::{self, DogAction, DogIdentity, DogRuntime, Probe, Stop};

use shep_client::shep_core::dogs::{DOG_LINK_FD_VAR, DOG_LINK_PIPE_VAR};

use crate::{config::Section, error::Error};

/// The `[<name>]` section to read when `$SHEP_DOG_NAME` is unset, which
/// means nothing adopted this process. It matches the name `shep adopt`
/// picks for this binary.
const DEFAULT_NAME: &str = "sandbox";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    Probe::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).answer::<Section>();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("shep-sandbox: {err}");
            ExitCode::from(err.exit_code())
        }
    }
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
        return Err(Error::Bare);
    }

    let mut stop = Stop::on_stop_signals();
    let paths = dogs::resolve_paths(&|key| std::env::var_os(key))?;
    let runtime = DogRuntime::start(identity, paths).await?;
    let _: Section = runtime.config()?;

    stop.wait().await;
    Ok(())
}
