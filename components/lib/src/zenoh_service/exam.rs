//! The service exam: the core driven against a session, printing one `[zenoh-exam] service ...` line for
//! each thing it did. `scripts/check/check-zenoh-profile0.py --slice service` owns what those lines must
//! be, and `scripts/lib/zenoh_service_exam.py` owns the bytes, the order and the classes.

use super::{Core, Item, Peer, Row, ServiceError};
use crate::zenoh_profile0::encode::{self, FrameEncoder};
use crate::zenoh_profile0::session::{Config, Event, Params, Role, SessionError, SessionOf};
use crate::zenoh_profile0::{MAX_BATCH_BYTES, MAX_KEY_BYTES, Mapping, StreamFramer};
use slime_proto::zenoh_service_grants as grants;
use std::string::String;
use std::vec::Vec;

/// Three holders, four ring slots, 12 payload bytes and five rows.
pub type Exam<'k> = Core<'k, 3, 4, 12, 5>;
/// The session: the service is its listener. It may declare three subscriptions and takes the peer's two.
pub type Svc = SessionOf<1, 5, MAX_KEY_BYTES, 4>;
/// The peer's session, which takes the service's three and may declare two.
pub type Far = SessionOf<1, 5, MAX_KEY_BYTES, 4>;

pub const KEYS: [&str; 4] = [
    "0/slime_demo/counter/slime_demo_msgs::msg::dds_::Counter_/RIHS01_a82fd5ffcb96d0a197a5ad3680d1c4e6ba43a962928ecd592fb565eb8129595b",
    "0/slime_demo/extra/second",
    "0/slime_demo/extra/third",
    "0/slime_demo/extra/fourth",
];
pub const HOLDERS: [&str; 3] = ["a", "b", "c"];
/// (holder, key, publish, subscribe).
pub const ROWS: [(usize, usize, bool, bool); 5] = [
    (0, 0, true, true),
    (0, 1, false, true),
    (1, 0, false, true),
    (1, 2, true, false),
    (2, 1, false, true),
];
pub const GID: [u8; 16] = [0xe0; 16];
pub const SERVICE_SN: u32 = 7000;
const LEASE_MS: u64 = 10_000;

pub fn params() -> Params {
    Params {
        local: 3,
        per_side: 2,
        events: 4,
        ..Params::DEMO
    }
}

pub fn rows() -> [Row<'static>; 5] {
    ROWS.map(|(holder, key, publish, subscribe)| Row {
        holder,
        key: KEYS[key],
        publish,
        subscribe,
    })
}

pub fn line(text: &str) {
    println!("[zenoh-exam] service {text}");
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn payload(sequence: u8) -> [u8; 3] {
    [sequence, 2, 3]
}

fn service_config() -> Config<'static> {
    Config {
        zid: &[0xb2; 8],
        lease_ms: LEASE_MS,
        initial_sn: SERVICE_SN,
        cookie: &[0xc0; 32],
    }
}

fn peer_config() -> Config<'static> {
    Config {
        zid: &[0xa1; 8],
        lease_ms: LEASE_MS,
        initial_sn: 9000,
        cookie: &[],
    }
}

/// A batch carried over a byte stream and read back, so the exam sees what a link would deliver.
pub fn through_stream(batch: &[u8]) -> Vec<u8> {
    let mut wire = [0u8; MAX_BATCH_BYTES + 2];
    let total = encode::stream_batch(&mut wire, batch).expect("stream");
    let mut framer = StreamFramer::new();
    assert_eq!(framer.push(&wire[..total]), total);
    framer
        .peek_batch()
        .expect("prefix")
        .expect("complete")
        .to_vec()
}

/// Opens the service (the listener) and its peer (the connector).
pub fn open() -> (Svc, Far) {
    let mut service =
        Svc::with_params(Role::Listener, &service_config(), params()).expect("service");
    let mut peer = Far::with_params(Role::Connector, &peer_config(), params()).expect("peer");
    let mut out = [0u8; MAX_BATCH_BYTES];
    let syn = peer
        .connect(0, &mut out)
        .expect("connect")
        .batch(&out)
        .to_vec();
    let ack = service
        .on_batch(&through_stream(&syn), 1, &mut out)
        .expect("init ack")
        .batch(&out)
        .to_vec();
    let open_syn = peer
        .on_batch(&through_stream(&ack), 2, &mut out)
        .expect("open syn")
        .batch(&out)
        .to_vec();
    let open_ack = service
        .on_batch(&through_stream(&open_syn), 3, &mut out)
        .expect("open ack")
        .batch(&out)
        .to_vec();
    peer.on_batch(&through_stream(&open_ack), 4, &mut out)
        .expect("open");
    assert!(service.is_open() && peer.is_open());
    (service, peer)
}

/// A FRAME carrying one Put to `key` from the peer. The peer's own session refuses a Put to a key the
/// service never declared, so the frame is written directly, at the sequence number the service expects.
fn put_from_peer(service: &Svc, key: &str, sequence: i64) -> Vec<u8> {
    let attachment = encode::attachment(sequence, 1000 + sequence, &GID).expect("attachment");
    let mut out = [0u8; MAX_BATCH_BYTES];
    let mut frame = FrameEncoder::new(&mut out, service.expected_rx_sn()).expect("frame");
    frame
        .push_put(
            Mapping::Receiver,
            key,
            &attachment,
            &payload(sequence as u8),
        )
        .expect("put");
    let length = frame.finish().expect("finish");
    out[..length].to_vec()
}

/// Declares subscriber `id` on `key` from the peer, and returns the frame the service took.
fn peer_declares(
    service: &mut Svc,
    core: &mut Exam<'_>,
    peer: &mut Far,
    id: u32,
    key: &str,
    now: u64,
) {
    let mut out = [0u8; MAX_BATCH_BYTES];
    let frame = peer
        .send_declare_subscriber(id, key, &mut out)
        .expect("the peer declares")
        .batch(&out)
        .to_vec();
    let body = through_stream(&frame);
    let outcome = core
        .on_batch(service, &body, now, &mut out)
        .expect("the service takes it");
    assert_eq!(
        outcome
            .events
            .iter()
            .filter(|event| matches!(event, Event::SubscriberDeclared { .. }))
            .count(),
        1
    );
}

fn delivered(phase: &str, core: &mut Exam<'_>, holder: usize) {
    while let Some(item) = core.take(holder).expect("a declared holder") {
        match item {
            Item::Sample(sample) => {
                let first = sample.payload().first().copied().unwrap_or(0);
                assert_eq!(
                    sample.sequence,
                    i64::from(first),
                    "the payload carries its sequence"
                );
                line(&format!(
                    "delivered {phase} {} {} {}",
                    HOLDERS[holder],
                    sample.sequence,
                    hex(sample.payload())
                ));
            }
            Item::Lost { count } => panic!("an unexpected loss of {count} in {phase}"),
        }
    }
}

fn class(error: ServiceError) -> &'static str {
    match error {
        ServiceError::NotGranted => "not-granted",
        ServiceError::UnknownHolder => "unknown-holder",
        ServiceError::TooLarge => "too-large",
        ServiceError::InvalidGrants => "invalid-grants",
        ServiceError::Session(SessionError::UndeclaredKey) => "undeclared-key",
        other => panic!("a refusal the exam does not name: {other:?}"),
    }
}

fn refused<T: core::fmt::Debug>(name: &str, outcome: Result<T, ServiceError>) {
    let error = outcome.expect_err(name);
    line(&format!("refused {name} {}", class(error)));
}

fn size_lines() {
    let sizes = Exam::sizes();
    line(&format!("size slot {}", sizes.slot));
    line(&format!("size row {}", sizes.row));
    line(&format!("size holder {}", sizes.holder));
    line(&format!("size fixed {}", sizes.fixed));
}

fn constant_lines() {
    for (name, value) in [
        ("FORMAT_VERSION", grants::FORMAT_VERSION),
        ("MAX_KEY_BYTES", grants::MAX_KEY_BYTES),
        ("MAX_COUNT", grants::MAX_COUNT),
        ("MAX_SAMPLE_BYTES", grants::MAX_SAMPLE_BYTES),
        ("MAX_HOLDER_NAME_BYTES", grants::MAX_HOLDER_NAME_BYTES),
        ("SLOT_OVERHEAD_BYTES", grants::SLOT_OVERHEAD_BYTES),
        ("ROW_OVERHEAD_BYTES", grants::ROW_OVERHEAD_BYTES),
        ("HOLDER_OVERHEAD_BYTES", grants::HOLDER_OVERHEAD_BYTES),
        ("SERVICE_FIXED_BYTES", grants::SERVICE_FIXED_BYTES),
    ] {
        line(&format!("constant {name} {value}"));
    }
}

/// Every section, in the order the exam lists them.
pub fn run() {
    let (mut service, mut peer) = open();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    line("limits 3 4 12 3");
    for (holder, key, publish, subscribe) in ROWS {
        line(&format!(
            "grant {} k{} {}{}",
            HOLDERS[holder],
            key + 1,
            if publish { 'p' } else { '-' },
            if subscribe { 's' } else { '-' }
        ));
    }
    size_lines();
    constant_lines();

    // The service declares one subscription for each distinct subscribe key, and the peer takes both.
    let mut out = [0u8; MAX_BATCH_BYTES];
    for (id, name) in [(1u32, "declare-1"), (2, "declare-2")] {
        let sent = core
            .declare_next(&mut service, &mut out)
            .expect("declare")
            .expect("one is left");
        let frame = out[..sent].to_vec();
        let body = through_stream(&frame);
        let mut reply = [0u8; MAX_BATCH_BYTES];
        let outcome = peer
            .on_batch(&body, 10 + u64::from(id), &mut reply)
            .expect("the peer takes it");
        let declared = outcome
            .events
            .iter()
            .filter(|event| matches!(event, Event::SubscriberDeclared { id: held } if *held == id))
            .count();
        assert_eq!(declared, 1);
        line(&format!("frame {name} {}", hex(&frame)));
    }
    assert!(
        core.declare_next(&mut service, &mut out)
            .expect("nothing left")
            .is_none()
    );
    line("ok static-declaration-at-open");

    // A publication before the peer declares the key reaches the local holders and is not sent.
    let published = core
        .publish(&mut service, 0, KEYS[0], 99, 1099, &payload(99), &mut out)
        .expect("a granted publication");
    assert_eq!(published.peer, Peer::NotSubscribed);
    line(&format!(
        "published a k1 {} not-subscribed",
        published.reached
    ));
    for holder in [0, 1] {
        delivered("before-the-peer-declares", &mut core, holder);
    }

    // The peer declares k1 and k3, and the same publication is sent.
    peer_declares(&mut service, &mut core, &mut peer, 1, KEYS[0], 20);
    peer_declares(&mut service, &mut core, &mut peer, 2, KEYS[2], 21);
    let published = core
        .publish(&mut service, 0, KEYS[0], 100, 1100, &payload(100), &mut out)
        .expect("a granted publication");
    let Peer::Sent { sent } = published.peer else {
        panic!("the peer declared k1")
    };
    line(&format!("published a k1 {} sent", published.reached));
    line(&format!("frame publish-a {}", hex(&out[..sent])));
    let published = core
        .publish(&mut service, 1, KEYS[2], 101, 1101, &payload(101), &mut out)
        .expect("a granted publication");
    let Peer::Sent { sent } = published.peer else {
        panic!("the peer declared k3")
    };
    line(&format!("published b k3 {} sent", published.reached));
    line(&format!("frame publish-b {}", hex(&out[..sent])));
    for holder in [0, 1] {
        delivered("after-the-peer-declares", &mut core, holder);
    }
    line("ok publication-goes-local-and-to-a-subscribed-peer");

    // A refused publication reaches no ring and writes nothing to the peer.
    let before = (
        core.held(0).unwrap(),
        core.held(1).unwrap(),
        core.held(2).unwrap(),
        service.next_tx_sn(),
    );
    refused(
        "publish-without-a-publish-right",
        core.publish(&mut service, 1, KEYS[0], 7, 1007, &payload(7), &mut out),
    );
    refused(
        "publish-to-a-key-with-no-row",
        core.publish(&mut service, 2, KEYS[2], 7, 1007, &payload(7), &mut out),
    );
    refused(
        "publish-by-an-unknown-holder",
        core.publish(&mut service, 3, KEYS[0], 7, 1007, &payload(7), &mut out),
    );
    refused(
        "publish-payload-over-the-sample-size",
        core.publish(&mut service, 0, KEYS[0], 7, 1007, &[0u8; 13], &mut out),
    );
    let after = (
        core.held(0).unwrap(),
        core.held(1).unwrap(),
        core.held(2).unwrap(),
        service.next_tx_sn(),
    );
    assert_eq!(
        before, after,
        "a refused publication changes no ring and spends no sequence number"
    );
    line("ok a-refused-publication-reaches-no-one");

    // A sample from the peer reaches every holder with a subscribe row on its key.
    for (sequence, key) in [(10, 0), (11, 1)] {
        let frame = put_from_peer(&service, KEYS[key], sequence);
        let body = through_stream(&frame);
        core.on_batch(&mut service, &body, 30 + sequence as u64, &mut out)
            .expect("a Put to a key a holder subscribed to");
    }
    for holder in [0, 1, 2] {
        delivered("from-the-peer", &mut core, holder);
    }
    let stray = put_from_peer(&service, KEYS[2], 12);
    let body = through_stream(&stray);
    refused(
        "peer-put-to-a-key-nobody-subscribed",
        core.on_batch(&mut service, &body, 50, &mut out).map(|_| ()),
    );
    line("ok delivery-reaches-every-subscribed-holder");

    // A holder that never takes loses its oldest samples, and is told once.
    for sequence in 20..26 {
        let frame = put_from_peer(&service, KEYS[0], sequence);
        let body = through_stream(&frame);
        core.on_batch(&mut service, &body, 60 + sequence as u64, &mut out)
            .expect("a Put to k1");
        delivered("while-b-is-full", &mut core, 0);
    }
    let (held, lost) = core.held(1).unwrap();
    line(&format!("held b-full {held} {lost}"));
    let Some(Item::Lost { count }) = core.take(1).unwrap() else {
        panic!("the first take reports the loss")
    };
    line(&format!("lost b-catches-up b {count}"));
    delivered("b-catches-up", &mut core, 1);
    let (held, lost) = core.held(1).unwrap();
    line(&format!("held b-drained {held} {lost}"));
    line("ok a-slow-holder-loses-its-oldest-and-is-told-once");
    refused("take-by-an-unknown-holder", core.take(3));

    // Construction refuses grants the core cannot serve.
    let honest = rows();
    let mut bad_holder = honest;
    bad_holder[0].holder = 3;
    refused(
        "grants-name-an-unknown-holder",
        Exam::new(&bad_holder, GID, &params()),
    );
    let mut repeated = honest;
    repeated[1] = honest[0];
    refused(
        "grants-repeat-a-holder-and-key",
        Exam::new(&repeated, GID, &params()),
    );
    let mut no_right = honest;
    no_right[1].subscribe = false;
    refused(
        "grants-give-a-row-no-right",
        Exam::new(&no_right, GID, &params()),
    );
    let mut too_many = honest;
    too_many[3] = Row {
        holder: 1,
        key: KEYS[3],
        publish: false,
        subscribe: true,
    };
    refused(
        "grants-subscribe-more-keys-than-the-session-holds",
        Exam::new(
            &too_many,
            GID,
            &Params {
                local: 2,
                per_side: 3,
                ..params()
            },
        ),
    );
    refused(
        "grants-with-no-rows",
        Core::<3, 4, 12, 0>::new(&[], GID, &params()),
    );
    refused(
        "grants-slot-size-differs-from-the-session",
        Core::<3, 4, 11, 5>::new(&honest, GID, &params()),
    );
    line("ok the-core-refuses-grants-it-cannot-serve");
}
