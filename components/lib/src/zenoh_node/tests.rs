//! Host tests for the shared node pieces and the two drivers.
//!
//! The drivers run the real publisher and subscriber control flow against each other over a
//! scripted stream, so what the booted nodes print and the order they act in is judged here
//! without QEMU.

use super::*;
use crate::zenoh_link::{ByteLink, Link, LinkError};
use crate::zenoh_profile0::session::Role;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::string::{String, ToString};
use std::vec::Vec;

#[derive(Default)]
struct Pipe {
    bytes: VecDeque<u8>,
}

struct End {
    rx: Rc<RefCell<Pipe>>,
    tx: Rc<RefCell<Pipe>>,
    /// Most bytes one send accepts; `None` is unlimited.
    send_cap: Option<usize>,
}

impl ByteLink for End {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        let take = self.send_cap.map_or(buf.len(), |cap| cap.min(buf.len()));
        self.tx.borrow_mut().bytes.extend(buf.iter().take(take));
        Ok(take)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        let mut pipe = self.rx.borrow_mut();
        let mut count = 0;
        while count < buf.len() {
            match pipe.bytes.pop_front() {
                Some(byte) => {
                    buf[count] = byte;
                    count += 1;
                }
                None => break,
            }
        }
        Ok(count)
    }

    fn eof(&self) -> bool {
        false
    }
}

/// A console that records every message, with a clock the test advances.
struct Recorder {
    lines: Vec<String>,
    now: u64,
}

impl Recorder {
    fn new() -> Self {
        Self::at(0)
    }

    fn at(now: u64) -> Self {
        Self {
            lines: Vec::new(),
            now,
        }
    }
}

impl Host for Recorder {
    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn idle(&mut self) -> bool {
        self.now += 1;
        true
    }

    fn write(&mut self, line: &[u8]) {
        self.lines
            .push(String::from_utf8(line.to_vec()).expect("console lines are ASCII"));
    }
}

struct Outcome {
    publisher: Vec<String>,
    subscriber: Vec<String>,
    published: usize,
    received: usize,
}

fn connect(send_cap: Option<usize>) -> (Link<End>, Link<End>) {
    let a_to_b = Rc::new(RefCell::new(Pipe::default()));
    let b_to_a = Rc::new(RefCell::new(Pipe::default()));
    let publisher = Link::new(
        End {
            rx: b_to_a.clone(),
            tx: a_to_b.clone(),
            send_cap,
        },
        Role::Connector,
        &publisher_config(),
        1,
    )
    .expect("publisher link");
    let subscriber = Link::new(
        End {
            rx: a_to_b,
            tx: b_to_a,
            send_cap,
        },
        Role::Listener,
        &subscriber_config(),
        1,
    )
    .expect("subscriber link");
    (publisher, subscriber)
}

/// Alternates the two nodes' steps, as two components sharing a CPU do when each yields.
fn exchange(send_cap: Option<usize>) -> Outcome {
    exchange_from(send_cap, 0)
}

/// [`exchange`] with both clocks starting at `start_ms`, as when setup took that long.
fn exchange_from(send_cap: Option<usize>, start_ms: u64) -> Outcome {
    let (mut pub_link, mut sub_link) = connect(send_cap);
    let (mut pub_host, mut sub_host) = (Recorder::at(start_ms), Recorder::at(start_ms));
    let (mut publisher, mut subscriber) = (Publisher::new(), Subscriber::new());
    let (mut published, mut received) = (None, None);
    for _ in 0..4000 {
        if published.is_none() {
            match publisher
                .step(&mut pub_link, &mut pub_host)
                .expect("publisher")
            {
                Step::Done(count) => published = Some(count),
                Step::Continue => {}
            }
            pub_host.idle();
        }
        if received.is_none() {
            match subscriber
                .step(&mut sub_link, &mut sub_host)
                .expect("subscriber")
            {
                Step::Done(count) => received = Some(count),
                Step::Continue => {}
            }
            sub_host.idle();
        }
        if published.is_some() && received.is_some() {
            break;
        }
    }
    Outcome {
        publisher: pub_host.lines,
        subscriber: sub_host.lines,
        published: published.expect("the publisher finished"),
        received: received.expect("the subscriber finished"),
    }
}

fn hex_of(line: &str) -> &str {
    line.rsplit("hex=").next().expect("hex").trim_end()
}

#[test]
fn the_two_drivers_exchange_the_four_declared_samples() {
    let outcome = exchange(None);
    assert_eq!(outcome.published, 4);
    assert_eq!(outcome.received, 4);
    let validated: Vec<&String> = outcome
        .subscriber
        .iter()
        .filter(|line| line.contains("sample validated"))
        .collect();
    assert_eq!(validated.len(), 4);
    for (index, line) in validated.iter().enumerate() {
        let (sequence, value) = SAMPLES[index];
        assert_eq!(
            line.as_str(),
            std::format!(
                "[ros2-demo-subscriber] sample validated sequence={sequence} value={value}\n"
            )
        );
    }
}

#[test]
fn what_the_subscriber_received_is_what_the_publisher_sent_byte_for_byte() {
    for cap in [None, Some(7), Some(1)] {
        let outcome = exchange(cap);
        let sent: Vec<&str> = outcome
            .publisher
            .iter()
            .filter(|l| l.contains("wire sent"))
            .map(|l| hex_of(l))
            .collect();
        let received: Vec<&str> = outcome
            .subscriber
            .iter()
            .filter(|l| l.contains("wire received"))
            .map(|l| hex_of(l))
            .collect();
        assert_eq!(sent.len(), 4, "send cap {cap:?}");
        assert_eq!(sent, received, "send cap {cap:?}");
        for hexed in &sent {
            assert_eq!(hexed.len(), 184 * 2, "every frame is 184 bytes");
        }
    }
}

#[test]
fn teardown_undeclares_before_it_closes_and_the_summary_comes_first() {
    let outcome = exchange(None);
    let order: Vec<&str> = outcome
        .subscriber
        .iter()
        .filter(|l| {
            l.contains("received count=")
                || l.contains("undeclared subscriber")
                || l.contains("session closing")
        })
        .map(|l| l.as_str())
        .collect();
    assert_eq!(
        order,
        [
            "[rpi5-ros2-demo] received count=4 sequences=0,1,2,3 values=10,20,30,40\n",
            "[ros2-demo-subscriber] undeclared subscriber id=1\n",
            "[ros2-demo-subscriber] session closing samples=4\n",
        ]
    );
}

#[test]
fn the_publisher_publishes_only_after_the_subscriber_declared_the_key() {
    let outcome = exchange(None);
    let matched = outcome
        .publisher
        .iter()
        .position(|l| l.contains("declaration matched"))
        .expect("declaration matched");
    let first_sent = outcome
        .publisher
        .iter()
        .position(|l| l.contains("wire sent sample=0"))
        .expect("first sample");
    assert!(matched < first_sent);
    let open = outcome
        .publisher
        .iter()
        .position(|l| l.contains("session open"))
        .expect("session open");
    assert!(open < matched);
}

#[test]
fn every_console_line_is_one_complete_ascii_line() {
    let outcome = exchange(None);
    for line in outcome.publisher.iter().chain(&outcome.subscriber) {
        assert!(line.ends_with('\n'), "{line:?}");
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        assert!(line.is_ascii(), "{line:?}");
        assert!(line.starts_with('['), "{line:?}");
        assert!(
            line.len() <= 640,
            "{line:?} would not fit the wire line buffer"
        );
    }
}

#[test]
fn a_subscriber_refuses_a_sample_that_differs_from_the_declared_one() {
    // The publisher declares the demo key; hand the subscriber a wrong value through the checker.
    let good = crate::ros_cdr::Counter {
        sequence: 0,
        value: 10,
    };
    let mut bytes = [0u8; crate::ros_cdr::MAX_SERIALIZED_BYTES];
    good.encode(&mut bytes).expect("encode");
    assert_eq!(check_sample(0, &bytes), Ok((0, 10)));
    assert_eq!(check_sample(1, &bytes), Err(b"sample value".as_slice()));
    assert_eq!(
        check_sample(4, &bytes),
        Err(b"unexpected sample".as_slice())
    );
    assert_eq!(check_sample(0, &bytes[..11]), Err(b"cdr decode".as_slice()));
}

#[test]
fn a_peer_that_closes_early_fails_the_node_that_has_not_finished() {
    let (mut pub_link, mut sub_link) = connect(None);
    let (mut pub_host, mut sub_host) = (Recorder::new(), Recorder::new());
    let (mut publisher, mut subscriber) = (Publisher::new(), Subscriber::new());
    // Alternate until the publisher has sent its first sample, then close the subscriber's side
    // before the publisher has sent the rest: the exchange cannot complete.
    for _ in 0..400 {
        publisher
            .step(&mut pub_link, &mut pub_host)
            .expect("publisher");
        subscriber
            .step(&mut sub_link, &mut sub_host)
            .expect("subscriber");
        if pub_host
            .lines
            .iter()
            .any(|l| l.contains("wire sent sample=0"))
        {
            break;
        }
    }
    assert!(
        pub_host
            .lines
            .iter()
            .any(|l| l.contains("wire sent sample=0")),
        "the publisher reached its first sample"
    );
    let sent_before = pub_host
        .lines
        .iter()
        .filter(|l| l.contains("wire sent"))
        .count();
    assert!(
        sent_before < SAMPLES.len(),
        "the exchange is still in progress"
    );
    let handle = sub_link.handle();
    sub_link.close(handle, 0).expect("close");
    sub_link.pump(handle, 1).expect("flush the close");
    let mut failure = None;
    for _ in 0..40 {
        match publisher.step(&mut pub_link, &mut pub_host) {
            Err(error) => {
                failure = Some(error);
                break;
            }
            Ok(Step::Done(count)) => panic!("the publisher finished having sent {count}"),
            Ok(Step::Continue) => {}
        }
    }
    assert_eq!(failure, Some(NodeError::PeerClosedEarly));
}

#[test]
fn step_maps_every_service_reply_a_stream_can_see() {
    let reply = |success, status, transferred| Reply {
        success,
        status,
        transferred,
        end_of_stream: false,
    };
    assert_eq!(step(&reply(true, 0, 5), 8), Ok(5));
    assert_eq!(step(&reply(true, 0, 0), 8), Ok(0));
    assert_eq!(
        step(&reply(true, 0, 9), 8),
        Err(LinkError::Reset),
        "a reply that moved more than was offered is a protocol fault"
    );
    assert_eq!(step(&reply(false, net::STATUS_WOULD_BLOCK, 0), 8), Ok(0));
    assert_eq!(
        step(&reply(false, net::STATUS_RESET, 0), 8),
        Err(LinkError::Reset)
    );
    assert_eq!(
        step(&reply(false, net::STATUS_TIMEOUT, 0), 8),
        Err(LinkError::Closed)
    );
    assert_eq!(
        step(&reply(false, net::STATUS_DENIED, 0), 8),
        Err(LinkError::Closed)
    );
}

struct Scripted {
    sends: VecDeque<Reply>,
    recvs: VecDeque<Reply>,
}

impl Connection for Scripted {
    fn send(&mut self, _: &[u8]) -> Result<Reply, LinkError> {
        self.sends.pop_front().ok_or(LinkError::Closed)
    }

    fn recv(&mut self, _: &mut [u8]) -> Result<Reply, LinkError> {
        self.recvs.pop_front().ok_or(LinkError::Closed)
    }
}

#[test]
fn a_stream_reports_eof_only_for_a_successful_empty_receive_with_the_end_flag() {
    let eof = Reply {
        success: true,
        status: 0,
        transferred: 0,
        end_of_stream: true,
    };
    let would_block = Reply {
        success: false,
        status: net::STATUS_WOULD_BLOCK,
        transferred: 0,
        end_of_stream: false,
    };
    let mut stream = Stream::new(Scripted {
        sends: VecDeque::new(),
        recvs: VecDeque::from([would_block, eof]),
    });
    let mut buf = [0u8; 8];
    assert_eq!(stream.recv(&mut buf), Ok(0));
    assert!(!stream.eof(), "a would-block is not the end of the stream");
    assert_eq!(stream.recv(&mut buf), Ok(0));
    assert!(stream.eof());
}

#[test]
fn a_clipped_line_is_never_emitted() {
    let mut line = Line::<8>::new();
    line.put(b"12345").put(b"67890");
    assert!(line.clipped());
    assert_eq!(line.finish(), b"");
    let mut fits = Line::<8>::new();
    fits.put(b"1234").put_number(56);
    assert_eq!(fits.finish(), b"123456");
}

#[test]
fn the_longest_wire_line_fits_with_room_to_spare() {
    let frame = [0xabu8; 184];
    let line = wire_line(PUBLISHER, b"wire sent", 3, &frame);
    assert!(!line.clipped());
    assert!(line.finish().len() < 640);
    assert!(line.finish().ends_with(b"\n"));
    // A full 512-byte batch would not fit; the line is dropped, not truncated.
    let full = [0u8; 512];
    assert!(wire_line(PUBLISHER, b"wire sent", 0, &full).clipped());
}

#[test]
fn signed_values_print_with_their_sign() {
    let mut line = Line::<32>::new();
    line.put_signed(-40)
        .put(b" ")
        .put_signed(40)
        .put(b" ")
        .put_signed(0);
    assert_eq!(line.finish(), b"-40 40 0");
    let mut extreme = Line::<32>::new();
    extreme.put_signed(i64::MIN);
    assert_eq!(extreme.finish(), b"-9223372036854775808");
}

#[test]
fn a_denial_is_judged_only_by_the_denied_status_with_nothing_moved() {
    let denied = Reply {
        success: false,
        status: net::STATUS_DENIED,
        transferred: 0,
        end_of_stream: false,
    };
    assert!(is_denial(&denied));
    for other in [
        Reply {
            status: net::STATUS_UNSUPPORTED,
            ..denied
        },
        Reply {
            status: net::STATUS_MALFORMED,
            ..denied
        },
        Reply {
            status: net::STATUS_REFUSED,
            ..denied
        },
        Reply {
            success: true,
            status: 0,
            ..denied
        },
        Reply {
            transferred: 1,
            ..denied
        },
    ] {
        assert!(!is_denial(&other), "{other:?} must not count as a denial");
    }
}

#[test]
fn the_scouting_request_is_well_formed_and_names_the_multicast_group_over_udp() {
    let request = scouting_request();
    assert!(slime_proto::valid_network_request(&request));
    assert_eq!(request.transport, net::TRANSPORT_UDP);
    assert_eq!(request.op, net::OP_CONNECT);
    assert_eq!(&request.endpoint[..4], &SCOUT_GROUP);
    assert_eq!(request.port, SCOUT_PORT);
    // It must not be the declared endpoint under another transport: that would be a different test.
    assert_ne!(
        (&request.endpoint[..4], request.port),
        (&ENDPOINT[..], PORT)
    );
}

#[test]
fn denial_and_open_lines_match_the_markers_the_plane_arm_reads() {
    assert_eq!(
        denial_line(PUBLISHER, b"scouting").finish(),
        b"[ros2-demo-publisher] denial class=scouting refused=1\n"
    );
    assert_eq!(
        open_line(SUBSCRIBER, b"listener").finish(),
        b"[ros2-demo-subscriber] session open role=listener initial_sn=0 lease_ms=2000\n"
    );
    let key = key_line(PUBLISHER, b"declaration matched key=");
    assert!(
        key.finish()
            .starts_with(b"[ros2-demo-publisher] declaration matched key=0/slime_demo/counter/")
    );
    assert_eq!(KEY.len(), 129);
    let _ = String::new().to_string();
}

#[test]
fn the_handshake_deadline_starts_at_the_clock_not_at_zero() {
    let outcome = exchange_from(None, 10_000);
    assert_eq!((outcome.published, outcome.received), (4, 4));
}

#[test]
fn the_subscriber_is_not_done_until_its_close_has_been_sent() {
    let (mut pub_link, mut sub_link) = connect(None);
    let (mut pub_host, mut sub_host) = (Recorder::new(), Recorder::new());
    let (mut publisher, mut subscriber) = (Publisher::new(), Subscriber::new());
    for _ in 0..4000 {
        publisher
            .step(&mut pub_link, &mut pub_host)
            .expect("publisher");
        pub_host.idle();
        let step = subscriber
            .step(&mut sub_link, &mut sub_host)
            .expect("subscriber");
        sub_host.idle();
        assert_eq!(step, Step::Continue, "closing is not complete yet");
        if sub_host
            .lines
            .iter()
            .any(|l| l.contains("session closing samples=4"))
        {
            break;
        }
    }
    assert!(sub_link.send_pending(), "the CLOSE is queued, not yet sent");
    sub_link.io_mut().send_cap = Some(0);
    assert_eq!(
        subscriber.step(&mut sub_link, &mut sub_host),
        Ok(Step::Continue),
        "a CLOSE that has not left is not a finished subscriber"
    );
    assert!(sub_link.send_pending());
    sub_link.io_mut().send_cap = None;
    assert_eq!(
        subscriber.step(&mut sub_link, &mut sub_host),
        Ok(Step::Done(4))
    );
    assert!(!sub_link.send_pending());
    let mut finished = None;
    for _ in 0..40 {
        match publisher.step(&mut pub_link, &mut pub_host) {
            Ok(Step::Done(count)) => {
                finished = Some(count);
                break;
            }
            other => assert_eq!(other, Ok(Step::Continue)),
        }
    }
    assert_eq!(finished, Some(4), "the publisher received the CLOSE");
}
