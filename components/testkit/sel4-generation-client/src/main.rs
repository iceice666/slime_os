#![no_std]
#![no_main]

//! The seL4 generation plane's unprivileged client: M6.5 in userspace
//! (P5.4.3).
//!
//! Holds an RPC endpoint to the manager and **nothing else**. That is the whole
//! point of the plane: M6.5 requires `BOOT_UPDATE` to be scoped by manifest to
//! the management service, so a component that wants to inspect, stage, select,
//! or roll back a generation must ask rather than act.
//!
//! The arms:
//!
//! * LIST and INSPECT return the live root's identity;
//! * INSPECT of a generation no closure contains is refused;
//! * STAGE of the candidate succeeds; STAGE of an unknown generation is refused
//!   *before* BootState changes, which the gate checks against the disk image;
//! * ROLLBACK clears the staged generation;
//! * STAGE then SELECT promotes, and SELECT naming the wrong generation is
//!   refused;
//! * a direct `BlockTransact` is refused, because no slot this component holds
//!   names a device — the authority claim, checked rather than asserted.

use slime_proto::block::{self, WireBlockRequest};
use slime_proto::generation::{self, WireGenerationReply, WireGenerationRequest};

/// The RPC endpoint to the manager, and this component's only grant.
const RPC_SLOT: u32 = 0;
/// A slot naming no device — every slot this component holds, in fact.
const NO_DEVICE_SLOT: u32 = RPC_SLOT;

/// Kept in step with `sel4-generation-manager`.
const KNOWN_GOOD: [u8; 32] = [0x11; 32];
const CANDIDATE: [u8; 32] = [0x22; 32];
const UNKNOWN: [u8; 32] = [0x99; 32];

const STATUS_OK: i32 = 0;
const STATUS_UNKNOWN_GENERATION: i32 = -2;
const STATUS_NO_PENDING: i32 = -3;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-generation-client] idle without an endpoint\n");
        slime_rt::exit(0);
    }

    // LIST: the live root, through the service.
    let listed = call(generation::OP_LIST, [0; 32]);
    if listed.status != STATUS_OK || identity_of(&listed) != KNOWN_GOOD {
        fail(b"list");
    }
    slime_rt::debug_write(b"[sel4-generation-client] listed the known-good root\n");

    // INSPECT of a generation the closure does not contain.
    if call(generation::OP_INSPECT, UNKNOWN).status != STATUS_UNKNOWN_GENERATION {
        fail(b"unknown inspect accepted");
    }
    slime_rt::debug_write(b"[sel4-generation-client] unknown generation refused\n");

    // STAGE of an unknown generation. Refused, and — the part the gate checks
    // from outside — refused without touching BootState.
    if call(generation::OP_STAGE, UNKNOWN).status != STATUS_UNKNOWN_GENERATION {
        fail(b"unknown stage accepted");
    }
    slime_rt::debug_write(b"[sel4-generation-client] unknown stage refused\n");

    // STAGE the candidate, then roll it back.
    let staged = call(generation::OP_STAGE, CANDIDATE);
    if staged.status != STATUS_OK || identity_of(&staged) != CANDIDATE {
        fail(b"stage");
    }
    slime_rt::debug_write(b"[sel4-generation-client] staged the candidate\n");

    let rolled = call(generation::OP_ROLLBACK, [0; 32]);
    if rolled.status != STATUS_OK || identity_of(&rolled) != KNOWN_GOOD {
        fail(b"rollback");
    }
    slime_rt::debug_write(b"[sel4-generation-client] rolled back to known-good\n");

    // Rolling back with nothing staged is refused rather than silently
    // succeeding, so a client cannot mistake "nothing to do" for "done".
    if call(generation::OP_ROLLBACK, [0; 32]).status != STATUS_NO_PENDING {
        fail(b"empty rollback accepted");
    }
    slime_rt::debug_write(b"[sel4-generation-client] rollback with no pending refused\n");

    // Stage again, then confirm health. SELECT naming the wrong generation is
    // refused: only the generation actually staged may be promoted.
    if call(generation::OP_STAGE, CANDIDATE).status != STATUS_OK {
        fail(b"restage");
    }
    if call(generation::OP_SELECT, KNOWN_GOOD).status != STATUS_UNKNOWN_GENERATION {
        fail(b"wrong select accepted");
    }
    slime_rt::debug_write(b"[sel4-generation-client] wrong select refused\n");

    let selected = call(generation::OP_SELECT, CANDIDATE);
    if selected.status != STATUS_OK || identity_of(&selected) != CANDIDATE {
        fail(b"select");
    }
    slime_rt::debug_write(b"[sel4-generation-client] promoted the candidate\n");

    // The authority claim. This component was granted one endpoint; there is no
    // slot it holds that names a block device, so it cannot forge a transition
    // even though it knows the on-disk format perfectly well.
    let probe = WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op: block::OP_READ,
        flags: 0,
        reserved: 0,
        lba: 0,
        sector_count: 1,
        buffer_phys: 0,
        buffer_pages: 0,
    };
    let mut reply = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact(NO_DEVICE_SLOT, &probe.encode(), &mut reply) >= 0 {
        fail(b"direct device access accepted");
    }
    slime_rt::debug_write(b"[sel4-generation-client] direct device access refused\n");

    slime_rt::debug_write(b"[sel4-generation-client] generation client complete\n");
}

/// One request/reply round trip with the manager.
fn call(op: u8, identity: [u8; 32]) -> WireGenerationReply {
    let words = identity_words(identity);
    let request = WireGenerationRequest {
        magic: generation::GENERATION_MAGIC,
        version: generation::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: [0; 6],
        generation0: words[0],
        generation1: words[1],
        generation2: words[2],
        generation3: words[3],
    };
    let encoded = request.encode();
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &[]) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(b"send"),
            _ => break,
        }
    }
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RPC_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(b"recv"),
            received => {
                // The manager answers with data. A reply carrying a capability
                // would be it handing over authority the client did not earn.
                if caps.iter().any(|slot| *slot != 0) {
                    fail(b"the manager attached a capability");
                }
                let Some(reply) = WireGenerationReply::decode(&bytes[..received as usize]) else {
                    fail(b"reply decode");
                };
                if reply.magic != generation::GENERATION_MAGIC
                    || reply.version != generation::FORMAT_VERSION
                {
                    fail(b"reply header");
                }
                return reply;
            }
        }
    }
}

fn identity_words(identity: [u8; 32]) -> [u64; 4] {
    let mut words = [0u64; 4];
    for (index, word) in words.iter_mut().enumerate() {
        let start = index * 8;
        *word = u64::from_le_bytes(
            identity[start..start + 8]
                .try_into()
                .expect("eight-byte chunk"),
        );
    }
    words
}

fn identity_of(reply: &WireGenerationReply) -> [u8; 32] {
    let mut identity = [0u8; 32];
    for (index, word) in [
        reply.generation0,
        reply.generation1,
        reply.generation2,
        reply.generation3,
    ]
    .iter()
    .enumerate()
    {
        identity[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    identity
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-generation-client] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// The RPC endpoint init's declared edge places, and the discriminator.
///
/// The plane declares this executable twice — the instance init spawns, and a
/// root-owned `idle` one whose endpoint at this slot is a loopback nobody sends
/// on. Both hold a real endpoint, so *arrival* separates them: the root delivers
/// a nonzero boot action only to the bootstrap instance, so `startup_arg` cannot.
const RUN_TOKEN_SLOT: u32 = 1;
/// Yields given up before concluding no peer will speak. The idle instance
/// always exhausts this bound, so it is a latency rather than a safety margin.
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
