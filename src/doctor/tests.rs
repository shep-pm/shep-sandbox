use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use shep_client::shep_core::{config::AppConfig, protocol::SANDBOX_SINCE};

use super::{
    Command, Env, Os, Output, Report, SandboxedSheep, Status,
    flock::{self, FlockError},
    run,
};

const PROBE_SETTINGS: &str = "/tmp/probe.json";

/// How a scripted command ends.
#[derive(Clone)]
enum Reply {
    Exit(i32, &'static str),
    Absent,
}

/// A host that answers from tables. A command it was not scripted for fails
/// the test, so a check cannot run something unseen.
struct Fake {
    os: Os,
    files: BTreeMap<PathBuf, String>,
    replies: BTreeMap<String, Reply>,
    flock: Result<Vec<SandboxedSheep>, FlockError>,
    ran: RefCell<Vec<Command>>,
    removed: RefCell<Vec<PathBuf>>,
    written: RefCell<Vec<String>>,
}

impl Fake {
    /// Linux on Ubuntu 24.04 with everything in place.
    fn linux() -> Self {
        let mut fake = Self {
            os: Os::Linux,
            files: BTreeMap::new(),
            replies: BTreeMap::new(),
            flock: Ok(Vec::new()),
            ran: RefCell::default(),
            removed: RefCell::default(),
            written: RefCell::default(),
        };
        fake.file("/etc/os-release", "ID=ubuntu\nVERSION_ID=\"24.04\"\n");
        fake.file(
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            "0\n",
        );
        fake.file(
            "/sys/kernel/security/apparmor/profiles",
            "docker-default (enforce)\n",
        );
        fake.reply("crabpot --version", Reply::Exit(0, "crabpot 0.9.0\n"));
        fake.reply("bwrap --version", Reply::Exit(0, "bubblewrap 0.9.0\n"));
        fake.reply(
            &format!("crabpot -s {PROBE_SETTINGS} -- sh -c test -e /proc/self/fd/3"),
            Reply::Exit(0, ""),
        );
        fake
    }

    fn macos() -> Self {
        let mut fake = Self::linux();
        fake.os = Os::Mac;
        fake.files.clear();
        fake.file("/usr/bin/sandbox-exec", "");
        fake
    }

    fn file(&mut self, path: &str, contents: &str) {
        self.files.insert(path.into(), contents.to_owned());
    }

    fn reply(&mut self, line: &str, reply: Reply) {
        self.replies.insert(line.to_owned(), reply);
    }

    fn sheep(&mut self, name: &str, settings: &str, cwd: &str) {
        self.flock
            .as_mut()
            .expect("a readable flock")
            .push(SandboxedSheep {
                name: name.to_owned(),
                settings: settings.into(),
                cwd: cwd.into(),
            });
    }
}

fn line(command: &Command) -> String {
    let mut line = command.program.clone();
    for arg in &command.args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}

impl Env for Fake {
    fn os(&self) -> Os {
        self.os
    }

    fn run(&self, command: &Command) -> io::Result<Output> {
        self.ran.borrow_mut().push(command.clone());
        match self.replies.get(&line(command)) {
            Some(Reply::Exit(code, stderr)) => Ok(Output {
                code: Some(*code),
                stdout: if *code == 0 {
                    (*stderr).to_owned()
                } else {
                    String::new()
                },
                stderr: if *code == 0 {
                    String::new()
                } else {
                    (*stderr).to_owned()
                },
            }),
            Some(Reply::Absent) => Err(io::ErrorKind::NotFound.into()),
            None => panic!("unscripted command: {}", line(command)),
        }
    }

    fn read(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn write_temp(&self, contents: &str) -> io::Result<PathBuf> {
        self.written.borrow_mut().push(contents.to_owned());
        Ok(PROBE_SETTINGS.into())
    }

    fn remove(&self, path: &Path) {
        self.removed.borrow_mut().push(path.to_owned());
    }

    fn sandboxed_sheep(&self) -> Result<Vec<SandboxedSheep>, FlockError> {
        self.flock.clone()
    }
}

fn status<'a>(report: &'a Report, name: &str) -> &'a Status {
    &report
        .checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("no check named {name}: {report}"))
        .status
}

fn fix<'a>(report: &'a Report, name: &str) -> &'a str {
    match status(report, name) {
        Status::Fail { fix, .. } => fix,
        other => panic!("{name} did not fail: {other:?}"),
    }
}

fn is_pass(report: &Report, name: &str) -> bool {
    matches!(status(report, name), Status::Pass(_))
}

fn is_skip(report: &Report, name: &str) -> bool {
    matches!(status(report, name), Status::Skip(_))
}

fn is_fail(report: &Report, name: &str) -> bool {
    matches!(status(report, name), Status::Fail { .. })
}

#[test]
fn crabpot_on_path_passes_with_its_version() {
    let report = run(&Fake::linux());

    assert_eq!(
        status(&report, "crabpot"),
        &Status::Pass("crabpot 0.9.0".to_owned())
    );
}

#[test]
fn crabpot_missing_fails_with_an_install_command() {
    let mut fake = Fake::linux();
    fake.reply("crabpot --version", Reply::Absent);

    let report = run(&fake);

    assert!(is_fail(&report, "crabpot"));
    assert!(fix(&report, "crabpot").contains("cargo install crabpot"));
}

#[test]
fn bwrap_on_path_passes_with_its_version() {
    let report = run(&Fake::linux());

    assert_eq!(
        status(&report, "bwrap"),
        &Status::Pass("bubblewrap 0.9.0".to_owned())
    );
}

#[test]
fn bwrap_missing_fails_with_an_install_command() {
    let mut fake = Fake::linux();
    fake.reply("bwrap --version", Reply::Absent);

    let report = run(&fake);

    assert!(is_fail(&report, "bwrap"));
    assert!(fix(&report, "bwrap").contains("apt install bubblewrap"));
}

#[test]
fn sandbox_exec_is_checked_on_macos_and_bwrap_is_skipped() {
    let report = run(&Fake::macos());

    assert!(is_pass(&report, "sandbox-exec"));
    assert!(is_skip(&report, "bwrap"));
}

#[test]
fn a_missing_sandbox_exec_fails_on_macos() {
    let mut fake = Fake::macos();
    fake.files.clear();

    assert!(is_fail(&run(&fake), "sandbox-exec"));
}

#[test]
fn sandbox_exec_is_skipped_on_linux_and_says_so() {
    let report = run(&Fake::linux());

    assert_eq!(
        status(&report, "sandbox-exec"),
        &Status::Skip("macOS only".to_owned())
    );
}

#[test]
fn a_restricted_userns_sysctl_fails_with_the_sysctl_file_to_write() {
    let mut fake = Fake::linux();
    fake.file(
        "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
        "1\n",
    );

    let report = run(&fake);

    let fix = fix(&report, "apparmor_restrict_unprivileged_userns");
    assert!(fix.contains(
        "echo 'kernel.apparmor_restrict_unprivileged_userns = 0' | \
         sudo tee /etc/sysctl.d/60-apparmor-userns.conf"
    ));
    assert!(fix.contains("sudo sysctl --system"));
}

#[test]
fn an_open_userns_sysctl_passes() {
    assert!(is_pass(
        &run(&Fake::linux()),
        "apparmor_restrict_unprivileged_userns"
    ));
}

#[test]
fn a_loaded_bwrap_profile_fails_with_the_disable_link_and_the_unload() {
    let mut fake = Fake::linux();
    fake.file(
        "/sys/kernel/security/apparmor/profiles",
        "bwrap-userns-restrict (enforce)\nfoo (enforce)\n",
    );

    let report = run(&fake);

    let fix = fix(&report, "bwrap-userns-restrict");
    assert!(
        fix.contains("sudo ln -s /etc/apparmor.d/bwrap-userns-restrict /etc/apparmor.d/disable/")
    );
    assert!(fix.contains("sudo apparmor_parser -R /etc/apparmor.d/bwrap-userns-restrict"));
}

#[test]
fn a_bwrap_profile_that_is_not_loaded_passes() {
    assert!(is_pass(&run(&Fake::linux()), "bwrap-userns-restrict"));
}

#[test]
fn an_unreadable_profile_list_is_skipped_with_the_reason() {
    let mut fake = Fake::linux();
    fake.files
        .remove(Path::new("/sys/kernel/security/apparmor/profiles"));

    let report = run(&fake);

    let Status::Skip(reason) = status(&report, "bwrap-userns-restrict") else {
        panic!("not skipped: {report}");
    };
    assert!(reason.contains("cannot read"));
}

#[test]
fn the_apparmor_checks_are_skipped_before_ubuntu_24_04() {
    let mut fake = Fake::linux();
    fake.file("/etc/os-release", "ID=ubuntu\nVERSION_ID=\"22.04\"\n");

    let report = run(&fake);

    assert!(is_skip(&report, "apparmor_restrict_unprivileged_userns"));
    assert!(is_skip(&report, "bwrap-userns-restrict"));
}

#[test]
fn the_apparmor_checks_are_skipped_off_ubuntu() {
    let mut fake = Fake::linux();
    fake.file("/etc/os-release", "ID=debian\nVERSION_ID=\"12\"\n");

    assert!(is_skip(
        &run(&fake),
        "apparmor_restrict_unprivileged_userns"
    ));
}

#[test]
fn the_apparmor_checks_run_on_a_later_ubuntu() {
    let mut fake = Fake::linux();
    fake.file("/etc/os-release", "ID=ubuntu\nVERSION_ID=\"25.10\"\n");
    fake.file(
        "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
        "1\n",
    );

    assert!(is_fail(
        &run(&fake),
        "apparmor_restrict_unprivileged_userns"
    ));
}

#[test]
fn the_apparmor_checks_are_skipped_on_macos() {
    let report = run(&Fake::macos());

    assert!(is_skip(&report, "apparmor_restrict_unprivileged_userns"));
    assert!(is_skip(&report, "bwrap-userns-restrict"));
}

#[test]
fn the_fd3_probe_passes_when_the_descriptor_reaches_the_command() {
    let fake = Fake::linux();

    let report = run(&fake);

    assert!(is_pass(&report, "shepherd channel"));
    let ran = fake.ran.borrow();
    let probe = ran
        .iter()
        .find(|command| {
            command
                .args
                .last()
                .is_some_and(|a| a.contains("/proc/self/fd/3"))
        })
        .expect("the probe ran");
    assert!(probe.pipe_on_fd3);
    assert_eq!(*fake.removed.borrow(), [PathBuf::from(PROBE_SETTINGS)]);
}

#[test]
fn the_fd3_probe_writes_settings_with_the_network_and_filesystem_keys_crabpot_requires() {
    let fake = Fake::linux();

    run(&fake);

    let written = fake.written.borrow();
    let [contents] = written.as_slice() else {
        panic!("expected one settings file, wrote {written:?}");
    };
    let settings: serde_json::Value = serde_json::from_str(contents).expect("JSON settings");
    assert_eq!(
        settings,
        serde_json::json!({
            "network": { "allowedDomains": [], "deniedDomains": [] },
            "filesystem": { "denyRead": [], "allowWrite": [], "denyWrite": [] },
        })
    );
}

#[test]
fn the_fd3_probe_fails_naming_crabpot_307_when_the_descriptor_is_closed() {
    let mut fake = Fake::linux();
    fake.reply(
        &format!("crabpot -s {PROBE_SETTINGS} -- sh -c test -e /proc/self/fd/3"),
        Reply::Exit(1, ""),
    );

    let report = run(&fake);

    let fix = fix(&report, "shepherd channel");
    assert!(fix.contains("shep-pm/crabpot#307"));
    assert!(fix.contains("Upgrade crabpot"));
}

#[test]
fn the_fd3_probe_blames_crabpot_not_the_descriptor_when_crabpot_refuses() {
    let mut fake = Fake::linux();
    fake.reply(
        &format!("crabpot -s {PROBE_SETTINGS} -- sh -c test -e /proc/self/fd/3"),
        Reply::Exit(125, "bwrap: No permissions to create new namespace\n"),
    );

    let report = run(&fake);

    let Status::Fail { detail, fix } = status(&report, "shepherd channel") else {
        panic!("not failed: {report}");
    };
    assert!(detail.contains("No permissions to create new namespace"));
    assert!(!fix.contains("crabpot#307"));
}

#[test]
fn the_fd3_probe_is_skipped_off_linux_and_never_runs() {
    let fake = Fake::macos();

    let report = run(&fake);

    assert_eq!(
        status(&report, "shepherd channel"),
        &Status::Skip("Linux only".to_owned())
    );
    assert!(fake.ran.borrow().iter().all(|command| !command.pipe_on_fd3));
}

#[test]
fn an_empty_flock_is_skipped_and_says_so() {
    let report = run(&Fake::linux());

    assert!(is_skip(&report, "flock"));
}

#[test]
fn a_shepherd_older_than_sandbox_support_is_skipped_with_its_protocol() {
    let mut fake = Fake::linux();
    fake.flock = Err(FlockError::Old {
        protocol: SANDBOX_SINCE - 1,
    });

    let report = run(&fake);

    let Status::Skip(reason) = status(&report, "flock") else {
        panic!("not skipped: {report}");
    };
    assert!(reason.contains("protocol 13"), "{reason}");
    assert!(reason.contains("14"), "{reason}");
}

#[test]
fn a_flock_that_could_not_be_read_is_skipped_with_why() {
    let mut fake = Fake::linux();
    fake.flock = Err(FlockError::Failed("no shepherd is running".to_owned()));

    let report = run(&fake);

    assert_eq!(
        status(&report, "flock"),
        &Status::Skip("could not read the flock: no shepherd is running".to_owned())
    );
}

fn app(name: &str, cwd: Option<&str>, sandbox: Option<&str>) -> AppConfig {
    let mut app = AppConfig::minimal(name, "./srv");
    app.cwd = cwd.map(str::to_owned);
    app.sandbox = sandbox.map(str::to_owned);
    app
}

fn sandboxed(configs: &[AppConfig]) -> Vec<SandboxedSheep> {
    flock::sandboxed(configs, Path::new("/home/ops"))
}

#[test]
fn a_relative_sandbox_path_is_resolved_against_the_sheeps_cwd() {
    let sheep = sandboxed(&[app("web", Some("/srv/web"), Some("sandbox.json"))]);

    assert_eq!(
        sheep,
        [SandboxedSheep {
            name: "web".to_owned(),
            settings: "/srv/web/sandbox.json".into(),
            cwd: "/srv/web".into(),
        }]
    );
}

#[test]
fn an_absolute_sandbox_path_is_left_alone() {
    let sheep = sandboxed(&[app("web", Some("/srv/web"), Some("/etc/web.json"))]);

    assert_eq!(sheep[0].settings, Path::new("/etc/web.json"));
    assert_eq!(sheep[0].cwd, Path::new("/srv/web"));
}

#[test]
fn a_sheep_without_the_key_is_left_out() {
    let sheep = sandboxed(&[
        app("api", Some("/srv/api"), None),
        app("web", Some("/srv/web"), Some("sandbox.json")),
    ]);

    assert_eq!(sheep.len(), 1);
    assert_eq!(sheep[0].name, "web");
}

#[test]
fn a_cwd_that_is_unset_or_relative_is_made_absolute_against_the_given_directory() {
    let sheep = sandboxed(&[
        app("bare", None, Some("sandbox.json")),
        app("rel", Some("srv/rel"), Some("sandbox.json")),
    ]);

    assert_eq!(sheep[0].cwd, Path::new("/home/ops"));
    assert_eq!(sheep[0].settings, Path::new("/home/ops/sandbox.json"));
    assert_eq!(sheep[1].cwd, Path::new("/home/ops/srv/rel"));
    assert_eq!(
        sheep[1].settings,
        Path::new("/home/ops/srv/rel/sandbox.json")
    );
}

#[test]
fn a_sheep_whose_settings_crabpot_loads_passes() {
    let mut fake = Fake::linux();
    fake.sheep("web", "sandbox.json", "/srv/web");
    fake.file("/srv/web/sandbox.json", "{}");
    fake.reply("crabpot -s sandbox.json -- true", Reply::Exit(0, ""));

    let report = run(&fake);

    assert!(is_pass(&report, "sheep web"));
    let ran = fake.ran.borrow();
    let launch = ran.last().expect("crabpot ran");
    assert_eq!(launch.cwd.as_deref(), Some(Path::new("/srv/web")));
}

#[test]
fn a_sheep_with_no_settings_file_fails_with_how_to_make_one() {
    let mut fake = Fake::linux();
    fake.sheep("web", "/etc/web.json", "/srv/web");

    let report = run(&fake);

    let Status::Fail { detail, fix } = status(&report, "sheep web") else {
        panic!("not failed: {report}");
    };
    assert!(detail.contains("/etc/web.json does not exist"));
    assert!(fix.contains("shep sandbox init /etc/web.json"));
}

#[test]
fn a_relative_cwd_is_not_applied_twice_to_a_relative_settings_path() {
    let mut fake = Fake::linux();
    fake.sheep("web", "sandbox.json", "srv/web");
    fake.file("srv/web/sandbox.json", "{}");
    fake.reply("crabpot -s sandbox.json -- true", Reply::Exit(0, ""));

    let report = run(&fake);

    assert!(is_pass(&report, "sheep web"), "{report}");
}

#[test]
fn a_sheep_crabpot_refuses_prints_crabpots_stderr() {
    let mut fake = Fake::linux();
    fake.sheep("web", "/etc/web.json", "/srv/web");
    fake.file("/etc/web.json", "{");
    fake.reply(
        "crabpot -s /etc/web.json -- true",
        Reply::Exit(125, "settings: expected a value at line 1\n"),
    );

    let report = run(&fake);

    let Status::Fail { detail, .. } = status(&report, "sheep web") else {
        panic!("not failed: {report}");
    };
    assert!(detail.contains("settings: expected a value at line 1"));
}

#[test]
fn each_sheep_is_checked_on_its_own() {
    let mut fake = Fake::linux();
    fake.sheep("web", "/etc/web.json", "/srv/web");
    fake.sheep("api", "/etc/api.json", "/srv/api");
    fake.file("/etc/web.json", "{}");
    fake.reply("crabpot -s /etc/web.json -- true", Reply::Exit(0, ""));

    let report = run(&fake);

    assert!(is_pass(&report, "sheep web"));
    assert!(is_fail(&report, "sheep api"));
}

#[test]
fn the_exit_code_is_0_when_every_check_passes_or_is_skipped() {
    let report = run(&Fake::linux());

    assert_eq!(report.exit_code(), 0, "{report}");
}

#[test]
fn the_exit_code_is_1_when_any_check_fails() {
    let mut fake = Fake::linux();
    fake.reply("bwrap --version", Reply::Absent);

    assert_eq!(run(&fake).exit_code(), 1);
}

#[test]
fn a_failure_prints_the_check_the_detail_and_each_fix_line() {
    let mut fake = Fake::linux();
    fake.reply("bwrap --version", Reply::Absent);

    let printed = run(&fake).to_string();

    assert!(
        printed.contains("ok    crabpot: crabpot 0.9.0\n"),
        "{printed}"
    );
    assert!(
        printed.contains("FAIL  bwrap: bwrap could not run: not found\n"),
        "{printed}"
    );
    assert!(
        printed.contains("      sudo apt install bubblewrap\n"),
        "{printed}"
    );
    assert!(
        printed.contains("skip  sandbox-exec: macOS only\n"),
        "{printed}"
    );
}
