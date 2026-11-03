use std::{cell::RefCell, collections::VecDeque, path::Path, rc::Rc, time::Duration};

use serde_json::{Map, Value, json};
use shep_client::{ConnectError, dogs::ShepherdError};
use tokio::time::timeout;

use super::{Heard, Session, Shepherd, follow};
use crate::launch::effective_settings;

const CWD: &str = "/work/web";
const SHEP_HOME: &str = "/home/me/.shep";

fn table(value: Value) -> Map<String, Value> {
    let Value::Object(table) = value else {
        panic!("a table is a JSON object: {value}");
    };
    table
}

/// The settings `launch` would hand crabpot for `value`.
fn settings(value: Value) -> Map<String, Value> {
    effective_settings(table(value), Path::new(CWD), Path::new(SHEP_HOME))
        .expect("crabpot reads the table")
}

/// A connection that says what it is scripted to say, then stays quiet, or
/// is lost when `lost` is set.
#[derive(Default)]
struct FakeSession {
    heard: VecDeque<Heard>,
    tables: VecDeque<Result<Option<Map<String, Value>>, ShepherdError>>,
    lost: bool,
    asked: Rc<RefCell<Vec<String>>>,
}

impl FakeSession {
    fn table(mut self, value: Value) -> Self {
        self.tables.push_back(Ok(Some(table(value))));
        self
    }

    fn no_table(mut self) -> Self {
        self.tables.push_back(Ok(None));
        self
    }

    fn table_fails(mut self) -> Self {
        self.tables
            .push_back(Err(ShepherdError::from(ConnectError::HandshakeClosed)));
        self
    }

    fn hears(mut self, sheep: &str) -> Self {
        self.heard.push_back(Heard::Sheep(sheep.to_owned()));
        self
    }

    fn then_lost(mut self) -> Self {
        self.lost = true;
        self
    }
}

impl Session for FakeSession {
    async fn heard(&mut self) -> Option<Heard> {
        match self.heard.pop_front() {
            Some(heard) => Some(heard),
            None if self.lost => None,
            None => std::future::pending().await,
        }
    }

    async fn table(&mut self, sheep: &str) -> Result<Option<Map<String, Value>>, ShepherdError> {
        self.asked.borrow_mut().push(sheep.to_owned());
        self.tables.pop_front().expect("a table was scripted")
    }
}

/// A shepherd that answers each connect with the next scripted outcome, and
/// never answers once they run out.
#[derive(Default)]
struct FakeShepherd {
    connects: VecDeque<Result<FakeSession, ShepherdError>>,
    connected_at: Vec<tokio::time::Instant>,
    asked: Rc<RefCell<Vec<String>>>,
    /// Set for a connection that cannot be dialled again.
    link_only: bool,
}

impl FakeShepherd {
    fn with(session: FakeSession) -> Self {
        let mut shepherd = Self::default();
        shepherd.then(session);
        shepherd
    }

    fn then(&mut self, mut session: FakeSession) {
        session.asked = Rc::clone(&self.asked);
        self.connects.push_back(Ok(session));
    }

    fn then_unreachable(&mut self) {
        self.connects
            .push_back(Err(ShepherdError::from(ConnectError::HandshakeClosed)));
    }
}

impl Shepherd for FakeShepherd {
    type Session = FakeSession;

    async fn connect(&mut self) -> Result<FakeSession, ShepherdError> {
        self.connected_at.push(tokio::time::Instant::now());
        match self.connects.pop_front() {
            Some(outcome) => outcome,
            None => std::future::pending().await,
        }
    }

    fn redials(&self) -> bool {
        !self.link_only
    }
}

/// What the follower wrote, as text.
struct Said {
    control: String,
    log: String,
}

/// Follows `web` from `last` until the shepherd has nothing more to say.
async fn followed(shepherd: &mut FakeShepherd, last: Map<String, Value>) -> Said {
    let mut control = Vec::new();
    let mut log = Vec::new();
    let finished = timeout(
        Duration::from_secs(3600),
        follow(
            shepherd,
            "web",
            Path::new(CWD),
            Path::new(SHEP_HOME),
            last,
            &mut control,
            &mut log,
        ),
    )
    .await;
    assert!(finished.is_err(), "the follower returned on its own");
    Said {
        control: String::from_utf8(control).expect("UTF-8"),
        log: String::from_utf8(log).expect("UTF-8"),
    }
}

/// As [`followed`], for a shepherd the follower gives up on by itself.
async fn followed_to_the_end(shepherd: &mut FakeShepherd, last: Map<String, Value>) -> Said {
    let mut control = Vec::new();
    let mut log = Vec::new();
    timeout(
        Duration::from_secs(3600),
        follow(
            shepherd,
            "web",
            Path::new(CWD),
            Path::new(SHEP_HOME),
            last,
            &mut control,
            &mut log,
        ),
    )
    .await
    .expect("the follower ends once the link is lost");
    Said {
        control: String::from_utf8(control).expect("UTF-8"),
        log: String::from_utf8(log).expect("UTF-8"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_change_to_allowed_domains_is_sent_as_one_json_line() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table(json!({"network": {"allowedDomains": ["example.com"]}}))
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control.matches('\n').count(), 1, "{:?}", said.control);
    assert!(said.control.ends_with('\n'));
    let line: Value = serde_json::from_str(&said.control).expect("a JSON line");
    assert_eq!(
        line,
        json!({
            "network": {"allowedDomains": ["example.com"], "deniedDomains": []},
            "filesystem": {
                "denyRead": ["~", SHEP_HOME],
                "allowRead": [{"path": CWD, "literal": true}],
                "allowWrite": [CWD],
                "denyWrite": [],
            },
        })
    );
    assert_eq!(said.log, "");
}

#[tokio::test(start_paused = true)]
async fn a_change_to_a_filesystem_key_is_logged_as_restart_needed_and_nothing_is_sent() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table(json!({"filesystem": {"denyRead": ["/etc/secrets"]}}))
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "");
    assert_eq!(
        said.log,
        "shep-sandbox: web: filesystem.denyRead changed, restart the sheep to apply it\n"
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_to_both_sends_the_network_lists_and_logs_the_rest() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table(json!({
                "network": {"deniedDomains": ["bad.example"], "allowLocalBinding": true},
                "filesystem": {"denyWrite": ["/etc"]},
            }))
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(
        said.control.matches('\n').count(),
        1,
        "{:?} {:?}",
        said.control,
        said.log
    );
    let line: Value = serde_json::from_str(&said.control).expect("a JSON line");
    assert_eq!(line["network"]["deniedDomains"], json!(["bad.example"]));
    assert_eq!(
        said.log,
        "shep-sandbox: web: filesystem.denyWrite changed, restart the sheep to apply it\n\
         shep-sandbox: web: network.allowLocalBinding changed, restart the sheep to apply it\n"
    );
}

#[tokio::test(start_paused = true)]
async fn a_table_crabpot_refuses_is_logged_and_nothing_is_sent() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table(json!({"network": {"allowedDomains": ["*"]}}))
            .hears("web")
            .table(json!({"network": {"allowedDomains": ["example.com"]}}))
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    let (refused, kept) = said.log.split_once('\n').expect("a line per edit");
    assert!(refused.starts_with("shep-sandbox: web: "), "{refused}");
    assert!(refused.contains("network.allowedDomains"), "{refused}");
    assert!(refused.ends_with("the settings in force stay"), "{refused}");
    assert_eq!(kept, "");
    // The good edit after it is still applied against the last good settings.
    let line: Value = serde_json::from_str(&said.control).expect("one JSON line");
    assert_eq!(line["network"]["allowedDomains"], json!(["example.com"]));
}

#[tokio::test(start_paused = true)]
async fn a_removed_table_is_logged_and_nothing_is_sent() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .no_table()
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "");
    assert_eq!(
        said.log,
        "shep-sandbox: web: the table was removed, the next start is refused\n"
    );
}

#[tokio::test(start_paused = true)]
async fn an_event_for_another_sheep_is_ignored() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .hears("api")
            .hears("api"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!((said.control.as_str(), said.log.as_str()), ("", ""));
    assert_eq!(
        *shepherd.asked.borrow(),
        ["web"],
        "only the fetch on connecting"
    );
}

#[tokio::test(start_paused = true)]
async fn a_lost_connection_keeps_the_last_lists_and_refetches_on_reconnect() {
    let mut shepherd = FakeShepherd::with(FakeSession::default().table(json!({})).then_lost());
    shepherd.then_unreachable();
    shepherd.then(
        FakeSession::default().table(json!({"network": {"allowedDomains": ["example.com"]}})),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(
        said.log,
        "shep-sandbox: web: lost the shepherd, keeping the sandbox as it is until it is back\n",
        "one line for the loss, none for the attempts that failed"
    );
    let line: Value = serde_json::from_str(&said.control).expect("the edit made while away");
    assert_eq!(line["network"]["allowedDomains"], json!(["example.com"]));
    let [first, second, third] = shepherd.connected_at[..] else {
        panic!("three connects: {:?}", shepherd.connected_at);
    };
    assert!(
        third - second > second - first,
        "the wait grows: {:?} then {:?}",
        second - first,
        third - second
    );
}

#[tokio::test(start_paused = true)]
async fn a_table_that_cannot_be_read_is_read_again_after_reconnecting() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table_fails()
            .hears("web"),
    );
    shepherd.then(
        FakeSession::default().table(json!({"network": {"allowedDomains": ["example.com"]}})),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    let line: Value = serde_json::from_str(&said.control).expect("the edit that could not be read");
    assert_eq!(line["network"]["allowedDomains"], json!(["example.com"]));
    assert!(said.log.contains("cannot read the table"), "{}", said.log);
}

#[tokio::test(start_paused = true)]
async fn a_key_changed_back_to_the_value_the_sandbox_runs_needs_no_restart() {
    let mut shepherd = FakeShepherd::with(
        FakeSession::default()
            .table(json!({}))
            .table(json!({"filesystem": {"denyRead": ["/etc/secrets"]}}))
            .hears("web")
            .table(json!({}))
            .hears("web"),
    );

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "");
    assert_eq!(
        said.log,
        "shep-sandbox: web: filesystem.denyRead changed, restart the sheep to apply it\n"
    );
}

#[tokio::test(start_paused = true)]
async fn a_table_the_shepherd_keeps_refusing_is_retried_with_a_growing_wait() {
    let mut shepherd = FakeShepherd::default();
    for _ in 0..4 {
        shepherd.then(FakeSession::default().table_fails());
    }

    let said = followed(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "");
    assert!(!said.log.contains("lost the shepherd"), "{}", said.log);
    let waits: Vec<_> = shepherd
        .connected_at
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    // The connect after the last scripted one never answers, so it ends the run.
    let secs: Vec<u64> = waits.iter().map(Duration::as_secs).collect();
    assert_eq!(secs, [1, 2, 4, 8]);
}

#[tokio::test(start_paused = true)]
async fn a_lost_link_keeps_the_settings_and_says_a_restart_is_needed() {
    let mut shepherd = FakeShepherd::with(FakeSession::default().table(json!({})).then_lost());
    shepherd.link_only = true;
    shepherd.then(
        FakeSession::default().table(json!({"network": {"allowedDomains": ["example.com"]}})),
    );

    let said = followed_to_the_end(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "", "the settings in force are left alone");
    assert_eq!(
        said.log,
        "shep-sandbox: web: lost the link to the shepherd, the next change to the table needs a restart of the sheep to apply\n"
    );
    assert_eq!(
        shepherd.connected_at.len(),
        1,
        "the link is never dialled again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_link_whose_table_cannot_be_read_is_not_dialled_again() {
    let mut shepherd = FakeShepherd::with(FakeSession::default().table_fails());
    shepherd.link_only = true;

    let said = followed_to_the_end(&mut shepherd, settings(json!({}))).await;

    assert_eq!(said.control, "");
    assert!(said.log.contains("cannot read the table"), "{}", said.log);
    assert!(said.log.contains("needs a restart"), "{}", said.log);
    assert_eq!(shepherd.connected_at.len(), 1);
}
