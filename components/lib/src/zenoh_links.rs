//! Host tests for Zenoh Profile 0 links declared by a named configuration. Each `[zenoh-exam] links`
//! line is printed only after the assertions of its scenario passed; `scripts/lib/zenoh_links_exam.py`
//! judges them.

use crate::zenoh_profile0::fragment::tests::{self as codec, CAPTURES};
use crate::zenoh_profile0::limits::{DEMO, HEAP, LARGE, Limits};
use std::string::String;
use std::vec::Vec;

fn line(text: &str) {
    println!("[zenoh-exam] links {text}");
}

fn ok(name: &str) {
    line(&format!("ok {name}"));
}

fn config_line(limits: &Limits) -> String {
    std::format!(
        "config {} {} {} {} {} {} {} {} {} {} {} {}",
        limits.name,
        limits.mode.name(),
        limits.batch_bytes,
        limits.payload_bytes,
        limits.key_bytes,
        limits.attachment_bytes,
        limits.max_keys,
        limits.max_subscribers,
        limits.outstanding_samples,
        limits.stack_bytes,
        limits.reassembly_timeout_ms,
        limits.heap_bytes
    )
}

#[test]
fn the_pinned_configurations_are_the_ones_the_contract_judges() {
    for limits in [DEMO, LARGE, HEAP] {
        limits.check().expect("the contract admits it");
        line(&config_line(&limits));
    }
}

#[test]
fn our_sender_splits_a_put_as_the_stock_peer_did() {
    // The stock peer addressed the alias it had declared, in the receiver mapping (header 0x1d).
    for (size, batch, sn, lengths, headers, digests) in CAPTURES {
        let message = codec::put(0x1d, 1, None, size);
        let bodies = codec::split(&message, batch, sn);
        assert_eq!(bodies.len(), lengths.len());
        let capture = std::format!("{size}-{batch}");
        for (index, body) in bodies.iter().enumerate() {
            assert_eq!(body.len(), lengths[index]);
            assert_eq!(body[0], headers[index]);
            assert_eq!(codec::sha(body), digests[index]);
            line(&std::format!(
                "fragment {capture} {index} {} {:02x} {}",
                body.len(),
                body[0],
                codec::sha(body)
            ));
        }
        ok(&std::format!("encode-stock-{capture}"));
    }
}

use crate::zenoh_profile0::fragment::{decode_fragment, decode_put, fits_one_frame};
use crate::zenoh_profile0::reassembly::Reassembly;

const FULL_KEY_SN: u32 = 1000;
const BATCH: usize = 2048;

fn digest_of(digests: &[String]) -> String {
    codec::sha(digests.join("\n").as_bytes())
}

/// Joins a run of bodies the way a heap session does and reads the Put they make.
fn reassemble(bodies: &[Vec<u8>], batch: usize, first_sn: u32, any_key_len: usize) -> Vec<u8> {
    let total: usize = bodies.iter().map(Vec::len).sum();
    let mut storage = std::vec![0u8; total];
    let mut reassembly = Reassembly::new(5000, total);
    for body in bodies {
        let fragment = decode_fragment(body, Some(batch)).expect("fragment");
        reassembly
            .accept(&fragment, &mut storage, first_sn, 0)
            .expect("accepted");
    }
    let (message, _) = reassembly.finish(&storage).expect("finished");
    let mut rules = codec::rules();
    rules.max_key = any_key_len;
    rules.max_payload = u64::from(u32::MAX);
    let view = decode_put(message, &rules).expect("a Put");
    view.payload.to_vec()
}

#[test]
fn the_stock_fragments_reassemble_to_the_payload_they_carried() {
    for (size, batch, sn, ..) in CAPTURES {
        let message = codec::put(0x1d, 1, None, size);
        let bodies = codec::split(&message, batch, sn);
        let payload = reassemble(&bodies, batch, sn, 129);
        assert_eq!(payload, codec::payload(size));
        let capture = std::format!("{size}-{batch}");
        line(&std::format!("reassembled {capture} {size} {}", codec::sha(&payload)));
        ok(&std::format!("reassemble-stock-{capture}"));
    }
}

/// The largest payload whose full-key Put travels whole in one FRAME numbered `sn` at `batch`.
fn whole_edge(batch: usize, sn: u32) -> usize {
    let mut size = 0;
    while fits_one_frame(codec::full_key_put(codec::KEY, size + 1).len(), batch, sn) {
        size += 1;
    }
    size
}

#[test]
fn a_put_travels_whole_until_its_frame_overruns_the_batch() {
    let edge = whole_edge(BATCH, FULL_KEY_SN);
    assert_eq!(codec::split(&codec::full_key_put(codec::KEY, edge), BATCH, FULL_KEY_SN).len(), 1);
    assert_eq!(codec::split(&codec::full_key_put(codec::KEY, edge + 1), BATCH, FULL_KEY_SN).len(), 2);
    line(&std::format!("boundary {edge} whole 1"));
    line(&std::format!("boundary {} fragmented 2", edge + 1));
    ok("boundary-whole-then-fragmented");
}

#[test]
fn the_edge_between_whole_and_fragmented_follows_the_width_of_the_sequence_number() {
    for sn in [127u32, 128, 16_383, 16_384, 2_097_151, 2_097_152, 268_435_455, 268_435_456] {
        let edge = whole_edge(BATCH, sn);
        let whole = codec::split(&codec::full_key_put(codec::KEY, edge), BATCH, sn);
        let over = codec::split(&codec::full_key_put(codec::KEY, edge + 1), BATCH, sn);
        assert_eq!((whole.len(), over.len()), (1, 2), "{sn}");
        line(&std::format!("boundarysn {sn} {edge} whole 1"));
        line(&std::format!("boundarysn {sn} {} fragmented 2", edge + 1));
    }
    ok("boundary-at-sequence-width-changes");
}

#[test]
fn our_put_is_cut_into_the_bodies_the_reference_cuts() {
    let message = codec::full_key_put(codec::KEY, 5000);
    for (index, body) in codec::split(&message, BATCH, FULL_KEY_SN).iter().enumerate() {
        line(&std::format!("fullkey {index} {} {:02x} {}", body.len(), body[0], codec::sha(body)));
    }
    ok("put-roundtrip-5000-2048");
    for start in [127u32, 16_383, 2_097_151, 268_435_455, 0xFFFF_FFFE] {
        let bodies = codec::split(&message, BATCH, start);
        assert_eq!(reassemble(&bodies, BATCH, start, 129), codec::payload(5000));
        for (index, body) in bodies.iter().enumerate() {
            assert!(body.len() <= BATCH - 2);
            line(&std::format!("widthsplit {start} {index} {} {:02x} {}", body.len(), body[0], codec::sha(body)));
        }
    }
    ok("put-roundtrip-across-sequence-width");
}

#[test]
fn a_put_of_the_declared_payload_is_cut_into_nine_fragments() {
    let message = codec::full_key_put(codec::KEY, 16_384);
    let bodies = codec::split(&message, BATCH, FULL_KEY_SN);
    assert_eq!(bodies.len(), 9);
    assert_eq!(reassemble(&bodies, BATCH, FULL_KEY_SN, 129), codec::payload(16_384));
    for (index, body) in bodies.iter().enumerate() {
        line(&std::format!("heapfrag {index} {} {:02x} {}", body.len(), body[0], codec::sha(body)));
    }
    ok("put-roundtrip-at-the-declared-payload");
}

#[test]
fn a_complete_put_may_be_one_final_fragment() {
    let message = codec::full_key_put(codec::KEY, 100);
    for sn in [1000u32, 0xFFFF_FFFF] {
        let bodies = codec::split(&message, BATCH, sn);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0][0] & 0x40, 0, "the more flag is clear");
        assert_eq!(reassemble(&bodies, BATCH, sn, 129), codec::payload(100));
        let mut storage = std::vec![0u8; message.len()];
        let mut reassembly = Reassembly::new(5000, message.len());
        let fragment = decode_fragment(&bodies[0], Some(BATCH)).expect("fragment");
        reassembly.accept(&fragment, &mut storage, sn, 0).expect("accepted");
        let (_, after) = reassembly.finish(&storage).expect("finished");
        line(&std::format!(
            "single {sn} {} {} {after} {}",
            bodies[0].len(),
            codec::sha(&bodies[0]),
            codec::sha(&message)
        ));
    }
    ok("reassemble-single-final-fragment");
}

#[test]
fn a_run_whose_interior_fragments_are_shorter_than_the_maximum_is_taken() {
    let message = codec::full_key_put(codec::KEY, 5000);
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    let (mut at, mut sn) = (0usize, FULL_KEY_SN);
    for lead in [Some(1000usize), Some(1), Some(1500), None, None] {
        let header = 1 + codec::leb(u64::from(sn)).len();
        let room = BATCH - 2 - header;
        let take = lead.unwrap_or(room).min(room).min(message.len() - at);
        let more = at + take < message.len();
        let mut body = std::vec![0x06 | 0x20 | if more { 0x40 } else { 0 }];
        body.extend(codec::leb(u64::from(sn)));
        body.extend(&message[at..at + take]);
        bodies.push(body);
        at += take;
        sn += 1;
        if !more {
            break;
        }
    }
    assert_eq!(reassemble(&bodies, BATCH, FULL_KEY_SN, 129), codec::payload(5000));
    let lengths: Vec<String> = bodies.iter().map(|b| b.len().to_string()).collect();
    let digests: Vec<String> = bodies.iter().map(|b| codec::sha(b)).collect();
    line(&std::format!(
        "underfilled {BATCH} {} {} {} {}",
        bodies.len(),
        lengths.join(","),
        digest_of(&digests),
        codec::sha(&message)
    ));
    ok("reassemble-underfilled-interior");
}

// ---------------------------------------------------------------------------------------------
// A connected pair of links over scripted streams, and the heap the receiving end lends.

use crate::zenoh_link::{ByteLink, Error, LinkError, LinkHandle, LinkOf, Notice};
use crate::zenoh_profile0::encode::{self, FrameEncoder};
use crate::zenoh_profile0::limits::Mode;
use crate::zenoh_profile0::session::{Config, Params, Role, SessionError};
use crate::zenoh_profile0::{Bounds, Mapping, Refusal};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Default)]
struct Pipe {
    bytes: VecDeque<u8>,
}

struct Scripted {
    rx: Rc<RefCell<Pipe>>,
    tx: Rc<RefCell<Pipe>>,
}

impl ByteLink for Scripted {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        self.tx.borrow_mut().bytes.extend(buf.iter());
        Ok(buf.len())
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

/// A lease no test clock reaches, so only the reassembly timeout and the lease the exam names expire.
const LEASE_MS: u64 = 4_000_000_000;

fn config(zid: &[u8], cookie: &[u8], initial_sn: u32) -> Config<'static> {
    Config {
        zid: Vec::leak(zid.to_vec()),
        lease_ms: LEASE_MS,
        initial_sn,
        cookie: Vec::leak(cookie.to_vec()),
    }
}

/// `F` framed batch, `K` aliases, `S` subscriber slots (the peer's and our own), `B` key bytes, `P` payload
/// bytes copied into a fixed notice and `Q` notices.
struct Pair<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
> {
    a: LinkOf<Scripted, F, K, S, B, P, Q>,
    b: LinkOf<Scripted, F, K, S, B, P, Q>,
    ha: LinkHandle,
    hb: LinkHandle,
    a_to_b: Rc<RefCell<Pipe>>,
    b_to_a: Rc<RefCell<Pipe>>,
    heap_a: Vec<u8>,
    heap_b: Vec<u8>,
    sequence: i64,
    bounds: Bounds,
    /// The sequence number of the next raw FRAME injected as the peer.
    peer_sn: u32,
}

type HeapPair = Pair<2048, 4, 5, 129, 0, 8>;

fn pair_of<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
>(
    limits: &Limits,
    heap: usize,
) -> Pair<F, K, S, B, P, Q> {
    pair_params(Params::from_limits(limits), heap)
}

fn pair_params<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
>(
    params: Params,
    heap: usize,
) -> Pair<F, K, S, B, P, Q> {
    pair_sn(params, heap, 1000)
}

/// A pair whose connector numbers its first FRAME `a_sn`.
fn pair_sn<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
>(
    params: Params,
    heap: usize,
    a_sn: u32,
) -> Pair<F, K, S, B, P, Q> {
    let a_to_b = Rc::new(RefCell::new(Pipe::default()));
    let b_to_a = Rc::new(RefCell::new(Pipe::default()));
    let a = LinkOf::with_params(
        Scripted { rx: b_to_a.clone(), tx: a_to_b.clone() },
        Role::Connector,
        &config(&[0xa1, 0xa2], b"", a_sn),
        params,
        1,
    )
    .expect("connector");
    let b = LinkOf::with_params(
        Scripted { rx: a_to_b.clone(), tx: b_to_a.clone() },
        Role::Listener,
        &config(&[0xb1, 0xb2], b"cookie-b", 2000),
        params,
        1,
    )
    .expect("listener");
    let (ha, hb) = (a.handle(), b.handle());
    Pair {
        a,
        b,
        ha,
        hb,
        a_to_b,
        b_to_a,
        heap_a: std::vec![0; heap],
        heap_b: std::vec![0; heap],
        sequence: 0,
        bounds: params.bounds,
        peer_sn: 0,
    }
}

impl<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
> Pair<F, K, S, B, P, Q>
{
    /// Pumps both ends until neither moves a byte or reads a batch.
    fn run(&mut self, now: u64) {
        for _ in 0..4000 {
            let a = self.a.pump_with(self.ha, now, &mut self.heap_a).expect("a pump");
            let b = self.b.pump_with(self.hb, now, &mut self.heap_b).expect("b pump");
            let quiet = a.batches + b.batches + a.sent_bytes + b.sent_bytes == 0
                && !self.a.send_pending()
                && !self.b.send_pending();
            if quiet {
                return;
            }
        }
        panic!("the pair did not settle");
    }

    fn open(&mut self) {
        self.a.open(self.ha, 0).expect("open");
        self.run(0);
        assert!(self.a.is_open() && self.b.is_open(), "handshake");
        while self.a.next_notice().is_some() {}
        while self.b.next_notice().is_some() {}
        self.peer_sn = self.b.session().expected_rx_sn();
    }

    fn declare(&mut self, id: u32, key: &str) {
        self.b.declare_subscriber(self.hb, id, key).expect("declare");
        self.run(1);
        while self.a.next_notice().is_some() {}
    }

    /// Sends one Put from `a` to `b`, whole or in fragments, settling the pair after each part.
    fn send(&mut self, key: &str, payload: &[u8], now: u64) {
        self.sequence += 1;
        let attachment = encode::attachment(self.sequence, 1000 + self.sequence, &codec::GID).expect("attachment");
        match self.a.publish(self.ha, key, &attachment, payload) {
            Ok(()) => self.run(now),
            Err(Error::NeedsFragments) => {
                let mut at = 0;
                loop {
                    match self.a.publish_part(self.ha, key, &attachment, payload, at).expect("fragment") {
                        Some(next) => {
                            at = next;
                            self.run(now);
                        }
                        None => {
                            self.run(now);
                            return;
                        }
                    }
                }
            }
            Err(error) => panic!("publish: {error:?}"),
        }
    }

    /// Takes the next notice of `b` and, when it is a stored sample, reads its payload from the heap.
    fn take_sample(&mut self) -> Option<Vec<u8>> {
        match self.b.next_notice()? {
            Notice::SampleStored(sample) => Some(sample.payload(&self.heap_b).to_vec()),
            Notice::SampleDelivered(sample) => Some(sample.payload().to_vec()),
            _ => Some(Vec::new()),
        }
    }
}

/// A payload of `size` bytes that differs from every other `index`, so an overwrite is noticed.
fn distinct_payload(size: usize, index: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 7 + 1 + index * 13) % 251) as u8).collect()
}

fn heap_pair() -> HeapPair {
    let mut pair = pair_of(&HEAP, HEAP.heap_bytes as usize);
    pair.open();
    pair.declare(1, codec::KEY);
    pair
}

#[test]
fn a_heap_link_holds_the_queued_samples_and_one_message_in_progress() {
    let mut pair = heap_pair();
    let held = 8;
    let mut sent = Vec::new();
    for index in 0..held {
        let payload = distinct_payload(16_384, index);
        sent.push(codec::sha(&payload));
        pair.send(codec::KEY, &payload, 10);
    }
    assert_eq!(pair.b.notices_queued(), held, "the owner has drained none");
    // A ninth message is joined into the ninth cell, and its last fragment waits for room in the queue.
    let payload = distinct_payload(16_384, held);
    sent.push(codec::sha(&payload));
    pair.send(codec::KEY, &payload, 10);
    assert_eq!(pair.b.notices_queued(), held);
    assert!(pair.b.session().reassembling(), "the ninth message is in progress");
    let mut back = Vec::new();
    for index in 0..held {
        let sample = pair.take_sample().expect("a notice");
        assert_eq!(sample, distinct_payload(16_384, index), "sample {index} is intact");
        back.push(codec::sha(&sample));
    }
    pair.run(11);
    let sample = pair.take_sample().expect("the ninth sample");
    assert_eq!(sample, payload);
    back.push(codec::sha(&sample));
    assert_eq!(sent, back);
    line(&std::format!("queue {held} 1 16384 {} {}", digest_of(&sent), digest_of(&back)));
    ok("heap-fills-notice-queue");
    // With the queue drained a further message is joined and delivered.
    let payload = distinct_payload(16_384, held + 1);
    pair.send(codec::KEY, &payload, 12);
    assert_eq!(pair.take_sample().expect("a notice"), payload);
    line(&std::format!("drained {held} 16384 {}", codec::sha(&payload)));
    ok("heap-reassembles-after-queue-drained");
}

#[test]
fn a_put_of_the_declared_payload_is_joined_into_the_declared_heap() {
    let mut pair = heap_pair();
    let payload = codec::payload(16_384);
    pair.send(codec::KEY, &payload, 10);
    assert_eq!(pair.take_sample().expect("a notice"), payload);
    assert_eq!(HEAP.heap_needed(), HEAP.heap_bytes);
    line(&std::format!("payload 16384 {} {}", HEAP.heap_bytes, codec::sha(&payload)));
    ok("heap-declared-payload-boundary");
    ok("heap-storage-from-component");
}

mod scenarios;

/// The refusals the exam pins, by name and class.
const REFUSALS: [(&str, &str); 94] = [
    ("fixed-refuses-fragment", "unsupported-message"),
    ("fixed-refuses-best-effort-fragment", "unsupported-message"),
    ("fixed-refuses-fragment-with-extension", "unsupported-message"),
    ("fixed-refuses-empty-fragment", "unsupported-message"),
    ("fixed-refuses-fragment-sequence-noncanonical", "unsupported-message"),
    ("fixed-refuses-fragment-sequence-over-32-bits", "unsupported-message"),
    ("fixed-refuses-fragment-sequence-truncated", "unsupported-message"),
    ("fragment-best-effort", "unsupported-message"),
    ("fragment-extension", "unsupported-extension"),
    ("empty-fragment", "malformed-message"),
    ("fragment-before-open", "unexpected-message"),
    ("fragment-sequence-gap", "fragment-sequence"),
    ("fragment-sequence-truncated", "truncated"),
    ("fragment-sequence-duplicate", "fragment-sequence"),
    ("fragment-sequence-backward", "fragment-sequence"),
    ("fragment-interior-marked-final", "truncated"),
    ("fragment-final-marked-more", "reassembly-timeout"),
    ("fragment-stale-first-sequence", "sequence-mismatch"),
    ("fragment-skipped-first-sequence", "sequence-mismatch"),
    ("frame-during-reassembly", "unexpected-message"),
    ("fragments-of-two-messages-in-one-run", "trailing-bytes"),
    ("best-effort-fragment-during-reassembly", "unsupported-message"),
    ("fragmented-put-without-attachment", "malformed-message"),
    ("fragmented-put-with-short-attachment", "malformed-message"),
    ("fragmented-put-gid-length-15", "malformed-message"),
    ("fragmented-put-gid-length-17", "malformed-message"),
    ("fragmented-put-attachment-34-bytes", "over-bound"),
    ("fragmented-put-alias-scope-zero", "invalid-keyexpr"),
    ("fragmented-put-alias-scope-65536", "leb128-overflow"),
    ("message-over-declared-heap", "over-bound"),
    ("payload-over-declared-with-enough-storage", "over-bound"),
    ("demo-refuses-payload-13-bytes", "over-bound"),
    ("large-refuses-payload-1025-bytes", "over-bound"),
    ("fragmented-put-key-130-bytes", "over-bound"),
    ("fragmented-put-key-wildcard", "wildcard-keyexpr"),
    ("fragmented-put-key-leading-slash", "invalid-keyexpr"),
    ("fragmented-put-key-double-slash", "invalid-keyexpr"),
    ("fragmented-put-key-invalid-utf8", "invalid-keyexpr"),
    ("fragmented-put-key-not-subscribed", "undeclared-key"),
    ("fragmented-put-full-key-nonzero-scope", "invalid-keyexpr"),
    ("fragmented-put-empty-key", "invalid-keyexpr"),
    ("fragmented-put-key-trailing-slash", "invalid-keyexpr"),
    ("fragmented-put-key-question-mark", "invalid-keyexpr"),
    ("fragmented-put-key-hash", "invalid-keyexpr"),
    ("fragmented-put-key-dollar", "wildcard-keyexpr"),
    ("fragmented-put-key-truncated", "truncated"),
    ("fragmented-push-delete-body", "unsupported-message"),
    ("fragmented-push-ends-before-body", "truncated"),
    ("fragmented-put-scope-unterminated", "truncated"),
    ("fragmented-put-key-length-unterminated", "truncated"),
    ("fragmented-put-attachment-length-unterminated", "truncated"),
    ("fragmented-put-payload-length-unterminated", "truncated"),
    ("fragmented-push-extension-flag", "unsupported-extension"),
    ("fragmented-put-timestamp-flag", "unsupported-message"),
    ("fragmented-put-encoding-flag", "unsupported-message"),
    ("fragmented-put-unknown-extension", "unsupported-extension"),
    ("fragmented-put-extension-chained", "unsupported-extension"),
    ("fragmented-put-alias-for-another-key", "undeclared-key"),
    ("fragmented-put-receiver-mapped-alias", "invalid-keyexpr"),
    ("fragmented-put-key-length-over-16-bits", "leb128-overflow"),
    ("fragmented-put-attachment-length-over-32-bits", "leb128-overflow"),
    ("fragmented-put-scope-256", "invalid-keyexpr"),
    ("fragmented-put-key-length-256", "over-bound"),
    ("fragmented-put-attachment-length-65536", "over-bound"),
    ("fragmented-put-payload-length-65536", "over-bound"),
    ("fragmented-put-payload-truncated", "truncated"),
    ("fragmented-put-scope-32768", "invalid-keyexpr"),
    ("fragmented-put-key-length-32768", "over-bound"),
    ("fragmented-put-attachment-length-2147483648", "over-bound"),
    ("fragmented-put-payload-length-2147483648", "over-bound"),
    ("fragmented-put-attachment-truncated", "truncated"),
    ("reassembly-one-ms-after-the-timeout", "reassembly-timeout"),
    ("fragment-sequence-noncanonical", "leb128-noncanonical"),
    ("fragment-sequence-over-32-bits", "leb128-overflow"),
    ("fragmented-put-scope-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-key-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-attachment-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-payload-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-payload-length-over-32-bits", "leb128-overflow"),
    ("fragmented-put-undeclared-alias", "invalid-keyexpr"),
    ("fragment-over-the-announced-batch", "length-exceeds-batch"),
    ("reassembly-timeout", "reassembly-timeout"),
    ("storage-refused", "out-of-memory"),
    ("fifth-subscriber", "subscriber-limit"),
    ("fifth-alias", "over-bound"),
    ("tenth-subscriber-in-nine", "subscriber-limit"),
    ("tenth-alias-in-nine", "over-bound"),
    ("tenth-subscriber-in-nine-fixed", "subscriber-limit"),
    ("tenth-alias-in-nine-fixed", "over-bound"),
    ("257th-subscriber-in-256-heap", "subscriber-limit"),
    ("257th-alias-in-256-heap", "over-bound"),
    ("257th-subscriber-in-256-fixed", "subscriber-limit"),
    ("257th-alias-in-256-fixed", "over-bound"),
    ("fragmented-put-multibyte-key-over-declared", "over-bound"),
];

#[test]
fn every_refusal_carries_the_class_the_exam_names() {
    // A row on its own thread: the largest configurations hold their tables on the stack. Every row
    // runs, and the ones that did not hold are named together.
    let mut failed = Vec::new();
    for (name, expected) in REFUSALS {
        let outcome = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(move || scenarios::run(name))
            .expect("thread")
            .join();
        match outcome {
            Ok(got) if got == expected => line(&std::format!("refused {name} {got}")),
            Ok(got) => failed.push(std::format!("{name}: refused as {got}, the exam names {expected}")),
            Err(_) => failed.push(std::format!("{name}: the scenario did not hold")),
        }
    }
    assert!(failed.is_empty(), "{} rows:\n{}", failed.len(), failed.join("\n"));
}
