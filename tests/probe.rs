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
    Command::new(DOG_BIN)
        .args(args)
        // A probe must not need a shepherd, and `shep adopt` asks these of a
        // candidate it cannot name yet.
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
    for verb in ["init", "doctor"] {
        let output = run(&[verb]);

        assert_eq!(output.status.code(), Some(2), "{verb}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not built yet"),
            "{verb}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
