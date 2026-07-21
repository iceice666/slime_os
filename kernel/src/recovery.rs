//! Signed recovery-index scrub and redundant BootState reconstruction.

use alloc::vec;
use alloc::vec::Vec;

use boot_contracts::bootstate::{BootState, SLOT_BYTES};
use boot_contracts::generation::{Generation, MAX_GENERATION_BYTES, generation_identity};
use boot_contracts::recovery::RecoveryIndex;
use boot_contracts::release::{INITIAL_TRUST_ROOT, RELEASE_BYTES, Release};

use crate::block_device::{BlockDevice, BlockError};
use crate::block_proto::SECTOR_SIZE;
use crate::capability::PciFunctionInfo;
use crate::gpt::Partition;
use crate::object_store::{BlockIo, IoError, ObjectStore, StoreError};
use crate::sha256::Sha256;

const DIRECTORY_MAGIC: [u8; 8] = *b"SLIMEBT\0";
const DIRECTORY_VERSION: u32 = 1;
const DIRECTORY_HEADER: usize = 96;
const DIRECTORY_ENTRY: usize = 96;
const DIRECTORY_OFFSET: usize = 4096;
const RELEASES_OFFSET: usize = 8192;
const GENERATIONS_OFFSET: usize = 16 * 1024;
const BOOT_STORE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GENERATIONS: usize =
    (RELEASES_OFFSET - DIRECTORY_OFFSET - DIRECTORY_HEADER) / DIRECTORY_ENTRY;
pub const FLAG_INTERRUPT_AFTER_FIRST_SLOT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryError {
    Device,
    BadBootStore,
    BadRecoveryIndex,
    WrongTarget,
    BadRelease,
    BrokenGenerationClosure,
    MissingGeneration,
    MissingStateObject,
    Store,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryResult {
    pub generation: [u8; 32],
    pub state_root: [u8; 32],
}

#[derive(Clone, Copy)]
struct DirectoryEntry {
    identity: [u8; 32],
    generation_offset: usize,
    generation_len: usize,
    release_offset: usize,
}

struct DeviceIo<'a>(&'a mut BlockDevice);

impl BlockIo for DeviceIo<'_> {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), IoError> {
        self.0.read_sector(lba, out).map_err(io_error)
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), IoError> {
        self.0.write_sector(lba, data).map_err(io_error)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.0.flush().map_err(io_error)
    }
}

pub fn reconstruct(function: PciFunctionInfo, flags: u32) -> Result<RecoveryResult, RecoveryError> {
    if flags & !FLAG_INTERRUPT_AFTER_FIRST_SLOT != 0 {
        return Err(RecoveryError::BadRecoveryIndex);
    }
    let index_bytes = crate::boot::recovery_index();
    let index = RecoveryIndex::decode(index_bytes).map_err(|_| RecoveryError::BadRecoveryIndex)?;
    if packed_bdf(function) != index.target_pci_bdf {
        return Err(RecoveryError::WrongTarget);
    }

    let mut device = BlockDevice::init(function).map_err(|_| RecoveryError::Device)?;
    crate::serial_println!("[recovery] target device initialized");
    let entries = read_directory(&mut device)?;
    crate::serial_println!("[recovery] bootstore directory verified");
    let target = scrub_generations(&mut device, &entries, &index)?;
    crate::serial_println!("[recovery] generation closure verified");
    scrub_state_store(&mut device, &index)?;
    crate::serial_println!("[recovery] state store verified");

    let slot_a_state = BootState {
        sequence: 1,
        known_good: target,
        pending: None,
        remaining_attempts: 0,
        generation_root: index.generation_root,
        state_root: index.state_root,
        accepted_release_sequence: index.accepted_release_sequence,
    };
    let slot_a = slot_a_state
        .encode()
        .map_err(|_| RecoveryError::BadRecoveryIndex)?;
    let slot_b = BootState {
        sequence: 2,
        ..slot_a_state
    }
    .encode()
    .map_err(|_| RecoveryError::BadRecoveryIndex)?;

    write_slot(&mut device, 0, &slot_a)?;
    device.flush().map_err(|_| RecoveryError::Device)?;
    if flags & FLAG_INTERRUPT_AFTER_FIRST_SLOT != 0 {
        return Err(RecoveryError::Interrupted);
    }
    write_slot(&mut device, 1, &slot_b)?;
    device.flush().map_err(|_| RecoveryError::Device)?;
    Ok(RecoveryResult {
        generation: target,
        state_root: index.state_root,
    })
}

fn read_directory(device: &mut BlockDevice) -> Result<Vec<DirectoryEntry>, RecoveryError> {
    let boot_sectors = BOOT_STORE_BYTES / SECTOR_SIZE;
    if device.capacity_sectors() < boot_sectors as u64 + 3 {
        return Err(RecoveryError::BadBootStore);
    }
    let header = read_range(device, DIRECTORY_OFFSET, DIRECTORY_HEADER)?;
    if header[..8] != DIRECTORY_MAGIC
        || u32_at(&header, 8)? != DIRECTORY_VERSION
        || u32_at(&header, 12)? as usize != DIRECTORY_HEADER
        || u64_at(&header, 16)? != 0
        || u32_at(&header, 28)? != 0
        || u64_at(&header, 40)? as usize != BOOT_STORE_BYTES
    {
        return Err(RecoveryError::BadBootStore);
    }
    let count = u32_at(&header, 24)? as usize;
    if !(1..=MAX_GENERATIONS).contains(&count)
        || u64_at(&header, 32)? as usize != count * DIRECTORY_ENTRY
    {
        return Err(RecoveryError::BadBootStore);
    }
    let expected: [u8; 32] = header[48..80].try_into().unwrap();
    if boot_store_hash(device)? != expected {
        return Err(RecoveryError::BadBootStore);
    }
    let raw = read_range(
        device,
        DIRECTORY_OFFSET + DIRECTORY_HEADER,
        count * DIRECTORY_ENTRY,
    )?;
    let mut entries = Vec::with_capacity(count);
    let mut previous = [0u8; 32];
    for position in 0..count {
        let record = &raw[position * DIRECTORY_ENTRY..(position + 1) * DIRECTORY_ENTRY];
        let identity: [u8; 32] = record[..32].try_into().unwrap();
        let generation_offset = u64_at(record, 32)? as usize;
        let generation_len = u64_at(record, 40)? as usize;
        let release_offset = u64_at(record, 48)? as usize;
        let release_len = u64_at(record, 56)? as usize;
        if (position > 0 && identity <= previous)
            || generation_offset < GENERATIONS_OFFSET
            || !generation_offset.is_multiple_of(4096)
            || generation_len == 0
            || generation_len > MAX_GENERATION_BYTES
            || generation_offset
                .checked_add(generation_len)
                .is_none_or(|end| end > BOOT_STORE_BYTES)
            || release_offset < RELEASES_OFFSET
            || !release_offset.is_multiple_of(RELEASE_BYTES)
            || release_len != RELEASE_BYTES
            || release_offset
                .checked_add(release_len)
                .is_none_or(|end| end > GENERATIONS_OFFSET)
            || record[64..].iter().any(|byte| *byte != 0)
        {
            return Err(RecoveryError::BadBootStore);
        }
        entries.push(DirectoryEntry {
            identity,
            generation_offset,
            generation_len,
            release_offset,
        });
        previous = identity;
    }
    Ok(entries)
}

fn boot_store_hash(device: &mut BlockDevice) -> Result<[u8; 32], RecoveryError> {
    let mut hasher = Sha256::new();
    let mut sector = [0u8; SECTOR_SIZE];
    for lba in (SLOT_BYTES * 2 / SECTOR_SIZE) as u64..(BOOT_STORE_BYTES / SECTOR_SIZE) as u64 {
        device
            .read_sector(lba, &mut sector)
            .map_err(|_| RecoveryError::Device)?;
        let absolute = lba as usize * SECTOR_SIZE;
        let checksum_start = DIRECTORY_OFFSET + 48;
        let checksum_end = DIRECTORY_OFFSET + 80;
        if absolute <= checksum_start && checksum_end <= absolute + SECTOR_SIZE {
            sector[checksum_start - absolute..checksum_end - absolute].fill(0);
        }
        hasher.update(&sector);
    }
    Ok(hasher.finalize())
}

fn scrub_generations(
    device: &mut BlockDevice,
    entries: &[DirectoryEntry],
    index: &RecoveryIndex<'_>,
) -> Result<[u8; 32], RecoveryError> {
    let mut identities = [[0u8; 32]; MAX_GENERATIONS];
    let mut parents = [None; MAX_GENERATIONS];
    let mut found_target = false;
    let mut root = Sha256::new();
    for (position, entry) in entries.iter().enumerate() {
        let generation_bytes = read_range(device, entry.generation_offset, entry.generation_len)?;
        if generation_identity(&generation_bytes) != entry.identity {
            return Err(RecoveryError::BadBootStore);
        }
        let generation =
            Generation::decode(&generation_bytes).map_err(|_| RecoveryError::BadBootStore)?;
        let release_bytes = read_range(device, entry.release_offset, RELEASE_BYTES)?;
        let release = Release::decode(&release_bytes).map_err(|_| RecoveryError::BadRelease)?;
        release
            .verify_generation(&generation, &INITIAL_TRUST_ROOT)
            .map_err(|_| RecoveryError::BadRelease)?;
        identities[position] = generation.identity;
        parents[position] = generation.parent;
        root.update(&generation.identity);
        if generation.identity == index.target_generation {
            if release.sequence > index.accepted_release_sequence {
                return Err(RecoveryError::BadRelease);
            }
            validate_state_bindings(&generation, index)?;
            found_target = true;
        }
    }
    if root.finalize() != index.generation_root {
        return Err(RecoveryError::BrokenGenerationClosure);
    }
    for parent in parents[..entries.len()].iter().flatten() {
        if !identities[..entries.len()].contains(parent) {
            return Err(RecoveryError::BrokenGenerationClosure);
        }
    }
    if !found_target {
        return Err(RecoveryError::MissingGeneration);
    }
    Ok(index.target_generation)
}

fn scrub_state_store(
    device: &mut BlockDevice,
    index: &RecoveryIndex<'_>,
) -> Result<(), RecoveryError> {
    if index.state_last_lba >= device.capacity_sectors() {
        return Err(RecoveryError::Store);
    }
    let partition = Partition {
        first_lba: index.state_first_lba,
        last_lba: index.state_last_lba,
        type_guid: crate::gpt::SLIME_STORE_TYPE_GUID,
    };
    let mut io = DeviceIo(device);
    let store = ObjectStore::open(&mut io, &partition).map_err(|_| RecoveryError::Store)?;
    store.scrub(&mut io).map_err(|_| RecoveryError::Store)?;
    for position in 0..index.state_count() {
        let entry = index
            .state(position)
            .ok_or(RecoveryError::MissingStateObject)?;
        let (_, len) = store
            .stat(&entry.object_identity)
            .ok_or(RecoveryError::MissingStateObject)?;
        let mut payload = vec![0u8; len as usize];
        store
            .get(&mut io, &entry.object_identity, &mut payload)
            .map_err(|error| match error {
                StoreError::NotFound => RecoveryError::MissingStateObject,
                _ => RecoveryError::Store,
            })?;
    }
    Ok(())
}

fn validate_state_bindings(
    generation: &Generation<'_>,
    index: &RecoveryIndex<'_>,
) -> Result<(), RecoveryError> {
    if index.state_count() == 0 {
        return Ok(());
    }
    if generation.state_count() != index.state_count() {
        return Err(RecoveryError::MissingStateObject);
    }
    for position in 0..generation.state_count() {
        let binding = generation
            .state(position)
            .map_err(|_| RecoveryError::BadBootStore)?;
        let entry = index
            .state(position)
            .ok_or(RecoveryError::MissingStateObject)?;
        if boot_contracts::recovery::binding_identity(binding.name) != entry.binding_identity
            || binding.schema_version != entry.schema_version
        {
            return Err(RecoveryError::MissingStateObject);
        }
    }
    Ok(())
}

fn read_range(
    device: &mut BlockDevice,
    offset: usize,
    len: usize,
) -> Result<Vec<u8>, RecoveryError> {
    let end = offset.checked_add(len).ok_or(RecoveryError::BadBootStore)?;
    if end > BOOT_STORE_BYTES {
        return Err(RecoveryError::BadBootStore);
    }
    let mut output = vec![0u8; len];
    let first_lba = offset / SECTOR_SIZE;
    let last_lba = end.div_ceil(SECTOR_SIZE);
    let mut sector = [0u8; SECTOR_SIZE];
    for lba in first_lba..last_lba {
        device
            .read_sector(lba as u64, &mut sector)
            .map_err(|_| RecoveryError::Device)?;
        let sector_start = lba * SECTOR_SIZE;
        let copy_start = offset.max(sector_start);
        let copy_end = end.min(sector_start + SECTOR_SIZE);
        output[copy_start - offset..copy_end - offset]
            .copy_from_slice(&sector[copy_start - sector_start..copy_end - sector_start]);
    }
    Ok(output)
}

fn write_slot(
    device: &mut BlockDevice,
    slot: usize,
    bytes: &[u8; SLOT_BYTES],
) -> Result<(), RecoveryError> {
    let lba = (slot * SLOT_BYTES / SECTOR_SIZE) as u64;
    let sector: &[u8; SECTOR_SIZE] = bytes
        .as_slice()
        .try_into()
        .expect("BootState slot equals one sector");
    device
        .write_sector(lba, sector)
        .map_err(|_| RecoveryError::Device)
}

pub const fn packed_bdf(function: PciFunctionInfo) -> u32 {
    ((function.segment as u32) << 16)
        | ((function.bus as u32) << 8)
        | ((function.device as u32) << 3)
        | function.function as u32
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, RecoveryError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(RecoveryError::BadBootStore)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, RecoveryError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(RecoveryError::BadBootStore)?
            .try_into()
            .unwrap(),
    ))
}

fn io_error(error: BlockError) -> IoError {
    match error {
        BlockError::Timeout => IoError::Timeout,
        _ => IoError::Device,
    }
}
