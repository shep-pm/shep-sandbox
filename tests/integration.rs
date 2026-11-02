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
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

const DOG_BIN: &str = env!("CARGO_BIN_EXE_shep-sandbox");

/// The name the connection test adopts the dog under, so it does not claim
/// the `sandbox` name that routes a sheep's launch.
const DOG_NAME: &str = "sandbox-it";

/// The name a sheep's launch goes through: the shepherd spawns a sheep that
/// carries an `[app.dogs.sandbox]` table by this dog.
const LAUNCH_NAME: &str = "sandbox";

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
            "adopt", DOG_BIN, "--name", DOG_NAME, "--yes", "--style", "bare",
        ]);
    }

    /// Adopt the dog as [`LAUNCH_NAME`], which is the name the shepherd
    /// spawns a sandboxed sheep through.
    fn adopt_launch_dog(&self) {
        self.ok(&[
            "adopt",
            DOG_BIN,
            "--name",
            LAUNCH_NAME,
            "--yes",
            "--style",
            "bare",
        ]);
    }

    /// Start the one sheep a Flockfile holds, which runs `script` from `cwd`
    /// under the table `[app.dogs.sandbox]` holds.
    fn start_boxed(&self, cwd: &Path, script: &Path, table: &str) -> Output {
        let flockfile = self.home.path().join("Flockfile.toml");
        fs::write(
            &flockfile,
            format!(
                "[[app]]\nname = \"boxed\"\nscript = \"{}\"\ncwd = \"{}\"\nautorestart = false\n\n[app.dogs.sandbox]\n{table}\n",
                script.display(),
                cwd.display()
            ),
        )
        .expect("a Flockfile");
        self.run(&[
            "start",
            flockfile.to_str().expect("Flockfile path"),
            "--style",
            "bare",
        ])
    }

    /// The `last_exit` code of the sheep named `boxed`, once it has one.
    fn boxed_exit(&self) -> Option<i64> {
        let listing: serde_json::Value =
            serde_json::from_str(&self.ok(&["list", "--format", "json"])).expect("JSON listing");
        listing["data"]
            .as_array()?
            .iter()
            .find(|sheep| sheep["name"] == "boxed")?["last_exit"]["code"]
            .as_i64()
    }

    /// The [`DOG_NAME`] entry of `shep dogs`.
    fn dog(&self) -> serde_json::Value {
        let listing: serde_json::Value =
            serde_json::from_str(&self.ok(&["dogs", "--format", "json"])).expect("JSON listing");
        listing["data"]
            .as_array()
            .expect("a data array")
            .iter()
            .find(|dog| dog["name"] == DOG_NAME)
            .cloned()
            .expect("the adopted dog is listed")
    }
}

impl Drop for Shepherd {
    fn drop(&mut self) {
        // Ignored: a test that already failed must report its own reason.
        let _ = self.run(&["kill", "--style", "bare"]);
    }
}

/// Poll `ready` until it answers true, or fail naming `what`.
fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
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

    shepherd.ok(&["stop", DOG_NAME, "--style", "bare"]);
    wait_until("the dog to exit", || {
        shepherd.dog()["last_exit"]["code"] == 0
    });
    assert_eq!(shepherd.dog()["status"], "stopped");
}

/// A script, executable, in `dir`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Wait for `file` to hold text, and return it.
fn read_when_written(file: &Path) -> String {
    wait_until(&format!("{} to be written", file.display()), || {
        fs::read_to_string(file).is_ok_and(|text| !text.is_empty())
    });
    fs::read_to_string(file).expect("the file was written")
}

#[test]
fn a_sheep_with_the_sandbox_dog_runs_its_command_under_crabpot() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_launch_dog();
    let cwd = tempfile::tempdir().expect("the sheep's cwd");
    let script = script(cwd.path(), "run.sh", "echo ran > ran.txt");

    let started = shepherd.start_boxed(cwd.path(), &script, "");

    assert!(started.status.success(), "{started:?}");
    assert_eq!(read_when_written(&cwd.path().join("ran.txt")), "ran\n");
}

#[test]
fn a_sheep_cannot_write_outside_its_cwd_through_the_dog() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_launch_dog();
    let cwd = tempfile::tempdir().expect("the sheep's cwd");
    let outside = tempfile::tempdir().expect("a directory beside it");
    let escaped = outside.path().join("escaped.txt");
    let script = script(
        cwd.path(),
        "run.sh",
        &format!(
            "echo inside > inside.txt\nif echo outside > {path} 2>/dev/null; then echo wrote; else echo denied; fi > verdict.txt",
            path = escaped.display()
        ),
    );

    let started = shepherd.start_boxed(cwd.path(), &script, "");

    assert!(started.status.success(), "{started:?}");
    assert_eq!(
        read_when_written(&cwd.path().join("verdict.txt")),
        "denied\n"
    );
    assert!(cwd.path().join("inside.txt").exists());
    assert!(!escaped.exists(), "the sheep wrote outside its cwd");
}

#[test]
fn a_sheep_whose_table_crabpot_refuses_exits_125_and_never_runs_unsandboxed() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_launch_dog();
    let cwd = tempfile::tempdir().expect("the sheep's cwd");
    let script = script(cwd.path(), "run.sh", "echo ran > ran.txt");

    // Only a denied entry may be `*`. The shepherd may refuse the table when
    // it starts the sheep, or the dog when it launches it, and either way
    // the command must not run.
    let started = shepherd.start_boxed(cwd.path(), &script, "network.allowedDomains = [\"*\"]");

    if started.status.success() {
        wait_until("the sheep to exit", || shepherd.boxed_exit().is_some());
        assert_eq!(shepherd.boxed_exit(), Some(125));
    }
    assert!(!cwd.path().join("ran.txt").exists(), "the command ran");
}

#[test]
fn the_sandboxed_command_cannot_connect_to_the_shepherds_control_socket() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_launch_dog();
    let cwd = tempfile::tempdir().expect("the sheep's cwd");
    // The dog binary asks the shepherd over `$SHEP_HOME/run/shep.sock`, so
    // what it reaches from inside is what any process of the sheep reaches.
    let script = script(
        cwd.path(),
        "run.sh",
        &format!(
            "test -n \"$SHEP_HOME\" && echo set > home.txt\n{dog} launch boxed -- true 2> reach.txt\necho $? > code.txt",
            dog = DOG_BIN
        ),
    );

    let started = shepherd.start_boxed(cwd.path(), &script, "");

    assert!(started.status.success(), "{started:?}");
    assert_eq!(read_when_written(&cwd.path().join("code.txt")), "125\n");
    assert_eq!(
        fs::read_to_string(cwd.path().join("home.txt")).unwrap_or_default(),
        "set\n"
    );
    let reach = fs::read_to_string(cwd.path().join("reach.txt")).expect("the dog's refusal");
    assert!(reach.contains("cannot reach the shepherd"), "{reach}");

    // The same call from outside the sandbox does reach it.
    let outside = Command::new(DOG_BIN)
        .args(["launch", "boxed", "--", "true"])
        .env("SHEP_HOME", shepherd.home.path())
        .env_remove("SHEP_DOG_NAME")
        .current_dir(cwd.path())
        .output()
        .expect("the dog ran");
    assert_eq!(outside.status.code(), Some(0), "{outside:?}");
}
