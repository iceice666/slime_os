#![no_std]

use boot_contracts::bootstate::{BootState, SLOT_BYTES};
use boot_contracts::generation::{Generation, generation_identity};
use boot_contracts::kernel_image::{ImageError, KernelImage};
use boot_contracts::sha256::Sha256;

use boot_contracts::release::{INITIAL_TRUST_ROOT, RELEASE_BYTES, Release};
pub const DIRECTORY_MAGIC: [u8; 8] = *b"SLIMEBT\0";
pub const DIRECTORY_VERSION: u32 = 1;
pub const DIRECTORY_HEADER: usize = 96;
pub const DIRECTORY_ENTRY: usize = 96;
pub const DIRECTORY_OFFSET: usize = 4096;
pub const RELEASES_OFFSET: usize = 8192;
pub const GENERATIONS_OFFSET: usize = 16 * 1024;
pub const BOOT_STORE_BYTES: usize = 32 * 1024 * 1024;

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
        let offset = self.entry_offset + index * DIRECTORY_ENTRY;
        let identity: [u8; 32] = self.bytes[offset..offset + 32].try_into().unwrap();
        let start = u64_at(self.bytes, offset + 32)? as usize;
        let len = u64_at(self.bytes, offset + 40)? as usize;
        let release_start = u64_at(self.bytes, offset + 48)? as usize;
        let release_len = u64_at(self.bytes, offset + 56)? as usize;
        if self.bytes[offset + 64..offset + DIRECTORY_ENTRY]
            .iter()
            .any(|byte| *byte != 0)
            || start < GENERATIONS_OFFSET
            || start % 4096 != 0
            || release_start < RELEASES_OFFSET
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
        if release_end > GENERATIONS_OFFSET {
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
    if bytes.len() != BOOT_STORE_BYTES {
        return Err(BootError::BadDirectoryBounds);
    }
    let header = bytes
        .get(DIRECTORY_OFFSET..DIRECTORY_OFFSET + DIRECTORY_HEADER)
        .ok_or(BootError::Truncated)?;
    if header[..8] != DIRECTORY_MAGIC {
        return Err(BootError::BadDirectoryMagic);
    }
    if u32_at(header, 8)? != DIRECTORY_VERSION || u32_at(header, 12)? as usize != DIRECTORY_HEADER {
        return Err(BootError::UnsupportedDirectoryVersion);
    }
    if u64_at(header, 16)? != 0 {
        return Err(BootError::UnknownDirectoryFlags);
    }
    let count = u32_at(header, 24)? as usize;
    if u32_at(header, 28)? != 0
        || !(1..=64).contains(&count)
        || u64_at(header, 32)? as usize != count * DIRECTORY_ENTRY
        || u64_at(header, 40)? as usize != bytes.len()
    {
        return Err(BootError::BadDirectoryBounds);
    }
    let expected: [u8; 32] = header[48..80].try_into().unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes[SLOT_BYTES * 2..DIRECTORY_OFFSET + 48]);
    hasher.update(&[0u8; 32]);
    hasher.update(&bytes[DIRECTORY_OFFSET + 80..]);
    if hasher.finalize() != expected {
        return Err(BootError::BadDirectoryHash);
    }
    let directory = BootDirectory {
        bytes,
        count,
        entry_offset: DIRECTORY_OFFSET + DIRECTORY_HEADER,
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
