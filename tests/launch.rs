//! `launch`, run as the shepherd runs it, with no shepherd to answer. Each
//! test's command would leave a marker file if it ran, and none may.

use std::process::{Command, Output};

const DOG_BIN: &str = env!("CARGO_BIN_EXE_shep-sandbox");

/// Run `launch api -- touch <marker>` with `$SHEP_HOME` set to `home`, or
/// unset for `None`, and report whether the command ran.
fn launch(home: Option<&str>, tail: &[&str]) -> (Output, bool) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let marker = dir.path().join("ran");
    let mut command = Command::new(DOG_BIN);
    command
        .args(["launch", "api"])
        .args(tail)
        .current_dir(dir.path())
        .env_remove("SHEP_DOG_NAME")
        .env_remove("SHEP_DOG_FD")
        .env_remove("SHEP_DOG_PIPE");
    if tail.contains(&"--") && tail.last() != Some(&"--") {
        command.arg(&marker);
    }
    match home {
        Some(home) => command.env("SHEP_HOME", home),
        None => command.env_remove("SHEP_HOME"),
    };
    let output = command.output().expect("the dog binary ran");
    (output, marker.exists())
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
#[cfg(unix)]
fn the_command_exits_125_when_no_shepherd_can_be_reached() {
    let (output, ran) = launch(Some("/nonexistent/shep-home"), &["--", "touch"]);

    assert_eq!(output.status.code(), Some(125), "{}", stderr(&output));
    assert!(stderr(&output).contains("shepherd"), "{}", stderr(&output));
    assert_eq!(stderr(&output).lines().count(), 1, "{}", stderr(&output));
    assert!(!ran, "the command ran with no shepherd to give it settings");
}

#[test]
#[cfg(unix)]
fn the_command_exits_125_when_shep_home_is_unset() {
    let (output, ran) = launch(None, &["--", "touch"]);

    assert_eq!(output.status.code(), Some(125), "{}", stderr(&output));
    assert!(stderr(&output).contains("SHEP_HOME"), "{}", stderr(&output));
    assert!(!ran);
}

#[test]
#[cfg(unix)]
fn a_launch_with_no_command_exits_125() {
    let (output, _) = launch(Some("/nonexistent/shep-home"), &["--"]);

    assert_eq!(output.status.code(), Some(125), "{}", stderr(&output));
    assert!(stderr(&output).contains("command"), "{}", stderr(&output));
}

#[test]
#[cfg(not(unix))]
fn launch_says_the_platform_is_not_supported() {
    let (output, ran) = launch(Some("/nonexistent/shep-home"), &["--", "touch"]);

    assert_eq!(output.status.code(), Some(125), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("not supported"),
        "{}",
        stderr(&output)
    );
    assert!(!ran);
}
