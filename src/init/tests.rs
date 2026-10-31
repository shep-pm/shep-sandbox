use std::fs;

use serde_json::Value;
use tempfile::tempdir;

use super::{Error, SKELETON, create, run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

fn skeleton() -> Value {
    serde_json::from_str(SKELETON).expect("the skeleton is JSON")
}

#[test]
fn with_no_path_it_writes_sandbox_json_in_the_directory() {
    let dir = tempdir().unwrap();

    let written = run(dir.path(), &[]).unwrap();

    assert_eq!(written, dir.path().join("sandbox.json"));
    assert_eq!(fs::read_to_string(written).unwrap(), SKELETON);
}

#[test]
fn a_relative_path_is_read_against_the_directory() {
    let dir = tempdir().unwrap();

    let written = run(dir.path(), &args(&["web.json"])).unwrap();

    assert_eq!(written, dir.path().join("web.json"));
    assert_eq!(fs::read_to_string(written).unwrap(), SKELETON);
    assert!(!dir.path().join("sandbox.json").exists());
}

#[test]
fn an_absolute_path_is_written_where_it_says() {
    let dir = tempdir().unwrap();
    let elsewhere = tempdir().unwrap();
    let target = elsewhere.path().join("api.json");

    let written = run(dir.path(), &args(&[target.to_str().unwrap()])).unwrap();

    assert_eq!(written, target);
    assert_eq!(fs::read_to_string(target).unwrap(), SKELETON);
}

#[test]
fn a_file_that_exists_is_refused_and_left_as_it_was() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("sandbox.json");
    fs::write(&target, "mine").unwrap();

    let err = run(dir.path(), &[]).unwrap_err();

    assert!(
        matches!(&err, Error::Exists(path) if *path == target),
        "{err}"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "mine");
}

#[test]
fn a_directory_that_does_not_exist_is_a_write_failure() {
    let dir = tempdir().unwrap();

    let err = run(dir.path(), &args(&["missing/sandbox.json"])).unwrap_err();

    assert!(matches!(err, Error::Write { .. }), "{err}");
}

#[test]
fn a_file_whose_write_fails_is_removed_so_a_retry_can_create_it() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("sandbox.json");

    let err = create(&target, |_| Err(std::io::Error::other("disk full"))).unwrap_err();

    assert!(matches!(err, Error::Write { .. }), "{err}");
    assert!(!target.exists());
    run(dir.path(), &[]).unwrap();
}

#[test]
fn a_second_path_or_a_flag_is_a_usage_error() {
    let dir = tempdir().unwrap();

    for list in [&["a.json", "b.json"][..], &["--force"][..]] {
        let err = run(dir.path(), &args(list)).unwrap_err();

        assert!(matches!(err, Error::Usage { .. }), "{list:?}: {err}");
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn the_skeleton_names_every_setting_crabpot_reads_as_a_list() {
    let skeleton = skeleton();

    for (group, key) in [
        ("filesystem", "allowWrite"),
        ("filesystem", "denyRead"),
        ("filesystem", "denyWrite"),
        ("network", "allowedDomains"),
        ("network", "deniedDomains"),
        ("network", "publish"),
        ("network", "allowTcp"),
    ] {
        assert!(skeleton[group][key].is_array(), "{group}.{key}");
    }
}

#[test]
fn the_skeleton_opens_the_cwd_for_writing_and_asks_for_nothing_else() {
    let skeleton = skeleton();

    assert_eq!(
        skeleton["filesystem"]["allowWrite"],
        serde_json::json!(["."])
    );
    for (group, key) in [
        ("filesystem", "denyRead"),
        ("filesystem", "denyWrite"),
        ("network", "allowedDomains"),
        ("network", "deniedDomains"),
        ("network", "publish"),
        ("network", "allowTcp"),
    ] {
        assert_eq!(skeleton[group][key], serde_json::json!([]), "{group}.{key}");
    }
}

#[test]
fn every_key_besides_the_settings_is_a_comment_crabpot_drops() {
    fn settings(object: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|key| !key.starts_with("//"))
            .collect();
        keys.sort_unstable();
        keys
    }
    let skeleton = skeleton();

    assert_eq!(settings(&skeleton), ["filesystem", "network"]);
    assert_eq!(
        settings(&skeleton["filesystem"]),
        ["allowWrite", "denyRead", "denyWrite"]
    );
    assert_eq!(
        settings(&skeleton["network"]),
        ["allowTcp", "allowedDomains", "deniedDomains", "publish"]
    );
}
