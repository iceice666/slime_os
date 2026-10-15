//! Host tests for Zenoh Profile 0 links declared by a named configuration. Each `[zenoh-exam] links`
//! line is printed only after the assertions of its scenario passed; `scripts/lib/zenoh_links_exam.py`
//! judges them.

use crate::zenoh_profile0::fragment::tests::{self as codec, CAPTURES};
use crate::zenoh_profile0::limits::{DEMO, HEAP, LARGE, Limits};
use std::string::String;

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
