//! GPT validation and store-partition selection (M5.4).
//!
//! Validates the protective MBR, the primary and backup GPT header copies,
//! partition-entry-array bounds and CRCs, and rejects overlapping partitions,
//! integer overflow, and unsupported versions before any partition byte is
//! exposed. Copy selection follows one documented rule: a single valid copy
//! is used with the damage reported; when both copies validate they must
//! agree on disk GUID and table contents, otherwise the device is rejected
//! as conflicting. Partition selection happens only here, so every store
//! byte stays inside the validated partition bounds.

use alloc::vec::Vec;

use crate::block_proto::SECTOR_SIZE;
use crate::crc32::crc32;

/// Partition type GUID marking the Slime OS object-store partition. Stored
/// and compared as raw GPT bytes on both the host builder and the kernel.
pub const SLIME_STORE_TYPE_GUID: [u8; 16] = *b"SLIMEOSSTOREGPT!";

const GPT_MAGIC: [u8; 8] = *b"EFI PART";
const GPT_VERSION: u32 = 0x0001_0000;
const MIN_HEADER_SIZE: u32 = 92;
const PMBR_TYPE: u8 = 0xEE;
const PMBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];
const PMBR_ENTRIES_OFFSET: usize = 446;
const PMBR_ENTRY_SIZE: usize = 16;

/// Hard bound on the partition entry count accepted from a header. The UEFI
/// minimum table is 128 entries; larger declared tables are rejected as
/// out-of-bounds rather than read unboundedly.
pub const MAX_PARTITION_ENTRIES: u32 = 128;
const MIN_ENTRY_SIZE: u32 = 128;
const MAX_ENTRY_SIZE: u32 = 512;

/// Reads one 512-byte sector by absolute LBA into `out`. The store service
/// backs this with the shared virtio device; tests back it with mock disks.
pub type SectorReader<'a> = dyn FnMut(u64, &mut [u8; SECTOR_SIZE]) -> Result<(), GptError> + 'a;

/// Every way GPT validation can fail. Total: malformed metadata maps to one
/// of these, never to a panic or an out-of-bounds device request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptError {
    /// The sector reader itself failed.
    Device,
    /// No protective MBR signature or 0xEE protective entry.
    ProtectiveMbr,
    BadMagic,
    UnsupportedVersion,
    BadHeaderSize,
    BadHeaderCrc,
    BadEntriesCrc,
    /// A declared offset, count, or partition range leaves the device or the
    /// usable LBA span.
    OutOfBounds,
    /// Checked arithmetic overflowed.
    Overflow,
    /// Two in-use partition entries cover the same LBA.
    Overlap,
    /// Neither header copy validates.
    NoValidCopy,
    /// Both copies validate but disagree on disk identity or table contents.
    ConflictingCopies,
    NoStorePartition,
    AmbiguousStorePartition,
}

/// A validated, bounded partition range (inclusive LBAs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub first_lba: u64,
    pub last_lba: u64,
    pub type_guid: [u8; 16],
}

/// Which copy satisfied validation, and what happened to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Both copies validated and agree; the primary was used.
    None,
    /// The backup was damaged; the primary was used.
    BackupDamaged(GptError),
    /// The primary was damaged; the backup was used.
    PrimaryDamaged(GptError),
}

/// The validated object-store partition plus the recovery report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorePartition {
    pub partition: Partition,
    pub recovery: Recovery,
}

#[derive(Debug, Clone, Copy)]
struct Header {
    backup_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: [u8; 16],
    entries_lba: u64,
    entry_count: u32,
    entry_size: u32,
    entries_crc: u32,
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32 field"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("u64 field"))
}

fn check_pmbr(sector: &[u8; SECTOR_SIZE]) -> Result<(), GptError> {
    if sector[SECTOR_SIZE - 2..] != PMBR_SIGNATURE {
        return Err(GptError::ProtectiveMbr);
    }
    let protective =
        (0..4).any(|index| sector[PMBR_ENTRIES_OFFSET + index * PMBR_ENTRY_SIZE + 4] == PMBR_TYPE);
    if !protective {
        return Err(GptError::ProtectiveMbr);
    }
    Ok(())
}

fn parse_header(sector: &[u8; SECTOR_SIZE], expected_lba: u64) -> Result<Header, GptError> {
    if sector[..8] != GPT_MAGIC {
        return Err(GptError::BadMagic);
    }
    if read_u32(sector, 8) != GPT_VERSION {
        return Err(GptError::UnsupportedVersion);
    }
    let header_size = read_u32(sector, 12);
    if header_size < MIN_HEADER_SIZE || header_size as usize > SECTOR_SIZE {
        return Err(GptError::BadHeaderSize);
    }
    let stored_crc = read_u32(sector, 16);
    let mut covered = [0u8; SECTOR_SIZE];
    covered[..header_size as usize].copy_from_slice(&sector[..header_size as usize]);
    covered[16..20].fill(0);
    if crc32(&covered[..header_size as usize]) != stored_crc {
        return Err(GptError::BadHeaderCrc);
    }
    if read_u64(sector, 24) != expected_lba {
        return Err(GptError::OutOfBounds);
    }
    let header = Header {
        backup_lba: read_u64(sector, 32),
        first_usable: read_u64(sector, 40),
        last_usable: read_u64(sector, 48),
        disk_guid: sector[56..72].try_into().expect("disk GUID field"),
        entries_lba: read_u64(sector, 72),
        entry_count: read_u32(sector, 80),
        entry_size: read_u32(sector, 84),
        entries_crc: read_u32(sector, 88),
    };
    if header.entry_count == 0
        || header.entry_count > MAX_PARTITION_ENTRIES
        || header.entry_size < MIN_ENTRY_SIZE
        || header.entry_size > MAX_ENTRY_SIZE
        || !header.entry_size.is_multiple_of(8)
    {
        return Err(GptError::OutOfBounds);
    }
    Ok(header)
}

/// Read and CRC-check one partition entry array. This is a copy-integrity
/// check only: a failure here (bad array location or CRC) is recoverable
/// from the other copy. Returns the validated entry bytes (array length).
fn read_entries(
    reader: &mut SectorReader<'_>,
    header: &Header,
    capacity: u64,
) -> Result<Vec<u8>, GptError> {
    let array_bytes = (header.entry_count as usize)
        .checked_mul(header.entry_size as usize)
        .ok_or(GptError::Overflow)?;
    let array_sectors = array_bytes.div_ceil(SECTOR_SIZE) as u64;
    let array_end = header
        .entries_lba
        .checked_add(array_sectors)
        .ok_or(GptError::Overflow)?;
    if header.entries_lba < 2 || array_end > capacity {
        return Err(GptError::OutOfBounds);
    }

    let mut bytes = alloc::vec![0u8; array_sectors as usize * SECTOR_SIZE];
    for index in 0..array_sectors {
        let lba = header
            .entries_lba
            .checked_add(index)
            .ok_or(GptError::Overflow)?;
        let sector: &mut [u8; SECTOR_SIZE] = (&mut bytes
            [index as usize * SECTOR_SIZE..(index as usize + 1) * SECTOR_SIZE])
            .try_into()
            .expect("sector-aligned entry buffer");
        reader(lba, sector)?;
    }
    if crc32(&bytes[..array_bytes]) != header.entries_crc {
        return Err(GptError::BadEntriesCrc);
    }
    bytes.truncate(array_bytes);
    Ok(bytes)
}

/// Parse and bound every in-use entry from a CRC-validated entry array.
/// These checks cover the metadata's semantic content, shared by both
/// copies, so a failure here is a hard reject (`OutOfBounds`/`Overlap`) that
/// no other copy can recover. Runs once, after copy selection.
fn parse_partitions(
    entry_bytes: &[u8],
    header: &Header,
    capacity: u64,
) -> Result<Vec<Partition>, GptError> {
    if header.first_usable < 2
        || header.last_usable < header.first_usable
        || header.last_usable >= capacity
    {
        return Err(GptError::OutOfBounds);
    }

    let mut partitions = Vec::new();
    for index in 0..header.entry_count as usize {
        let entry = &entry_bytes[index * header.entry_size as usize..];
        let type_guid: [u8; 16] = entry[..16].try_into().expect("type GUID field");
        if type_guid == [0u8; 16] {
            continue;
        }
        let first_lba = read_u64(entry, 32);
        let last_lba = read_u64(entry, 40);
        if first_lba > last_lba || first_lba < header.first_usable || last_lba > header.last_usable
        {
            return Err(GptError::OutOfBounds);
        }
        partitions.push(Partition {
            first_lba,
            last_lba,
            type_guid,
        });
    }
    partitions.sort_by_key(|partition| partition.first_lba);
    for pair in partitions.windows(2) {
        if pair[1].first_lba <= pair[0].last_lba {
            return Err(GptError::Overlap);
        }
    }
    Ok(partitions)
}

/// One full copy's integrity: header at `header_lba` plus its entry array,
/// with only CRC/structure checked. Partition semantics are deferred to
/// `parse_partitions` so shared malformed content is not misreported as
/// recoverable copy damage.
fn validate_copy(
    reader: &mut SectorReader<'_>,
    capacity: u64,
    header_lba: u64,
) -> Result<(Header, Vec<u8>), GptError> {
    if header_lba == 0 || header_lba >= capacity {
        return Err(GptError::OutOfBounds);
    }
    let mut sector = [0u8; SECTOR_SIZE];
    reader(header_lba, &mut sector)?;
    let header = parse_header(&sector, header_lba)?;
    // Cross-pointer sanity: the primary names the backup at the last LBA;
    // the backup names the primary at LBA 1.
    let expected_backup = if header_lba == 1 { capacity - 1 } else { 1 };
    if header.backup_lba != expected_backup {
        return Err(GptError::OutOfBounds);
    }
    let entry_bytes = read_entries(reader, &header, capacity)?;
    Ok((header, entry_bytes))
}

/// Validate both GPT copies and select the object-store partition.
///
/// `reader` fetches one 512-byte sector by absolute LBA; `capacity` is the
/// device size in sectors. Copy-conflict rule: when both copies validate,
/// they must agree on disk GUID and entry-array CRC, otherwise the device is
/// rejected (`ConflictingCopies`) rather than guessed. Partition bounds and
/// overlaps are checked once on the selected copy, so shared malformed
/// metadata is a hard reject, not a false recovery.
pub fn validate_store_partition(
    reader: &mut SectorReader<'_>,
    capacity: u64,
) -> Result<StorePartition, GptError> {
    if capacity < 3 {
        return Err(GptError::OutOfBounds);
    }
    let mut pmbr = [0u8; SECTOR_SIZE];
    reader(0, &mut pmbr)?;
    check_pmbr(&pmbr)?;

    let primary = validate_copy(reader, capacity, 1);
    let backup_lba = capacity - 1;
    let backup = validate_copy(reader, capacity, backup_lba);

    let (header, entry_bytes, recovery) = match (primary, backup) {
        (Ok((primary_header, primary_entries)), Ok((backup_header, backup_entries))) => {
            if primary_header.disk_guid != backup_header.disk_guid
                || primary_header.entries_crc != backup_header.entries_crc
            {
                return Err(GptError::ConflictingCopies);
            }
            drop(backup_entries);
            (primary_header, primary_entries, Recovery::None)
        }
        (Ok((primary_header, primary_entries)), Err(error)) => (
            primary_header,
            primary_entries,
            Recovery::BackupDamaged(error),
        ),
        (Err(error), Ok((backup_header, backup_entries))) => (
            backup_header,
            backup_entries,
            Recovery::PrimaryDamaged(error),
        ),
        (Err(_), Err(_)) => return Err(GptError::NoValidCopy),
    };

    let partitions = parse_partitions(&entry_bytes, &header, capacity)?;
    let mut matches = partitions
        .iter()
        .filter(|partition| partition.type_guid == SLIME_STORE_TYPE_GUID);
    let Some(partition) = matches.next() else {
        return Err(GptError::NoStorePartition);
    };
    if matches.next().is_some() {
        return Err(GptError::AmbiguousStorePartition);
    }
    Ok(StorePartition {
        partition: *partition,
        recovery,
    })
}
