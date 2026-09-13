//! Immutable disk-backed generation selection for the seL4 root task.
//!
//! The root image contains mechanism only. Runtime generation bytes and their
//! signed release live in the explicitly attached boot disk's GPT partition.

extern crate alloc;

use crate::boot_selector_block::{SECTOR_BYTES, VirtioBlock};
use boot_contracts::bootstate::{
    BOOTSTORE_CAPACITY, BOOTSTORE_DIRECTORY_OFFSET, BOOTSTORE_ENTRY_GENERATION_LEN_OFFSET,
    BOOTSTORE_ENTRY_GENERATION_OFFSET_OFFSET, BOOTSTORE_ENTRY_LEN, BOOTSTORE_ENTRY_PADDING_OFFSET,
    BOOTSTORE_ENTRY_RELEASE_LEN_OFFSET, BOOTSTORE_ENTRY_RELEASE_OFFSET_OFFSET,
    BOOTSTORE_GENERATIONS_OFFSET, BOOTSTORE_HEADER_CAPACITY_OFFSET, BOOTSTORE_HEADER_CHECKSUM_END,
    BOOTSTORE_HEADER_CHECKSUM_OFFSET, BOOTSTORE_HEADER_DIRECTORY_LEN_OFFSET,
    BOOTSTORE_HEADER_ENTRY_COUNT_OFFSET, BOOTSTORE_HEADER_FORMAT_VERSION_OFFSET,
    BOOTSTORE_HEADER_HEADER_SIZE_OFFSET, BOOTSTORE_HEADER_LEN,
    BOOTSTORE_HEADER_REQUIRED_FLAGS_OFFSET, BOOTSTORE_HEADER_RESERVED_OFFSET, BOOTSTORE_MAGIC,
    BOOTSTORE_RELEASES_OFFSET, BOOTSTORE_VERSION, BootState, SLOT_BYTES, SelectedBootState, Slot,
    select_bootstate,
};
use boot_contracts::generation::{Generation, generation_identity};
use boot_contracts::gpt::{self, GptError, Partition};
use boot_contracts::release::{INITIAL_TRUST_ROOT, RELEASE_BYTES, Release};
use boot_contracts::sha256::Sha256;

const STATE_SLOT_A: u64 = 0;
const STATE_SLOT_B: u64 = 1;
const MAX_DIRECTORY_ENTRIES: usize = 64;
// The size of this buffer is a root CSlot budget decision, not a statement about
// the store: store v1 places generations after 16 KiB and declares no ceiling on
// how large one may be, so nothing but this constant bounds what the selector
// can hold.
//
// This buffer is `.bss`, so the loader creates one root CSlot per page of it
// before the root runs: at 8 MiB that is ~2048 of the root CNode's 4096 slots
// (`CONFIG_ROOT_CNODE_SIZE_BITS = 12`) spent on a buffer that is almost
// entirely zero. Measured directly — the selector root's `.bss` was 10.99 MB
// against the ordinary root's 2.60 MB, an 8.39 MB delta that is exactly this
// array — and the cost is observable as free root CSlots: 1188 on the selector
// image against 3017 on the demo image built from the same sources.
//
// That is not merely wasteful, it is a *capacity* limit on what the selector
// can boot. RP2's demo generation plans 1368 root CSlots and was refused with
// `PlanExceedsRootSlots { required: 1368, available: 1188 }` — a generation
// every non-selector image admits. 4 MiB restores ~1024 slots and still leaves
// 2.6x headroom over the largest generation this repository builds
// (`sel4-traffic`, 1.57 MB); the largest is what bounds this, since the
// selector must hold whichever candidate the store names.
//
// `scripts/build/build-generation.py`'s `SELECTOR_GENERATION_BYTES` states the
// same ceiling and refuses a larger blob at build time, so the two must move
// together. `check-sel4-boot-selection.py`'s `assert_ceiling_agrees` pins that
// agreement *and* measures the built generations against this value, so the
// headroom above is a checked property rather than a claim in this comment.
const SELECTOR_GENERATION_BYTES: usize = 4 * 1024 * 1024;

#[repr(align(8))]
struct GenerationBuffer([u8; SELECTOR_GENERATION_BYTES]);

static mut GENERATION_BUFFER: GenerationBuffer = GenerationBuffer([0; SELECTOR_GENERATION_BYTES]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectorError {
    NoBootDevice,
    Device,
    Gpt,
    Directory,
    BootState,
    MissingGeneration,
    Generation,
    Release,
    WrongBootBundle,
    Commit,
}

#[derive(Clone, Copy)]
struct DirectoryEntry {
    identity: [u8; 32],
    generation_offset: u64,
    generation_len: usize,
    release_offset: u64,
}

pub struct SelectedGeneration {
    pub generation: Generation<'static>,
    pub runtime: BootRuntime,
}

/// Durable selection context retained while the selected graph runs.
pub struct BootRuntime {
    partition_first_lba: u64,
    selected: SelectedBootState,
    running_identity: [u8; 32],
    release_sequence: u64,
    running_pending: bool,
}

impl BootRuntime {
    pub const fn running_identity(&self) -> [u8; 32] {
        self.running_identity
    }
    pub const fn running_pending(&self) -> bool {
        self.running_pending
    }
    pub const fn remaining_attempts(&self) -> u32 {
        self.selected.state.remaining_attempts
    }

    /// Health confirmation is the only transition that promotes a pending
    /// generation. The commit is older-slot-first and flushed before success.
    pub fn confirm(&mut self, device: &mut VirtioBlock) -> Result<(), SelectorError> {
        if !self.running_pending {
            return Err(SelectorError::BootState);
        }
        let promoted = self
            .selected
            .state
            .promote_pending(self.running_identity, self.release_sequence)
            .map_err(|_| SelectorError::BootState)?;
        self.selected = commit_state(
            device,
            self.partition_first_lba,
            self.selected.slot,
            promoted,
        )?;
        self.running_pending = false;
        Ok(())
    }

    /// An explicit unhealthy report never repairs the attempt consumed before
    /// launch. A fresh boot therefore observes the durable lower count.
    pub fn mark_unhealthy(&self) -> Result<(), SelectorError> {
        if self.running_pending {
            Ok(())
        } else {
            Err(SelectorError::BootState)
        }
    }
}

pub fn select(
    device: &mut VirtioBlock,
    expected_boot_bundle: &[u8; 32],
) -> Result<SelectedGeneration, SelectorError> {
    let partition = locate_partition(device)?;
    let entries = read_directory(device, &partition)?;
    let generation_root = directory_root(&entries);
    let mut selected = read_bootstate(device, partition.first_lba)?;
    if selected.state.generation_root != generation_root {
        return Err(SelectorError::BootState);
    }

    let selection_state = selected.state;
    let pending_exhausted =
        selection_state.pending.is_some() && selection_state.remaining_attempts == 0;
    let running_pending =
        selection_state.pending.is_some() && selection_state.remaining_attempts > 0;
    let running_identity = if running_pending {
        selection_state.pending.ok_or(SelectorError::BootState)?
    } else {
        selection_state.known_good
    };

    // A live candidate spends one attempt before any candidate bytes are read.
    // Exhausted pending state is cleared only after the known-good closure has
    // passed verification, so a damaged fallback cannot destroy retry evidence.
    if running_pending {
        let consumed = selected
            .state
            .consume_pending_attempt()
            .map_err(|_| SelectorError::BootState)?;
        selected = commit_state(device, partition.first_lba, selected.slot, consumed)?;
    }

    let entry = entries
        .iter()
        .flatten()
        .find(|entry| entry.identity == running_identity)
        .copied()
        .ok_or(SelectorError::MissingGeneration)?;
    let generation_bytes = read_generation(device, &partition, entry)?;
    if generation_identity(generation_bytes) != entry.identity {
        return Err(SelectorError::Generation);
    }
    let generation = Generation::decode(generation_bytes).map_err(|_| SelectorError::Generation)?;
    if !generation.is_v5() {
        return Err(SelectorError::Generation);
    }

    let release_bytes = read_release(device, &partition, entry)?;
    let release = Release::decode(&release_bytes).map_err(|_| SelectorError::Release)?;
    release
        .verify_generation(&generation, &INITIAL_TRUST_ROOT)
        .map_err(|_| SelectorError::Release)?;
    release
        .verify_boot_bundle(expected_boot_bundle)
        .map_err(|_| SelectorError::WrongBootBundle)?;
    if running_pending {
        if release.sequence <= selection_state.accepted_release_sequence {
            return Err(SelectorError::Release);
        }
    } else if release.sequence > selection_state.accepted_release_sequence {
        return Err(SelectorError::Release);
    }
    if pending_exhausted {
        let rolled_back = selected
            .state
            .rollback_pending()
            .map_err(|_| SelectorError::BootState)?;
        selected = commit_state(device, partition.first_lba, selected.slot, rolled_back)?;
    }

    Ok(SelectedGeneration {
        generation,
        runtime: BootRuntime {
            partition_first_lba: partition.first_lba,
            selected,
            running_identity,
            release_sequence: release.sequence,
            running_pending,
        },
    })
}

fn locate_partition(device: &mut VirtioBlock) -> Result<Partition, SelectorError> {
    let capacity = device.capacity_sectors();
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        device.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected =
        gpt::validate_store_partition(&mut reader, capacity).map_err(|_| SelectorError::Gpt)?;
    let sectors = selected
        .partition
        .last_lba
        .checked_sub(selected.partition.first_lba)
        .and_then(|span| span.checked_add(1))
        .ok_or(SelectorError::Gpt)?;
    if sectors < (BOOTSTORE_CAPACITY / SECTOR_BYTES) as u64 {
        return Err(SelectorError::Gpt);
    }
    Ok(selected.partition)
}

fn read_directory(
    device: &mut VirtioBlock,
    partition: &Partition,
) -> Result<[Option<DirectoryEntry>; MAX_DIRECTORY_ENTRIES], SelectorError> {
    let mut header_sector = [0u8; SECTOR_BYTES];
    read_partition_sector(
        device,
        partition,
        (BOOTSTORE_DIRECTORY_OFFSET / SECTOR_BYTES) as u64,
        &mut header_sector,
    )?;
    let header = &header_sector[..BOOTSTORE_HEADER_LEN];
    if header[..8] != BOOTSTORE_MAGIC
        || u32_at(header, BOOTSTORE_HEADER_FORMAT_VERSION_OFFSET) != BOOTSTORE_VERSION
        || u32_at(header, BOOTSTORE_HEADER_HEADER_SIZE_OFFSET) as usize != BOOTSTORE_HEADER_LEN
        || u64_at(header, BOOTSTORE_HEADER_REQUIRED_FLAGS_OFFSET) != 0
        || u32_at(header, BOOTSTORE_HEADER_RESERVED_OFFSET) != 0
        || u64_at(header, BOOTSTORE_HEADER_CAPACITY_OFFSET) as usize != BOOTSTORE_CAPACITY
    {
        return Err(SelectorError::Directory);
    }
    let count = u32_at(header, BOOTSTORE_HEADER_ENTRY_COUNT_OFFSET) as usize;
    if !(1..=MAX_DIRECTORY_ENTRIES).contains(&count)
        || u64_at(header, BOOTSTORE_HEADER_DIRECTORY_LEN_OFFSET) as usize
            != count * BOOTSTORE_ENTRY_LEN
    {
        return Err(SelectorError::Directory);
    }
    verify_directory_checksum(
        device,
        partition,
        &header[BOOTSTORE_HEADER_CHECKSUM_OFFSET..BOOTSTORE_HEADER_CHECKSUM_END],
    )?;

    let mut entries = [None; MAX_DIRECTORY_ENTRIES];
    let directory_start = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_LEN;
    let mut previous = [0u8; 32];
    for (index, slot) in entries.iter_mut().enumerate().take(count) {
        let offset = directory_start + index * BOOTSTORE_ENTRY_LEN;
        let mut record = [0u8; BOOTSTORE_ENTRY_LEN];
        read_partition_bytes(device, partition, offset as u64, &mut record)?;
        let record = &record[..];
        let identity: [u8; 32] = record[..32]
            .try_into()
            .map_err(|_| SelectorError::Directory)?;
        let generation_offset = u64_at(record, BOOTSTORE_ENTRY_GENERATION_OFFSET_OFFSET);
        let generation_len = u64_at(record, BOOTSTORE_ENTRY_GENERATION_LEN_OFFSET) as usize;
        let release_offset = u64_at(record, BOOTSTORE_ENTRY_RELEASE_OFFSET_OFFSET);
        let generation_end = generation_offset.checked_add(generation_len as u64);
        let release_end = release_offset.checked_add(RELEASE_BYTES as u64);
        if (index > 0 && identity <= previous)
            || generation_offset < BOOTSTORE_GENERATIONS_OFFSET as u64
            || !generation_offset.is_multiple_of(4096)
            || generation_len == 0
            || generation_len > SELECTOR_GENERATION_BYTES
            || generation_end.is_none_or(|end| end > BOOTSTORE_CAPACITY as u64)
            || release_offset < BOOTSTORE_RELEASES_OFFSET as u64
            || !release_offset.is_multiple_of(RELEASE_BYTES as u64)
            || release_end.is_none_or(|end| end > BOOTSTORE_GENERATIONS_OFFSET as u64)
            || u64_at(record, BOOTSTORE_ENTRY_RELEASE_LEN_OFFSET) as usize != RELEASE_BYTES
            || record[BOOTSTORE_ENTRY_PADDING_OFFSET..]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(SelectorError::Directory);
        }
        *slot = Some(DirectoryEntry {
            identity,
            generation_offset,
            generation_len,
            release_offset,
        });
        previous = identity;
    }
    Ok(entries)
}

fn verify_directory_checksum(
    device: &mut VirtioBlock,
    partition: &Partition,
    expected: &[u8],
) -> Result<(), SelectorError> {
    let mut hasher = Sha256::new();
    let first = (SLOT_BYTES * 2 / SECTOR_BYTES) as u64;
    let sectors = (BOOTSTORE_CAPACITY / SECTOR_BYTES) as u64;
    let checksum_start = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_OFFSET;
    let checksum_end = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_END;
    for relative in first..sectors {
        let mut sector = [0u8; SECTOR_BYTES];
        read_partition_sector(device, partition, relative, &mut sector)?;
        let sector_start = relative as usize * SECTOR_BYTES;
        let overlap_start = checksum_start
            .saturating_sub(sector_start)
            .min(SECTOR_BYTES);
        let overlap_end = checksum_end.saturating_sub(sector_start).min(SECTOR_BYTES);
        if overlap_start < overlap_end {
            sector[overlap_start..overlap_end].fill(0);
        }
        hasher.update(&sector);
    }
    let expected: [u8; 32] = expected.try_into().map_err(|_| SelectorError::Directory)?;
    if hasher.finalize() != expected {
        return Err(SelectorError::Directory);
    }
    Ok(())
}

fn directory_root(entries: &[Option<DirectoryEntry>; MAX_DIRECTORY_ENTRIES]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for entry in entries.iter().flatten() {
        hasher.update(&entry.identity);
    }
    hasher.finalize()
}

fn read_bootstate(
    device: &mut VirtioBlock,
    first_lba: u64,
) -> Result<SelectedBootState, SelectorError> {
    let mut a = [0u8; SLOT_BYTES];
    let mut b = [0u8; SLOT_BYTES];
    device
        .read_sector(first_lba + STATE_SLOT_A, &mut a)
        .map_err(|_| SelectorError::Device)?;
    device
        .read_sector(first_lba + STATE_SLOT_B, &mut b)
        .map_err(|_| SelectorError::Device)?;
    select_bootstate(&a, &b).map_err(|_| SelectorError::BootState)
}

fn commit_state(
    device: &mut VirtioBlock,
    first_lba: u64,
    selected: Slot,
    state: BootState,
) -> Result<SelectedBootState, SelectorError> {
    let target = selected.other();
    let lba = first_lba
        + match target {
            Slot::A => STATE_SLOT_A,
            Slot::B => STATE_SLOT_B,
        };
    let bytes = state.encode().map_err(|_| SelectorError::Commit)?;
    device
        .write_sector(lba, &bytes)
        .map_err(|_| SelectorError::Commit)?;
    device.flush().map_err(|_| SelectorError::Commit)?;
    let live = read_bootstate(device, first_lba)?;
    if live.slot != target || live.state != state {
        return Err(SelectorError::Commit);
    }
    Ok(live)
}

fn read_generation(
    device: &mut VirtioBlock,
    partition: &Partition,
    entry: DirectoryEntry,
) -> Result<&'static [u8], SelectorError> {
    // SAFETY: the root is single-threaded, selection runs once before graph
    // launch, and the returned bytes remain immutable for the rest of boot.
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(GENERATION_BUFFER) };
    read_partition_bytes(
        device,
        partition,
        entry.generation_offset,
        &mut buffer.0[..entry.generation_len],
    )?;
    Ok(&buffer.0[..entry.generation_len])
}

fn read_release(
    device: &mut VirtioBlock,
    partition: &Partition,
    entry: DirectoryEntry,
) -> Result<[u8; RELEASE_BYTES], SelectorError> {
    let mut bytes = [0u8; RELEASE_BYTES];
    read_partition_bytes(device, partition, entry.release_offset, &mut bytes)?;
    Ok(bytes)
}

fn read_partition_bytes(
    device: &mut VirtioBlock,
    partition: &Partition,
    offset: u64,
    out: &mut [u8],
) -> Result<(), SelectorError> {
    let mut copied = 0usize;
    while copied < out.len() {
        let absolute = offset as usize + copied;
        let relative_lba = (absolute / SECTOR_BYTES) as u64;
        let within = absolute % SECTOR_BYTES;
        let mut sector = [0u8; SECTOR_BYTES];
        read_partition_sector(device, partition, relative_lba, &mut sector)?;
        let count = core::cmp::min(SECTOR_BYTES - within, out.len() - copied);
        out[copied..copied + count].copy_from_slice(&sector[within..within + count]);
        copied += count;
    }
    Ok(())
}

fn read_partition_sector(
    device: &mut VirtioBlock,
    partition: &Partition,
    relative_lba: u64,
    out: &mut [u8; SECTOR_BYTES],
) -> Result<(), SelectorError> {
    let lba = partition
        .first_lba
        .checked_add(relative_lba)
        .ok_or(SelectorError::Device)?;
    if lba > partition.last_lba {
        return Err(SelectorError::Device);
    }
    device
        .read_sector(lba, out)
        .map_err(|_| SelectorError::Device)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
