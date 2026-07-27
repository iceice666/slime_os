#![no_std]
#![no_main]

//! C7.7 sample-plane lender, driven entirely through the real syscall ABI.
//!
//! Allocates a quota-charged shared buffer, fills it with a payload larger than
//! the kernel message bound, seals it irreversibly, loans the exact sealed
//! region to a receiver named by a `RIGHT_SUPERVISE` capability, and sends only
//! the 64-byte sample descriptor over an IPC channel. The payload itself never
//! enters the kernel queue.
//!
//! Unlike the in-harness `kernel/tests/sample_plane.rs` composition, both peers
//! here are separately spawned components holding capabilities granted by the
//! generation, so this exercises `SYS_SHARED_BUFFER_*`, the rights gates, the
//! loan receiver binding, and reclamation through real task termination.

use slime_proto::interface_schema::telemetry_stream::TYPE_TAG;
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, FLAG_LAST, FORMAT_VERSION, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, MAX_MSG};

slime_rt::entry!(main);

/// Endpoint to the receiver.
const PEER_SLOT: u32 = 0;
/// `SharedBufferFactory` granted by the generation.
const FACTORY_SLOT: u32 = 1;
/// `RIGHT_SUPERVISE` handle naming the receiver. The loan names its receiver
/// through this capability, never through an ambient task id.
const RECEIVER_SLOT: u32 = 2;

const PAGE: u64 = 4096;
const PAGES: usize = 2;
const PAYLOAD_LEN: u64 = PAGES as u64 * PAGE;
const BASE: u64 = 0x0000_0009_0000_0000;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sample-lender] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
    // A payload larger than the control-message bound is the whole point.
    if PAYLOAD_LEN <= MAX_MSG as u64 {
        fail(b"payload must exceed MAX_MSG");
    }

    // Denial arm: the factory capability carries creation authority only, so
    // naming it where a buffer is expected must be refused rather than
    // reinterpreted.
    if slime_rt::shared_buffer_seal(FACTORY_SLOT) != ERR_BAD_CAP {
        fail(b"factory slot accepted as a buffer");
    }
    slime_rt::debug_write(b"[sample-lender] factory is not a buffer\n");

    let buffer = match slime_rt::shared_buffer_create(FACTORY_SLOT, PAGES, true) {
        Ok(buffer) => buffer,
        Err(_) => fail(b"create"),
    };
    if buffer.id == 0 {
        fail(b"kernel assigned no identity");
    }
    slime_rt::debug_write(b"[sample-lender] buffer created\n");

    if slime_rt::shared_buffer_map(buffer.slot, BASE, 0, PAYLOAD_LEN, true) != ERR_SUCCESS {
        fail(b"writable map");
    }
    // SAFETY: the kernel installed a writable user mapping of exactly
    // `PAYLOAD_LEN` bytes at `BASE`, and it stays mapped until the unmap below.
    unsafe {
        let bytes = BASE as *mut u8;
        for index in 0..PAYLOAD_LEN as usize {
            bytes.add(index).write_volatile((index % 251) as u8);
        }
    }
    slime_rt::debug_write(b"[sample-lender] payload written\n");

    // A loan requires an irreversibly sealed source, so sealing must precede it.
    if slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN).is_ok() {
        fail(b"unsealed region was loanable");
    }
    slime_rt::debug_write(b"[sample-lender] unsealed loan denied\n");

    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {
        fail(b"seal");
    }
    // Sealing is irreversible: the live writable mapping was downgraded, and a
    // fresh writable mapping can never be obtained again.
    if slime_rt::shared_buffer_map(buffer.slot, BASE + PAYLOAD_LEN, 0, PAGE, true) == ERR_SUCCESS {
        fail(b"writable map survived seal");
    }
    slime_rt::debug_write(b"[sample-lender] seal is irreversible\n");

    let loan = match slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN) {
        Ok(loan) => loan,
        Err(_) => fail(b"loan"),
    };
    slime_rt::debug_write(b"[sample-lender] loan created\n");

    // Only the descriptor crosses the channel; it names the loan by its
    // unforgeable kernel-assigned identity.
    let descriptor = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: FORMAT_VERSION,
        flags: FLAG_LAST,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: loan.id,
        offset: 0,
        length: PAYLOAD_LEN,
        type_identity: TYPE_TAG,
        sequence: 1,
        reserved: [0; 8],
    };
    if slime_rt::send(PEER_SLOT, &descriptor.encode(), &[loan.slot]) != ERR_SUCCESS {
        fail(b"send descriptor");
    }
    slime_rt::debug_write(b"[sample-lender] descriptor sent\n");

    // Wait for the receiver to finish with the loan before tearing anything
    // down. Not politeness: the lender's own termination would settle every
    // loan it owns, so exiting early would reclaim the region out from under a
    // receiver that has not mapped it yet. This is also the C7.5 retention
    // property under test — the creator cannot reclaim pages while a valid loan
    // is outstanding.
    let mut done = [0u8; MAX_MSG];
    let mut no_caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(PEER_SLOT, &mut done, &mut no_caps) {
            slime_rt::ERR_WOULDBLOCK => {
                slime_rt::wait(&[slime_rt::WaitSource::Endpoint(PEER_SLOT)])
            }
            n if n < 0 => fail(b"await receiver"),
            _ => break,
        }
    }
    slime_rt::debug_write(b"[sample-lender] receiver settled\n");

    // With the loan returned, the creator may reclaim: drop the local mapping
    // and release the buffer, returning every page and charge.
    if slime_rt::shared_buffer_unmap(buffer.slot, BASE) != ERR_SUCCESS {
        fail(b"unmap");
    }
    if slime_rt::shared_buffer_release(buffer.slot) != ERR_SUCCESS {
        fail(b"release");
    }
    // The capability is invalidated by release, so naming it again must fail.
    if slime_rt::shared_buffer_release(buffer.slot) != ERR_BAD_CAP {
        fail(b"released buffer still nameable");
    }
    slime_rt::debug_write(b"[sample-lender] released\n");
    slime_rt::debug_write(b"[sample-lender] done\n");
}
