#![no_std]
#![no_main]

//! The seL4 generation plane's broker: M6.5 in userspace (P5.4.3).
//!
//! The one component in the plane that holds a block capability, and therefore
//! the only one that can touch BootState. Its clients hold an RPC endpoint and
//! nothing else — which is M6.5's authority claim: `BOOT_UPDATE` is scoped to
//! the management service by the manifest, and a component that wants to stage
//! or roll back a generation must ask.
//!
//! Five operations over `contracts/generation-manifest/v1`: LIST, INSPECT, STAGE,
//! SELECT, ROLLBACK. The first two are read-only; the last three are BootState
//! transitions, committed older-slot-first exactly as the rollback plane does,
//! because it is the same on-disk structure and the same invariant — no
//! transition overwrites the only valid root.
//!
//! What the oracle does in `generation_service::transact` behind syscall
//! `SYS_GENERATION_TRANSACT`, gated on a `GenerationControl` capability with
//! `RIGHT_BOOT_UPDATE`. Here the block capability *is* the gate: a client
//! cannot reach the device to forge a transition, because no slot it holds
//! names one.

extern crate alloc;

use boot_contracts::bootstate::{
    BootState, SLOT_BYTES, SelectionError, Slot, empty_state_root, select_bootstate,
};
use boot_contracts::gpt::{self, GptError};
use boot_contracts::object_store::{BlockIo, IoError};
use slime_proto::block::{self, WireBlockReply, WireBlockRequest};
use slime_proto::generation::{self, WireGenerationReply, WireGenerationRequest};

/// The block capability the generation grants this component.
const BLOCK_SLOT: u32 = 1;
/// The preinstalled direct endpoint shared with the client.
const CLIENT_SLOT: u32 = 0;
/// The supervision handle naming the client, minted by init at spawn.
///
/// A native Endpoint reports no peer death — `ERR_PEER_DEAD` is a logical-channel
/// answer the cutover deleted — so the loop below cannot learn its client is
/// gone from the endpoint. Death travels on a supervision capability, which is
/// why this handle is granted at all: it cannot exist before the task it names,
/// so it is a `MintedBinding` rather than a static grant.
const CLIENT_SUPERVISION_SLOT: u32 = 3;

const SECTOR_BYTES: usize = 512;
/// The BootState slots, partition-relative — the same layout the rollback and
/// recovery planes use, because it is the same structure.
const STATE_SLOT_A: u64 = 1024;
const STATE_SLOT_B: u64 = 1025;

/// The generation this plane starts from and the one a client may stage.
///
/// Anything else is outside the closure: `serve` recognizes exactly these two,
/// so staging or inspecting a third is refused before BootState changes, which
/// is M6.5's "fail before BootState changes on missing objects".
const KNOWN_GOOD: [u8; 32] = [0x11; 32];
const CANDIDATE: [u8; 32] = [0x22; 32];
const GENERATION_ROOT: [u8; 32] = [0x44; 32];
const STAGE_ATTEMPTS: u32 = 2;

const STATUS_OK: i32 = 0;
const STATUS_BAD_REQUEST: i32 = -1;
const STATUS_UNKNOWN_GENERATION: i32 = -2;
const STATUS_NO_PENDING: i32 = -3;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-generation-manager] idle without a client\n");
        slime_rt::exit(0);
    }

    let mut io = BlockCapability;
    let Some(partition) = locate_partition(&mut io) else {
        fail(b"partition");
    };
    let slots = StateSlots {
        first_lba: partition.first_lba,
    };

    // Initialize genesis only when neither redundant slot contains a valid
    // BootState. Rewriting slot A on every process start destroys the durable
    // attempt/promotion history the selector and manager share across boots.
    match slots.select(&mut io) {
        Ok(_) => {}
        Err(SelectionError::NoValidBootState) => {
            let genesis = BootState {
                sequence: 1,
                known_good: KNOWN_GOOD,
                pending: None,
                remaining_attempts: 0,
                generation_root: GENERATION_ROOT,
                state_root: empty_state_root(),
                accepted_release_sequence: 1,
            };
            if slots.write(&mut io, Slot::A, &genesis).is_err() {
                fail(b"genesis");
            }
        }
        Err(SelectionError::ConflictingSlots) => fail(b"conflicting bootstate"),
    }
    slime_rt::debug_write(b"[sel4-generation-manager] ready\n");

    // Serve until the client is gone. One client, and a bounded script: the
    // plane's subject is the authority split, not concurrency, and a second
    // client racing on the same BootState would make the sequence
    // nondeterministic without proving anything more.
    loop {
        let mut bytes = [0u8; slime_rt::MAX_MSG];
        let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
        let received = slime_rt::recv(CLIENT_SLOT, &mut bytes, &mut caps);
        match received {
            slime_rt::ERR_WOULDBLOCK => {
                // No request pending. The client is either still working or
                // gone; only its supervision handle can say which.
                if matches!(
                    slime_rt::supervision_status(CLIENT_SUPERVISION_SLOT),
                    Ok(Some(_))
                ) {
                    break;
                }
                slime_rt::yield_now();
                continue;
            }
            result if result < 0 => fail(b"client recv"),
            _ => {}
        }
        if caps.iter().any(|attached| *attached != 0) {
            fail(b"the client attached a capability");
        }
        let reply = serve(&mut io, &slots, &bytes[..received as usize]);
        let encoded = reply.encode();
        loop {
            match slime_rt::send(CLIENT_SLOT, &encoded, &[]) {
                slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
                result if result < 0 => fail(b"client send"),
                _ => break,
            }
        }
    }

    slime_rt::debug_write(b"[sel4-generation-manager] client closed\n");
}

/// One request. Decoding failures and unknown operations are answered, not
/// faulted: a client that sends garbage learns it did, and the manager stays up
/// for the others.
fn serve(io: &mut BlockCapability, slots: &StateSlots, bytes: &[u8]) -> WireGenerationReply {
    let Some(request) = WireGenerationRequest::decode(bytes) else {
        return reply(STATUS_BAD_REQUEST, None, 0);
    };
    if request.magic != generation::GENERATION_MAGIC
        || request.version != generation::FORMAT_VERSION
    {
        return reply(STATUS_BAD_REQUEST, None, 0);
    }
    let identity = identity_bytes(&request);
    let selected = match slots.select(io) {
        Ok(selected) => selected,
        Err(SelectionError::NoValidBootState) => return reply(STATUS_NO_PENDING, None, 0),
        Err(_) => return reply(STATUS_BAD_REQUEST, None, 0),
    };

    match request.op {
        // Read-only. Two generations are known: the current known-good and the
        // candidate a client may stage.
        generation::OP_LIST => {
            report(b"list", &selected.state);
            reply(STATUS_OK, Some(selected.state.known_good), 2)
        }
        generation::OP_INSPECT => {
            if identity != selected.state.known_good && identity != CANDIDATE {
                report(b"inspect-unknown", &selected.state);
                return reply(STATUS_UNKNOWN_GENERATION, None, 0);
            }
            report(b"inspect", &selected.state);
            reply(STATUS_OK, Some(identity), 1)
        }
        // Transitions. Each validates before it writes, so a refusal leaves
        // BootState exactly as it was.
        generation::OP_STAGE => {
            if identity != CANDIDATE {
                // The closure does not contain it. Refused *before* any write,
                // which is the property the gate checks against the image.
                report(b"stage-refused", &selected.state);
                return reply(STATUS_UNKNOWN_GENERATION, None, 0);
            }
            let Ok(staged) = selected.state.stage_pending(
                identity,
                STAGE_ATTEMPTS,
                GENERATION_ROOT,
                empty_state_root(),
            ) else {
                return reply(STATUS_BAD_REQUEST, None, 0);
            };
            let live = slots.commit(io, selected.slot, &staged);
            report(b"stage", &live.state);
            reply(STATUS_OK, Some(identity), 1)
        }
        // SELECT is the health confirmation: promote the pending generation the
        // client names, which must be the one actually staged.
        generation::OP_SELECT => {
            let Ok(promoted) = selected
                .state
                .promote_pending(identity, selected.state.accepted_release_sequence + 1)
            else {
                report(b"select-refused", &selected.state);
                return reply(STATUS_UNKNOWN_GENERATION, None, 0);
            };
            let live = slots.commit(io, selected.slot, &promoted);
            report(b"select", &live.state);
            reply(STATUS_OK, Some(live.state.known_good), 1)
        }
        generation::OP_ROLLBACK => {
            if selected.state.pending.is_none() {
                report(b"rollback-nothing", &selected.state);
                return reply(STATUS_NO_PENDING, Some(selected.state.known_good), 0);
            }
            let Ok(rolled) = selected.state.rollback_pending() else {
                return reply(STATUS_BAD_REQUEST, None, 0);
            };
            let live = slots.commit(io, selected.slot, &rolled);
            report(b"rollback", &live.state);
            reply(STATUS_OK, Some(live.state.known_good), 1)
        }
        _ => reply(STATUS_BAD_REQUEST, None, 0),
    }
}

fn reply(status: i32, identity: Option<[u8; 32]>, count: u32) -> WireGenerationReply {
    let words = identity.map(identity_words).unwrap_or([0; 4]);
    WireGenerationReply {
        magic: generation::GENERATION_MAGIC,
        version: generation::FORMAT_VERSION,
        status,
        flags: 0,
        count,
        generation_number: 0,
        release_sequence: 0,
        remaining_attempts: 0,
        generation0: words[0],
        generation1: words[1],
        generation2: words[2],
        generation3: words[3],
    }
}

/// The two BootState slots and the older-slot-first commit rule.
struct StateSlots {
    first_lba: u64,
}

impl StateSlots {
    fn lba(&self, slot: Slot) -> u64 {
        self.first_lba
            + match slot {
                Slot::A => STATE_SLOT_A,
                Slot::B => STATE_SLOT_B,
            }
    }

    fn select(
        &self,
        io: &mut BlockCapability,
    ) -> Result<boot_contracts::bootstate::SelectedBootState, SelectionError> {
        let mut a = [0u8; SLOT_BYTES];
        let mut b = [0u8; SLOT_BYTES];
        if io.read_sector(self.lba(Slot::A), &mut a).is_err()
            || io.read_sector(self.lba(Slot::B), &mut b).is_err()
        {
            fail(b"slot read");
        }
        select_bootstate(&a, &b)
    }

    fn write(&self, io: &mut BlockCapability, slot: Slot, state: &BootState) -> Result<(), ()> {
        let encoded = state.encode().map_err(|_| ())?;
        io.write_sector(self.lba(slot), &encoded).map_err(|_| ())?;
        io.flush().map_err(|_| ())
    }

    /// Write to the slot that was not selected, then re-select off the device.
    fn commit(
        &self,
        io: &mut BlockCapability,
        selected: Slot,
        state: &BootState,
    ) -> boot_contracts::bootstate::SelectedBootState {
        let target = selected.other();
        if self.write(io, target, state).is_err() {
            fail(b"commit write");
        }
        let live = match self.select(io) {
            Ok(live) => live,
            Err(_) => fail(b"commit select"),
        };
        if live.slot != target || &live.state != state {
            fail(b"the commit is not what a boot would select");
        }
        live
    }
}

/// The device, reached through the granted capability.
struct BlockCapability;

impl BlockIo for BlockCapability {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_READ, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_sector(BLOCK_SLOT, &request.encode(), &mut reply, out);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_WRITE, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_write(BLOCK_SLOT, &request.encode(), data, &mut reply);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let request = block_request(block::OP_FLUSH, 0);
        let mut reply = [0u8; block::REPLY_LEN];
        if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply) < 0 {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

fn locate_partition(io: &mut BlockCapability) -> Option<gpt::Partition> {
    let capacity = device_capacity(io)?;
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected = gpt::validate_store_partition(&mut reader, capacity).ok()?;
    let last = selected.partition.first_lba.checked_add(STATE_SLOT_B)?;
    (last <= selected.partition.last_lba).then_some(selected.partition)
}

/// The device's sector count, measured by binary search over readable LBAs.
fn device_capacity(io: &mut BlockCapability) -> Option<u64> {
    let mut sector = [0u8; SECTOR_BYTES];
    io.read_sector(0, &mut sector).ok()?;
    let mut low = 0u64;
    let mut high = 1u64;
    while io.read_sector(high, &mut sector).is_ok() {
        low = high;
        high = high.checked_mul(2)?;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if io.read_sector(middle, &mut sector).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(low + 1)
}

fn block_request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count: if op == block::OP_FLUSH { 0 } else { 1 },
        buffer_phys: 0,
        buffer_pages: 0,
    }
}

fn decode_block_reply(bytes: &[u8; block::REPLY_LEN]) -> WireBlockReply {
    WireBlockReply::decode(bytes).unwrap_or(WireBlockReply {
        magic: 0,
        version: 0,
        status: -1,
        sectors_done: 0,
    })
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

fn identity_bytes(request: &WireGenerationRequest) -> [u8; 32] {
    let mut identity = [0u8; 32];
    for (index, word) in [
        request.generation0,
        request.generation1,
        request.generation2,
        request.generation3,
    ]
    .iter()
    .enumerate()
    {
        identity[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    identity
}

/// One served operation, as the gate reads it: which operation, and the
/// BootState it left behind.
fn report(op: &[u8], state: &BootState) {
    let mut line = [0u8; 160];
    let mut len = 0;
    len += copy(&mut line[len..], b"[sel4-generation-manager] ");
    len += copy(&mut line[len..], op);
    len += copy(&mut line[len..], b" seq=");
    len += copy(&mut line[len..], &decimal(state.sequence));
    len += copy(&mut line[len..], b" pending=");
    len += copy(
        &mut line[len..],
        if state.pending.is_some() { b"1" } else { b"0" },
    );
    len += copy(&mut line[len..], b" attempts=");
    len += copy(&mut line[len..], &decimal(state.remaining_attempts as u64));
    len += copy(&mut line[len..], b" release=");
    len += copy(&mut line[len..], &decimal(state.accepted_release_sequence));
    len += copy(&mut line[len..], b"\n");
    slime_rt::debug_write(&line[..len]);
}

fn copy(out: &mut [u8], source: &[u8]) -> usize {
    let len = source.len().min(out.len());
    out[..len].copy_from_slice(&source[..len]);
    len
}

struct Decimal {
    bytes: [u8; 20],
    start: usize,
}

impl core::ops::Deref for Decimal {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
}

fn decimal(mut value: u64) -> Decimal {
    let mut bytes = [0u8; 20];
    let mut start = bytes.len();
    loop {
        start -= 1;
        bytes[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    Decimal { bytes, start }
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-generation-manager] fail: ");
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
const RUN_TOKEN_SLOT: u32 = 2;
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
