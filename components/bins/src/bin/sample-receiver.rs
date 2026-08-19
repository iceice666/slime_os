#![no_std]
#![no_main]

//! C7.7 sample-plane receiver, driven entirely through the real syscall ABI.
//!
//! Receives a 64-byte sample descriptor plus the transferred `SharedBufferLoan`
//! capability over an IPC channel, validates the descriptor before mapping or
//! allocating anything, maps only the loaned bytes read-only, reconstructs a
//! payload larger than the kernel message bound, and returns the loan exactly
//! once.

use slime_proto::interface_schema::telemetry_stream::TYPE_TAG;
use slime_proto::sample_descriptor::{DESCRIPTOR_LEN, WireSampleDescriptor};
use slime_proto::valid_sample_descriptor;
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// Endpoint to the lender.
const PEER_SLOT: u32 = 0;

const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_000A_0000_0000;

/// Yields the empty-budget probe gives up before concluding no lender exists.
///
/// Generous against the real run, whose lender is spawned first and reaches its
/// send after a create, a map, a write, and a seal, while still bounding the
/// probe's exit. Too small would make the real receiver report no peer; too
/// large only costs the probe time.
const PROBE_YIELDS: usize = 4096;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sample-receiver] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// The generation declares this instance `autostart = false`, so the only copy
/// that ever runs is the one its owner spawned. The guard that used to stand
/// here keyed on `startup_arg == 0` to skip a root-autostarted probe copy — a
/// discriminator that cannot work, because the root delivers a nonzero boot
/// action only to the bootstrap instance and every spawned child reads zero
/// too, so it skipped the real run as well.
fn main(_startup_arg: u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    // The plane respawns this component once, after the first copy exits,
    // purely to show the spawn budget released its dead. That retry is granted
    // nothing beyond its declared edge, and its lender is gone — so nothing
    // will ever arrive and there is nothing to verify. Exiting 0 says so;
    // failing would make a `required` instance's deliberate emptiness fatal
    // (B51).
    //
    // Discriminated by the *peer*, not by the endpoint. The edge is now a
    // declared capability the root installs into every copy, so `recv` no
    // longer answers `ERR_BAD_CAP` for the probe — it blocks, because a native
    // Endpoint with no sender is indistinguishable from one whose sender has
    // not spoken yet. A bounded non-blocking wait is what separates them: the
    // real run's lender is already running and sends promptly, while the
    // probe's never will.
    let length = 'received: {
        for _ in 0..PROBE_YIELDS {
            match slime_rt::recv(PEER_SLOT, &mut message, &mut received) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                n if n < 0 => fail(b"recv"),
                n => break 'received n,
            }
        }
        slime_rt::debug_write(b"[sample-receiver] no peer granted\n");
        slime_rt::exit(0)
    };
    // Exactly one control message crossed the channel, and it is the bound.
    if length != DESCRIPTOR_LEN as i64 || length != MAX_MSG as i64 {
        fail(b"descriptor is not exactly one message");
    }
    // A loan is a root-owned logical capability rather than a kernel object,
    // so the native Endpoint message carries only the descriptor. Claim the
    // finalized export addressed to this receiver before naming the loan.
    let loan_slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"import loan"));
    slime_rt::debug_write(b"[sample-receiver] descriptor received\n");

    let descriptor = match WireSampleDescriptor::decode(&message) {
        Some(descriptor) => descriptor,
        None => fail(b"decode"),
    };

    // A descriptor naming a different loan must be refused before anything is
    // mapped or allocated, even though the receiver does hold a real loan.
    let stale = WireSampleDescriptor {
        loan_id: descriptor.loan_id ^ 1,
        ..descriptor
    };
    if valid_sample_descriptor(&stale, descriptor.loan_id, TYPE_TAG, PAGE) {
        fail(b"stale descriptor validated");
    }
    if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length + PAGE) == ERR_SUCCESS
    {
        fail(b"map escaped the loaned region");
    }
    slime_rt::debug_write(b"[sample-receiver] malformed descriptor mapped nothing\n");

    if !valid_sample_descriptor(&descriptor, descriptor.loan_id, TYPE_TAG, PAGE) {
        fail(b"admitted descriptor rejected");
    }
    if slime_rt::shared_buffer_loan_map(loan_slot, BASE, descriptor.offset, descriptor.length)
        != ERR_SUCCESS
    {
        fail(b"loan map");
    }
    slime_rt::debug_write(b"[sample-receiver] loaned bytes mapped\n");

    // A loan is read-only: the receiver holds no write authority over a sealed
    // region, so a writable mapping of the same bytes must be refused.
    if slime_rt::shared_buffer_map(loan_slot, BASE + descriptor.length, 0, PAGE, true)
        == ERR_SUCCESS
    {
        fail(b"loan granted write access");
    }
    slime_rt::debug_write(b"[sample-receiver] loan stays read-only\n");

    // Reconstruct the whole payload — larger than MAX_MSG — from the buffer.
    // SAFETY: the kernel mapped exactly `descriptor.length` bytes read-only at
    // `BASE`, and they stay mapped until the return below.
    let mismatch = unsafe {
        let bytes = BASE as *const u8;
        (0..descriptor.length as usize)
            .find(|index| bytes.add(*index).read_volatile() != (*index % 251) as u8)
    };
    if mismatch.is_some() {
        fail(b"payload mismatch");
    }
    slime_rt::debug_write(b"[sample-receiver] payload verified\n");

    if slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS {
        fail(b"return");
    }
    // Single-return: the identity is consumed, so a second return finds nothing.
    if slime_rt::shared_buffer_return(loan_slot) != ERR_BAD_CAP {
        fail(b"loan returned twice");
    }
    slime_rt::debug_write(b"[sample-receiver] loan returned once\n");

    // Tell the lender the loan is settled so it can reclaim. Until this lands,
    // the lender must not release: the creator cannot reclaim pages while a
    // valid loan is outstanding (C7.5).
    if slime_rt::send(PEER_SLOT, b"settled", &[]) != ERR_SUCCESS {
        fail(b"signal settled");
    }
    slime_rt::debug_write(b"[sample-receiver] done\n");
}
