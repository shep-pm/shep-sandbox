//! The link the shepherd spawned `launch` holding, spoken by hand.
//!
//! The shepherd hands a launched sheep one connection, scoped to that sheep's
//! own table: it answers `Hello`, the table, and a subscription to table
//! changes, and refuses every other request. shep-client opens an inherited
//! connection only through `DogRuntime::start`, which asks for the dog's own
//! section before it returns, and the launch link refuses that. The framing
//! and the three requests are small, so the dog speaks them itself rather than
//! lose the link.

use core::fmt;
use std::{collections::VecDeque, io};

use shep_client::{
    ConnectError, DEADLINE_GRACE, DEFAULT_DEADLINE, HANDSHAKE_TIMEOUT, RequestError,
    shep_core::{
        config::DogTable,
        protocol::{
            BusEvent, Envelope, Hello, HelloReply, MAX_FRAME_BYTES, PROTOCOL_VERSION, Reply,
            Request, Response, ServerFrame, decode_frame, encode_frame, reply_id,
        },
    },
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The bytes under a [`Link`].
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug> Stream for T {}

/// The bytes of a frame's length, which is a big-endian `u32`.
const HEADER: usize = 4;

/// One connection to the shepherd, which a [`Link`] drives from one task.
#[derive(Debug)]
pub struct Link {
    stream: Box<dyn Stream>,
    /// Bytes read and not yet made into a frame. A read cut short keeps its
    /// bytes here, so dropping a pending call loses none of them.
    unread: Vec<u8>,
    /// Events that arrived while a request waited for its reply.
    events: VecDeque<BusEvent>,
    next_id: u64,
}

impl Link {
    /// Takes the connection the shepherd spawned this process holding, and
    /// handshakes over it.
    ///
    /// # Errors
    ///
    /// [`ConnectError::Io`] when the descriptor `$SHEP_DOG_FD` names is not a
    /// usable socket, and as [`Self::open`] after that.
    #[cfg(unix)]
    pub async fn inherited() -> Result<Self, ConnectError> {
        use shep_client::shep_core::dogs::DOG_LINK_FD_VAR;

        let unusable = |reason: &dyn fmt::Display| {
            ConnectError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{DOG_LINK_FD_VAR}: {reason}"),
            ))
        };
        let stream = shep_channel::take_inherited_socket(DOG_LINK_FD_VAR)
            .map_err(|err| unusable(&err))?
            .ok_or_else(|| unusable(&"it is not set"))?;
        stream.set_nonblocking(true).map_err(ConnectError::Io)?;
        let stream = tokio::net::UnixStream::from_std(stream).map_err(ConnectError::Io)?;
        Self::open(stream).await
    }

    /// Handshakes over `stream`, which is anonymous: the shepherd knows what
    /// the link is for from the way it was handed down.
    ///
    /// # Errors
    ///
    /// The `ConnectError` of a handshake that did not finish: `Io` and
    /// `Wire` for a stream or reply that fails, `HandshakeClosed` and
    /// `HandshakeTimeout` for a shepherd that does not answer, and
    /// `ProtocolMismatch` for one that refuses the version.
    pub async fn open(stream: impl Stream + 'static) -> Result<Self, ConnectError> {
        let mut link = Self {
            stream: Box::new(stream),
            unread: Vec::new(),
            events: VecDeque::new(),
            next_id: 1,
        };
        let hello = Hello {
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL_VERSION,
            dog_name: None,
        };
        let hello = encode_frame(&hello).map_err(ConnectError::Wire)?;
        link.send(&hello).await.map_err(ConnectError::Io)?;
        let reply = tokio::time::timeout(HANDSHAKE_TIMEOUT, link.frame())
            .await
            .map_err(|_elapsed| ConnectError::HandshakeTimeout {
                after: HANDSHAKE_TIMEOUT,
            })?
            .map_err(ConnectError::Io)?
            .ok_or(ConnectError::HandshakeClosed)?;
        let reply: HelloReply = decode_frame(&reply).map_err(ConnectError::Wire)?;
        reply.map_err(|refusal| ConnectError::ProtocolMismatch {
            client: PROTOCOL_VERSION,
            daemon_version: refusal.daemon_version,
            message: refusal.message,
        })?;
        Ok(link)
    }

    /// The table `dog` holds for `sheep`, or `None` when it holds none.
    ///
    /// # Errors
    ///
    /// As [`Self::request`], and `UnexpectedReply` for any answer but a table.
    pub async fn sheep_dog_table(
        &mut self,
        sheep: &str,
        dog: &str,
    ) -> Result<Option<DogTable>, RequestError> {
        let asked = Request::SheepDogSettingsResolved {
            name: sheep.to_owned(),
            dog: dog.to_owned(),
        };
        match self.request(asked).await? {
            Response::SheepDogTable { table } => Ok(table),
            other => Err(RequestError::UnexpectedReply {
                asked: "SheepDogSettingsResolved",
                answered: other.name(),
            }),
        }
    }

    /// Subscribes to `topics`. The events come from [`Self::event`].
    ///
    /// # Errors
    ///
    /// As [`Self::request`].
    pub async fn subscribe(&mut self, topics: Vec<String>) -> Result<(), RequestError> {
        self.request(Request::Subscribe { topics }).await.map(drop)
    }

    /// The next event, or `None` once the shepherd closed the link or sent
    /// bytes that are no frame. Safe to drop while pending.
    pub async fn event(&mut self) -> Option<BusEvent> {
        loop {
            if let Some(event) = self.events.pop_front() {
                return Some(event);
            }
            let frame = self.frame().await.ok()??;
            if let Ok(ServerFrame::Event(event)) = decode_frame(&frame) {
                return Some(event);
            }
        }
    }

    /// Sends `body` and waits for its reply, keeping the events that arrive
    /// first for [`Self::event`].
    ///
    /// # Errors
    ///
    /// - `Rpc`: the shepherd refused the request.
    /// - `Timeout`: no reply came within the default deadline and its grace.
    /// - `Closed`: the link closed first.
    /// - `Wire` and `Undecodable`: the request would not encode, or the reply
    ///   would not decode.
    async fn request(&mut self, body: Request) -> Result<Response, RequestError> {
        let id = self.next_id;
        self.next_id += 1;
        let deadline_ms = u64::try_from(DEFAULT_DEADLINE.as_millis()).ok();
        let envelope = Envelope {
            id,
            deadline_ms,
            body,
        };
        let frame = encode_frame(&envelope).map_err(RequestError::Wire)?;
        self.send(&frame)
            .await
            .map_err(|_lost| RequestError::Closed)?;
        tokio::time::timeout(DEFAULT_DEADLINE + DEADLINE_GRACE, self.reply(id))
            .await
            .map_err(|_elapsed| RequestError::Timeout {
                after: DEFAULT_DEADLINE,
            })?
    }

    /// Reads frames until the reply to request `id`.
    async fn reply(&mut self, id: u64) -> Result<Response, RequestError> {
        loop {
            let frame = self
                .frame()
                .await
                .ok()
                .flatten()
                .ok_or(RequestError::Closed)?;
            match decode_frame::<ServerFrame>(&frame) {
                Ok(ServerFrame::Reply(Reply {
                    id: answered,
                    result,
                })) if answered == id => {
                    return result.map_err(RequestError::Rpc);
                }
                Ok(ServerFrame::Event(event)) => self.events.push_back(event),
                // The reply to a request that gave up, or a frame this build
                // does not know.
                Ok(_) => {}
                Err(err) if reply_id(&frame) == Some(id) => {
                    return Err(RequestError::Undecodable(err));
                }
                Err(_) => {}
            }
        }
    }

    /// Writes `payload` as one frame.
    async fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        let length = u32::try_from(payload.len())
            .map_err(|_big| io::Error::new(io::ErrorKind::InvalidInput, "the frame is too long"))?;
        let mut frame = Vec::with_capacity(HEADER + payload.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await
    }

    /// The next frame's payload, or `None` at the end of the stream. Safe to
    /// drop while pending.
    async fn frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(frame) = self.take_frame()? {
                return Ok(Some(frame));
            }
            if self.stream.read_buf(&mut self.unread).await? == 0 {
                return Ok(None);
            }
        }
    }

    /// The first whole frame in `unread`, taken out of it.
    fn take_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some(header) = self.unread.first_chunk::<HEADER>() else {
            return Ok(None);
        };
        let length = usize::try_from(u32::from_be_bytes(*header))
            .ok()
            .filter(|length| *length <= MAX_FRAME_BYTES)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the frame is too long"))?;
        let end = HEADER + length;
        if self.unread.len() < end {
            return Ok(None);
        }
        let frame = self.unread[HEADER..end].to_vec();
        self.unread.drain(..end);
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests;
