//! How `launch` reaches the shepherd and asks it for the sheep's table.

use std::path::PathBuf;

use serde_json::{Map, Value};
use shep_client::{
    ReconnectingClient,
    dogs::{DogIdentity, ShepherdError},
    shep_core::{config::DogTable, dogs::DOG_LINK_FD_VAR, paths::ShepPaths},
};

use super::{Fetched, Refusal};
use crate::follow::{Connection, Held};

/// The paths `$SHEP_HOME` names.
///
/// # Errors
///
/// [`Refusal::NoHome`] when it is unset, and [`Refusal::Home`] when it names no
/// usable directory.
fn shep_paths() -> Result<shep_client::shep_core::paths::ShepPaths, Refusal> {
    if std::env::var_os("SHEP_HOME").is_none() {
        return Err(Refusal::NoHome);
    }
    shep_client::dogs::resolve_paths(&|key| std::env::var_os(key)).map_err(Refusal::Home)
}

/// How `launch` reaches the shepherd.
pub(super) trait Reach {
    /// A connection to the shepherd.
    type Conn;

    /// Takes the link the shepherd spawned this process holding.
    async fn over_link(&mut self) -> Result<Self::Conn, ShepherdError>;

    /// Dials the shepherd's socket.
    async fn over_socket(&mut self) -> Result<Self::Conn, ShepherdError>;

    /// The table of `sheep`, asked over `conn`.
    async fn table(
        &mut self,
        conn: &mut Self::Conn,
        sheep: &str,
    ) -> Result<Option<Map<String, Value>>, ShepherdError>;
}

/// The sheep's table over one connection, which is the link when `has_link`
/// and the socket otherwise. A link that fails is refused and never retried on
/// the socket, because an inherited link can be taken once.
pub(super) async fn fetch_over<R: Reach>(
    reach: &mut R,
    has_link: bool,
    shep_home: PathBuf,
    sheep: &str,
) -> Result<Fetched<R::Conn>, Refusal> {
    let mut connection = if has_link {
        reach.over_link().await
    } else {
        reach.over_socket().await
    }
    .map_err(Refusal::Shepherd)?;
    let table = reach
        .table(&mut connection, sheep)
        .await
        .map_err(Refusal::Shepherd)?;
    Ok(Fetched {
        shep_home,
        table,
        connection,
    })
}

/// The shepherd `$SHEP_HOME` names. The link the shepherd handed down is
/// spoken by `src/link.rs`, because shep-client opens it only for a dog that
/// then asks for its own section, which a launch link refuses. The socket is
/// reached with the dog connection of `shep-client`.
struct Wire {
    paths: ShepPaths,
    identity: DogIdentity,
}

impl Reach for Wire {
    type Conn = Connection;

    #[cfg(unix)]
    async fn over_link(&mut self) -> Result<Connection, ShepherdError> {
        Ok(Connection::Link(crate::link::Link::inherited().await?))
    }

    #[cfg(not(unix))]
    async fn over_link(&mut self) -> Result<Connection, ShepherdError> {
        Err(
            shep_client::ConnectError::Io(std::io::Error::from(std::io::ErrorKind::Unsupported))
                .into(),
        )
    }

    async fn over_socket(&mut self) -> Result<Connection, ShepherdError> {
        let client = ReconnectingClient::connect_as(&self.paths.socket, &self.identity).await?;
        Ok(Connection::Socket(client))
    }

    async fn table(
        &mut self,
        conn: &mut Connection,
        sheep: &str,
    ) -> Result<Option<Map<String, Value>>, ShepherdError> {
        let table = match conn {
            Connection::Link(link) => link.sheep_dog_table(sheep, crate::DEFAULT_NAME).await?,
            Connection::Socket(client) => {
                client.sheep_dog_table(sheep, crate::DEFAULT_NAME).await?
            }
        };
        Ok(table.map(DogTable::into_map))
    }
}

/// The sheep's `[app.dogs.sandbox]` table, asked of the shepherd `$SHEP_HOME`
/// names. The connection and the runtime that drives it stay open in the
/// result, for the follower.
pub(super) fn fetch_table(sheep: &str) -> Result<Fetched<Held>, Refusal> {
    let paths = shep_paths()?;
    let has_link = std::env::var_os(DOG_LINK_FD_VAR).is_some();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Refusal::Runtime)?;
    let identity = DogIdentity::from_env(&|key| std::env::var(key).ok(), crate::DEFAULT_NAME);
    let mut wire = Wire {
        paths: paths.clone(),
        identity,
    };
    let Fetched {
        shep_home,
        table,
        connection,
    } = runtime.block_on(fetch_over(&mut wire, has_link, paths.home, sheep))?;
    // An inherited link cannot be dialled again, so the follower redials the
    // socket only for a launch that started without one.
    let socket = (!has_link).then_some(paths.socket);
    Ok(Fetched {
        shep_home,
        table,
        connection: Held::new(runtime, connection, socket),
    })
}
