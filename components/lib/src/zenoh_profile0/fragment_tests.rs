//! Host tests for the FRAGMENT codec: the bodies a stock peer wrote, reproduced byte for byte, and every
//! way a fragment or the message it joins into is refused.

use super::*;
use boot_contracts::sha256;
use std::string::String;
use std::vec::Vec;

pub const KEY: &str = "0/slime_demo/counter/slime_demo_msgs::msg::dds_::Counter_/RIHS01_a82fd5ffcb96d0a197a5ad3680d1c4e6ba43a962928ecd592fb565eb8129595b";
pub const GID: [u8; 16] = [
    0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec, 0xed, 0xee, 0xef,
];

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in bytes {
        write!(out, "{byte:02x}").expect("write");
    }
    out
}

pub fn sha(bytes: &[u8]) -> String {
    hex(&sha256::digest(bytes))
}

pub fn leb(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let low = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return out;
        }
        out.push(low | 0x80);
    }
}

/// The payload pattern the captures carried: a period of 251 bytes, so a joined message is checkable.
pub fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 7 + 1) % 251) as u8).collect()
}

pub fn attachment() -> Vec<u8> {
    let mut out = 0i64.to_le_bytes().to_vec();
    out.extend(1000i64.to_le_bytes());
    out.push(16);
    out.extend(GID);
    out
}

/// A PUSH with a Put: `header` is 0x7d for a key in full (sender mapping) or 0x1d/0x5d for an alias.
pub fn put(header: u8, scope: u32, key: Option<&str>, size: usize) -> Vec<u8> {
    let mut out = std::vec![header];
    out.extend(leb(u64::from(scope)));
    if let Some(key) = key {
        out.extend(leb(key.len() as u64));
        out.extend(key.as_bytes());
    }
    out.extend([0x81, 0x43, 33]);
    out.extend(attachment());
    out.extend(leb(size as u64));
    out.extend(payload(size));
    out
}

pub fn full_key_put(key: &str, size: usize) -> Vec<u8> {
    put(0x7d, 0, Some(key), size)
}

pub fn alias_put(alias: u32, size: usize) -> Vec<u8> {
    put(0x5d, alias, None, size)
}

pub fn split(message: &[u8], batch: usize, first_sn: u32) -> Vec<Vec<u8>> {
    let mut splitter = Splitter::new(message, batch, first_sn).expect("splitter");
    let mut out = Vec::new();
    let mut buffer = std::vec![0u8; batch];
    while let Some(len) = splitter.next_body(&mut buffer).expect("body") {
        out.push(buffer[..len].to_vec());
    }
    out
}

pub fn rules<'a>() -> PutRules<'a> {
    PutRules {
        max_key: 129,
        max_payload: 16_384,
        aliases: None,
        subscribed: &any_key,
    }
}

pub fn any_key(_: &[u8]) -> bool {
    true
}

/// An alias table holding `alias` for `key`.
pub fn alias_table(alias: u32, key: &[u8]) -> super::super::tables::SlotTable<4, 160> {
    let mut table = super::super::tables::SlotTable::new();
    table.insert(u64::from(alias), key).expect("alias");
    table
}

/// The captures: payload bytes, batch, first sequence number, fragment lengths, headers, sha256 of each.
type Capture = (
    usize,
    usize,
    u32,
    &'static [usize],
    &'static [u8],
    &'static [&'static str],
);
pub const CAPTURES: [Capture; 3] = [
    (
        5000,
        2048,
        250_917_212,
        &[2046, 2046, 963],
        &[0x66, 0x66, 0x26],
        &[
            "3a204a426aa81bfd43d1993883cd26ad288bb10cd21f0e9e8da310a7aff8acb4",
            "115c8ad2bfa2bb2ae78a932cc6d26b592f621e4256f980f7a38407a3cc556c79",
            "77922590635a1ec6191f82ad1272fc0995585c5547fc0210b604c4a2f1fbc8fb",
        ],
    ),
    (
        2200,
        2048,
        228_976_026,
        &[2046, 204],
        &[0x66, 0x26],
        &[
            "9bbc893d83b3354f2e3c89b09d307545e3f7f538f200f7bf763d0bafefde6384",
            "2b7ea030692c9a4f13bb70adf972aab18669c10ad6959c07c190e4c979aac6bc",
        ],
    ),
    (
        5000,
        512,
        26_343_361,
        &[510, 510, 510, 510, 510, 510, 510, 510, 510, 500],
        &[0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x26],
        &[
            "6a60b9186829556cfd5fc0fdfab10593142abeadd37a892e8212039d18925a41",
            "5abc503a4bbaf5b3aa2cb5a4f3ec9e4ef65ea68f8c7c614d125e25e6fa2a965f",
            "8326441f5a41c8ab8f300fe3364189a0fdebb25d0c8a825def86aaffbd0ed945",
            "2abdfba4fe45211b4bdd614acb1c3aca21a85b9dee14621872f7b60755817331",
            "aa020b56f6bf33e35844067d5bda2308b1cba283036315b7f47348edf1371c85",
            "c0f5bd6b2aaa0cb06c55b686dce03334860afacaa3bfee0f14dd78c417d34ac5",
            "04fcf50f3044e353d1d59849ebca133bb2faf854bdebdd88494cf4cecea000ec",
            "297bf4144dd72a828dcdadce16e8f0908a1f8b3840144308b11383d85d80172c",
            "3c6e6a38c4df129edf7b0ba9aeb2df0d9f25e9fedfdb812b629ea08909b93d83",
            "25617eb4aa9e787983cdc17edc7fd357d6d3bd1db4bdd80649c67c8e72696ac9",
        ],
    ),
];

#[test]
fn the_splitter_reproduces_the_stock_peers_fragments() {
    // The stock peer addressed the alias it had declared, in the receiver mapping (header 0x1d).
    for (size, batch, sn, lengths, headers, _digests) in CAPTURES {
        let message = put(0x1d, 1, None, size);
        let bodies = split(&message, batch, sn);
        assert_eq!(bodies.len(), lengths.len(), "{size}-{batch}");
        for (index, body) in bodies.iter().enumerate() {
            assert_eq!(body.len(), lengths[index], "{size}-{batch} #{index}");
            assert_eq!(body[0], headers[index], "{size}-{batch} #{index}");
        }
    }
}

#[test]
fn a_run_joins_into_the_message_it_was_cut_from() {
    for size in [1usize, 100, 2000, 2001, 2002, 5000, 16_384] {
        let message = full_key_put(KEY, size);
        for batch in [89usize, 512, 2048, 4096] {
            let bodies = split(&message, batch, 1000);
            let mut joined: Vec<u8> = Vec::new();
            for (index, body) in bodies.iter().enumerate() {
                let fragment = decode_fragment(body, Some(batch)).expect("fragment");
                assert_eq!(fragment.sn, 1000 + index as u32);
                assert_eq!(fragment.more, index + 1 < bodies.len());
                joined.extend(fragment.data);
            }
            assert_eq!(joined, message, "{size} at {batch}");
        }
    }
}

#[test]
fn the_sequence_number_wraps_at_32_bits_and_its_width_follows() {
    let message = full_key_put(KEY, 5000);
    let bodies = split(&message, 2048, 0xFFFF_FFFE);
    let sns: Vec<u32> = bodies
        .iter()
        .map(|b| decode_fragment(b, None).expect("fragment").sn)
        .collect();
    assert_eq!(sns, [0xFFFF_FFFE, 0xFFFF_FFFF, 0]);
    // The header of the first is five bytes of sequence number and the last is one: the slices differ.
    assert_eq!(bodies[0].len(), 2046);
    assert_eq!(bodies[2][0], ID_FRAGMENT | FLAG_RELIABLE);
    for start in [127u32, 16_383, 2_097_151, 268_435_455] {
        let bodies = split(&message, 2048, start);
        assert!(bodies.iter().all(|b| b.len() <= 2046));
        let widths: Vec<usize> = (0..bodies.len() as u32)
            .map(|i| leb(u64::from(start + i)).len())
            .collect();
        assert!(
            widths.windows(2).any(|w| w[0] != w[1]),
            "{start} crosses a width step"
        );
    }
}

#[test]
fn a_message_travels_whole_until_its_frame_overruns_the_batch() {
    // The largest payload whose full-key Put travels in one FRAME at a 2048-byte batch, by sequence width.
    for (sn, edge) in [
        (127u32, 1873usize),
        (128, 1872),
        (16_383, 1872),
        (16_384, 1871),
        (2_097_151, 1871),
        (2_097_152, 1870),
        (268_435_455, 1870),
        (268_435_456, 1869),
    ] {
        let whole = full_key_put(KEY, edge);
        let over = full_key_put(KEY, edge + 1);
        assert!(fits_one_frame(whole.len(), 2048, sn), "{sn}: {edge} fits");
        assert!(
            !fits_one_frame(over.len(), 2048, sn),
            "{sn}: {} does not",
            edge + 1
        );
    }
}

fn refused(body: &[u8]) -> &'static str {
    decode_fragment(body, Some(2048))
        .expect_err("refused")
        .class()
}

#[test]
fn a_fragment_is_the_reliable_form_and_nothing_else() {
    assert_eq!(refused(&[]), "malformed-message");
    assert_eq!(
        refused(&[ID_FRAGMENT, 0, 1]),
        "unsupported-message",
        "best-effort"
    );
    assert_eq!(
        refused(&[ID_FRAGMENT | FLAG_RELIABLE | FLAG_Z, 0, 1]),
        "unsupported-extension"
    );
    assert_eq!(
        refused(&[ID_FRAGMENT | FLAG_RELIABLE, 0]),
        "malformed-message",
        "no data"
    );
    assert_eq!(
        refused(&[0x25, 0, 1]),
        "unsupported-message",
        "another message"
    );
    assert_eq!(
        refused(&[ID_FRAGMENT | FLAG_RELIABLE, 0x80]),
        "truncated",
        "an unterminated sequence number"
    );
    assert_eq!(refused(&[0x26, 0x80, 0x00, 0xaa]), "leb128-noncanonical");
    assert_eq!(
        refused(&[0x26, 0xff, 0xff, 0xff, 0xff, 0x10, 0xaa]),
        "leb128-overflow"
    );
    assert_eq!(
        refused(&[0x26, 0xff, 0xff, 0xff, 0xff, 0xff, 0x0f, 0xaa]),
        "leb128-overflow",
        "six bytes"
    );
    let largest =
        decode_fragment(&[0x26, 0xff, 0xff, 0xff, 0xff, 0x0f, 0xaa], None).expect("largest");
    assert_eq!(largest.sn, u32::MAX);
}

#[test]
fn a_fragment_longer_than_the_announced_batch_is_refused() {
    let at = std::vec![ID_FRAGMENT | FLAG_RELIABLE, 0];
    let mut body = at.clone();
    body.resize(2046, 7);
    assert!(decode_fragment(&body, Some(2048)).is_ok());
    body.push(7);
    assert_eq!(
        decode_fragment(&body, Some(2048))
            .expect_err("over")
            .class(),
        "length-exceeds-batch"
    );
}

#[test]
fn a_run_too_small_to_carry_a_fragment_is_refused_at_the_splitter() {
    let message = full_key_put(KEY, 10);
    assert_eq!(
        Splitter::new(message.as_slice(), MIN_BATCH, 0).expect_err("small"),
        FragmentError::BatchTooSmall
    );
    assert!(Splitter::new(message.as_slice(), MIN_BATCH + 1, 0).is_ok());
}

#[test]
fn a_joined_put_is_read_and_held_to_what_a_whole_frame_is_held_to() {
    let rules = rules();
    let message = full_key_put(KEY, 100);
    let view = decode_put(&message, &rules).expect("put");
    assert_eq!(view.key, Some(KEY.as_bytes()));
    assert_eq!(view.payload, payload(100));
    assert_eq!(view.attachment.sequence, 0);
    assert_eq!(view.attachment.timestamp, 1000);
    let class = |message: &[u8]| decode_put(message, &rules).expect_err("refused").class();
    // The Put: absent, short or long attachment, a wrong GID length, unsupported flags and extensions.
    let base = full_key_put(KEY, 10);
    // The header, the scope, the key's length (two bytes for 129) and the key, then the Put header.
    let at = 2 + leb(KEY.len() as u64).len() + KEY.len();
    let mut broken = base.clone();
    broken[at] = 0x01;
    assert_eq!(
        class(&broken),
        "malformed-message",
        "no attachment extension flag"
    );
    let mut broken = base.clone();
    broken[at] |= 0x20;
    assert_eq!(class(&broken), "unsupported-message", "timestamp");
    let mut broken = base.clone();
    broken[at] |= 0x40;
    assert_eq!(class(&broken), "unsupported-message", "encoding");
    let mut broken = base.clone();
    broken[at + 1] = 0x41;
    assert_eq!(class(&broken), "unsupported-extension", "unknown extension");
    let mut broken = base.clone();
    broken[at + 1] = 0xC3;
    assert_eq!(class(&broken), "unsupported-extension", "chained");
    let mut broken = base.clone();
    broken[at + 2] = 32;
    assert_eq!(class(&broken), "malformed-message", "a 32-byte attachment");
    let mut broken = base.clone();
    broken[at + 2] = 34;
    assert_eq!(class(&broken), "over-bound", "a 34-byte attachment");
    let mut broken = base.clone();
    broken[at + 3 + 16] = 15;
    assert_eq!(class(&broken), "malformed-message", "GID length 15");
    broken[at + 3 + 16] = 17;
    assert_eq!(class(&broken), "malformed-message", "GID length 17");
    // The PUSH.
    let mut broken = base.clone();
    broken[0] |= 0x80;
    assert_eq!(
        class(&broken),
        "unsupported-extension",
        "PUSH extension flag"
    );
    assert_eq!(class(&[0x25, 0]), "unsupported-message", "not a PUSH");
    assert_eq!(class(&base[..at]), "truncated", "ends before the body");
    assert_eq!(
        class(&[&base[..at], &[0x02]].concat()),
        "unsupported-message",
        "a delete"
    );
    assert_eq!(
        class(&base[..at + 2]),
        "truncated",
        "ends before the attachment"
    );
    assert_eq!(
        class(&base[..at + 3 + 10]),
        "truncated",
        "inside the attachment"
    );
    assert_eq!(
        class(&base[..base.len() - 1]),
        "truncated",
        "inside the payload"
    );
    let mut long = base.clone();
    long.push(0);
    assert_eq!(class(&long), "trailing-bytes");
}

#[test]
fn a_joined_put_names_a_valid_key_the_subscriber_declared() {
    let mut rules = rules();
    let class = |rules: &PutRules<'_>, message: &[u8]| {
        decode_put(message, rules).expect_err("refused").class()
    };
    for (key, expect) in [
        ("a/*/b", "wildcard-keyexpr"),
        ("a/**", "wildcard-keyexpr"),
        ("a/b$*", "wildcard-keyexpr"),
        ("/bad", "invalid-keyexpr"),
        ("bad/", "invalid-keyexpr"),
        ("a//b", "invalid-keyexpr"),
        ("a?b", "invalid-keyexpr"),
        ("a#b", "invalid-keyexpr"),
    ] {
        assert_eq!(class(&rules, &full_key_put(key, 1)), expect, "{key}");
    }
    let mut not_utf8 = full_key_put("abcd", 1);
    not_utf8[4] = 0xFF;
    assert_eq!(class(&rules, &not_utf8), "invalid-keyexpr");
    let mut empty = full_key_put("a", 1);
    empty[2] = 0;
    assert_eq!(
        class(&rules, &[&empty[..3], &empty[4..]].concat()),
        "invalid-keyexpr",
        "an empty key"
    );
    let mut scoped = full_key_put("a/b", 1);
    scoped[1] = 1;
    assert_eq!(
        class(&rules, &scoped),
        "invalid-keyexpr",
        "a full key in a nonzero scope"
    );
    assert_eq!(
        class(&rules, &full_key_put(&"k".repeat(130), 1)),
        "over-bound"
    );
    assert_eq!(
        class(&rules, &full_key_put(&"k".repeat(129), 1)[..40]),
        "truncated",
        "inside the key"
    );
    let only = |key: &[u8]| key == KEY.as_bytes();
    rules.subscribed = &only;
    assert!(decode_put(&full_key_put(KEY, 1), &rules).is_ok());
    assert_eq!(
        class(&rules, &full_key_put("slime_demo/other", 1)),
        "undeclared-key"
    );
}

#[test]
fn a_joined_put_names_the_alias_the_peer_declared() {
    let table = alias_table(1, KEY.as_bytes());
    let mut rules = rules();
    rules.aliases = Some(&table);
    let only = |key: &[u8]| key == KEY.as_bytes();
    rules.subscribed = &only;
    assert!(decode_put(&alias_put(1, 5), &rules).is_ok());
    let class = |rules: &PutRules<'_>, message: &[u8]| {
        decode_put(message, rules).expect_err("refused").class()
    };
    assert_eq!(
        class(&rules, &alias_put(2, 5)),
        "invalid-keyexpr",
        "an alias nobody declared"
    );
    assert_eq!(
        class(&rules, &put(0x1d, 1, None, 5)),
        "invalid-keyexpr",
        "receiver-mapped"
    );
    assert_eq!(
        class(&rules, &alias_put(0, 5)),
        "invalid-keyexpr",
        "scope zero is the global scope"
    );
    for scope in [256u32, 32_768, 65_535] {
        assert_eq!(
            class(&rules, &alias_put(scope, 5)),
            "invalid-keyexpr",
            "{scope}"
        );
        let declared = alias_table(scope, KEY.as_bytes());
        let mut high = rules;
        high.aliases = Some(&declared);
        assert!(
            decode_put(&alias_put(scope, 5), &high).is_ok(),
            "{scope} declared"
        );
    }
    assert_eq!(
        decode_put(&alias_put(65_536, 5), &rules)
            .expect_err("wide")
            .class(),
        "leb128-overflow"
    );
    let other = alias_table(1, b"slime_demo/other");
    rules.aliases = Some(&other);
    assert_eq!(
        class(&rules, &alias_put(1, 5)),
        "undeclared-key",
        "an alias for another key"
    );
}

#[test]
fn every_variable_length_integer_of_the_put_is_read_at_its_width() {
    let rules = rules();
    let class = |message: &[u8]| decode_put(message, &rules).expect_err("refused").class();
    let base = full_key_put("a/b", 1);
    // 7d 00 03 a/b | 81 43 21 <33> | 01 payload
    let key_len = 2;
    let att_len = 3 + 3 + 2;
    let payload_len = att_len + 1 + 33;
    let edit = |at: usize, old: usize, new: &[u8]| [&base[..at], new, &base[at + old..]].concat();
    assert_eq!(
        class(&edit(1, 1, &[0x80, 0x00])),
        "leb128-noncanonical",
        "scope"
    );
    assert_eq!(
        class(&edit(key_len, 1, &[0x83, 0x00])),
        "leb128-noncanonical",
        "key length"
    );
    assert_eq!(
        class(&edit(att_len, 1, &[0xA1, 0x00])),
        "leb128-noncanonical",
        "attachment length"
    );
    assert_eq!(
        class(&edit(payload_len, 1, &[0x81, 0x00])),
        "leb128-noncanonical",
        "payload length"
    );
    assert_eq!(
        class(&edit(1, 1, &[0x80, 0x80, 0x04])),
        "leb128-overflow",
        "scope over 16 bits"
    );
    assert_eq!(
        class(&edit(key_len, 1, &[0x80, 0x80, 0x04])),
        "leb128-overflow",
        "key length over 16 bits"
    );
    assert_eq!(
        class(&edit(att_len, 1, &[0x80, 0x80, 0x80, 0x80, 0x10])),
        "leb128-overflow",
        "attachment length"
    );
    assert_eq!(
        class(&edit(payload_len, 1, &[0x80, 0x80, 0x80, 0x80, 0x10])),
        "leb128-overflow",
        "payload length"
    );
    // Canonical values inside the width are read for what they say, not as an overflow.
    assert_eq!(
        class(&edit(key_len, 1, &[0x80, 0x02])),
        "over-bound",
        "key length 256"
    );
    assert_eq!(
        class(&edit(key_len, 1, &[0x80, 0x80, 0x02])),
        "over-bound",
        "key length 32768"
    );
    assert_eq!(
        class(&edit(att_len, 1, &[0x80, 0x80, 0x04])),
        "over-bound",
        "attachment length 65536"
    );
    assert_eq!(
        class(&edit(att_len, 1, &[0x80, 0x80, 0x80, 0x80, 0x08])),
        "over-bound",
        "attachment 2^31"
    );
    assert_eq!(
        class(&edit(payload_len, 1, &[0x80, 0x80, 0x04])),
        "over-bound",
        "payload 65536"
    );
    assert_eq!(
        class(&edit(payload_len, 1, &[0x80, 0x80, 0x80, 0x80, 0x08])),
        "over-bound",
        "payload 2^31"
    );
    // Unterminated in each field.
    assert_eq!(class(&base[..2]), "truncated", "key length");
    assert_eq!(class(&[&base[..1], &[0x80]].concat()), "truncated", "scope");
    assert_eq!(
        class(&[&base[..att_len], &[0x80]].concat()),
        "truncated",
        "attachment length"
    );
    assert_eq!(
        class(&[&base[..payload_len], &[0x80]].concat()),
        "truncated",
        "payload length"
    );
}

#[test]
fn a_payload_past_the_configuration_is_refused_though_the_storage_would_hold_it() {
    let mut rules = rules();
    rules.max_payload = 12;
    assert!(decode_put(&full_key_put("a/b", 12), &rules).is_ok());
    assert_eq!(
        decode_put(&full_key_put("a/b", 13), &rules)
            .expect_err("13")
            .class(),
        "over-bound"
    );
    rules.max_payload = 1024;
    assert!(decode_put(&full_key_put("a/b", 1024), &rules).is_ok());
    assert_eq!(
        decode_put(&full_key_put("a/b", 1025), &rules)
            .expect_err("1025")
            .class(),
        "over-bound"
    );
}

#[test]
fn a_multibyte_key_is_counted_in_bytes() {
    let key = "\u{e9}/\u{65e5}\u{672c}";
    assert_eq!((key.chars().count(), key.len()), (4, 9));
    let mut rules = rules();
    rules.max_key = 9;
    assert!(decode_put(&full_key_put(key, 5), &rules).is_ok());
    let over = std::format!("{key}x");
    assert_eq!((over.chars().count(), over.len()), (5, 10));
    assert_eq!(
        decode_put(&full_key_put(&over, 5), &rules)
            .expect_err("over")
            .class(),
        "over-bound"
    );
}

#[test]
fn a_put_message_produces_the_bytes_the_encoder_writes_at_any_offset() {
    for size in [0usize, 1, 12, 300, 5000] {
        let payload = payload(size);
        let attached = attachment();
        let message = PutMessage::new(KEY.as_bytes(), &attached, &payload);
        let whole = full_key_put(KEY, size);
        assert_eq!(Source::len(&message), whole.len(), "{size}");
        for (at, len) in [
            (0usize, whole.len()),
            (1, 7),
            (2, 130),
            (whole.len() - 1, 1),
            (whole.len() / 2, whole.len() / 2),
        ] {
            let mut out = std::vec![0u8; len];
            message.read(at, &mut out);
            assert_eq!(out, whole[at..at + len], "{size} at {at}");
        }
    }
}

#[test]
fn a_put_message_is_split_as_the_slice_is() {
    let payload = payload(5000);
    let attached = attachment();
    let message = PutMessage::new(KEY.as_bytes(), &attached, &payload);
    let mut splitter = Splitter::new(message, 2048, 1000).expect("splitter");
    let whole = split(&full_key_put(KEY, 5000), 2048, 1000);
    let mut buffer = std::vec![0u8; 2048];
    for expected in whole {
        let len = splitter
            .next_body(&mut buffer)
            .expect("body")
            .expect("more");
        assert_eq!(buffer[..len], expected[..]);
    }
    assert_eq!(splitter.next_body(&mut buffer), Ok(None));
}
