#![no_std]

use boot_contracts::bootstate::{BootState, SLOT_BYTES, SLOT_COUNT};
use boot_contracts::generation::{Generation, generation_identity};
use boot_contracts::kernel_image::{ImageError, KernelImage};
use boot_contracts::sha256::Sha256;

pub use boot_contracts::bootstate::{
    BOOTSTORE_CAPACITY, BOOTSTORE_DIRECTORY_OFFSET, BOOTSTORE_ENTRY_GENERATION_LEN_OFFSET,
    BOOTSTORE_ENTRY_GENERATION_OFFSET_OFFSET, BOOTSTORE_ENTRY_LEN, BOOTSTORE_ENTRY_PADDING_OFFSET,
    BOOTSTORE_ENTRY_RELEASE_LEN_OFFSET, BOOTSTORE_ENTRY_RELEASE_OFFSET_OFFSET,
    BOOTSTORE_GENERATIONS_OFFSET, BOOTSTORE_HEADER_CAPACITY_OFFSET, BOOTSTORE_HEADER_CHECKSUM_END,
    BOOTSTORE_HEADER_CHECKSUM_OFFSET, BOOTSTORE_HEADER_DIRECTORY_LEN_OFFSET,
    BOOTSTORE_HEADER_ENTRY_COUNT_OFFSET, BOOTSTORE_HEADER_FORMAT_VERSION_OFFSET,
    BOOTSTORE_HEADER_HEADER_SIZE_OFFSET, BOOTSTORE_HEADER_LEN,
    BOOTSTORE_HEADER_REQUIRED_FLAGS_OFFSET, BOOTSTORE_HEADER_RESERVED_OFFSET, BOOTSTORE_MAGIC,
    BOOTSTORE_RELEASES_OFFSET, BOOTSTORE_VERSION,
};
use boot_contracts::release::{INITIAL_TRUST_ROOT, RELEASE_BYTES, Release};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootError {
    Truncated,
    BadDirectoryMagic,
    UnsupportedDirectoryVersion,
    UnknownDirectoryFlags,
    BadDirectoryHash,
    BadDirectoryBounds,
    NoValidBootState,
    ConflictingSlots,
    MissingGeneration,
    BadGenerationRoot,
    BadGenerationHash,
    BadObjectHash,
    BadKernelImage,
    BadRelease,
    KernelImage(ImageError),
    TooManyMemoryEntries,
    MissingFramebuffer,
    UnsupportedFramebuffer,
    AddressOverflow,
    PageTableExhausted,
}

#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntry<'a> {
    pub identity: [u8; 32],
    pub bytes: &'a [u8],
    pub release_bytes: &'a [u8],
}

pub struct BootDirectory<'a> {
    bytes: &'a [u8],
    count: usize,
    entry_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

#[derive(Debug, Clone, Copy)]
pub struct SelectedBootState {
    pub slot: Slot,
    pub state: BootState,
}

impl<'a> BootDirectory<'a> {
    pub fn count(&self) -> usize {
        self.count
    }

    pub fn entry(&self, index: usize) -> Result<DirectoryEntry<'a>, BootError> {
        if index >= self.count {
            return Err(BootError::BadDirectoryBounds);
        }
        let offset = self.entry_offset + index * BOOTSTORE_ENTRY_LEN;
        let identity: [u8; 32] = self.bytes[offset..offset + 32].try_into().unwrap();
        let start = u64_at(
            self.bytes,
            offset + BOOTSTORE_ENTRY_GENERATION_OFFSET_OFFSET,
        )? as usize;
        let len = u64_at(self.bytes, offset + BOOTSTORE_ENTRY_GENERATION_LEN_OFFSET)? as usize;
        let release_start =
            u64_at(self.bytes, offset + BOOTSTORE_ENTRY_RELEASE_OFFSET_OFFSET)? as usize;
        let release_len = u64_at(self.bytes, offset + BOOTSTORE_ENTRY_RELEASE_LEN_OFFSET)? as usize;
        if self.bytes[offset + BOOTSTORE_ENTRY_PADDING_OFFSET..offset + BOOTSTORE_ENTRY_LEN]
            .iter()
            .any(|byte| *byte != 0)
            || start < BOOTSTORE_GENERATIONS_OFFSET
            || start % 4096 != 0
            || release_start < BOOTSTORE_RELEASES_OFFSET
            || release_start % RELEASE_BYTES != 0
            || release_len != RELEASE_BYTES
        {
            return Err(BootError::BadDirectoryBounds);
        }
        let end = start
            .checked_add(len)
            .ok_or(BootError::BadDirectoryBounds)?;
        let release_end = release_start
            .checked_add(release_len)
            .ok_or(BootError::BadDirectoryBounds)?;
        if release_end > BOOTSTORE_GENERATIONS_OFFSET {
            return Err(BootError::BadDirectoryBounds);
        }
        let bytes = self
            .bytes
            .get(start..end)
            .ok_or(BootError::BadDirectoryBounds)?;
        let release_bytes = self
            .bytes
            .get(release_start..release_end)
            .ok_or(BootError::BadDirectoryBounds)?;
        Ok(DirectoryEntry {
            identity,
            bytes,
            release_bytes,
        })
    }
}

pub fn decode_directory(bytes: &[u8]) -> Result<BootDirectory<'_>, BootError> {
    if bytes.len() != BOOTSTORE_CAPACITY {
        return Err(BootError::BadDirectoryBounds);
    }
    let header = bytes
        .get(BOOTSTORE_DIRECTORY_OFFSET..BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_LEN)
        .ok_or(BootError::Truncated)?;
    if header[..8] != BOOTSTORE_MAGIC {
        return Err(BootError::BadDirectoryMagic);
    }
    if u32_at(header, BOOTSTORE_HEADER_FORMAT_VERSION_OFFSET)? != BOOTSTORE_VERSION
        || u32_at(header, BOOTSTORE_HEADER_HEADER_SIZE_OFFSET)? as usize != BOOTSTORE_HEADER_LEN
    {
        return Err(BootError::UnsupportedDirectoryVersion);
    }
    if u64_at(header, BOOTSTORE_HEADER_REQUIRED_FLAGS_OFFSET)? != 0 {
        return Err(BootError::UnknownDirectoryFlags);
    }
    let count = u32_at(header, BOOTSTORE_HEADER_ENTRY_COUNT_OFFSET)? as usize;
    if u32_at(header, BOOTSTORE_HEADER_RESERVED_OFFSET)? != 0
        || !(1..=64).contains(&count)
        || u64_at(header, BOOTSTORE_HEADER_DIRECTORY_LEN_OFFSET)? as usize
            != count * BOOTSTORE_ENTRY_LEN
        || u64_at(header, BOOTSTORE_HEADER_CAPACITY_OFFSET)? as usize != bytes.len()
    {
        return Err(BootError::BadDirectoryBounds);
    }
    let expected: [u8; 32] = header
        [BOOTSTORE_HEADER_CHECKSUM_OFFSET..BOOTSTORE_HEADER_CHECKSUM_END]
        .try_into()
        .unwrap();
    let mut hasher = Sha256::new();
    hasher.update(
        &bytes[SLOT_BYTES * SLOT_COUNT
            ..BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_OFFSET],
    );
    hasher.update(&[0u8; 32]);
    hasher.update(&bytes[BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_END..]);
    if hasher.finalize() != expected {
        return Err(BootError::BadDirectoryHash);
    }
    let directory = BootDirectory {
        bytes,
        count,
        entry_offset: BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_LEN,
    };
    let mut previous = [0u8; 32];
    for index in 0..count {
        let entry = directory.entry(index)?;
        if index > 0 && entry.identity <= previous {
            return Err(BootError::BadDirectoryBounds);
        }
        previous = entry.identity;
    }
    Ok(directory)
}

pub fn select_bootstate(
    a: &[u8; SLOT_BYTES],
    b: &[u8; SLOT_BYTES],
) -> Result<SelectedBootState, BootError> {
    let a = BootState::decode(a);
    let b = BootState::decode(b);
    match (a, b) {
        (Ok(a), Ok(b)) if a.sequence > b.sequence => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Ok(a), Ok(b)) if b.sequence > a.sequence => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (Ok(a), Ok(b)) if a == b => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Ok(_), Ok(_)) => Err(BootError::ConflictingSlots),
        (Ok(a), Err(_)) => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Err(_), Ok(b)) => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (Err(_), Err(_)) => Err(BootError::NoValidBootState),
    }
}

pub fn select_bootstate_for_directory(
    a: &[u8; SLOT_BYTES],
    b: &[u8; SLOT_BYTES],
    directory: &BootDirectory<'_>,
) -> Result<SelectedBootState, BootError> {
    let root = directory_root(directory)?;
    let a = BootState::decode(a)
        .ok()
        .filter(|state| state.generation_root == root);
    let b = BootState::decode(b)
        .ok()
        .filter(|state| state.generation_root == root);
    match (a, b) {
        (Some(a), Some(b)) if a.sequence > b.sequence => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Some(a), Some(b)) if b.sequence > a.sequence => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (Some(a), Some(b)) if a == b => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Some(_), Some(_)) => Err(BootError::ConflictingSlots),
        (Some(a), None) => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (None, Some(b)) => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (None, None) => Err(BootError::BadGenerationRoot),
    }
}

fn directory_root(directory: &BootDirectory<'_>) -> Result<[u8; 32], BootError> {
    let mut root = Sha256::new();
    for index in 0..directory.count() {
        root.update(&directory.entry(index)?.identity);
    }
    Ok(root.finalize())
}

pub fn selected_generation_identity(state: &BootState) -> [u8; 32] {
    match (state.pending, state.remaining_attempts) {
        (Some(pending), attempts) if attempts > 0 => pending,
        _ => state.known_good,
    }
}

pub fn select_generation<'a>(
    directory: &'a BootDirectory<'a>,
    state: &BootState,
) -> Result<DirectoryEntry<'a>, BootError> {
    let mut root = Sha256::new();
    let mut selected = None;
    let selected_identity = selected_generation_identity(state);
    for index in 0..directory.count() {
        let entry = directory.entry(index)?;
        root.update(&entry.identity);
        if entry.identity == selected_identity {
            selected = Some(entry);
        }
    }
    if root.finalize() != state.generation_root {
        return Err(BootError::BadGenerationRoot);
    }
    selected.ok_or(BootError::MissingGeneration)
}

pub fn verify_generation<'a>(
    bytes: &'a [u8],
    expected: &[u8; 32],
) -> Result<Generation<'a>, BootError> {
    if generation_identity(bytes) != *expected {
        return Err(BootError::BadGenerationHash);
    }
    Generation::decode(bytes).map_err(|error| match error {
        boot_contracts::generation::DecodeError::BadObjectHash => BootError::BadObjectHash,
        _ => BootError::BadGenerationHash,
    })
}

pub fn verify_release(
    entry: &DirectoryEntry<'_>,
    generation: &Generation<'_>,
    state: &BootState,
    running_pending: bool,
) -> Result<u64, BootError> {
    let release = Release::decode(entry.release_bytes).map_err(|_| BootError::BadRelease)?;
    release
        .verify_generation(generation, &INITIAL_TRUST_ROOT)
        .map_err(|_| BootError::BadRelease)?;
    if running_pending {
        if release.sequence <= state.accepted_release_sequence {
            return Err(BootError::BadRelease);
        }
    } else if release.sequence > state.accepted_release_sequence {
        return Err(BootError::BadRelease);
    }
    Ok(release.sequence)
}

pub fn verify_kernel<'a>(generation: &Generation<'a>) -> Result<KernelImage<'a>, BootError> {
    let object = generation
        .object(generation.kernel_object)
        .map_err(|_| BootError::BadKernelImage)?;
    KernelImage::decode(object.bytes).map_err(BootError::KernelImage)
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, BootError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(BootError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, BootError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(BootError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
