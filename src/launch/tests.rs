use std::{cell::Cell, ffi::OsString, io, path::Path};

use serde_json::{Map, Value, json};
use shep_client::{
    ConnectError,
    dogs::{HomeError, ShepherdError},
};

use super::{
    Fetched, Invocation, Plan, Reach, Refusal, effective_settings, fetch_over, launch, parse_args,
    withhold_link,
};

fn os(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

const SHEP_HOME: &str = "/home/me/.shep";

fn fetched(table: Option<Map<String, Value>>) -> Result<Fetched<()>, Refusal> {
    Ok(Fetched {
        shep_home: SHEP_HOME.into(),
        table,
        connection: (),
    })
}

fn table(value: Value) -> Map<String, Value> {
    let Value::Object(table) = value else {
        panic!("a table is a JSON object: {value}");
    };
    table
}

#[test]
fn the_sheep_name_and_the_command_are_split_at_the_double_dash() {
    let invocation = parse_args(&os(&["api", "--", "node", "--inspect", "--", "server.js"]))
        .expect("a sheep and a command");

    assert_eq!(
        invocation,
        Invocation {
            sheep: "api".into(),
            command: os(&["node", "--inspect", "--", "server.js"]),
        }
    );
}

#[test]
fn a_missing_command_is_refused() {
    for args in [&["api", "--"][..], &["api"], &["api", "node"]] {
        let refusal = parse_args(&os(args)).expect_err(&format!("{args:?} names no command"));

        assert!(matches!(refusal, Refusal::NoCommand), "{args:?}: {refusal}");
    }
}

#[test]
fn a_missing_sheep_name_is_refused() {
    for args in [&["--", "node"][..], &[]] {
        let refusal = parse_args(&os(args)).expect_err(&format!("{args:?} names no sheep"));

        assert!(matches!(refusal, Refusal::NoSheep), "{args:?}: {refusal}");
    }
}

#[test]
fn the_cwd_is_added_to_allow_write_once() {
    let cwd = Path::new("/work/api");

    let once = effective_settings(table(json!({})), cwd, Path::new(SHEP_HOME))
        .expect("an empty table parses");
    assert_eq!(once["filesystem"]["allowWrite"], json!(["/work/api"]));

    let already = table(json!({"filesystem": {"allowWrite": ["/var/cache", "/work/api"]}}));
    let twice = effective_settings(already, cwd, Path::new(SHEP_HOME))
        .expect("a table that already lists it");
    assert_eq!(
        twice["filesystem"]["allowWrite"],
        json!(["/var/cache", "/work/api"])
    );
}

#[test]
fn home_and_shep_home_are_denied_and_the_cwd_re_opened_once() {
    let (cwd, shep_home) = (Path::new("/work/api"), Path::new(SHEP_HOME));

    let once = effective_settings(table(json!({})), cwd, shep_home).expect("an empty table parses");
    assert_eq!(
        once["filesystem"]["denyRead"],
        json!(["~", "/home/me/.shep"])
    );
    assert_eq!(
        once["filesystem"]["allowRead"],
        json!([{"path": "/work/api", "literal": true}])
    );

    let twice = effective_settings(once.clone(), cwd, shep_home).expect("its own output parses");
    assert_eq!(twice, once);
}

#[test]
fn a_tables_own_deny_and_allow_entries_are_kept() {
    let table_with_entries = table(json!({"filesystem": {
        "denyRead": ["~/.ssh"],
        "allowRead": ["~/.nvm"],
    }}));

    let settings = effective_settings(
        table_with_entries,
        Path::new("/work/api"),
        Path::new("/home/me/.shep"),
    )
    .expect("a table with both lists parses");

    assert_eq!(
        settings["filesystem"]["denyRead"],
        json!(["~/.ssh", "~", "/home/me/.shep"])
    );
    assert_eq!(
        settings["filesystem"]["allowRead"],
        json!(["~/.nvm", {"path": "/work/api", "literal": true}])
    );
}

#[test]
fn a_shep_home_outside_home_is_still_denied() {
    let settings = effective_settings(
        table(json!({})),
        Path::new("/work/api"),
        Path::new("/srv/shep"),
    )
    .expect("an empty table parses");

    assert_eq!(
        settings["filesystem"]["denyRead"],
        json!(["~", "/srv/shep"])
    );
}

#[test]
fn a_cwd_with_glob_characters_is_re_opened_as_a_literal_path() {
    let settings = effective_settings(
        table(json!({})),
        Path::new("/home/me/app[s]"),
        Path::new(SHEP_HOME),
    )
    .expect("an empty table parses");

    assert_eq!(
        settings["filesystem"]["allowRead"],
        json!([{"path": "/home/me/app[s]", "literal": true}])
    );
}

#[test]
fn the_effective_settings_carry_the_five_lists_crabpot_requires() {
    let settings = effective_settings(
        table(json!({})),
        Path::new("/work/api"),
        Path::new(SHEP_HOME),
    )
    .expect("parses");

    assert_eq!(settings["network"]["allowedDomains"], json!([]));
    assert_eq!(settings["network"]["deniedDomains"], json!([]));
    assert_eq!(settings["filesystem"]["denyWrite"], json!([]));
}

#[test]
fn every_refusal_maps_to_exit_125() {
    let refusals = [
        Refusal::NoSheep,
        Refusal::NoCommand,
        Refusal::NoHome,
        Refusal::Home(HomeError::Empty),
        Refusal::Shepherd(ShepherdError::from(ConnectError::HandshakeClosed)),
        Refusal::NoTable {
            sheep: "api".into(),
        },
        effective_settings(
            table(json!({"network": {"allowedDomains": ["*"]}})),
            Path::new("/work"),
            Path::new(SHEP_HOME),
        )
        .expect_err("crabpot refuses a wildcard allow"),
        Refusal::Cwd(io::Error::other("gone")),
        Refusal::SettingsFile(io::Error::other("disk full")),
        Refusal::ControlPipe(io::Error::other("too many open files")),
        Refusal::Runtime(io::Error::other("no threads")),
        Refusal::Unsupported,
    ];

    for refusal in refusals {
        assert_eq!(refusal.exit_code(), 125, "{refusal}");
        assert!(!refusal.to_string().is_empty());
        assert!(!refusal.to_string().contains('\n'), "{refusal}");
    }
}

fn args() -> Vec<OsString> {
    os(&["api", "--", "node", "server.js"])
}

#[test]
fn a_launch_that_succeeds_hands_the_run_the_settings_and_the_whole_command() {
    let ran = Cell::new(None);

    let refusal = launch(
        &args(),
        Path::new("/work/api"),
        |sheep| {
            assert_eq!(sheep, "api");
            fetched(Some(table(
                json!({"network": {"allowedDomains": ["example.com"]}}),
            )))
        },
        |plan: Plan<()>| {
            ran.set(Some(plan));
            Refusal::SettingsFile(io::Error::other("the stand-in returned"))
        },
    );

    assert!(matches!(refusal, Refusal::SettingsFile(_)), "{refusal}");
    let plan = ran.take().expect("the run was reached");
    assert_eq!(plan.command, os(&["node", "server.js"]));
    assert_eq!(
        plan.settings["filesystem"]["allowWrite"],
        json!(["/work/api"])
    );
    assert_eq!(
        plan.settings["network"]["allowedDomains"],
        json!(["example.com"])
    );
}

#[test]
fn a_failure_before_the_run_never_reaches_the_command() {
    type Fetch = fn(&str) -> Result<Fetched<()>, Refusal>;
    let cases: [(&str, Vec<OsString>, Fetch); 5] = [
        ("no command", os(&["api", "--"]), |_| {
            fetched(Some(Map::new()))
        }),
        ("no shepherd", args(), |_| Err(Refusal::NoHome)),
        ("no table", args(), |_| fetched(None)),
        ("a table crabpot refuses", args(), |_| {
            fetched(Some(table(json!({"network": "open"}))))
        }),
        ("a refused sheep", os(&["--", "node"]), |_| {
            fetched(Some(Map::new()))
        }),
    ];

    for (what, args, fetch) in cases {
        let reached = Cell::new(false);

        let refusal = launch(&args, Path::new("/work/api"), fetch, |_plan| {
            reached.set(true);
            Refusal::Unsupported
        });

        assert!(!reached.get(), "{what}: the run was reached");
        assert_eq!(refusal.exit_code(), 125, "{what}: {refusal}");
    }
}

/// A shepherd that records how it was reached and what was asked over each
/// connection.
#[derive(Default)]
struct FakeReach {
    reached: Vec<&'static str>,
    asked: Vec<(&'static str, String)>,
    link_fails: bool,
}

impl Reach for FakeReach {
    type Conn = &'static str;

    async fn over_link(&mut self) -> Result<&'static str, ShepherdError> {
        self.reached.push("link");
        if self.link_fails {
            return Err(ShepherdError::from(ConnectError::HandshakeClosed));
        }
        Ok("link")
    }

    async fn over_socket(&mut self) -> Result<&'static str, ShepherdError> {
        self.reached.push("socket");
        Ok("socket")
    }

    async fn table(
        &mut self,
        conn: &mut &'static str,
        sheep: &str,
    ) -> Result<Option<Map<String, Value>>, ShepherdError> {
        self.asked.push((*conn, sheep.to_owned()));
        Ok(Some(Map::new()))
    }
}

#[tokio::test]
async fn a_launch_with_an_inherited_link_never_dials_the_socket() {
    let mut reach = FakeReach::default();

    let fetched = fetch_over(&mut reach, true, SHEP_HOME.into(), "api")
        .await
        .expect("the link answers");

    assert_eq!(reach.reached, ["link"]);
    assert_eq!(fetched.connection, "link");
    assert_eq!(
        reach.asked,
        [("link", "api".to_owned())],
        "the table came over the connection handed on"
    );
}

#[tokio::test]
async fn a_launch_without_a_link_dials_the_socket() {
    let mut reach = FakeReach::default();

    let fetched = fetch_over(&mut reach, false, SHEP_HOME.into(), "api")
        .await
        .expect("the socket answers");

    assert_eq!(reach.reached, ["socket"]);
    assert_eq!(fetched.connection, "socket");
    assert_eq!(reach.asked, [("socket", "api".to_owned())]);
}

#[tokio::test]
async fn a_link_that_fails_is_refused_and_not_retried_on_the_socket() {
    let mut reach = FakeReach {
        link_fails: true,
        ..FakeReach::default()
    };

    let refusal = fetch_over(&mut reach, true, SHEP_HOME.into(), "api")
        .await
        .expect_err("the link is lost");

    assert!(matches!(refusal, Refusal::Shepherd(_)), "{refusal}");
    assert_eq!(reach.reached, ["link"]);
    assert!(reach.asked.is_empty());
}

/// Whether a child of this process sees descriptor `fd` open and the variable
/// `var` set, as `(fd open, var set)`.
fn child_sees(fd: i32, var: &str) -> (bool, bool) {
    let probe = |script: String| {
        std::process::Command::new("sh")
            .args(["-c", &script])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("sh runs")
            .success()
    };
    (
        probe(format!(": >&{fd}")),
        probe(format!("[ -n \"${{{var}+set}}\" ]")),
    )
}

#[test]
#[allow(unsafe_code)] // Sets a variable only this test reads, as the shepherd would.
fn the_command_inherits_neither_fd_4_nor_shep_dog_fd() {
    use std::os::fd::AsRawFd;

    const VAR: &str = "SHEP_SANDBOX_TEST_LINK_FD";
    let (ours, _theirs) = std::os::unix::net::UnixStream::pair().expect("a socketpair");
    let fd = ours.as_raw_fd();
    // The shepherd hands the link down without close-on-exec, which std's
    // sockets have by default.
    rustix::io::fcntl_setfd(&ours, rustix::io::FdFlags::empty()).expect("clears the flag");
    // SAFETY: nothing else in the test binary reads this variable.
    unsafe { std::env::set_var(VAR, fd.to_string()) };
    assert_eq!(
        child_sees(fd, VAR),
        (true, true),
        "without it the child holds both, so the check below proves something"
    );

    withhold_link(VAR).expect("the descriptor is open");

    assert_eq!(child_sees(fd, VAR), (false, false));
    assert!(
        ours.as_raw_fd() == fd && rustix::io::fcntl_getfd(&ours).is_ok(),
        "this process keeps the link"
    );
}

#[test]
fn a_launch_without_a_link_has_nothing_to_withhold() {
    withhold_link("SHEP_SANDBOX_TEST_UNSET_LINK_FD").expect("no variable, no descriptor");
}
