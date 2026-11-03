use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use shep_client::{
    ConnectError, RequestError,
    shep_core::{
        config::DogTable,
        protocol::{
            BusEvent, Envelope, Hello, HelloAck, PROTOCOL_VERSION, Reply, Request, Response,
            RpcError, RpcErrorCode, ServerFrame, decode_frame, encode_frame,
        },
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex},
    task::JoinHandle,
};

use super::Link;

/// The sheep and the dog the fake shepherd's link belongs to.
const SHEEP: &str = "boxed";
const DOG: &str = "sandbox";

/// What the fake shepherd saw.
#[derive(Debug, Default)]
struct Seen {
    hello: Option<Hello>,
    requests: Vec<Request>,
}

async fn read_frame(stream: &mut DuplexStream) -> Option<Vec<u8>> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).await.ok()?;
    let mut frame = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut frame).await.ok()?;
    Some(frame)
}

async fn write_frame(stream: &mut DuplexStream, value: &impl Serialize) {
    let payload = encode_frame(value).expect("the value encodes");
    let length = u32::try_from(payload.len()).expect("a short frame");
    stream
        .write_all(&length.to_be_bytes())
        .await
        .expect("the link is open");
    stream.write_all(&payload).await.expect("the link is open");
}

fn ack() -> HelloAck {
    HelloAck {
        daemon_version: "9.9.9".into(),
        protocol: PROTOCOL_VERSION,
        pid: 1,
        min_supported: None,
    }
}

fn reply(id: u64, result: Result<Response, RpcError>) -> ServerFrame {
    ServerFrame::Reply(Reply { id, result })
}

fn changed(sheep: &str) -> ServerFrame {
    ServerFrame::Event(BusEvent::DogSheepSettingsChanged {
        dog: DOG.into(),
        sheep: sheep.into(),
    })
}

/// The shepherd's end of a launch link. It admits what shep's gate admits for
/// a launched sheep: the handshake, that sheep's table for that dog, and a
/// subscription to that dog's tables. It refuses everything else as
/// `Forbidden`, and says each request it saw when the link closes.
/// `before` is sent ahead of the answer to the first request after the
/// handshake.
fn launch_gate(
    mut shepherd: DuplexStream,
    before: Vec<ServerFrame>,
) -> JoinHandle<(Seen, DuplexStream)> {
    tokio::spawn(async move {
        let mut seen = Seen::default();
        let hello = read_frame(&mut shepherd).await.expect("a hello");
        seen.hello = Some(decode_frame(&hello).expect("a hello frame"));
        write_frame(&mut shepherd, &Ok::<_, RpcError>(ack())).await;
        let mut before = Some(before);
        while let Some(frame) = read_frame(&mut shepherd).await {
            let Envelope { id, body, .. } = decode_frame(&frame).expect("a request");
            for frame in before.take().unwrap_or_default() {
                write_frame(&mut shepherd, &frame).await;
            }
            let result = match &body {
                Request::SheepDogSettingsResolved { name, dog } if name == SHEEP && dog == DOG => {
                    let table: DogTable =
                        serde_json::from_value(json!({"network": {"allowedDomains": ["a.test"]}}))
                            .expect("a table");
                    Ok(Response::SheepDogTable { table: Some(table) })
                }
                Request::Subscribe { topics } if topics == &[format!("config.sheep.{DOG}")] => {
                    Ok(Response::Subscribed)
                }
                other => Err(RpcError {
                    code: RpcErrorCode::Forbidden,
                    message: format!("{other:?} is not allowed on a launch link"),
                    daemon_version: None,
                }),
            };
            seen.requests.push(body);
            write_frame(&mut shepherd, &reply(id, result)).await;
        }
        (seen, shepherd)
    })
}

#[tokio::test]
async fn a_link_reads_its_sheeps_table_and_asks_for_nothing_else() {
    let (near, far) = duplex(4096);
    let gate = launch_gate(far, Vec::new());

    let mut link = Link::open(near).await.expect("the handshake completes");
    let table = link
        .sheep_dog_table(SHEEP, DOG)
        .await
        .expect("the shepherd answers")
        .expect("the sheep has a table");
    link.subscribe(vec![format!("config.sheep.{DOG}")])
        .await
        .expect("the subscription is admitted");
    drop(link);

    assert_eq!(
        table.as_map().get("network"),
        Some(&json!({"allowedDomains": ["a.test"]}))
    );
    let (seen, _shepherd) = gate.await.expect("the gate ran");
    assert_eq!(
        seen.requests,
        [
            Request::SheepDogSettingsResolved {
                name: SHEEP.into(),
                dog: DOG.into()
            },
            Request::Subscribe {
                topics: vec![format!("config.sheep.{DOG}")]
            },
        ],
        "a launch link refuses any other request, the dog's own section included"
    );
}

#[tokio::test]
async fn a_link_says_who_it_is_by_staying_anonymous() {
    let (near, far) = duplex(4096);
    let gate = launch_gate(far, Vec::new());

    let link = Link::open(near).await.expect("the handshake completes");
    drop(link);

    let (seen, _shepherd) = gate.await.expect("the gate ran");
    let hello = seen.hello.expect("a hello");
    assert_eq!(hello.dog_name, None);
    assert_eq!(hello.protocol, PROTOCOL_VERSION);
}

#[tokio::test]
async fn a_request_the_shepherd_refuses_carries_its_refusal() {
    let (near, far) = duplex(4096);
    let _gate = launch_gate(far, Vec::new());
    let mut link = Link::open(near).await.expect("the handshake completes");

    let err = link
        .sheep_dog_table("another", DOG)
        .await
        .expect_err("another sheep's table is refused");

    let RequestError::Rpc(refusal) = err else {
        panic!("a refusal from the shepherd, got {err:?}");
    };
    assert_eq!(refusal.code, RpcErrorCode::Forbidden);
}

#[tokio::test]
async fn events_that_arrive_before_a_reply_are_heard_afterwards() {
    let (near, far) = duplex(4096);
    let _gate = launch_gate(far, vec![changed("boxed"), changed("other")]);
    let mut link = Link::open(near).await.expect("the handshake completes");

    link.subscribe(vec![format!("config.sheep.{DOG}")])
        .await
        .expect("the subscription is admitted");

    for sheep in ["boxed", "other"] {
        let Some(BusEvent::DogSheepSettingsChanged { sheep: heard, .. }) = link.event().await
        else {
            panic!("a change of {sheep}'s table");
        };
        assert_eq!(heard, sheep);
    }
}

#[tokio::test]
async fn a_pushed_event_reaches_a_link_that_is_waiting_for_one() {
    let (near, mut far) = duplex(4096);
    tokio::spawn(async move {
        let _hello = read_frame(&mut far).await.expect("a hello");
        write_frame(&mut far, &Ok::<_, RpcError>(ack())).await;
        write_frame(&mut far, &changed("boxed")).await;
        // Held open, so the event is the only thing the link hears.
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let mut link = Link::open(near).await.expect("the handshake completes");

    let heard = link.event().await;

    assert!(matches!(
        heard,
        Some(BusEvent::DogSheepSettingsChanged { sheep, .. }) if sheep == "boxed"
    ));
}

#[tokio::test]
async fn a_link_the_shepherd_closes_hears_nothing_more() {
    let (near, mut far) = duplex(4096);
    tokio::spawn(async move {
        let _hello = read_frame(&mut far).await.expect("a hello");
        write_frame(&mut far, &Ok::<_, RpcError>(ack())).await;
        // `far` drops here, which closes the link.
    });
    let mut link = Link::open(near).await.expect("the handshake completes");

    assert!(link.event().await.is_none());
    let err = link
        .sheep_dog_table(SHEEP, DOG)
        .await
        .expect_err("nobody answers");
    assert_eq!(err, RequestError::Closed);
}

#[tokio::test]
async fn frames_cut_into_small_reads_are_put_back_together() {
    // Eight bytes at a time, so every frame arrives in pieces.
    let (near, far) = duplex(8);
    let _gate = launch_gate(far, vec![changed("boxed")]);
    let mut link = Link::open(near).await.expect("the handshake completes");

    let table = link
        .sheep_dog_table(SHEEP, DOG)
        .await
        .expect("the shepherd answers");

    assert!(table.is_some());
    assert!(matches!(
        link.event().await,
        Some(BusEvent::DogSheepSettingsChanged { .. })
    ));
}

#[tokio::test]
async fn a_shepherd_that_refuses_the_protocol_is_a_connect_error() {
    let (near, mut far) = duplex(4096);
    tokio::spawn(async move {
        let _hello = read_frame(&mut far).await.expect("a hello");
        let refusal = RpcError {
            code: RpcErrorCode::ProtocolMismatch,
            message: "too old".into(),
            daemon_version: Some("0.1.0".into()),
        };
        write_frame(&mut far, &Err::<HelloAck, _>(refusal)).await;
    });

    let err = Link::open(near).await.expect_err("the shepherd refuses");

    assert!(
        matches!(err, ConnectError::ProtocolMismatch { ref message, .. } if message == "too old"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn a_shepherd_that_closes_before_it_answers_is_a_closed_handshake() {
    let (near, far) = duplex(4096);
    drop(far);

    let err = Link::open(near).await;

    assert!(
        matches!(
            err,
            Err(ConnectError::HandshakeClosed | ConnectError::Io(_))
        ),
        "got {err:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reply_that_never_comes_times_out_instead_of_hanging() {
    let (near, mut far) = duplex(4096);
    tokio::spawn(async move {
        let _hello = read_frame(&mut far).await.expect("a hello");
        write_frame(&mut far, &Ok::<_, RpcError>(ack())).await;
        // Reads the request and says nothing, with the link left open.
        let _request = read_frame(&mut far).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    let mut link = Link::open(near).await.expect("the handshake completes");

    let err = link
        .sheep_dog_table(SHEEP, DOG)
        .await
        .expect_err("no answer comes");

    assert!(matches!(err, RequestError::Timeout { .. }), "got {err:?}");
}
