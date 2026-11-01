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
    process::{Command, Output, Stdio},
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
fn doctor_reaches_the_binary_through_shep_and_prints_its_checks() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();

    let output = shepherd
        .command()
        .args(["sandbox", "doctor"])
        .output()
        .expect("shep ran");

    // Whether the runner has crabpot decides 0 or 1, and 2 would mean the
    // verb never reached the doctor.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{:?}: {}",
        output.status.code(),
        stderr(&output)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("crabpot"), "{printed}");
}

#[test]
#[cfg(target_os = "linux")]
fn doctor_finds_descriptor_3_reaching_a_command_under_the_real_crabpot() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();

    let output = shepherd
        .command()
        .args(["sandbox", "doctor"])
        .output()
        .expect("shep ran");

    let printed = String::from_utf8_lossy(&output.stdout);
    let channel = printed
        .lines()
        .find(|line| line.contains("shepherd channel"))
        .unwrap_or_else(|| panic!("no shepherd channel line: {printed}"));
    assert!(channel.starts_with("ok "), "{channel}\n{printed}");
}

#[test]
fn init_reaches_the_binary_through_shep_and_writes_the_file() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();
    let dir = tempfile::tempdir().expect("a temp dir");

    let output = shepherd
        .command()
        .current_dir(dir.path())
        .args(["sandbox", "init"])
        .output()
        .expect("shep ran");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(dir.path().join("sandbox.json").is_file());
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

#[test]
fn shep_sandbox_with_no_subcommand_exits_2_beside_a_running_shepherd() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.adopt_dog();
    wait_until("the dog's handshake", || {
        shepherd.dog()["handshook"] == true
    });

    let mut child = shepherd
        .command()
        .arg("sandbox")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("shep ran");
    let mut status = None;
    // A dog that connected as the adopted one would wait here for a stop.
    wait_until("`shep sandbox` to exit", || {
        status = child.try_wait().expect("exit status");
        status.is_some()
    });
    let _ = child.kill();

    assert_eq!(status.and_then(|status| status.code()), Some(2));
}

/// A TCP port nothing holds right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("its address")
        .port()
}

#[test]
fn init_from_a_running_sheep_publishes_what_it_listens_on_and_opens_its_cwd() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();
    let cwd = tempfile::tempdir().expect("a cwd for the sheep");
    let cwd = cwd.path().canonicalize().expect("a canonical cwd");
    let port = free_port();
    // Not `exec`: the listener is a child of the sheep, as a wrapped service is.
    let serve = shepherd.home.path().join("serve.sh");
    fs::write(
        &serve,
        format!("#!/bin/sh\npython3 -m http.server {port} --bind 127.0.0.1\n"),
    )
    .expect("serve script");
    fs::set_permissions(&serve, fs::Permissions::from_mode(0o755)).expect("chmod");
    shepherd.ok(&[
        "start",
        serve.to_str().expect("script path"),
        "--name",
        "svc",
        "--cwd",
        cwd.to_str().expect("cwd path"),
        "--style",
        "bare",
    ]);
    wait_until("the sheep to listen", || {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    let dir = tempfile::tempdir().expect("a temp dir");

    let output = shepherd
        .command()
        .current_dir(dir.path())
        .args(["sandbox", "init", "--from", "svc"])
        .output()
        .expect("shep ran");

    assert!(output.status.success(), "{}", stderr(&output));
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("sandbox.json")).unwrap())
            .expect("JSON");
    assert_eq!(
        written["network"]["publish"],
        serde_json::json!([format!("127.0.0.1:{port}")])
    );
    assert_eq!(
        written["filesystem"]["allowWrite"],
        serde_json::json!([cwd.to_str().unwrap()])
    );
}

#[test]
fn init_from_a_sheep_that_is_stopped_is_refused_by_name() {
    let shepherd = Shepherd::new();
    shepherd.boot();
    shepherd.ok(&["stop", "idle", "--style", "bare"]);
    shepherd.adopt_dog();
    let dir = tempfile::tempdir().expect("a temp dir");

    let output = shepherd
        .command()
        .current_dir(dir.path())
        .args(["sandbox", "init", "--from", "idle"])
        .output()
        .expect("shep ran");

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("idle is not running"));
    assert!(!dir.path().join("sandbox.json").exists());
}

#[test]
fn doctor_names_a_sheep_started_with_sandbox_set() {
    let shepherd = Shepherd::new();
    shepherd.adopt_dog();
    let cwd = tempfile::tempdir().expect("a cwd for the sheep");
    let cwd = cwd.path().canonicalize().expect("a canonical cwd");
    fs::write(cwd.join("idle.sh"), "#!/bin/sh\nsleep 300\n").expect("idle script");
    fs::set_permissions(cwd.join("idle.sh"), fs::Permissions::from_mode(0o755)).expect("chmod");
    let made = shepherd
        .command()
        .current_dir(&cwd)
        .args(["sandbox", "init"])
        .output()
        .expect("shep ran");
    assert!(made.status.success(), "{}", stderr(&made));
    let flockfile = cwd.join("flock.toml");
    fs::write(
        &flockfile,
        format!(
            "[[app]]\nname = \"boxed\"\nscript = \"{dir}/idle.sh\"\ncwd = \"{dir}\"\nsandbox = \"sandbox.json\"\n",
            dir = cwd.to_str().expect("cwd path")
        ),
    )
    .expect("flockfile");
    shepherd.ok(&[
        "start",
        flockfile.to_str().expect("flockfile path"),
        "--style",
        "bare",
    ]);

    let output = shepherd
        .command()
        .args(["sandbox", "doctor"])
        .output()
        .expect("shep ran");

    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.lines().any(|line| line.contains("sheep boxed")),
        "{printed}"
    );
    assert!(!printed.contains("flock:"), "{printed}");
}
