use std::{cell::RefCell, collections::VecDeque, path::Path, rc::Rc, time::Duration};

use serde_json::{Map, Value, json};
use shep_client::{ConnectError, dogs::ShepherdError};
use tokio::time::timeout;

use super::{Heard, Session, Shepherd, follow};
use crate::launch::effective_settings;

const CWD: &str = "/work/web";

fn table(value: Value) -> Map<String, Value> {
    let Value::Object(table) = value else {
        panic!("a table is a JSON object: {value}");
    };
    table
}

/// The settings `launch` would hand crabpot for `value`.
fn settings(value: Value) -> Map<String, Value> {
    effective_settings(table(value), Path::new(CWD)).expect("crabpot reads the table")
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
            "filesystem": {"denyRead": [], "allowWrite": [CWD], "denyWrite": []},
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
        "shep-sandbox: web: the table was removed, the next start runs without the sandbox\n"
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
