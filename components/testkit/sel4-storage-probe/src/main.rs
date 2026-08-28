#![no_std]
#![no_main]

//! The seL4 storage plane's subject (P5.4.2c).
//!
//! A userspace component that reaches a real disk through nothing but a
//! capability its generation granted. Every claim it makes is about authority
//! or about bytes that came off the device:
//!
//! * a read returns the fixture's own signature, so the sector crossed rather
//!   than being fabricated;
//! * a write, a flush, and a read-back agree byte for byte;
//! * a request past the device's capacity is refused;
//! * a malformed request is refused before any sector moves.
//!
//! Separate from the oracle's `storage-probe` rather than a branch inside it.
//! That component reads through `buffer_phys`, an ambient pointer the retired
//! kernel dereferences on the caller's behalf; on seL4 the payload crosses in
//! the caller's own transfer window, so the two do not share a body. What they
//! do share is the wire contract: the same `contracts/block/v1` request and
//! reply records, decoded by the same generated bindings.

use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

/// The block capability.
const BLOCK_SLOT: u32 = 1;
/// A slot holding no block capability, for the refusal arm.
const EMPTY_SLOT: u32 = 2;

const SECTOR_BYTES: usize = 512;

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

    // Read: the fixture's bytes, through a capability, from a real device.
    let mut sector = [0u8; SECTOR_BYTES];
    let reply = read(0, &mut sector);
    if reply.status != 0 || reply.sectors_done != 1 {
        fail(b"read status");
    }
    if &sector[..FIXTURE_SIGNATURE.len()] != FIXTURE_SIGNATURE {
        fail(b"read payload");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] sector 0 verified\n");

    let mut written = [0u8; SECTOR_BYTES];
    written[..SCRATCH_SIGNATURE.len()].copy_from_slice(SCRATCH_SIGNATURE);
    if write(SCRATCH_LBA, &written).status != 0 {
        fail(b"write status");
    }
    if flush().status != 0 {
        fail(b"flush status");
    }
    let mut read_back = [0u8; SECTOR_BYTES];
    if read(SCRATCH_LBA, &mut read_back).status != 0 {
        fail(b"read-back status");
    }
    if read_back != written {
        fail(b"read-back payload");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] write flushed and verified\n");

    let mut discard = [0u8; SECTOR_BYTES];
    if read(1 << 40, &mut discard).status == 0 {
        fail(b"out-of-range accepted");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] out-of-range refused\n");

    let mut malformed = request(block::OP_READ, 0);
    malformed.magic ^= 0xffff_ffff;
    let mut reply_bytes = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact(BLOCK_SLOT, &malformed.encode(), &mut reply_bytes) >= 0 {
        fail(b"malformed accepted");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] malformed refused\n");

    let plain = request(block::OP_READ, 0);
    if slime_rt::block_transact(EMPTY_SLOT, &plain.encode(), &mut reply_bytes) >= 0 {
        fail(b"ungranted slot accepted");
    }
    slime_rt::debug_write(b"[sel4-storage-probe] ungranted slot refused\n");

    slime_rt::debug_write(b"[sel4-storage-probe] storage plane complete\n");
}

fn request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count: if op == block::OP_FLUSH { 0 } else { 1 },
        buffer_pages: 1,
        buffer_phys: 0,
    }
}

fn read(lba: u64, sector: &mut [u8; SECTOR_BYTES]) -> WireBlockReply {
    let mut reply = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact_sector(
        BLOCK_SLOT,
        &request(block::OP_READ, lba).encode(),
        &mut reply,
        sector,
    ) < 0
    {
        return refused();
    }
    decode(&reply)
}

fn write(lba: u64, sector: &[u8; SECTOR_BYTES]) -> WireBlockReply {
    let mut reply = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact_write(
        BLOCK_SLOT,
        &request(block::OP_WRITE, lba).encode(),
        sector,
        &mut reply,
    ) < 0
    {
        return refused();
    }
    decode(&reply)
}

fn flush() -> WireBlockReply {
    let mut reply = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact(
        BLOCK_SLOT,
        &request(block::OP_FLUSH, 0).encode(),
        &mut reply,
    ) < 0
    {
        return refused();
    }
    decode(&reply)
}

fn decode(reply: &[u8; block::REPLY_LEN]) -> WireBlockReply {
    WireBlockReply::decode(reply)
        .filter(|reply| reply.magic == block::BLOCK_MAGIC && reply.version == block::FORMAT_VERSION)
        .unwrap_or_else(|| fail(b"reply shape"))
}

fn refused() -> WireBlockReply {
    WireBlockReply {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        status: -1,
        sectors_done: 0,
    }
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
