//! Follows a running sheep's `[app.dogs.sandbox]` table.
//!
//! The shepherd tells the dog when the table changes. A change to a network
//! list reaches the running sandbox as one JSON line on crabpot's control
//! descriptor, and a change to any other key is said once on stderr, which is
//! the sheep's err log, because crabpot applies it only at the next start.

use std::{
    collections::BTreeMap,
    io::{self, PipeWriter, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use serde_json::{Map, Value};
use shep_client::{
    ConnectError, EventStream, Lagged, ReconnectingClient,
    dogs::ShepherdError,
    shep_core::{config::DogTable, protocol::BusEvent},
};

use crate::{launch::effective_settings, link::Link};

/// What the shepherd said, once the follower is subscribed.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    /// The table of this sheep changed.
    Sheep(String),
    /// Events were dropped before they were read, so any table may have
    /// changed.
    Lagged,
}

/// One live connection to the shepherd, subscribed to table changes.
pub trait Session {
    /// The next thing the shepherd said, or `None` once the connection is
    /// lost.
    async fn heard(&mut self) -> Option<Heard>;

    /// The sheep's table now, or `None` when it carries none.
    async fn table(&mut self, sheep: &str) -> Result<Option<Map<String, Value>>, ShepherdError>;
}

/// Where connections to the shepherd come from.
pub trait Shepherd {
    /// The connection this makes.
    type Session: Session;

    /// Connects and subscribes.
    async fn connect(&mut self) -> Result<Self::Session, ShepherdError>;

    /// Whether a connection that is lost can be made again. An inherited link
    /// cannot: the shepherd handed it down once.
    fn redials(&self) -> bool {
        true
    }
}

/// The keys crabpot swaps in a running sandbox.
const LIVE_KEYS: [&str; 3] = [
    "network.allowedDomains",
    "network.deniedDomains",
    "network.deniedResolvedAddresses",
];

/// What the follower says when an inherited link is lost, as it cannot be
/// made again.
const LINK_LOST: &str = "lost the link to the shepherd, the next change to the table \
                         needs a restart of the sheep to apply";

/// The wait before the first reconnect, doubled for each one that fails.
const FIRST_WAIT: Duration = Duration::from_secs(1);

/// The longest wait between reconnects.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// Follows `sheep`'s table until the control pipe closes.
///
/// `last` is the settings the sandbox started with. A change to a network list
/// goes to `control` as one JSON line, and everything else the follower has to
/// say goes to `log`. A lost connection keeps the settings in force, and the
/// follower reconnects with a growing wait, unless the shepherd cannot be
/// redialled, which ends the follower after one line saying that the next
/// change needs a restart.
pub async fn follow(
    shepherd: &mut impl Shepherd,
    sheep: &str,
    cwd: &Path,
    shep_home: &Path,
    last: Map<String, Value>,
    control: &mut impl Write,
    log: &mut impl Write,
) {
    let mut follower = Follower {
        sheep,
        cwd,
        shep_home,
        last,
        control,
        log,
    };
    let mut wait = FIRST_WAIT;
    let mut reported = false;
    loop {
        match shepherd.connect().await {
            Ok(mut session) => match follower.session(&mut session).await {
                Ended::Closed => return,
                Ended::Lost | Ended::Unreadable if !shepherd.redials() => {
                    follower.say(LINK_LOST);
                    return;
                }
                Ended::Lost => {
                    wait = FIRST_WAIT;
                    follower
                        .say("lost the shepherd, keeping the sandbox as it is until it is back");
                    reported = true;
                }
                // Said already, and the wait keeps growing: the shepherd is
                // there, so a connection that works is no sign it is well.
                Ended::Unreadable => {}
            },
            // The link was handed down once and is gone, whatever it failed
            // on: a connection made again would find nothing to take.
            Err(_) if !shepherd.redials() => {
                follower.say(LINK_LOST);
                return;
            }
            Err(err) if !reported => {
                follower.say(&format!(
                    "cannot reach the shepherd ({err}), edits to the table wait until it is back"
                ));
                reported = true;
            }
            Err(_) => {}
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(LONGEST_WAIT);
    }
}

/// What a read of the table leaves the session to do.
enum Flow {
    /// Keep following.
    Go,
    /// Drop the connection, so the follower reconnects and reads again.
    Lost,
    /// The control pipe is closed.
    Closed,
}

/// How a connection ended.
enum Ended {
    /// The control pipe is closed.
    Closed,
    /// The shepherd went away after the table had been read.
    Lost,
    /// The first read of the table failed.
    Unreadable,
}

struct Follower<'a, C, L> {
    sheep: &'a str,
    cwd: &'a Path,
    shep_home: &'a Path,
    last: Map<String, Value>,
    control: &'a mut C,
    log: &'a mut L,
}

impl<C: Write, L: Write> Follower<'_, C, L> {
    /// Follows one connection until it ends.
    async fn session(&mut self, session: &mut impl Session) -> Ended {
        match self.refresh(session).await {
            Flow::Go => {}
            Flow::Lost => return Ended::Unreadable,
            Flow::Closed => return Ended::Closed,
        }
        while let Some(heard) = session.heard().await {
            let mine = match &heard {
                Heard::Sheep(sheep) => sheep == self.sheep,
                Heard::Lagged => true,
            };
            if !mine {
                continue;
            }
            match self.refresh(session).await {
                Flow::Go => {}
                Flow::Lost => return Ended::Lost,
                Flow::Closed => return Ended::Closed,
            }
        }
        Ended::Lost
    }

    /// Reads the table and applies it.
    async fn refresh(&mut self, session: &mut impl Session) -> Flow {
        let table = match session.table(self.sheep).await {
            Ok(Some(table)) => table,
            Ok(None) => {
                self.say("the table was removed, the next start is refused");
                return Flow::Go;
            }
            Err(err) => {
                // Ending the session reads the table again once reconnected.
                self.say(&format!("cannot read the table: {err}"));
                return Flow::Lost;
            }
        };
        match effective_settings(table, self.cwd, self.shep_home) {
            Ok(settings) => self.apply(settings),
            Err(refusal) => {
                self.say(&format!("{refusal}, the settings in force stay"));
                Flow::Go
            }
        }
    }

    /// Sends the network lists if one changed, and names every other key that
    /// did. Only the network lists become the settings in force, because
    /// the sandbox runs no other key's new value.
    fn apply(&mut self, settings: Map<String, Value>) -> Flow {
        let changed = changed_keys(&self.last, &settings);
        if changed.iter().any(|key| LIVE_KEYS.contains(&key.as_str())) {
            let mut line = serde_json::to_vec(&settings).expect("a JSON object has a text form");
            line.push(b'\n');
            if self
                .control
                .write_all(&line)
                .and_then(|()| self.control.flush())
                .is_err()
            {
                return Flow::Closed;
            }
        }
        for key in changed
            .iter()
            .filter(|key| !LIVE_KEYS.contains(&key.as_str()))
        {
            self.say(&format!("{key} changed, restart the sheep to apply it"));
        }
        for key in LIVE_KEYS {
            self.last = with_key(&self.last, key, &settings);
        }
        Flow::Go
    }

    /// One line of the sheep's err log.
    fn say(&mut self, message: &str) {
        // One write, so the line is not split by the sheep's own output into
        // the same log. A log nobody can write to has no one to tell.
        let line = format!("shep-sandbox: {}: {message}\n", self.sheep);
        let _ = self.log.write_all(line.as_bytes());
    }
}

/// `base` with the value of `key` (`group.name`) taken from `from`, or without
/// it where `from` has none.
fn with_key(base: &Map<String, Value>, key: &str, from: &Map<String, Value>) -> Map<String, Value> {
    let (group, name) = key.split_once('.').expect("a live key names a group");
    let value = from.get(group).and_then(|group| group.get(name)).cloned();
    let mut next = base.clone();
    let slot = next
        .entry(group)
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(slot) = slot {
        match value {
            Some(value) => slot.insert(name.to_owned(), value),
            None => slot.remove(name),
        };
    }
    next
}

/// The keys whose value differs between `old` and `new`, sorted. A key inside
/// a group is named `group.key`.
fn changed_keys(old: &Map<String, Value>, new: &Map<String, Value>) -> Vec<String> {
    let (old, new) = (leaves(old), leaves(new));
    let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|key| old.get(*key) != new.get(*key))
        .cloned()
        .collect()
}

/// Each value of `settings` by its name, flattening one level of groups.
fn leaves(settings: &Map<String, Value>) -> BTreeMap<String, &Value> {
    let mut leaves = BTreeMap::new();
    for (name, value) in settings {
        match value {
            Value::Object(group) => {
                for (key, value) in group {
                    leaves.insert(format!("{name}.{key}"), value);
                }
            }
            _ => {
                leaves.insert(name.clone(), value);
            }
        }
    }
    leaves
}

/// A connection to the shepherd that `launch` asked for the table over.
#[derive(Debug)]
pub enum Connection {
    /// The link the shepherd spawned `launch` holding.
    Link(Link),
    /// A dial of the shepherd's socket.
    Socket(ReconnectingClient),
}

/// The connections to the shepherd: the one `launch` opened, then the socket
/// when there is one to dial.
#[derive(Debug)]
struct Wire {
    first: Option<Connection>,
    socket: Option<PathBuf>,
}

/// The topic that carries the changes of this dog's tables.
fn topic() -> String {
    format!("config.sheep.{}", crate::DEFAULT_NAME)
}

/// A connection to the shepherd, subscribed to this dog's tables.
#[derive(Debug)]
enum Live {
    Link(Link),
    Socket {
        client: ReconnectingClient,
        events: EventStream,
    },
}

impl Live {
    async fn subscribe(connection: Connection) -> Result<Self, ShepherdError> {
        match connection {
            Connection::Link(mut link) => {
                link.subscribe(vec![topic()]).await?;
                Ok(Self::Link(link))
            }
            Connection::Socket(client) => {
                let events = client.subscribe(vec![topic()]).await?;
                Ok(Self::Socket { client, events })
            }
        }
    }
}

impl Shepherd for Wire {
    type Session = Live;

    async fn connect(&mut self) -> Result<Live, ShepherdError> {
        let connection = match (self.first.take(), &self.socket) {
            (Some(connection), _) => connection,
            (None, Some(socket)) => Connection::Socket(ReconnectingClient::connect(socket).await?),
            (None, None) => {
                return Err(ConnectError::Io(io::Error::other(
                    "the inherited link cannot be dialled again",
                ))
                .into());
            }
        };
        Live::subscribe(connection).await
    }

    fn redials(&self) -> bool {
        self.socket.is_some()
    }
}

impl Session for Live {
    async fn heard(&mut self) -> Option<Heard> {
        loop {
            let event = match self {
                Self::Link(link) => Ok(link.event().await?),
                Self::Socket { events, .. } => events.next().await?,
            };
            match event {
                Ok(BusEvent::DogSheepSettingsChanged { sheep, .. }) => {
                    return Some(Heard::Sheep(sheep));
                }
                Ok(_) => {}
                Err(Lagged { .. }) => return Some(Heard::Lagged),
            }
        }
    }

    async fn table(&mut self, sheep: &str) -> Result<Option<Map<String, Value>>, ShepherdError> {
        let table = match self {
            Self::Link(link) => link.sheep_dog_table(sheep, crate::DEFAULT_NAME).await?,
            Self::Socket { client, .. } => {
                client.sheep_dog_table(sheep, crate::DEFAULT_NAME).await?
            }
        };
        Ok(table.map(DogTable::into_map))
    }
}

/// The connection `launch` fetched the table over, with the runtime that
/// drives it, for the follower to take over.
#[derive(Debug)]
pub struct Held {
    runtime: tokio::runtime::Runtime,
    wire: Wire,
}

impl Held {
    /// `connection` as the follower's first, on `runtime`, which spawned its
    /// tasks. `socket` is where to dial once `connection` is lost, or `None`
    /// for a connection that cannot be dialled again.
    pub fn new(
        runtime: tokio::runtime::Runtime,
        connection: Connection,
        socket: Option<PathBuf>,
    ) -> Self {
        Self {
            runtime,
            wire: Wire {
                first: Some(connection),
                socket,
            },
        }
    }
}

/// Follows `sheep`'s table on a thread of its own, over the connection `held`
/// carries to the shepherd whose home is `shep_home`, and writes the network
/// lists to `control`. The thread ends with the process, or once `control` is
/// closed.
///
/// # Errors
///
/// Whatever starting the thread fails with.
pub fn spawn(
    held: Held,
    sheep: String,
    cwd: PathBuf,
    shep_home: PathBuf,
    last: Map<String, Value>,
    mut control: PipeWriter,
) -> io::Result<()> {
    thread::Builder::new()
        .name("follow".into())
        .spawn(move || {
            let Held { runtime, mut wire } = held;
            runtime.block_on(follow(
                &mut wire,
                &sheep,
                &cwd,
                &shep_home,
                last,
                &mut control,
                &mut io::stderr(),
            ));
        })
        .map(drop)
}

#[cfg(test)]
mod tests;
