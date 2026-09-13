#![no_std]
#![no_main]

//! The seL4 recovery plane's subject: M5.9 in userspace (P5.4.2c).
//!
//! Recovery is what happens when both BootState slots are gone: there is no
//! root to boot, and one must be reconstructed from a signed index rather than
//! guessed. M5.9's exit condition is that reconstruction produces a *verified*
//! bootable root **without modifying any device not named by an explicit
//! capability**, so this component proves two things at once — that it can
//! rebuild, and that it cannot reach past what it was granted.
//!
//! The sequence:
//!
//! * both BootState slots are corrupt, so selection refuses — nothing is
//!   executed on an unverified root;
//! * the recovery index decodes: bounds, ascending binding order, and a
//!   content-addressed state root over every binding;
//! * every state object the index names is retrieved from the object store and
//!   its payload re-hashed, so a closure with a missing or corrupted object
//!   fails before anything is written;
//! * the reconstructed BootState is written to both slots at sequences 1 and 2,
//!   each flushed, so an interruption after the first still leaves one verified
//!   root;
//! * the result is re-selected off the device and must be the index's target.
//!
//! Then the negative arm, which is the one M5.9 actually names: this component
//! reaches two disks through the userspace block driver over IO0 rings (B84).
//! One ring is read-write on the recovery disk; the other is read-only on the
//! guard disk. Reading through the guard ring proves it reaches the attached
//! disk; a write on that ring is refused with `STATUS_BAD_RIGHTS` by the
//! driver, which checks the generation-declared ring-authority table rather
//! than any per-caller capability. The gate is the declared ring, not the
//! client's good behaviour.
//!
//! Kernel-resident in the oracle: `recovery::reconstruct` does all of the above
//! behind a syscall gated on `GenerationControl` plus a selected block
//! capability. Here the ring's declared authority is the gate.

extern crate alloc;

use boot_contracts::bootstate::{
    BootState, SLOT_BYTES, SelectionError, Slot, empty_state_root, select_bootstate,
};
use boot_contracts::gpt::{self, GptError};
use boot_contracts::object_store::{BlockIo as ObjectBlockIo, IoError, ObjectStore};
use boot_contracts::recovery::RecoveryIndex;
use slime_components::block_io::{BlockError, BlockIo};
use slime_proto::io_queue;

const FACTORY_SLOT: u32 = 3;
const PRIMARY_PEER_SLOT: u32 = 8;
const GUARD_PEER_SLOT: u32 = 9;
const PRIMARY_RING_BASE: u64 = 0x0000_001f_0000_0000;
const PRIMARY_DATA_BASE: u64 = 0x0000_001f_0001_0000;
const GUARD_RING_BASE: u64 = 0x0000_001f_0010_0000;
const GUARD_DATA_BASE: u64 = 0x0000_001f_0011_0000;

const SECTOR_BYTES: usize = 512;
const GUARD_SIGNATURE: &[u8; 8] = b"GUARDDSK";

/// The two BootState slots, partition-relative — the same layout the rollback
/// plane uses, because it is the same on-disk structure.
const STATE_SLOT_A: u64 = 1024;
const STATE_SLOT_B: u64 = 1025;
/// Where the fixture writes the recovery index, above the BootState slots.
const INDEX_LBA: u64 = 1026;
const INDEX_SECTORS: u64 = 4;
/// Where the index records its own total length
/// (`RECOVERY_INDEX_TOTAL_LEN_OFFSET` in the generated layout).
const INDEX_BYTES_OFFSET: usize = 136;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-recovery-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    let primary_request = binding(b"notification:recovery-primary-request-ready+signal");
    let primary_completion = binding(b"notification:recovery-primary-completion-ready+wait");
    let guard_request = binding(b"notification:recovery-guard-request-ready+signal");
    let guard_completion = binding(b"notification:recovery-guard-completion-ready+wait");
    // SAFETY: all four bases are page-aligned, pairwise disjoint addresses in
    // this component's free VSpace, and nothing else maps them.
    let primary = unsafe {
        BlockIo::attach(
            FACTORY_SLOT,
            PRIMARY_PEER_SLOT,
            primary_request,
            primary_completion,
            PRIMARY_RING_BASE,
            PRIMARY_DATA_BASE,
        )
    }
    .unwrap_or_else(|_| fail(b"primary block attach"));
    // SAFETY: covered by the disjoint mapping invariant above.
    let guard = unsafe {
        BlockIo::attach(
            FACTORY_SLOT,
            GUARD_PEER_SLOT,
            guard_request,
            guard_completion,
            GUARD_RING_BASE,
            GUARD_DATA_BASE,
        )
    }
    .unwrap_or_else(|_| fail(b"guard block attach"));
    let mut io = BlockCapability { driver: primary };
    let mut guard = BlockCapability { driver: guard };
    let partition = match locate_partition(&mut io) {
        Some(partition) => partition,
        None => fail(b"partition"),
    };
    let first = partition.first_lba;

    // No bootable root. Both slots are corrupt, so there is nothing to select
    // and nothing may be executed — recovery exists for exactly this state.
    let a = read_slot(&mut io, first + STATE_SLOT_A);
    let b = read_slot(&mut io, first + STATE_SLOT_B);
    match select_bootstate(&a, &b) {
        Err(SelectionError::NoValidBootState) => {}
        _ => fail(b"a corrupt pair produced a root"),
    }
    slime_rt::debug_write(b"[sel4-recovery-probe] dual corruption refused\n");

    // The signed recovery index: bounds, ascending binding order, and a state
    // root over every binding. A malformed index fails here, before any read
    // it describes and long before any write.
    let mut index_bytes = alloc::vec![0u8; (INDEX_SECTORS as usize) * SECTOR_BYTES];
    for sector in 0..INDEX_SECTORS {
        let start = (sector as usize) * SECTOR_BYTES;
        let chunk: &mut [u8; SECTOR_BYTES] = (&mut index_bytes[start..start + SECTOR_BYTES])
            .try_into()
            .expect("sector-sized");
        if io.read_sector(first + INDEX_LBA + sector, chunk).is_err() {
            fail(b"index read");
        }
    }
    // Trimmed to its declared length before decoding. The index is read in
    // whole sectors and the decoder bounds on exact size, so the zero padding
    // that follows the record would otherwise read as a truncated one — the
    // declared `index_bytes` field is what says where it ends.
    let declared = u32::from_le_bytes([
        index_bytes[INDEX_BYTES_OFFSET],
        index_bytes[INDEX_BYTES_OFFSET + 1],
        index_bytes[INDEX_BYTES_OFFSET + 2],
        index_bytes[INDEX_BYTES_OFFSET + 3],
    ]) as usize;
    if declared > index_bytes.len() {
        fail(b"index length past the sectors it was read from");
    }
    let index = match RecoveryIndex::decode(&index_bytes[..declared]) {
        Ok(index) => index,
        Err(_) => fail(b"index decode"),
    };
    write_pair(
        b"[sel4-recovery-probe] index states=",
        index.state_count() as u64,
        b" release=",
        index.accepted_release_sequence,
    );

    // Every state object the index names, retrieved from the store and
    // re-hashed. `ObjectStore::get` verifies the complete payload against the
    // record's content hash before returning it, so a corrupted object is a
    // failure rather than a reconstruction built on bad bytes.
    let store = match ObjectStore::open(&mut io, &partition) {
        Ok(store) => store,
        Err(_) => fail(b"store open"),
    };
    for position in 0..index.state_count() {
        let Some(entry) = index.state(position) else {
            fail(b"index state entry");
        };
        let Some((_, payload_len)) = store.stat(&entry.object_identity) else {
            fail(b"a state object the index names is absent");
        };
        let mut payload = alloc::vec![0u8; payload_len as usize];
        if store
            .get(&mut io, &entry.object_identity, &mut payload)
            .is_err()
        {
            fail(b"a state object failed verification");
        }
    }
    write_pair(
        b"[sel4-recovery-probe] closure verified objects=",
        index.state_count() as u64,
        b" of=",
        index.state_count() as u64,
    );

    reconstruct(&mut io, &partition, first, &index, b"reconstruction");

    // Re-selected off the device: the root a fresh boot would pick must be the
    // index's target, at the higher of the two sequences.
    let a = read_slot(&mut io, first + STATE_SLOT_A);
    let b = read_slot(&mut io, first + STATE_SLOT_B);
    let selected = match select_bootstate(&a, &b) {
        Ok(selected) => selected,
        Err(_) => fail(b"reconstructed selection"),
    };
    if selected.state.known_good != index.target_generation
        || selected.state.pending.is_some()
        || selected.state.accepted_release_sequence != index.accepted_release_sequence
    {
        fail(b"reconstructed root");
    }
    write_pair(
        b"[sel4-recovery-probe] reconstructed seq=",
        selected.state.sequence,
        b" release=",
        selected.state.accepted_release_sequence,
    );

    // Both slots decode, so the reconstruction left redundancy rather than a
    // single root.
    if BootState::decode(&a).is_err() || BootState::decode(&b).is_err() {
        fail(b"a reconstructed slot does not decode");
    }
    slime_rt::debug_write(b"[sel4-recovery-probe] both slots decode\n");

    // Idempotent: re-run the complete algorithm from the durable index and the
    // disk state it just produced, rather than rewriting copied expected bytes.
    let durable_bytes = read_index(&mut io, first);
    let durable_index =
        RecoveryIndex::decode(&durable_bytes).unwrap_or_else(|_| fail(b"durable index decode"));
    reconstruct(
        &mut io,
        &partition,
        first,
        &durable_index,
        b"repeat reconstruction",
    );
    let repeated_a = read_slot(&mut io, first + STATE_SLOT_A);
    let repeated_b = read_slot(&mut io, first + STATE_SLOT_B);
    let repeated = select_bootstate(&repeated_a, &repeated_b)
        .unwrap_or_else(|_| fail(b"repeat reconstructed selection"));
    if repeated.state != selected.state || repeated_a != a || repeated_b != b {
        fail(b"reconstruction is not idempotent");
    }
    slime_rt::debug_write(b"[sel4-recovery-probe] recovery rerun from durable index converged\n");

    // The negative arm crosses the guard driver's real IO0 ring twice. A
    // successful read proves the second attached disk is reachable. The write
    // must then be refused specifically with STATUS_BAD_RIGHTS by that driver's
    // generation-declared ring-authority row, not by an absent endpoint or a
    // device error.
    let mut guard_sector = [0u8; SECTOR_BYTES];
    if guard.read_sector(0, &mut guard_sector).is_err()
        || &guard_sector[..GUARD_SIGNATURE.len()] != GUARD_SIGNATURE
    {
        fail(b"guard disk ring did not return the attached disk signature");
    }
    match guard.driver.write(0, &guard_sector) {
        Err(BlockError::Refused { status, .. }) if status == io_queue::STATUS_BAD_RIGHTS => {}
        _ => fail(b"guard disk write was not refused by driver rights"),
    }
    slime_rt::debug_write(b"[sel4-recovery-probe] guard write refused by driver rights\n");
    slime_rt::debug_write(b"[sel4-recovery-probe] reachable guard disk write refused\n");

    guard.shutdown();
    io.shutdown();

    slime_rt::debug_write(b"[sel4-recovery-probe] recovery plane complete\n");
}

fn read_index(io: &mut BlockCapability, first: u64) -> alloc::vec::Vec<u8> {
    let mut bytes = alloc::vec![0u8; (INDEX_SECTORS as usize) * SECTOR_BYTES];
    for sector in 0..INDEX_SECTORS {
        let start = sector as usize * SECTOR_BYTES;
        // `&mut [u8]` -> `&mut [u8; N]`, so the borrow has to be mutable
        // before `try_into` can reach that impl.
        let chunk: &mut [u8; SECTOR_BYTES] = (&mut bytes[start..start + SECTOR_BYTES])
            .try_into()
            .expect("sector-sized");
        io.read_sector(first + INDEX_LBA + sector, chunk)
            .unwrap_or_else(|_| fail(b"durable index read"));
    }
    let declared = u32::from_le_bytes(
        bytes[INDEX_BYTES_OFFSET..INDEX_BYTES_OFFSET + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    if declared > bytes.len() {
        fail(b"durable index length");
    }
    bytes.truncate(declared);
    bytes
}

fn reconstruct(
    io: &mut BlockCapability,
    partition: &gpt::Partition,
    first: u64,
    index: &RecoveryIndex<'_>,
    context: &[u8],
) {
    let store = ObjectStore::open(io, partition).unwrap_or_else(|_| fail(context));
    for position in 0..index.state_count() {
        let entry = index.state(position).unwrap_or_else(|| fail(context));
        let (_, payload_len) = store
            .stat(&entry.object_identity)
            .unwrap_or_else(|| fail(context));
        let mut payload = alloc::vec![0u8; payload_len as usize];
        store
            .get(io, &entry.object_identity, &mut payload)
            .unwrap_or_else(|_| fail(context));
    }
    drop(store);
    for (slot, sequence) in [(Slot::A, 1u64), (Slot::B, 2)] {
        let state = BootState {
            sequence,
            known_good: index.target_generation,
            pending: None,
            remaining_attempts: 0,
            generation_root: index.generation_root,
            state_root: if index.state_count() == 0 {
                empty_state_root()
            } else {
                index.state_root
            },
            accepted_release_sequence: index.accepted_release_sequence,
        };
        let encoded = state.encode().unwrap_or_else(|_| fail(context));
        let lba = first
            + if slot == Slot::A {
                STATE_SLOT_A
            } else {
                STATE_SLOT_B
            };
        io.write_sector(lba, &encoded)
            .unwrap_or_else(|_| fail(context));
        io.flush().unwrap_or_else(|_| fail(context));
    }
}

fn read_slot(io: &mut BlockCapability, lba: u64) -> [u8; SLOT_BYTES] {
    let mut sector = [0u8; SECTOR_BYTES];
    if io.read_sector(lba, &mut sector).is_err() {
        fail(b"slot read");
    }
    sector
}

/// The primary or guard device, reached through a userspace driver's IO0 ring.
struct BlockCapability {
    driver: BlockIo<'static>,
}

impl BlockCapability {
    fn shutdown(&mut self) {
        self.driver
            .shutdown()
            .unwrap_or_else(|_| fail(b"driver shutdown"));
    }
}

impl ObjectBlockIo for BlockCapability {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let reply = self.driver.read(lba, out).map_err(|_| IoError::Device)?;
        if reply.sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let reply = self.driver.write(lba, data).map_err(|_| IoError::Device)?;
        if reply.sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.driver.flush().map(|_| ()).map_err(|_| IoError::Device)
    }
}

fn locate_partition(io: &mut BlockCapability) -> Option<gpt::Partition> {
    let capacity = io.driver.capacity();
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected = gpt::validate_store_partition(&mut reader, capacity).ok()?;
    let last = selected
        .partition
        .first_lba
        .checked_add(INDEX_LBA + INDEX_SECTORS)?;
    (last <= selected.partition.last_lba).then_some(selected.partition)
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}

fn write_pair(prefix: &[u8], first: u64, middle: &[u8], second: u64) {
    let mut line = [0u8; 128];
    let mut len = 0;
    len += copy(&mut line[len..], prefix);
    len += copy(&mut line[len..], &decimal(first));
    len += copy(&mut line[len..], middle);
    len += copy(&mut line[len..], &decimal(second));
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
    slime_rt::debug_write(b"[sel4-recovery-probe] fail: ");
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
