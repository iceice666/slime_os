//! B59: the root-service ABI is a frozen numbering shared by two crates.
//!
//! `slime-root` dispatches on these labels and `components/runtime` sends them.
//! Before B59 each crate hand-authored the table; `slime-root/src/console.rs`
//! records a numbering disagreement between the two that produced silently
//! garbled keystrokes — no compile error, only a runtime misdecode. Sharing one
//! generated module removes that class of drift, but it does not stop someone
//! *renumbering* the shared table, which would silently invalidate every
//! component image built against an earlier generation.
//!
//! So the numbers are pinned here. This test fails on a renumbering, which is
//! the point: a deliberate ABI change must edit the contract *and* this list,
//! and reviewing the second edit is what makes the first one deliberate.

use slime_proto::syscall_abi::{
    ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK,
    FORMAT_VERSION, GRANT_RECORD_BYTES, GRANT_RIGHTS_OFFSET, GRANT_SLOT_OFFSET, MAX_CAPS_PER_MSG,
    MAX_MSG, capability_table_labels, capability_transfer_labels, directory_labels, fixture_labels,
    lifecycle_labels, shared_buffer_labels, spawn_labels, supervision_labels,
};

#[test]
fn operation_labels_are_frozen() {
    let labels: [(&str, u64); 23] = [
        ("lifecycle::EXIT", lifecycle_labels::EXIT),
        ("lifecycle::UNHEALTHY", lifecycle_labels::UNHEALTHY),
        ("spawn::SPAWN", spawn_labels::SPAWN),
        ("fixture::DIRECTIVE", fixture_labels::DIRECTIVE),
        ("supervision::STATUS", supervision_labels::STATUS),
        ("supervision::DERIVE", supervision_labels::DERIVE),
        ("capabilityTable::DROP", capability_table_labels::DROP),
        (
            "capabilityTable::OCCUPANCY",
            capability_table_labels::OCCUPANCY,
        ),
        ("directory::DERIVE", directory_labels::DERIVE),
        ("sharedBuffer::CREATE", shared_buffer_labels::CREATE),
        ("sharedBuffer::RELEASE", shared_buffer_labels::RELEASE),
        ("sharedBuffer::MAP", shared_buffer_labels::MAP),
        ("sharedBuffer::UNMAP", shared_buffer_labels::UNMAP),
        ("sharedBuffer::SEAL", shared_buffer_labels::SEAL),
        ("sharedBuffer::LOAN", shared_buffer_labels::LOAN),
        ("sharedBuffer::LOAN_MAP", shared_buffer_labels::LOAN_MAP),
        ("sharedBuffer::RETURN", shared_buffer_labels::RETURN),
        ("sharedBuffer::REVOKE", shared_buffer_labels::REVOKE),
        ("sharedBuffer::OCCUPANCY", shared_buffer_labels::OCCUPANCY),
        (
            "capabilityTransfer::EXPORT",
            capability_transfer_labels::EXPORT,
        ),
        (
            "capabilityTransfer::IMPORT",
            capability_transfer_labels::IMPORT,
        ),
        (
            "capabilityTransfer::EXPORT_CANCEL",
            capability_transfer_labels::EXPORT_CANCEL,
        ),
        (
            "capabilityTransfer::EXPORT_FINALIZE",
            capability_transfer_labels::EXPORT_FINALIZE,
        ),
    ];
    let expected: [u64; 23] = [
        3, 9, 4, 5, 12, 32, 13, 31, 15, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 33, 34, 35, 36,
    ];
    for ((name, actual), want) in labels.iter().zip(expected) {
        assert_eq!(*actual, want, "operation {name} was renumbered");
    }
}

/// Two operations sharing a label would make the root dispatch one request to
/// whichever arm its match happened to test first. The contract's own validator
/// rejects it, so this asserts the property survived generation.
#[test]
fn operation_labels_are_distinct() {
    let mut labels = [
        lifecycle_labels::EXIT,
        lifecycle_labels::UNHEALTHY,
        spawn_labels::SPAWN,
        fixture_labels::DIRECTIVE,
        supervision_labels::STATUS,
        supervision_labels::DERIVE,
        capability_table_labels::DROP,
        capability_table_labels::OCCUPANCY,
        directory_labels::DERIVE,
        shared_buffer_labels::CREATE,
        shared_buffer_labels::RELEASE,
        shared_buffer_labels::MAP,
        shared_buffer_labels::UNMAP,
        shared_buffer_labels::SEAL,
        shared_buffer_labels::LOAN,
        shared_buffer_labels::LOAN_MAP,
        shared_buffer_labels::RETURN,
        shared_buffer_labels::REVOKE,
        shared_buffer_labels::OCCUPANCY,
        capability_transfer_labels::EXPORT,
        capability_transfer_labels::IMPORT,
        capability_transfer_labels::EXPORT_CANCEL,
        capability_transfer_labels::EXPORT_FINALIZE,
    ];
    labels.sort_unstable();
    for pair in labels.windows(2) {
        assert_ne!(pair[0], pair[1], "two operations share a label");
    }
}

#[test]
fn status_codes_are_frozen() {
    assert_eq!(ERR_SUCCESS, 0);
    assert_eq!(ERR_BAD_CAP, -1);
    assert_eq!(ERR_PEER_DEAD, -2);
    assert_eq!(ERR_WOULDBLOCK, -3);
    assert_eq!(ERR_INVALID_ARG, -4);
    assert_eq!(ERR_OUT_OF_MEMORY, -5);
    // Only `ERR_SUCCESS` may be non-negative: every caller tests `< 0` for
    // failure, so a positive error code would read as success.
    for code in [
        ERR_BAD_CAP,
        ERR_PEER_DEAD,
        ERR_WOULDBLOCK,
        ERR_INVALID_ARG,
        ERR_OUT_OF_MEMORY,
    ] {
        assert!(code < 0, "error code {code} would read as success");
    }
}

/// The spawn-grant record layout crosses the syscall boundary: the runtime
/// encodes it into the transfer window and the root decodes the same offsets.
/// Before B59 the size was a `16` in each crate, kept in agreement by a doc
/// comment reading "Matches ...".
#[test]
fn spawn_grant_record_layout_is_frozen() {
    assert_eq!(GRANT_SLOT_OFFSET, 0);
    assert_eq!(GRANT_RIGHTS_OFFSET, 8);
    assert_eq!(GRANT_RECORD_BYTES, 16);
    // The two 8-byte fields must exactly fill the record: a gap would leave
    // bytes the encoder never writes and the decoder never reads.
    assert_eq!(GRANT_RIGHTS_OFFSET + 8, GRANT_RECORD_BYTES);
    assert_eq!(GRANT_SLOT_OFFSET + 8, GRANT_RIGHTS_OFFSET);
}

#[test]
fn message_bounds_are_frozen() {
    assert_eq!(FORMAT_VERSION, 1);
    assert_eq!(MAX_MSG, 64);
    assert_eq!(MAX_CAPS_PER_MSG, 1);
}
