#![no_std]
#![no_main]

//! The seL4 store plane's subject: M5.4 policy in userspace (P5.4.2c).
//!
//! The oracle put GPT validation, root selection, the object index, content
//! hashing, and commit ordering *inside the kernel* — `store_service` owns a
//! global store and `sys_store_transact` is syscall 7. That placement is what
//! this port does not reproduce. The root here mediates one thing, sectors, and
//! everything above them is this component:
//!
//! * validate the protective MBR, both GPT copies, and the entry-array CRCs,
//!   and select the store partition;
//! * pick the newest valid superblock root, tolerating a damaged slot;
//! * index the committed records and retrieve one by content hash, verifying
//!   its complete SHA-256 before the bytes are used;
//! * append and seal a new object, then prove the commit is durable by
//!   re-opening the store from disk;
//! * refuse a payload whose hash does not match what was asked for.
//!
//! None of that is root code. `boot_contracts::{gpt, object_store}` is the same
//! implementation the oracle links, driven here over `BlockTransact` through a
//! capability the generation granted — which is the whole claim: M5.4's
//! properties do not need kernel residence, they need a block device and an
//! allocator.
//!
//! This is the first component that allocates: `ObjectStore` reads a GPT entry
//! table and builds an object index in `Vec`s. The heap comes from the
//! runtime's `heap` feature, which the store-plane build enables; the last line
//! this component prints is its own footprint against that bound.

extern crate alloc;

use boot_contracts::gpt::{self, GptError, Recovery};
use boot_contracts::object_store::{BlockIo, IoError, ObjectStore, StoreError};
use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

/// The block capability, numbered as in `sel4-storage-probe`: the generation
/// grants it to this component, and the root places it above the component's
/// executables — of which it has none.
const BLOCK_SLOT: u32 = 1;
/// The endpoint `init` grants only to the instance it spawns.
///
/// The root launches every declared component (P5.2), so a second, unconfigured
/// copy of this one also runs holding the same device capability. Authority
/// cannot separate them — a generation grant names a component, not a task —
/// so the spawned instance is the one holding this.
const RUN_TOKEN_SLOT: u32 = 0;

const SECTOR_BYTES: usize = 512;

/// What the fixture seeds at object type 1, and what the retrieval arm must get
/// back. Kept in step with `scripts/build/build-store-fixture.py`.
const SEEDED_TYPE: u32 = 1;
const SEEDED_LEN: usize = 512;
const SEEDED_MESSAGE: &[u8] = b"Slime OS M5.4 object store fixture\n";

/// The object this component appends. Distinct content, so its hash is a new
/// identity rather than a deduplicated hit on the seeded one.
const APPENDED_TYPE: u32 = 7;
const APPENDED_PAYLOAD: &[u8] = b"seL4 store plane appended object\n";

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-store-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    let mut io = BlockCapability;

    // GPT: protective MBR, both header copies, entry-array CRCs, bounds, and
    // overlap — then the store partition selected by type GUID. A conflicting
    // pair of valid copies is a hard reject rather than a false recovery, which
    // is the property the `gpt-conflict` fixture exercises.
    let capacity = match device_capacity(&mut io) {
        Some(sectors) => sectors,
        None => fail(b"device capacity"),
    };
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected = match gpt::validate_store_partition(&mut reader, capacity) {
        Ok(selected) => selected,
        Err(error) => {
            // A refusal is a result, not a crash. Three fixtures are *supposed*
            // to land here — conflicting GPT copies, and anything that leaves
            // no valid copy — so the class is reported and the component exits
            // cleanly. The gate decides which class each fixture must produce;
            // a component that treated every refusal as failure could not tell
            // "rejected correctly" from "broke".
            report_gpt(error);
            slime_rt::debug_write(b"[sel4-store-probe] store plane refused\n");
            slime_rt::exit(0);
        }
    };
    let recovery = match selected.recovery {
        Recovery::None => "none",
        Recovery::BackupDamaged(error) => {
            report_gpt(error);
            "backup-damaged"
        }
        Recovery::PrimaryDamaged(error) => {
            report_gpt(error);
            "primary-damaged"
        }
    };
    write_line(
        b"[sel4-store-probe] partition first=",
        selected.partition.first_lba,
        b" last=",
        selected.partition.last_lba,
        b" recovery=",
        recovery.as_bytes(),
    );

    // Open: the newest valid superblock wins, a single damaged slot is
    // tolerated, and the record area is indexed within the committed bound. A
    // truncated or malformed record fails here rather than producing an
    // out-of-bounds read later.
    let mut store = match ObjectStore::open(&mut io, &selected.partition) {
        Ok(store) => store,
        Err(error) => {
            // Likewise. `superblock-both-damaged` must reach exactly here with
            // `no-valid-superblock`: neither root decodes, so the store fails
            // closed rather than inventing one.
            report_store(error);
            slime_rt::debug_write(b"[sel4-store-probe] store plane refused\n");
            slime_rt::exit(0);
        }
    };
    write_pair(
        b"[sel4-store-probe] opened seq=",
        store.sequence(),
        b" objects=",
        store.object_count() as u64,
    );

    // Retrieval by content: the seeded object's hash is computed here from the
    // payload the fixture wrote, so a store returning anything else — or
    // returning the right length with wrong bytes — fails the comparison.
    // An uncommitted record past the committed append point must be invisible.
    // The `interrupted-append` fixture writes a valid-magic, truncated record
    // there; the root's `append_lba` excludes it, so the index must not carry
    // it and the append below must overwrite it rather than skip past.
    let indexed = store.object_count();
    let expected = seeded_payload();
    let hash = slime_rt::sha256(&expected);
    let Some((obj_type, payload_len)) = store.stat(&hash) else {
        fail(b"seeded object absent");
    };
    if obj_type != SEEDED_TYPE || payload_len as usize != SEEDED_LEN {
        fail(b"seeded object shape");
    }
    let mut retrieved = alloc::vec![0u8; SEEDED_LEN];
    match store.get(&mut io, &hash, &mut retrieved) {
        Ok((got_type, got_len)) => {
            if got_type != SEEDED_TYPE || got_len != SEEDED_LEN || retrieved != expected {
                fail(b"seeded object payload");
            }
        }
        Err(error) => {
            report_store(error);
            fail(b"seeded object get");
        }
    }
    if indexed != 1 {
        fail(b"index counted an uncommitted record");
    }
    slime_rt::debug_write(b"[sel4-store-probe] seeded object verified\n");

    // A hash naming no object. `NotFound` rather than a partial read, and it
    // must not disturb the store.
    let absent = slime_rt::sha256(b"no object has this content");
    if store.stat(&absent).is_some() {
        fail(b"absent object present");
    }
    let mut discard = [0u8; 32];
    if store.get(&mut io, &absent, &mut discard) != Err(StoreError::NotFound) {
        fail(b"absent object not refused");
    }
    slime_rt::debug_write(b"[sel4-store-probe] unknown hash refused\n");

    // Append and seal. Record sectors, flush, then the *older* superblock slot,
    // flush — so an interruption anywhere leaves the previous root committed.
    let sequence_before = store.sequence();
    let appended = match store.put(&mut io, APPENDED_TYPE, APPENDED_PAYLOAD) {
        Ok(hash) => hash,
        Err(error) => {
            report_store(error);
            fail(b"append");
        }
    };
    if appended != slime_rt::sha256(APPENDED_PAYLOAD) {
        fail(b"append identity");
    }
    if store.sequence() != sequence_before + 1 {
        fail(b"append sequence");
    }
    write_pair(
        b"[sel4-store-probe] appended seq=",
        store.sequence(),
        b" objects=",
        store.object_count() as u64,
    );

    // Identical content again: an idempotent no-op returning the same identity,
    // not a second record. The object count proves nothing was appended.
    let count_before = store.object_count();
    match store.put(&mut io, APPENDED_TYPE, APPENDED_PAYLOAD) {
        Ok(hash) if hash == appended => {}
        _ => fail(b"duplicate not idempotent"),
    }
    if store.object_count() != count_before {
        fail(b"duplicate appended a record");
    }
    slime_rt::debug_write(b"[sel4-store-probe] duplicate content deduplicated\n");

    // Re-open from disk. This is what makes the append *durable* rather than
    // merely in-memory: a fresh open re-reads the superblocks, picks the newest
    // root, re-indexes the records, and must find the object committed above.
    let mut reopened = match ObjectStore::open(&mut io, &selected.partition) {
        Ok(store) => store,
        Err(error) => {
            report_store(error);
            fail(b"reopen");
        }
    };
    if reopened.sequence() != store.sequence() || reopened.object_count() != store.object_count() {
        fail(b"reopen root");
    }
    let mut round_trip = alloc::vec![0u8; APPENDED_PAYLOAD.len()];
    match reopened.get(&mut io, &appended, &mut round_trip) {
        Ok((got_type, got_len)) => {
            if got_type != APPENDED_TYPE
                || got_len != APPENDED_PAYLOAD.len()
                || round_trip != APPENDED_PAYLOAD
            {
                fail(b"reopen payload");
            }
        }
        Err(error) => {
            report_store(error);
            fail(b"reopen get");
        }
    }
    write_pair(
        b"[sel4-store-probe] reopened seq=",
        reopened.sequence(),
        b" objects=",
        reopened.object_count() as u64,
    );

    // Every committed payload re-hashed against its record. Opening validated
    // bounds; scrub is what proves integrity for objects nobody asked for.
    if let Err(error) = reopened.scrub(&mut io) {
        report_store(error);
        fail(b"scrub");
    }
    slime_rt::debug_write(b"[sel4-store-probe] scrub verified every object\n");

    // A payload larger than the format admits, refused before any device write.
    let oversized = alloc::vec![0u8; boot_contracts::object_store::MAX_OBJECT_PAYLOAD + 1];
    if reopened.put(&mut io, APPENDED_TYPE, &oversized) != Err(StoreError::PayloadTooLarge) {
        fail(b"oversized accepted");
    }
    slime_rt::debug_write(b"[sel4-store-probe] oversized payload refused\n");

    write_pair(
        b"[sel4-store-probe] heap used=",
        slime_rt::heap_used() as u64,
        b" capacity=",
        slime_rt::HEAP_BYTES as u64,
    );
    slime_rt::debug_write(b"[sel4-store-probe] store plane complete\n");
}

/// The device, reached through the granted capability.
///
/// This is the whole adapter: `BlockIo` is three methods, and each is one
/// `BlockTransact`. The oracle satisfies the same trait with a direct driver
/// handle inside the kernel; satisfying it from userspace over a mediated
/// capability is what moves the policy above without changing it.
struct BlockCapability;

impl BlockIo for BlockCapability {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = request(block::OP_READ, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_sector(BLOCK_SLOT, &request.encode(), &mut reply, out);
        if status < 0 || decode_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = request(block::OP_WRITE, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_write(BLOCK_SLOT, &request.encode(), data, &mut reply);
        if status < 0 || decode_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let request = request(block::OP_FLUSH, 0);
        let mut reply = [0u8; block::REPLY_LEN];
        if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply) < 0 {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

/// The device's sector count, needed by GPT validation to bound the backup
/// header and to reject an entry array that runs off the end.
///
/// Measured rather than declared. The block protocol's reply carries status and
/// a completion count, not geometry, and adding a capacity field would change a
/// record the frozen oracle also encodes. What the protocol *does* expose is the
/// bound itself: the root refuses `lba >= capacity`, so the largest readable LBA
/// is the last sector, and a binary search over the 64-bit space finds it in
/// bounded probes. Reads only, so measuring cannot damage the disk.
fn device_capacity(io: &mut BlockCapability) -> Option<u64> {
    let mut sector = [0u8; SECTOR_BYTES];
    if io.read_sector(0, &mut sector).is_err() {
        return None;
    }
    // Grow until a read is refused, so the search starts with a known-bad
    // upper bound instead of assuming a device size.
    let mut low = 0u64;
    let mut high = 1u64;
    while io.read_sector(high, &mut sector).is_ok() {
        low = high;
        high = high.checked_mul(2)?;
    }
    // `low` reads, `high` does not. Converge on the boundary between them.
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

fn request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count: if op == block::OP_FLUSH { 0 } else { 1 },
        // Zero, deliberately: there is no ambient addressing here. The oracle's
        // kernel dereferences this pointer on the caller's behalf; the payload
        // crosses in this component's own transfer window instead.
        buffer_phys: 0,
        buffer_pages: 0,
    }
}

fn decode_reply(bytes: &[u8; block::REPLY_LEN]) -> WireBlockReply {
    WireBlockReply::decode(bytes).unwrap_or(WireBlockReply {
        magic: 0,
        version: 0,
        status: -1,
        sectors_done: 0,
    })
}

/// Whether this task is the one `init` spawned. The same `ERR_BAD_CAP`
/// authority probe `sel4-storage-probe` and `fabric-call-time` use.
fn spawned_instance() -> bool {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    slime_rt::recv(RUN_TOKEN_SLOT, &mut bytes, &mut caps) != slime_rt::ERR_BAD_CAP
}

/// The payload the fixture seeds: a fixed-length record whose head is a known
/// message, so the retrieval arm compares against bytes rather than a length.
fn seeded_payload() -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec![0u8; SEEDED_LEN];
    data[..SEEDED_MESSAGE.len()].copy_from_slice(SEEDED_MESSAGE);
    for (index, byte) in data.iter_mut().enumerate().skip(SEEDED_MESSAGE.len()) {
        *byte = (index * 37 + 11) as u8;
    }
    data
}

fn report_gpt(error: GptError) {
    let name: &[u8] = match error {
        GptError::Device => b"device",
        GptError::ProtectiveMbr => b"protective-mbr",
        GptError::BadMagic => b"bad-magic",
        GptError::UnsupportedVersion => b"unsupported-version",
        GptError::BadHeaderSize => b"bad-header-size",
        GptError::BadHeaderCrc => b"bad-header-crc",
        GptError::BadEntriesCrc => b"bad-entries-crc",
        GptError::OutOfBounds => b"out-of-bounds",
        GptError::Overflow => b"overflow",
        GptError::Overlap => b"overlap",
        GptError::NoValidCopy => b"no-valid-copy",
        GptError::ConflictingCopies => b"conflicting-copies",
        GptError::NoStorePartition => b"no-store-partition",
        GptError::AmbiguousStorePartition => b"ambiguous-store-partition",
    };
    slime_rt::debug_write(b"[sel4-store-probe] gpt error=");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"\n");
}

fn report_store(error: StoreError) {
    let name: &[u8] = match error {
        StoreError::Io(_) => b"io",
        StoreError::PartitionTooSmall => b"partition-too-small",
        StoreError::NoValidSuperblock => b"no-valid-superblock",
        StoreError::CorruptRecord => b"corrupt-record",
        StoreError::TooManyObjects => b"too-many-objects",
        StoreError::StoreFull => b"store-full",
        StoreError::NotFound => b"not-found",
        StoreError::PayloadTooLarge => b"payload-too-large",
        StoreError::BufferTooSmall => b"buffer-too-small",
        StoreError::DuplicateIdentity => b"duplicate-identity",
        StoreError::HashMismatch => b"hash-mismatch",
    };
    slime_rt::debug_write(b"[sel4-store-probe] store error=");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"\n");
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

fn write_line(prefix: &[u8], first: u64, middle: &[u8], second: u64, tail: &[u8], name: &[u8]) {
    let mut line = [0u8; 160];
    let mut len = 0;
    len += copy(&mut line[len..], prefix);
    len += copy(&mut line[len..], &decimal(first));
    len += copy(&mut line[len..], middle);
    len += copy(&mut line[len..], &decimal(second));
    len += copy(&mut line[len..], tail);
    len += copy(&mut line[len..], name);
    len += copy(&mut line[len..], b"\n");
    slime_rt::debug_write(&line[..len]);
}

fn copy(out: &mut [u8], source: &[u8]) -> usize {
    let len = source.len().min(out.len());
    out[..len].copy_from_slice(&source[..len]);
    len
}

/// A decimal rendering with no allocator dependency, returned by value so the
/// callers above can concatenate without a formatter.
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
    slime_rt::debug_write(b"[sel4-store-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
