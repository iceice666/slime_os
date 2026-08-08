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

use crate::crc32::crc32;
use crate::store_disk::SECTOR_BYTES as SECTOR_SIZE;

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

#[cfg(test)]
mod tests {
    use super::*;

    const CAPACITY: u64 = 64;
    const ENTRY_SIZE: u32 = 128;
    const ENTRY_COUNT: u32 = 4;
    const PRIMARY_ENTRIES_LBA: u64 = 2;
    const BACKUP_ENTRIES_LBA: u64 = CAPACITY - 2;
    const FIRST_USABLE: u64 = 4;
    const LAST_USABLE: u64 = CAPACITY - 3;

    /// An in-memory disk. `validate_store_partition` reads through a closure, so
    /// every case here is bytes only — no block device, which is the whole reason
    /// this half of M5.4 is portable at all.
    struct Disk {
        sectors: alloc::vec::Vec<[u8; SECTOR_SIZE]>,
    }

    impl Disk {
        fn new() -> Self {
            Self {
                sectors: alloc::vec![[0u8; SECTOR_SIZE]; CAPACITY as usize],
            }
        }

        fn reader(&self) -> impl FnMut(u64, &mut [u8; SECTOR_SIZE]) -> Result<(), GptError> + '_ {
            move |lba, out| {
                let sector = self
                    .sectors
                    .get(lba as usize)
                    .ok_or(GptError::OutOfBounds)?;
                out.copy_from_slice(sector);
                Ok(())
            }
        }

        fn validate(&self) -> Result<StorePartition, GptError> {
            let mut reader = self.reader();
            validate_store_partition(&mut reader, CAPACITY)
        }
    }

    fn write_pmbr(disk: &mut Disk) {
        let sector = &mut disk.sectors[0];
        sector[PMBR_ENTRIES_OFFSET + 4] = PMBR_TYPE;
        sector[510..512].copy_from_slice(&PMBR_SIGNATURE);
    }

    /// One in-use store partition, plus one unrelated partition so the
    /// store-selection rules have something to discriminate against.
    fn entry_table() -> alloc::vec::Vec<u8> {
        let mut entries = alloc::vec![0u8; (ENTRY_COUNT * ENTRY_SIZE) as usize];
        entries[..16].copy_from_slice(&SLIME_STORE_TYPE_GUID);
        entries[32..40].copy_from_slice(&8u64.to_le_bytes());
        entries[40..48].copy_from_slice(&23u64.to_le_bytes());

        let second = ENTRY_SIZE as usize;
        entries[second..second + 16].copy_from_slice(b"OTHERPARTITION!!");
        entries[second + 32..second + 40].copy_from_slice(&24u64.to_le_bytes());
        entries[second + 40..second + 48].copy_from_slice(&31u64.to_le_bytes());
        entries
    }

    fn write_entries(disk: &mut Disk, lba: u64, entries: &[u8]) {
        for (index, chunk) in entries.chunks(SECTOR_SIZE).enumerate() {
            disk.sectors[lba as usize + index][..chunk.len()].copy_from_slice(chunk);
        }
    }

    /// Build one header sector. `my_lba` distinguishes primary from backup, which
    /// `parse_header` checks — a copy written at the wrong LBA is rejected rather
    /// than accepted as the other one.
    fn write_header(
        disk: &mut Disk,
        my_lba: u64,
        backup_lba: u64,
        entries_lba: u64,
        entries: &[u8],
        disk_guid: [u8; 16],
    ) {
        let mut sector = [0u8; SECTOR_SIZE];
        sector[..8].copy_from_slice(&GPT_MAGIC);
        sector[8..12].copy_from_slice(&GPT_VERSION.to_le_bytes());
        sector[12..16].copy_from_slice(&MIN_HEADER_SIZE.to_le_bytes());
        sector[24..32].copy_from_slice(&my_lba.to_le_bytes());
        sector[32..40].copy_from_slice(&backup_lba.to_le_bytes());
        sector[40..48].copy_from_slice(&FIRST_USABLE.to_le_bytes());
        sector[48..56].copy_from_slice(&LAST_USABLE.to_le_bytes());
        sector[56..72].copy_from_slice(&disk_guid);
        sector[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        sector[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        sector[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        sector[88..92].copy_from_slice(&crc32(entries).to_le_bytes());

        // The header CRC covers the header with its own field zeroed, so it is
        // computed last and over exactly `header_size` bytes.
        let mut covered = sector;
        covered[16..20].fill(0);
        let crc = crc32(&covered[..MIN_HEADER_SIZE as usize]);
        sector[16..20].copy_from_slice(&crc.to_le_bytes());
        disk.sectors[my_lba as usize] = sector;
    }

    fn valid_disk() -> Disk {
        let mut disk = Disk::new();
        write_pmbr(&mut disk);
        let entries = entry_table();
        write_entries(&mut disk, PRIMARY_ENTRIES_LBA, &entries);
        write_entries(&mut disk, BACKUP_ENTRIES_LBA, &entries);
        write_header(
            &mut disk,
            1,
            CAPACITY - 1,
            PRIMARY_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        disk
    }

    /// Without this the refusal corpus below could pass on a validator that
    /// refuses everything.
    #[test]
    fn a_well_formed_disk_resolves_the_store_partition() {
        let found = valid_disk().validate().expect("valid GPT");
        assert_eq!(found.partition.first_lba, 8);
        assert_eq!(found.partition.last_lba, 23);
        assert_eq!(found.partition.type_guid, SLIME_STORE_TYPE_GUID);
        assert_eq!(found.recovery, Recovery::None);
    }

    /// The protective MBR is what stops a legacy tool from seeing free space on a
    /// GPT disk, so its absence is a refusal rather than a warning.
    #[test]
    fn a_missing_protective_mbr_is_refused() {
        let mut disk = valid_disk();
        disk.sectors[0] = [0u8; SECTOR_SIZE];
        assert_eq!(disk.validate(), Err(GptError::ProtectiveMbr));

        let mut disk = valid_disk();
        disk.sectors[0][510..512].fill(0);
        assert_eq!(disk.validate(), Err(GptError::ProtectiveMbr));

        let mut disk = valid_disk();
        disk.sectors[0][PMBR_ENTRIES_OFFSET + 4] = 0x83;
        assert_eq!(disk.validate(), Err(GptError::ProtectiveMbr));
    }

    /// Redundancy is the point of two copies: either one alone must carry the
    /// disk, and the report must say which was used so a caller can repair it.
    #[test]
    fn either_copy_alone_recovers_and_reports_which() {
        let mut disk = valid_disk();
        disk.sectors[(CAPACITY - 1) as usize] = [0u8; SECTOR_SIZE];
        let found = disk.validate().expect("primary carries the disk");
        assert_eq!(found.partition.first_lba, 8);
        assert!(matches!(found.recovery, Recovery::BackupDamaged(_)));

        let mut disk = valid_disk();
        disk.sectors[1] = [0u8; SECTOR_SIZE];
        let found = disk.validate().expect("backup carries the disk");
        assert_eq!(found.partition.first_lba, 8);
        assert!(matches!(found.recovery, Recovery::PrimaryDamaged(_)));
    }

    #[test]
    fn both_copies_damaged_is_refused() {
        let mut disk = valid_disk();
        disk.sectors[1] = [0u8; SECTOR_SIZE];
        disk.sectors[(CAPACITY - 1) as usize] = [0u8; SECTOR_SIZE];
        assert_eq!(disk.validate(), Err(GptError::NoValidCopy));
    }

    /// Two copies that each validate but disagree is worse than one damaged
    /// copy: there is no basis for choosing, so picking either could mount the
    /// wrong disk. Checked on both fields the comparison covers.
    #[test]
    fn two_valid_copies_that_disagree_are_refused() {
        let mut disk = valid_disk();
        let entries = entry_table();
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &entries,
            *b"DIFFERENTGUID!!!",
        );
        assert_eq!(disk.validate(), Err(GptError::ConflictingCopies));

        let mut disk = valid_disk();
        let mut divergent = entry_table();
        divergent[32..40].copy_from_slice(&9u64.to_le_bytes());
        write_entries(&mut disk, BACKUP_ENTRIES_LBA, &divergent);
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &divergent,
            *b"SLIMEDISKGUID!!!",
        );
        assert_eq!(disk.validate(), Err(GptError::ConflictingCopies));
    }

    /// Overlapping partitions mean one LBA has two owners, so a write through
    /// either corrupts the other. Refused before any partition is selected.
    #[test]
    fn overlapping_partitions_are_refused() {
        let mut disk = valid_disk();
        let mut entries = entry_table();
        let second = ENTRY_SIZE as usize;
        entries[second + 32..second + 40].copy_from_slice(&20u64.to_le_bytes());
        write_entries(&mut disk, PRIMARY_ENTRIES_LBA, &entries);
        write_entries(&mut disk, BACKUP_ENTRIES_LBA, &entries);
        write_header(
            &mut disk,
            1,
            CAPACITY - 1,
            PRIMARY_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        assert_eq!(disk.validate(), Err(GptError::Overlap));
    }

    /// A partition leaving the usable span would let the store write over GPT
    /// metadata, which is the failure the bounds exist to prevent.
    #[test]
    fn a_partition_outside_the_usable_span_is_refused() {
        for (first, last) in [(FIRST_USABLE - 1, 23u64), (8, LAST_USABLE + 1), (23, 8)] {
            let mut disk = valid_disk();
            let mut entries = entry_table();
            entries[32..40].copy_from_slice(&first.to_le_bytes());
            entries[40..48].copy_from_slice(&last.to_le_bytes());
            write_entries(&mut disk, PRIMARY_ENTRIES_LBA, &entries);
            write_entries(&mut disk, BACKUP_ENTRIES_LBA, &entries);
            write_header(
                &mut disk,
                1,
                CAPACITY - 1,
                PRIMARY_ENTRIES_LBA,
                &entries,
                *b"SLIMEDISKGUID!!!",
            );
            write_header(
                &mut disk,
                CAPACITY - 1,
                1,
                BACKUP_ENTRIES_LBA,
                &entries,
                *b"SLIMEDISKGUID!!!",
            );
            assert_eq!(
                disk.validate(),
                Err(GptError::OutOfBounds),
                "range {first}..={last}",
            );
        }
    }

    /// No store partition and two store partitions are different errors, because
    /// a caller can create the first and must never guess between the second.
    #[test]
    fn a_missing_or_ambiguous_store_partition_is_named_exactly() {
        let mut disk = valid_disk();
        let mut entries = entry_table();
        entries[..16].copy_from_slice(b"OTHERPARTITION!!");
        entries[32..40].copy_from_slice(&8u64.to_le_bytes());
        entries[40..48].copy_from_slice(&15u64.to_le_bytes());
        write_entries(&mut disk, PRIMARY_ENTRIES_LBA, &entries);
        write_entries(&mut disk, BACKUP_ENTRIES_LBA, &entries);
        write_header(
            &mut disk,
            1,
            CAPACITY - 1,
            PRIMARY_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        assert_eq!(disk.validate(), Err(GptError::NoStorePartition));

        let mut disk = valid_disk();
        let mut entries = entry_table();
        let second = ENTRY_SIZE as usize;
        entries[second..second + 16].copy_from_slice(&SLIME_STORE_TYPE_GUID);
        write_entries(&mut disk, PRIMARY_ENTRIES_LBA, &entries);
        write_entries(&mut disk, BACKUP_ENTRIES_LBA, &entries);
        write_header(
            &mut disk,
            1,
            CAPACITY - 1,
            PRIMARY_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        write_header(
            &mut disk,
            CAPACITY - 1,
            1,
            BACKUP_ENTRIES_LBA,
            &entries,
            *b"SLIMEDISKGUID!!!",
        );
        assert_eq!(disk.validate(), Err(GptError::AmbiguousStorePartition));
    }

    /// A header whose own CRC does not cover it is a damaged copy, so a disk with
    /// both CRCs broken has no valid copy at all.
    #[test]
    fn a_broken_header_crc_damages_that_copy() {
        let mut disk = valid_disk();
        disk.sectors[1][16] ^= 0x01;
        assert!(matches!(
            disk.validate().expect("backup still valid").recovery,
            Recovery::PrimaryDamaged(GptError::BadHeaderCrc)
        ));
    }

    /// The entry array has its own CRC, separate from the header's, so a table
    /// corrupted without touching the header is still caught.
    #[test]
    fn a_broken_entries_crc_damages_that_copy() {
        let mut disk = valid_disk();
        disk.sectors[PRIMARY_ENTRIES_LBA as usize][0] ^= 0xFF;
        assert!(matches!(
            disk.validate().expect("backup still valid").recovery,
            Recovery::PrimaryDamaged(GptError::BadEntriesCrc)
        ));
    }

    /// A device too small to hold a PMBR and both headers cannot be a GPT disk,
    /// and is refused before any read is attempted.
    #[test]
    fn a_device_too_small_for_gpt_is_refused() {
        let disk = valid_disk();
        for capacity in [0u64, 1, 2] {
            let mut reader = disk.reader();
            assert_eq!(
                validate_store_partition(&mut reader, capacity),
                Err(GptError::OutOfBounds),
                "capacity {capacity}",
            );
        }
    }

    /// A reader that fails is a device error, not a malformed disk — the two are
    /// distinct because only one of them is worth retrying.
    #[test]
    fn a_failing_reader_reports_a_device_error() {
        let mut reader = |_lba: u64, _out: &mut [u8; SECTOR_SIZE]| Err(GptError::Device);
        assert_eq!(
            validate_store_partition(&mut reader, CAPACITY),
            Err(GptError::Device)
        );
    }
}
