#![no_std]
#![no_main]

//! The seL4 transfer plane's subject: M6.7 in userspace (P5.4.3).
//!
//! A generation crosses a persistence boundary. The manifest sits on a *source*
//! device this component may only read; the receiving BootState lives on a
//! *receiver* device it may write. Two capabilities, two rights masks, and the
//! separation between them is the milestone:
//!
//! * the source device refuses a write, checked first — every later claim about
//!   it being untouched rests on that;
//! * the manifest decodes, which validates its bounds, its ordering, and a
//!   self-excluding SHA-256 over the whole record;
//! * a tampered byte anywhere fails that digest, so an altered manifest cannot
//!   install;
//! * its **closure** is verified — every object it carries re-hashes to the
//!   identity it declares — before any BootState write, so an incomplete
//!   transfer costs the receiver nothing and consumes no attempt;
//! * state travels by declared policy, so nothing the source marked as not
//!   travelling is shipped;
//! * the generation stages **pending**, leaving the existing known-good root
//!   intact, and only health confirmation promotes it.
//!
//! The oracle does this in `kernel/src/storage/transfer.rs` behind a syscall
//! requiring both devices by capability. Here the two capabilities are the
//! whole gate: there is no operation this component can phrase that writes the
//! source.

extern crate alloc;

use boot_contracts::bootstate::{
    BootState, SLOT_BYTES, SelectionError, Slot, empty_state_root, select_bootstate,
};
use boot_contracts::gpt::{self, GptError};
use boot_contracts::object_store::{BlockIo, IoError};
use boot_contracts::recovery::binding_identity;
use boot_contracts::transfer::{
    self, STATE_FLAG_READ_ONLY, STATE_FLAG_TRAVEL, TransferError, TransferManifest,
};
use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

/// The endpoint `init` grants only to the instance it spawns.
const RUN_TOKEN_SLOT: u32 = 0;
/// The receiver device: read and write.
const RECEIVER_SLOT: u32 = 1;
/// The source device: read only. A write through it is refused by rights, which
/// is what makes "leave every ungranted device byte-identical" a property of
/// the capability rather than of this component's restraint.
const SOURCE_SLOT: u32 = 2;

const SECTOR_BYTES: usize = 512;

/// Where the source carries the transfer manifest, partition-relative.
const MANIFEST_LBA: u64 = 1030;
const MANIFEST_SECTORS: u64 = 16;

/// The receiver's BootState slots, matching every other plane's layout.
const STATE_SLOT_A: u64 = 1024;
const STATE_SLOT_B: u64 = 1025;

/// The receiver's existing known-good root, which the transfer must not disturb
/// until health is confirmed.
const RECEIVER_KNOWN_GOOD: [u8; 32] = [0x11; 32];
const RECEIVER_GENERATION_ROOT: [u8; 32] = [0x44; 32];

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if !spawned_instance() {
        slime_rt::debug_write(b"[sel4-transfer-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    let mut receiver = Device {
        slot: RECEIVER_SLOT,
    };
    let mut source = Device { slot: SOURCE_SLOT };

    // Read-only, checked before anything else.
    let mut probe = [0u8; SECTOR_BYTES];
    if source.read_sector(0, &mut probe).is_err() {
        fail(b"source read");
    }
    if source.write_sector(0, &probe).is_ok() {
        fail(b"the source device accepted a write");
    }
    slime_rt::debug_write(b"[sel4-transfer-probe] the source device refuses writes\n");

    // The receiver's existing root: a known-good generation with nothing
    // pending. Written here because the plane owns its own starting state.
    let Some(receiver_partition) = locate_partition(&mut receiver) else {
        fail(b"receiver partition");
    };
    let slots = StateSlots {
        first_lba: receiver_partition.first_lba,
    };
    let genesis = BootState {
        sequence: 1,
        known_good: RECEIVER_KNOWN_GOOD,
        pending: None,
        remaining_attempts: 0,
        generation_root: RECEIVER_GENERATION_ROOT,
        state_root: empty_state_root(),
        accepted_release_sequence: 1,
    };
    if slots.write(&mut receiver, Slot::A, &genesis).is_err() {
        fail(b"receiver genesis");
    }
    slime_rt::debug_write(b"[sel4-transfer-probe] receiver holds a known-good root\n");

    // Read the manifest off the source.
    let Some(source_partition) = locate_partition(&mut source) else {
        fail(b"source partition");
    };
    let mut bytes = alloc::vec![0u8; (MANIFEST_SECTORS as usize) * SECTOR_BYTES];
    for sector in 0..MANIFEST_SECTORS {
        let start = (sector as usize) * SECTOR_BYTES;
        let chunk: &mut [u8; SECTOR_BYTES] = (&mut bytes[start..start + SECTOR_BYTES])
            .try_into()
            .expect("sector-sized");
        if source
            .read_sector(source_partition.first_lba + MANIFEST_LBA + sector, chunk)
            .is_err()
        {
            fail(b"manifest read");
        }
    }
    // Trimmed to its declared length: the record is read in whole sectors and
    // the decoder bounds on exact size, so the zero padding behind it would
    // otherwise read as a truncated manifest.
    let declared = u64::from_le_bytes(
        bytes[transfer::HEADER_TOTAL_LEN_OFFSET..transfer::HEADER_TOTAL_LEN_OFFSET + 8]
            .try_into()
            .expect("eight bytes"),
    ) as usize;
    if declared > bytes.len() {
        fail(b"manifest length past the sectors it was read from");
    }
    let manifest = match TransferManifest::decode(&bytes[..declared]) {
        Ok(manifest) => manifest,
        Err(error) => {
            report_transfer(error);
            fail(b"manifest decode");
        }
    };
    write_pair(
        b"[sel4-transfer-probe] manifest objects=",
        manifest.object_count() as u64,
        b" states=",
        manifest.state_count() as u64,
    );

    // A tampered manifest must fail on the *digest* rather than on a field the
    // flip happened to land in. The last byte is metadata no bound covers, so
    // only the self-excluding hash catches it.
    let mut tampered = alloc::vec![0u8; declared];
    tampered.copy_from_slice(&bytes[..declared]);
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    // `TransferManifest` borrows, so it is not `PartialEq`; compare the error.
    match TransferManifest::decode(&tampered) {
        Err(TransferError::BadHash) => {}
        Err(other) => {
            report_transfer(other);
            fail(b"a tampered manifest failed on the wrong check");
        }
        Ok(_) => fail(b"a tampered manifest was accepted"),
    }
    slime_rt::debug_write(b"[sel4-transfer-probe] tampered manifest refused\n");

    // The closure, verified before any BootState write.
    let mut carried = 0;
    for index in 0..manifest.object_count() {
        let Ok(object) = manifest.object(index) else {
            fail(b"manifest object entry");
        };
        let Some(payload) = object.payload else {
            fail(b"the closure is incomplete");
        };
        if payload.len() != object.length {
            fail(b"object length");
        }
        if slime_rt::sha256(payload) != object.digest {
            fail(b"object content hash");
        }
        carried += 1;
    }
    write_pair(
        b"[sel4-transfer-probe] closure verified objects=",
        carried,
        b" of=",
        manifest.object_count() as u64,
    );

    // Validate the manifest against the source's state set, not against its own
    // already-filtered entries. One source binding travels read-only and one is
    // explicitly local; omission of either policy from this table changes the
    // proof rather than silently shrinking the universe being checked.
    let source_states = [
        (binding_identity("transferred-state"), true, true),
        (binding_identity("ephemeral-state"), false, false),
    ];
    let mut seen = [false; 2];
    let mut travelling = 0;
    let mut read_only = 0;
    for index in 0..manifest.state_count() {
        let Ok(state) = manifest.state(index) else {
            fail(b"manifest state entry");
        };
        let Some(source_index) = source_states
            .iter()
            .position(|entry| entry.0 == state.binding)
        else {
            fail(b"manifest shipped state absent from the source set");
        };
        if !source_states[source_index].1 || seen[source_index] {
            fail(b"manifest violated source travel policy");
        }
        seen[source_index] = true;
        travelling += 1;
        let is_read_only = state.flags & STATE_FLAG_READ_ONLY != 0;
        if is_read_only != source_states[source_index].2 {
            fail(b"manifest changed source read-only policy");
        }
        read_only += is_read_only as u64;
    }
    if source_states
        .iter()
        .enumerate()
        .any(|(index, entry)| entry.1 != seen[index])
    {
        fail(b"manifest omitted travelling source state");
    }
    write_pair(
        b"[sel4-transfer-probe] source-state travel entries=",
        travelling,
        b" read-only=",
        read_only,
    );

    // Stage pending. The known-good root is untouched, which is what makes a
    // failed activation recoverable.
    let selected = match slots.select(&mut receiver) {
        Ok(selected) => selected,
        Err(SelectionError::NoValidBootState) => fail(b"receiver has no root"),
        Err(_) => fail(b"receiver root conflict"),
    };
    let Ok(staged) = selected.state.stage_pending(
        manifest.generation,
        1,
        RECEIVER_GENERATION_ROOT,
        manifest.source_state_root,
    ) else {
        fail(b"stage pending");
    };
    let live = slots.commit(&mut receiver, selected.slot, &staged);
    if live.state.pending != Some(manifest.generation) {
        fail(b"the transferred generation is not pending");
    }
    if live.state.known_good != RECEIVER_KNOWN_GOOD {
        fail(b"staging changed the known-good root");
    }
    report(b"staged", &live.state);

    // Health confirmation promotes it, and only then does the known-good root
    // become the transferred generation.
    let Ok(promoted) = live
        .state
        .promote_pending(manifest.generation, manifest.release_sequence)
    else {
        fail(b"promote");
    };
    let live = slots.commit(&mut receiver, live.slot, &promoted);
    if live.state.known_good != manifest.generation || live.state.pending.is_some() {
        fail(b"promoted root");
    }
    if live.state.accepted_release_sequence != manifest.release_sequence {
        fail(b"the accepted release did not advance");
    }
    report(b"promoted", &live.state);

    slime_rt::debug_write(b"[sel4-transfer-probe] transfer plane complete\n");
}

/// The two BootState slots on the receiver, and the older-slot-first rule.
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
        io: &mut Device,
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

    fn write(&self, io: &mut Device, slot: Slot, state: &BootState) -> Result<(), ()> {
        let encoded = state.encode().map_err(|_| ())?;
        io.write_sector(self.lba(slot), &encoded).map_err(|_| ())?;
        io.flush().map_err(|_| ())
    }

    fn commit(
        &self,
        io: &mut Device,
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

/// One device, named by the capability slot the generation placed it at.
struct Device {
    slot: u32,
}

impl BlockIo for Device {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_READ, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status = slime_rt::block_transact_sector(self.slot, &request.encode(), &mut reply, out);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_WRITE, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status = slime_rt::block_transact_write(self.slot, &request.encode(), data, &mut reply);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let request = block_request(block::OP_FLUSH, 0);
        let mut reply = [0u8; block::REPLY_LEN];
        if slime_rt::block_transact(self.slot, &request.encode(), &mut reply) < 0 {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

fn locate_partition(io: &mut Device) -> Option<gpt::Partition> {
    let capacity = device_capacity(io)?;
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    gpt::validate_store_partition(&mut reader, capacity)
        .ok()
        .map(|selected| selected.partition)
}

/// The device's sector count, measured by binary search over readable LBAs.
/// Reads only, so it is safe on the source.
fn device_capacity(io: &mut Device) -> Option<u64> {
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

fn spawned_instance() -> bool {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    slime_rt::recv(RUN_TOKEN_SLOT, &mut bytes, &mut caps) != slime_rt::ERR_BAD_CAP
}

fn report_transfer(error: TransferError) {
    let name: &[u8] = match error {
        TransferError::Truncated => b"truncated",
        TransferError::BadMagic => b"bad-magic",
        TransferError::UnsupportedVersion => b"unsupported-version",
        TransferError::UnknownFlags => b"unknown-flags",
        TransferError::BadBounds => b"bad-bounds",
        TransferError::BadHash => b"bad-hash",
        TransferError::BadEntry => b"bad-entry",
    };
    slime_rt::debug_write(b"[sel4-transfer-probe] manifest error=");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"\n");
}

fn report(step: &[u8], state: &BootState) {
    let mut line = [0u8; 160];
    let mut len = 0;
    len += copy(&mut line[len..], b"[sel4-transfer-probe] ");
    len += copy(&mut line[len..], step);
    len += copy(&mut line[len..], b" seq=");
    len += copy(&mut line[len..], &decimal(state.sequence));
    len += copy(&mut line[len..], b" pending=");
    len += copy(
        &mut line[len..],
        if state.pending.is_some() { b"1" } else { b"0" },
    );
    len += copy(&mut line[len..], b" release=");
    len += copy(&mut line[len..], &decimal(state.accepted_release_sequence));
    len += copy(&mut line[len..], b"\n");
    slime_rt::debug_write(&line[..len]);
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
    slime_rt::debug_write(b"[sel4-transfer-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
