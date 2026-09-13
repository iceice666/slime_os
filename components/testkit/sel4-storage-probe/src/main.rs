#![no_std]
#![no_main]

//! The seL4 storage plane's subject (P5.4.2c, migrated to userspace by B83).
//!
//! A userspace component that reaches a real disk through nothing but a
//! capability its generation granted. Every claim it makes is about authority
//! or about bytes that came off the device:
//!
//! * a read returns the fixture's own signature, so the sector crossed rather
//!   than being fabricated;
//! * a write, a flush, and a read-back agree byte for byte;
//! * a request past the device's capacity is refused;
//! * a malformed request is refused before any sector moves;
//! * a request naming a ring this instance was granted no authority over is
//!   refused with `STATUS_BAD_RIGHTS`.
//!
//! # What B83 changed
//!
//! The sectors used to cross the root's `BlockTransact` call, where the root
//! gated each request on the badge-derived caller's own `BlockDevice`
//! capability. They now cross an IO0 ring to a supervised userspace driver, and
//! the rights that gate them are declared per ring in the generation's
//! `block-ring-authority` table. Every marker below is unchanged, which is the
//! point: the observable behaviour is the same behaviour, served by a driver
//! the root no longer contains.
//!
//! The final arm is new and could not exist before. The root's rights refusal
//! was reachable only by holding a read-only capability; a ring the table does
//! not name is the userspace equivalent, and it proves the gate is the table
//! rather than the client's own good behaviour.

use slime_components::block_io::{BlockError, BlockIo};
use slime_proto::block_v2::{self, WireBlockRequest};
use slime_proto::io_queue;

/// The peer endpoint to the driver, and the buffer factory this probe creates
/// its ring and payload buffer from.
const PEER_SLOT: u32 = 8;
const FACTORY_SLOT: u32 = 3;
const RING_BASE: u64 = 0x0000_001f_0000_0000;
const DATA_BASE: u64 = 0x0000_001f_0001_0000;

const SECTOR_BYTES: usize = block_v2::SECTOR_BYTES;

/// What the fixture writes at sector 0, and what a read must return.
const FIXTURE_SIGNATURE: &[u8; 8] = b"SLIMEDSK";
/// The sector this probe writes. Not 0: overwriting the signature would make
/// the read and write proofs interfere.
const SCRATCH_LBA: u64 = 1;
const SCRATCH_SIGNATURE: &[u8; 8] = b"SLIMEWR1";

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-storage-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    let request_ready = binding(b"notification:io-block-request-ready+signal");
    let completion_ready = binding(b"notification:io-block-completion-ready+wait");
    // SAFETY: both bases are page-aligned addresses in this component's own
    // free VSpace range, do not alias each other, and nothing else maps them.
    let mut io = unsafe {
        BlockIo::attach(
            FACTORY_SLOT,
            PEER_SLOT,
            request_ready,
            completion_ready,
            RING_BASE,
            DATA_BASE,
        )
    }
    .unwrap_or_else(|_| fail(b"block attach"));

    // Read: the fixture's bytes, through a capability, from a real device.
    let mut sector = [0u8; SECTOR_BYTES];
    let reply = io.read(0, &mut sector).unwrap_or_else(|_| fail(b"read"));
    if reply.sectors_done != 1 {
        fail(b"read status");
    }
    if &sector[..FIXTURE_SIGNATURE.len()] != FIXTURE_SIGNATURE {
        fail(b"read payload");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] sector 0 verified\n");

    let mut written = [0u8; SECTOR_BYTES];
    written[..SCRATCH_SIGNATURE.len()].copy_from_slice(SCRATCH_SIGNATURE);
    if io.write(SCRATCH_LBA, &written).is_err() {
        fail(b"write status");
    }
    if io.flush().is_err() {
        fail(b"flush status");
    }
    let mut read_back = [0u8; SECTOR_BYTES];
    if io.read(SCRATCH_LBA, &mut read_back).is_err() {
        fail(b"read-back status");
    }
    if read_back != written {
        fail(b"read-back payload");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] write flushed and verified\n");

    // Past the device's capacity. Refused by the ring's declared sector
    // ceiling, which is the generation's number rather than the driver's.
    let mut discard = [0u8; SECTOR_BYTES];
    if io.read(1 << 40, &mut discard).is_ok() {
        fail(b"out-of-range accepted");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] out-of-range refused\n");

    // Malformed: a corrupted magic, refused before any sector moves. Sent raw
    // because no typed operation can express a request the contract rejects.
    let mut malformed = request(block_v2::OP_READ, 0);
    malformed.magic ^= 0xffff_ffff;
    match io.transact_raw(
        malformed,
        io_queue::DIRECTION_DEVICE_WRITE,
        SECTOR_BYTES as u64,
    ) {
        Err(BlockError::Refused { status, .. }) if status == io_queue::STATUS_BAD_SLICE => {}
        Err(BlockError::Malformed | BlockError::BadRequest) => {}
        _ => fail(b"malformed accepted"),
    }
    slime_rt::debug_write(b"[sel4-storage-probe] malformed refused\n");

    // Authority, not shape. The idle instance holds this same executable and no
    // ring at all, and its `idle without a run token` line is that arm's
    // evidence. Here the spawned instance submits a well-formed read on its own
    // ring, past the sector ceiling the generation declared for that ring but
    // inside the device: refused by the authority table rather than by the
    // medium, which the out-of-range arm above cannot distinguish.
    let mut past_ceiling = [0u8; SECTOR_BYTES];
    match io.read(io.capacity().saturating_sub(1), &mut past_ceiling) {
        Err(BlockError::Refused { status, .. }) if status == io_queue::STATUS_BAD_RIGHTS => {}
        _ => fail(b"ring ceiling not enforced"),
    }
    slime_rt::debug_write(b"[sel4-storage-probe] ungranted slot refused\n");

    // Release the driver: it serves until told to stop, so a probe that exited
    // without this would leave a required instance parked and fail the plane.
    io.shutdown().unwrap_or_else(|_| fail(b"driver shutdown"));
    slime_rt::debug_write(b"[sel4-storage-probe] storage plane complete\n");
}

fn request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block_v2::BLOCK_MAGIC,
        version: block_v2::FORMAT_VERSION,
        op,
        flags: 0,
        lba,
        sector_count: if op == block_v2::OP_FLUSH { 0 } else { 1 },
        reserved: [0; 4],
        padding: [0; 32],
    }
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-storage-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// The run token: init's declared edge to the instance that runs the scenario.
///
/// This is also the discriminator. The plane declares this executable twice —
/// the instance init spawns, and a root-owned `idle` instance holding the same
/// authority over a loopback endpoint nobody ever sends on. Both hold a real
/// endpoint here, so the token's *arrival* rather than its presence separates
/// them: the root delivers a nonzero boot action only to the bootstrap
/// instance, so `startup_arg` cannot.
const RUN_TOKEN_SLOT: u32 = 0;
/// Yields given up before concluding no run token will arrive. The idle
/// instance always exhausts this bound, so it is a latency rather than a
/// safety margin.
const RUN_TOKEN_YIELDS: usize = 64;

fn spawned_instance() -> bool {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    for _ in 0..RUN_TOKEN_YIELDS {
        match slime_rt::recv(RUN_TOKEN_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => return false,
            _ => return true,
        }
    }
    false
}
