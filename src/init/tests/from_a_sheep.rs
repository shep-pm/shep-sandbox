use serde_json::{Value, json};

use super::{Applies, PathBuf, Socket, SocketAddr, Taken, fs, parse, skeleton, tempdir};

const LINUX: Applies = Applies {
    publish: true,
    allow_tcp: true,
};
const MACOS: Applies = Applies {
    publish: true,
    allow_tcp: false,
};
const WINDOWS: Applies = Applies {
    publish: false,
    allow_tcp: false,
};

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

fn taken(cwd: Option<&str>, sockets: &[Socket]) -> Taken {
    Taken::new("web", 4100, cwd.map(str::to_owned), sockets)
}

fn written_on(applies: Applies, cwd: Option<&str>, sockets: &[Socket]) -> (String, Value) {
    let text = super::super::render(&taken(cwd, sockets), applies);
    let value = serde_json::from_str(&text).expect("the file is JSON");
    (text, value)
}

fn written(cwd: Option<&str>, sockets: &[Socket]) -> (String, Value) {
    written_on(LINUX, cwd, sockets)
}

fn lines(file: &Value, key: &str) -> Vec<String> {
    file["network"][key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not a list in {file}"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn observed() -> Vec<Socket> {
    vec![
        Socket::Listening(addr("0.0.0.0:8080")),
        Socket::Listening(addr("[::]:8443")),
        Socket::Connected {
            local: addr("10.0.0.15:8080"),
            remote: addr("10.0.0.9:51000"),
        },
        Socket::Connected {
            local: addr("10.0.0.15:53924"),
            remote: addr("10.0.0.5:3306"),
        },
        Socket::Connected {
            local: addr("127.0.0.1:50001"),
            remote: addr("127.0.0.1:8080"),
        },
        Socket::Connected {
            local: addr("10.0.0.15:53925"),
            remote: addr("203.0.113.7:443"),
        },
    ]
}

#[test]
fn listeners_become_publish() {
    let (_, file) = written(Some("/srv/web"), &observed());

    assert_eq!(
        file["network"]["publish"],
        json!(["0.0.0.0:8080", "[::]:8443"])
    );
}

#[test]
fn dials_become_allow_tcp_without_accepted_connections_or_self_dials() {
    let (_, file) = written(Some("/srv/web"), &observed());

    assert_eq!(file["network"]["allowTcp"], json!(["10.0.0.5:3306"]));
}

#[test]
fn an_https_dial_is_a_comment_and_not_allow_tcp() {
    let (_, file) = written(Some("/srv/web"), &observed());

    assert_eq!(
        file["network"]["// dialed over HTTPS"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|line| line.contains("203.0.113.7:443"))
            .count(),
        1
    );
    assert_eq!(file["network"]["allowedDomains"], json!([]));
}

#[test]
fn no_https_dial_adds_no_comment() {
    let (_, file) = written(Some("/srv/web"), &[Socket::Listening(addr("0.0.0.0:8080"))]);

    assert!(file["network"].get("// dialed over HTTPS").is_none());
}

#[test]
fn on_linux_a_dial_lands_in_allow_tcp_and_no_comment() {
    let (_, file) = written_on(LINUX, None, &observed());

    assert_eq!(file["network"]["allowTcp"], json!(["10.0.0.5:3306"]));
    assert!(file["network"].get("// dialed directly").is_none());
}

#[test]
fn where_allow_tcp_is_not_applied_a_dial_lands_in_a_comment_and_allow_tcp_is_empty() {
    for applies in [MACOS, WINDOWS] {
        let (_, file) = written_on(applies, None, &observed());

        assert_eq!(file["network"]["allowTcp"], json!([]));
        let comment = lines(&file, "// dialed directly");
        assert_eq!(
            comment
                .iter()
                .filter(|line| line.as_str() == "10.0.0.5:3306")
                .count(),
            1,
            "{comment:?}"
        );
        assert!(!comment.iter().any(|line| line.contains("10.0.0.9")));
    }
}

#[test]
fn where_allow_tcp_is_not_applied_an_https_dial_stays_in_its_own_comment() {
    let (_, file) = written_on(MACOS, None, &observed());

    assert!(
        lines(&file, "// dialed over HTTPS")
            .iter()
            .any(|line| line.contains("203.0.113.7:443"))
    );
    assert!(
        !lines(&file, "// dialed directly")
            .iter()
            .any(|line| line.contains("203.0.113.7"))
    );
}

#[test]
fn no_direct_dial_adds_no_comment_where_allow_tcp_is_not_applied() {
    let (_, file) = written_on(MACOS, None, &[Socket::Listening(addr("0.0.0.0:8080"))]);

    assert!(file["network"].get("// dialed directly").is_none());
}

#[test]
fn on_macos_a_listener_lands_in_publish() {
    let (_, file) = written_on(MACOS, None, &observed());

    assert_eq!(
        file["network"]["publish"],
        json!(["0.0.0.0:8080", "[::]:8443"])
    );
    assert!(file["network"].get("// listening").is_none());
}

#[test]
fn where_publish_is_not_applied_a_listener_lands_in_a_comment_and_publish_is_empty() {
    let (_, file) = written_on(WINDOWS, None, &observed());

    assert_eq!(file["network"]["publish"], json!([]));
    let comment = lines(&file, "// listening");
    assert!(
        comment.iter().any(|line| line == "0.0.0.0:8080"),
        "{comment:?}"
    );
    assert!(
        comment.iter().any(|line| line == "[::]:8443"),
        "{comment:?}"
    );
}

/// crabpot v0.10.0 refuses a non-empty `publish` on Windows and a
/// non-empty `allowTcp` anywhere but Linux. The rule is spelled out here
/// by platform, apart from the code under test.
fn refused_keys(file: &Value) -> Vec<&'static str> {
    let filled = |key| {
        file["network"][key]
            .as_array()
            .is_some_and(|l| !l.is_empty())
    };
    let mut refused = Vec::new();
    if cfg!(not(any(target_os = "linux", target_os = "macos"))) && filled("publish") {
        refused.push("publish");
    }
    if cfg!(not(target_os = "linux")) && filled("allowTcp") {
        refused.push("allowTcp");
    }
    refused
}

fn run_taken(cwd: Option<&str>) -> Value {
    let dir = tempdir().unwrap();
    let args = parse(&["sandbox.json".to_owned()]).unwrap();
    let path: PathBuf =
        super::super::run(dir.path(), &args, Some(&taken(cwd, &observed()))).unwrap();
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn run_writes_the_taken_sheeps_contents_where_this_host_applies_them() {
    let file = run_taken(Some("/srv/web"));

    assert_eq!(file["filesystem"]["allowWrite"], json!(["/srv/web"]));
    assert!(
        file["// sandbox.json"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|line| line.contains("running sheep web") && line.contains("4100"))
    );
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert_eq!(
            file["network"]["publish"],
            json!(["0.0.0.0:8080", "[::]:8443"])
        );
    } else {
        assert_eq!(file["network"]["publish"], json!([]));
    }
    if cfg!(target_os = "linux") {
        assert_eq!(file["network"]["allowTcp"], json!(["10.0.0.5:3306"]));
    } else {
        assert_eq!(file["network"]["allowTcp"], json!([]));
        assert!(
            lines(&file, "// dialed directly")
                .iter()
                .any(|line| line == "10.0.0.5:3306")
        );
    }
}

#[test]
fn the_file_written_for_this_host_holds_no_key_crabpot_refuses_here() {
    let file = run_taken(None);

    assert_eq!(refused_keys(&file), Vec::<&str>::new());
}

#[test]
fn the_cwd_becomes_allow_write() {
    let (_, file) = written(Some("/srv/web"), &observed());

    assert_eq!(file["filesystem"]["allowWrite"], json!(["/srv/web"]));
}

#[test]
fn a_cwd_with_a_quote_is_escaped() {
    let (_, file) = written(Some("/srv/\"web\""), &[]);

    assert_eq!(file["filesystem"]["allowWrite"], json!(["/srv/\"web\""]));
}

#[test]
fn a_sheep_with_no_cwd_keeps_the_skeletons_write_root() {
    let (_, file) = written(None, &observed());

    assert_eq!(file["filesystem"]["allowWrite"], json!(["."]));
}

#[test]
fn the_file_says_which_sheep_it_was_taken_from() {
    let (_, file) = written(Some("/srv/web"), &observed());

    let head = file["// sandbox.json"].as_array().unwrap();
    assert!(
        head.iter()
            .filter_map(Value::as_str)
            .any(|line| line.contains("running sheep web") && line.contains("4100")),
        "{head:?}"
    );
}

#[test]
fn the_rest_is_the_plain_skeleton() {
    let (_, file) = written(Some("/srv/web"), &observed());
    let plain = skeleton();

    for (group, key) in [
        ("filesystem", "denyRead"),
        ("filesystem", "denyWrite"),
        ("network", "allowedDomains"),
        ("network", "deniedDomains"),
        ("network", "// publish"),
        ("network", "// allowTcp"),
        ("network", "// allowedDomains"),
    ] {
        assert_eq!(file[group][key], plain[group][key], "{group}.{key}");
    }
}
