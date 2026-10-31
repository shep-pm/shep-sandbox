//! The tier that drives a REAL shepherd.
//!
//! Gated behind the `integration` feature and needing `$SHEP_BIN` pointed at
//! a built `shep`, which a fresh clone does not have:
//!
//! ```text
//! cargo install --git https://github.com/shep-pm/shep --branch main shep --bin shep --locked
//! SHEP_BIN="$(command -v shep)" cargo test --features integration
//! ```
//!
//! Every test runs its shepherd in its own temporary `$SHEP_HOME`, set in the
//! child's environment as well as in `--home`: `shep adopt` spawns the binary
//! it vets with this process's environment, and one without a `SHEP_HOME`
//! would reach the operator's own shepherd.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
    time::{Duration, Instant},
};

const DOG_BIN: &str = env!("CARGO_BIN_EXE_shep-sandbox");

/// A ceiling on every poll, so a dog that never connects fails the test with
/// a sentence rather than hanging the job until CI's timeout.
const PATIENCE: Duration = Duration::from_secs(60);

/// One shepherd in its own temporary `$SHEP_HOME`, killed on drop.
struct Shepherd {
    home: tempfile::TempDir,
    shep: PathBuf,
}

impl Shepherd {
    fn new() -> Self {
        let shep = PathBuf::from(
            std::env::var("SHEP_BIN")
                .expect("the integration tier needs $SHEP_BIN pointing at a built shep binary"),
        );
        assert!(
            shep.is_file(),
            "$SHEP_BIN is not a file: {}",
            shep.display()
        );

        let home = tempfile::tempdir().expect("a temporary $SHEP_HOME");
        // The kernel bounds a unix socket path at 104 bytes on macOS and 108
        // on Linux, and a daemon that cannot bind says only that it exited.
        let socket = home.path().join("run/shep.sock");
        assert!(
            socket.as_os_str().len() < 100,
            "$TMPDIR is too deep for a unix socket: {} is {} bytes",
            socket.display(),
            socket.as_os_str().len()
        );
        Self { home, shep }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.shep);
        command.env("SHEP_HOME", self.home.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .arg("--home")
            .arg(self.home.path())
            .output()
            .expect("shep ran")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "shep {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Start a shepherd by starting a sheep that only sleeps, because `adopt`
    /// starts the dog now only when a shepherd is already running.
    fn boot(&self) {
        let idle = self.home.path().join("idle.sh");
        fs::write(&idle, "#!/bin/sh\nsleep 300\n").expect("idle script");
        fs::set_permissions(&idle, fs::Permissions::from_mode(0o755)).expect("chmod");
        self.ok(&[
            "start",
            idle.to_str().expect("script path"),
            "--name",
            "idle",
            "--style",
            "bare",
        ]);
    }

    fn adopt_dog(&self) {
        self.ok(&[
            "adopt", DOG_BIN, "--name", "sandbox", "--yes", "--style", "bare",
        ]);
    }

    /// The `sandbox` entry of `shep dogs`.
    fn dog(&self) -> serde_json::Value {
        let listing: serde_json::Value =
            serde_json::from_str(&self.ok(&["dogs", "--format", "json"])).expect("JSON listing");
        listing["data"]
            .as_array()
            .expect("a data array")
            .iter()
            .find(|dog| dog["name"] == "sandbox")
            .cloned()
            .expect("the sandbox dog is listed")
    }
}

impl Drop for Shepherd {
    fn drop(&mut self) {
        // Ignored: a test that already failed must report its own reason.
        let _ = self.run(&["kill", "--style", "bare"]);
    }
}

/// Poll `ready` until it answers true, or fail naming `what`.
fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_adopted_dog_connects_and_exits_0_when_stopped() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_dog();

    // `handshook` is the shepherd's own record that the dog connected under
    // its adopted name. A dog that idled without connecting is `silent`.
    wait_until("the dog's handshake", || {
        let dog = shepherd.dog();
        dog["status"] == "online" && dog["handshook"] == true
    });

    shepherd.ok(&["stop", "sandbox", "--style", "bare"]);
    wait_until("the dog to exit", || {
        shepherd.dog()["last_exit"]["code"] == 0
    });
    assert_eq!(shepherd.dog()["status"], "stopped");
}

#[test]
fn a_verb_reaches_the_binary_through_shep_and_exits_2() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();

    for verb in ["init", "doctor"] {
        let output = shepherd
            .command()
            .args(["sandbox", verb])
            .output()
            .expect("shep ran");

        assert_eq!(output.status.code(), Some(2), "{verb}: {}", stderr(&output));
        assert!(stderr(&output).contains("not built yet"), "{verb}");
    }
}

#[test]
fn shep_help_lists_the_dog_with_its_manifest_description() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();

    let help = String::from_utf8_lossy(
        &shepherd
            .command()
            .arg("--help")
            .output()
            .expect("shep ran")
            .stdout,
    )
    .into_owned();

    assert!(
        help.contains("Run a shep sheep inside a crabpot sandbox"),
        "{help}"
    );
}
