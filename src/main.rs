//! Run a shep sheep inside a crabpot sandbox.
//!
//! Adopted, the dog reads its `[sandbox]` section, checks each sheep's
//! `[app.dogs.sandbox]` table against crabpot before `shep` writes it, and
//! idles until the shepherd stops it. `launch <sheep> -- <command>` runs a
//! command under crabpot with the sheep's table instead.
//!
//! `shep adopt` asks `--version`, `--schema` and `--shep-manifest` of a
//! candidate it has not registered. [`Probe::answer`] replies on the first
//! line of [`main`], before this process opens a socket or reads `$SHEP_HOME`.

mod config;
mod error;
mod launch;
mod settings;

use std::{ffi::OsString, process::ExitCode};

use shep_client::{
    dogs::{self, DogAction, DogIdentity, DogRuntime, Probe, Stop, validate_answer},
    shep_core::dogs::{DOG_LINK_FD_VAR, DOG_LINK_PIPE_VAR, validate::VALIDATE_ACTION},
};

use crate::{config::Section, error::Error, settings::SheepSettings};

/// The `[<name>]` section to read when `$SHEP_DOG_NAME` is unset, which
/// means nothing adopted this process. It matches the name `shep adopt`
/// picks for this binary.
const DEFAULT_NAME: &str = "sandbox";

fn main() -> ExitCode {
    // First, because the Linux sandbox re-enters this binary with
    // `--crabpot-helper`, and anything else that ran first would run in there.
    crabpot::run_from_args();

    Probe::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .ask_for_channel()
        .validate()
        .launch()
        .answer_with_sheep::<Section, SheepSettings>();

    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|word| word == "launch") {
        let args: Vec<OsString> = args.collect();
        return launch::run(&args);
    }
    adopted()
}

#[tokio::main(flavor = "current_thread")]
async fn adopted() -> ExitCode {
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

    // With the channel asked for, the shepherd stops the dog by message, and
    // signals only once `kill_timeout` runs out.
    let (mut shutdown, request_shutdown) = Stop::new();
    shep_channel::serve()
        .on_action(VALIDATE_ACTION, |params, _name| {
            validate_answer(params, settings::verdict)
        })
        .on_shutdown(move || request_shutdown.request());

    let mut stop = Stop::on_stop_signals();
    let paths = dogs::resolve_paths(&|key| std::env::var_os(key))?;
    let runtime = DogRuntime::start(identity, paths).await?;
    let _: Section = runtime.config()?;

    tokio::select! {
        () = stop.wait() => {}
        () = shutdown.wait() => {}
    }
    Ok(())
}
