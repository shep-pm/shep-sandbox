use std::{cell::Cell, ffi::OsString, io, path::Path};

use serde_json::{Map, Value, json};
use shep_client::{
    ConnectError,
    dogs::{HomeError, ShepherdError},
};

use super::{Invocation, Plan, Refusal, effective_settings, launch, parse_args};

fn os(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
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
    for args in [&["api", "--"][..], &["api"], &["--", "node"], &[]] {
        let refusal = parse_args(&os(args)).expect_err(&format!("{args:?} names no command"));

        assert!(
            matches!(refusal, Refusal::NoCommand | Refusal::NoSheep),
            "{args:?}: {refusal}"
        );
    }
    let refusal = parse_args(&os(&["api", "--"])).expect_err("nothing after the dash");
    assert!(matches!(refusal, Refusal::NoCommand), "{refusal}");
}

#[test]
fn the_cwd_is_added_to_allow_write_once() {
    let cwd = Path::new("/work/api");

    let once = effective_settings(table(json!({})), cwd).expect("an empty table parses");
    assert_eq!(once["filesystem"]["allowWrite"], json!(["/work/api"]));

    let already = table(json!({"filesystem": {"allowWrite": ["/var/cache", "/work/api"]}}));
    let twice = effective_settings(already, cwd).expect("a table that already lists it");
    assert_eq!(
        twice["filesystem"]["allowWrite"],
        json!(["/var/cache", "/work/api"])
    );
}

#[test]
fn the_effective_settings_carry_the_five_lists_crabpot_requires() {
    let settings = effective_settings(table(json!({})), Path::new("/work/api")).expect("parses");

    assert_eq!(settings["network"]["allowedDomains"], json!([]));
    assert_eq!(settings["network"]["deniedDomains"], json!([]));
    assert_eq!(settings["filesystem"]["denyRead"], json!([]));
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
        )
        .expect_err("crabpot refuses a wildcard allow"),
        Refusal::Cwd(io::Error::other("gone")),
        Refusal::SettingsFile(io::Error::other("disk full")),
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
            Ok(Some(table(
                json!({"network": {"allowedDomains": ["example.com"]}}),
            )))
        },
        |plan: Plan| {
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
    type Fetch = fn(&str) -> Result<Option<Map<String, Value>>, Refusal>;
    let cases: [(&str, Vec<OsString>, Fetch); 5] = [
        ("no command", os(&["api", "--"]), |_| Ok(Some(Map::new()))),
        ("no shepherd", args(), |_| Err(Refusal::NoHome)),
        ("no table", args(), |_| Ok(None)),
        ("a table crabpot refuses", args(), |_| {
            Ok(Some(table(json!({"network": "open"}))))
        }),
        ("a refused sheep", os(&["--", "node"]), |_| {
            Ok(Some(Map::new()))
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
