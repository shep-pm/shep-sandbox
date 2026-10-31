//! What shep asks the binary before and around adopting it, asked the way
//! shep asks. Spawns the built binary, because a probe that `main` never
//! calls would pass any test that called `probe` directly.

use std::process::{Command, Output};

use shep_client::shep_core::{
    dogs::{MANIFEST_FLAG, SCHEMA_FLAG, VERSION_FLAG, parse_version_answer},
    protocol::PROTOCOL_VERSION,
};

const DOG_BIN: &str = env!("CARGO_BIN_EXE_shep-sandbox");

fn run(args: &[&str]) -> Output {
    // A probe must not need a shepherd, and `shep adopt` asks these of a
    // candidate it cannot name yet.
    run_named(None, args)
}

/// Run as `shep <name> <args>` does: named by `$SHEP_DOG_NAME`, and with no
/// connection to a shepherd handed down, which only a supervised dog has.
fn run_named(name: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(DOG_BIN);
    command
        .args(args)
        .env_remove("SHEP_DOG_FD")
        .env_remove("SHEP_DOG_PIPE")
        .env("SHEP_HOME", "/nonexistent/shep-home");
    match name {
        Some(name) => command.env("SHEP_DOG_NAME", name),
        None => command.env_remove("SHEP_DOG_NAME"),
    };
    command.output().expect("the dog binary ran")
}

fn run_in(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(DOG_BIN)
        .args(args)
        .current_dir(dir)
        .env_remove("SHEP_DOG_NAME")
        .env("SHEP_HOME", "/nonexistent/shep-home")
        .output()
        .expect("the dog binary ran")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

#[test]
fn the_version_answer_is_one_the_shepherds_own_parser_reads() {
    let output = run(&[VERSION_FLAG]);

    assert!(output.status.success());
    let parsed = parse_version_answer(&stdout(&output)).expect("a readable answer");
    assert_eq!(parsed.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(parsed.protocol, Some(PROTOCOL_VERSION));
}

#[test]
fn the_schema_answer_is_json_for_a_section_with_no_keys() {
    let output = run(&[SCHEMA_FLAG]);

    assert!(output.status.success());
    let schema: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("JSON");
    let properties = schema
        .get("properties")
        .map(|p| p.as_object().map(|o| o.len()));
    assert!(
        matches!(properties, None | Some(Some(0))),
        "the section has no keys yet: {schema}"
    );
}

#[test]
fn the_manifest_declares_the_sandbox_verb_with_init_and_doctor() {
    let output = run(&[MANIFEST_FLAG]);

    assert!(output.status.success());
    let manifest: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("JSON");
    let subcommands: Vec<&str> = manifest["verb"]["subcommands"]
        .as_array()
        .expect("a verb with subcommands")
        .iter()
        .map(|subcommand| subcommand["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(subcommands, ["init", "doctor"]);
}

#[test]
fn a_verb_that_is_not_built_yet_says_so_and_exits_2() {
    let output = run(&["doctor"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not built yet"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn init_writes_the_skeleton_where_the_path_says_and_will_not_overwrite_it() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sandbox.json");

    let first = run_in(dir.path(), &["init", target.to_str().unwrap()]);
    let second = run_in(dir.path(), &["init", target.to_str().unwrap()]);

    assert!(first.status.success(), "{first:?}");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).expect("JSON");
    assert!(written["filesystem"]["allowWrite"].is_array());
    assert_eq!(second.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));
}

#[test]
fn init_with_no_path_writes_sandbox_json_in_the_current_directory() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_in(dir.path(), &["init"]);

    assert!(output.status.success(), "{output:?}");
    assert!(dir.path().join("sandbox.json").is_file());
}

#[test]
fn init_from_a_sheep_with_no_shepherd_writes_nothing_and_says_so() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_in(dir.path(), &["init", "--from", "web"]);

    assert_eq!(output.status.code(), Some(5), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot reach the shepherd"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn init_with_from_and_no_sheep_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();

    let output = run_in(dir.path(), &["init", "--from"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn an_argument_the_dog_has_never_had_is_refused() {
    let output = run(&["--rotate-now"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--rotate-now"));
}

#[test]
fn run_by_hand_with_no_shepherd_the_dog_says_why_it_stopped() {
    let output = run(&[]);

    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
}

#[test]
fn a_verb_with_more_arguments_is_still_not_built_yet() {
    let output = run(&["doctor", "--verbose"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not built yet"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shep_sandbox_with_no_subcommand_prints_usage_and_exits_2() {
    // `shep sandbox` names the dog without handing it a connection, so it is
    // not the supervised dog and must not connect as one.
    let output = run_named(Some("sandbox"), &[]);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("init") && stderr.contains("doctor"),
        "{stderr}"
    );
}

#[test]
fn print_config_names_the_section_the_dog_was_adopted_under() {
    let output = run_named(Some("crab"), &["--print-config"]);

    assert!(output.status.success());
    assert!(stdout(&output).contains("[crab]"), "{}", stdout(&output));
}

#[test]
fn a_refused_argument_names_the_dog_it_was_adopted_as() {
    let output = run_named(Some("crab"), &["--bogus"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("crab does not understand --bogus"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
