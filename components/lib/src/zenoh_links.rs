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
