//! Integrity-checked, content-addressed object store (M5.4).
//!
//! Objects are immutable records addressed by the SHA-256 of their payload.
//! The store appends new records without modifying existing object bytes and
//! commits metadata through two fixed superblock slots: a commit writes the
//! older slot only after the record data is flushed, so an interruption at
//! any append/commit boundary preserves the previously committed root.
//!
//! Layout inside a validated GPT partition (LBAs are partition-relative):
//!
//! ```text
//! LBA 0: superblock slot A (one sector)
//! LBA 1: superblock slot B (one sector)
//! LBA 2..: append-only object records (header + payload, sector aligned)
//! ```
//!
//! Superblock header (64 bytes, CRC-32 over the first 60):
//!   u8[8] magic, u32 version, u32 header_size, u64 sequence,
//!   u64 append_lba, u32 object_count, u32 flags,
//!   u64 record_area_start, u64 partition_sectors, u32 reserved, u32 crc32
//!
//! Record header (64 bytes):
//!   u8[8] magic, u32 version, u32 header_size, u32 obj_type, u32 flags,
//!   u64 payload_len, u8[32] content_hash (SHA-256 of payload)

use alloc::vec::Vec;

use crate::crc32::crc32;
use crate::gpt::Partition;
use crate::sha256;
use crate::store_disk::SECTOR_BYTES as SECTOR_SIZE;

pub use crate::store_disk::{
    FORMAT_VERSION, MAX_OBJECT_PAYLOAD, MAX_OBJECTS, RECORD_AREA_START, RECORD_HEADER,
    RECORD_MAGIC, SUPERBLOCK_HEADER, SUPERBLOCK_MAGIC,
};
use crate::store_disk::{
    RECORD_CONTENT_HASH_OFFSET, RECORD_FORMAT_VERSION_OFFSET, RECORD_HEADER_SIZE_OFFSET,
    RECORD_OBJ_TYPE_OFFSET, RECORD_PAYLOAD_LEN_OFFSET, SLOT_A_LBA, SLOT_B_LBA,
    SUPERBLOCK_APPEND_LBA_OFFSET, SUPERBLOCK_CRC32_OFFSET, SUPERBLOCK_FLAGS_OFFSET,
    SUPERBLOCK_FORMAT_VERSION_OFFSET, SUPERBLOCK_HEADER_SIZE_OFFSET,
    SUPERBLOCK_OBJECT_COUNT_OFFSET, SUPERBLOCK_PARTITION_SECTORS_OFFSET,
    SUPERBLOCK_RECORD_AREA_START_OFFSET, SUPERBLOCK_RESERVED_OFFSET, SUPERBLOCK_SEQUENCE_OFFSET,
};

/// The device surface the store needs. Implemented by `VirtioBlock` for the
/// syscall service and by mock disks in tests.
pub trait BlockIo {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), IoError>;
    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoError {
    Device,
    Timeout,
}

/// Why one superblock slot failed to decode. Reported for observability; a
/// store opens as long as one slot is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperblockError {
    BadMagic,
    UnsupportedVersion,
    BadHeaderSize,
    BadCrc,
    BadBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Io(IoError),
    PartitionTooSmall,
    NoValidSuperblock,
    CorruptRecord,
    TooManyObjects,
    StoreFull,
    NotFound,
    PayloadTooLarge,
    BufferTooSmall,
    DuplicateIdentity,
    HashMismatch,
}

impl From<IoError> for StoreError {
    fn from(value: IoError) -> Self {
        StoreError::Io(value)
    }
}

/// Committed store metadata carried by each superblock slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub sequence: u64,
    pub append_lba: u64,
    pub object_count: u32,
}

/// One indexed object: where it starts and how to address it by content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub hash: [u8; 32],
    pub obj_type: u32,
    pub payload_len: u32,
    pub lba: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    A,
    B,
}

impl Slot {
    fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }

    fn lba(self) -> u64 {
        match self {
            Slot::A => SLOT_A_LBA,
            Slot::B => SLOT_B_LBA,
        }
    }
}

fn record_sectors(payload_len: u64) -> Result<u64, StoreError> {
    let bytes = RECORD_HEADER
        .checked_add(payload_len as usize)
        .ok_or(StoreError::CorruptRecord)?;
    Ok(bytes.div_ceil(SECTOR_SIZE) as u64)
}

pub fn encode_superblock(superblock: &Superblock, partition_sectors: u64) -> [u8; SECTOR_SIZE] {
    let mut sector = [0u8; SECTOR_SIZE];
    sector[..8].copy_from_slice(&SUPERBLOCK_MAGIC);
    sector[SUPERBLOCK_FORMAT_VERSION_OFFSET..SUPERBLOCK_HEADER_SIZE_OFFSET]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    sector[SUPERBLOCK_HEADER_SIZE_OFFSET..SUPERBLOCK_SEQUENCE_OFFSET]
        .copy_from_slice(&(SUPERBLOCK_HEADER as u32).to_le_bytes());
    sector[SUPERBLOCK_SEQUENCE_OFFSET..SUPERBLOCK_APPEND_LBA_OFFSET]
        .copy_from_slice(&superblock.sequence.to_le_bytes());
    sector[SUPERBLOCK_APPEND_LBA_OFFSET..SUPERBLOCK_OBJECT_COUNT_OFFSET]
        .copy_from_slice(&superblock.append_lba.to_le_bytes());
    sector[SUPERBLOCK_OBJECT_COUNT_OFFSET..SUPERBLOCK_FLAGS_OFFSET]
        .copy_from_slice(&superblock.object_count.to_le_bytes());
    sector[SUPERBLOCK_FLAGS_OFFSET..SUPERBLOCK_RECORD_AREA_START_OFFSET]
        .copy_from_slice(&0u32.to_le_bytes());
    sector[SUPERBLOCK_RECORD_AREA_START_OFFSET..SUPERBLOCK_PARTITION_SECTORS_OFFSET]
        .copy_from_slice(&RECORD_AREA_START.to_le_bytes());
    sector[SUPERBLOCK_PARTITION_SECTORS_OFFSET..SUPERBLOCK_RESERVED_OFFSET]
        .copy_from_slice(&partition_sectors.to_le_bytes());
    let crc = crc32(&sector[..SUPERBLOCK_CRC32_OFFSET]);
    sector[SUPERBLOCK_CRC32_OFFSET..SUPERBLOCK_HEADER].copy_from_slice(&crc.to_le_bytes());
    sector
}

pub fn decode_superblock(
    sector: &[u8; SECTOR_SIZE],
    partition_sectors: u64,
) -> Result<Superblock, SuperblockError> {
    if sector[..8] != SUPERBLOCK_MAGIC {
        return Err(SuperblockError::BadMagic);
    }
    if u32_field(sector, SUPERBLOCK_FORMAT_VERSION_OFFSET) != FORMAT_VERSION {
        return Err(SuperblockError::UnsupportedVersion);
    }
    if u32_field(sector, SUPERBLOCK_HEADER_SIZE_OFFSET) != SUPERBLOCK_HEADER as u32 {
        return Err(SuperblockError::BadHeaderSize);
    }
    let stored_crc = u32_field(sector, SUPERBLOCK_CRC32_OFFSET);
    if crc32(&sector[..SUPERBLOCK_CRC32_OFFSET]) != stored_crc {
        return Err(SuperblockError::BadCrc);
    }
    let superblock = Superblock {
        sequence: u64_field(sector, SUPERBLOCK_SEQUENCE_OFFSET),
        append_lba: u64_field(sector, SUPERBLOCK_APPEND_LBA_OFFSET),
        object_count: u32_field(sector, SUPERBLOCK_OBJECT_COUNT_OFFSET),
    };
    let record_area_start = u64_field(sector, SUPERBLOCK_RECORD_AREA_START_OFFSET);
    let recorded_partition = u64_field(sector, SUPERBLOCK_PARTITION_SECTORS_OFFSET);
    if record_area_start != RECORD_AREA_START
        || recorded_partition != partition_sectors
        || superblock.append_lba < RECORD_AREA_START
        || superblock.append_lba > partition_sectors
        || superblock.object_count as usize > MAX_OBJECTS
        || superblock.sequence == u64::MAX
    {
        return Err(SuperblockError::BadBounds);
    }
    Ok(superblock)
}

pub fn encode_record_header(obj_type: u32, payload: &[u8], hash: &[u8; 32]) -> [u8; RECORD_HEADER] {
    let mut header = [0u8; RECORD_HEADER];
    header[..8].copy_from_slice(&RECORD_MAGIC);
    header[RECORD_FORMAT_VERSION_OFFSET..RECORD_HEADER_SIZE_OFFSET]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    header[RECORD_HEADER_SIZE_OFFSET..RECORD_OBJ_TYPE_OFFSET]
        .copy_from_slice(&(RECORD_HEADER as u32).to_le_bytes());
    header[RECORD_OBJ_TYPE_OFFSET..RECORD_OBJ_TYPE_OFFSET + 4]
        .copy_from_slice(&obj_type.to_le_bytes());
    header[RECORD_PAYLOAD_LEN_OFFSET..RECORD_CONTENT_HASH_OFFSET]
        .copy_from_slice(&(payload.len() as u64).to_le_bytes());
    header[RECORD_CONTENT_HASH_OFFSET..RECORD_HEADER].copy_from_slice(hash);
    header
}

pub fn decode_record_header(sector: &[u8; SECTOR_SIZE]) -> Result<Entry, StoreError> {
    if sector[..8] != RECORD_MAGIC {
        return Err(StoreError::CorruptRecord);
    }
    if u32_field(sector, RECORD_FORMAT_VERSION_OFFSET) != FORMAT_VERSION {
        return Err(StoreError::CorruptRecord);
    }
    if u32_field(sector, RECORD_HEADER_SIZE_OFFSET) != RECORD_HEADER as u32 {
        return Err(StoreError::CorruptRecord);
    }
    let payload_len = u64_field(sector, RECORD_PAYLOAD_LEN_OFFSET);
    if payload_len > MAX_OBJECT_PAYLOAD as u64 {
        return Err(StoreError::CorruptRecord);
    }
    Ok(Entry {
        hash: sector[RECORD_CONTENT_HASH_OFFSET..RECORD_HEADER]
            .try_into()
            .expect("hash field"),
        obj_type: u32_field(sector, RECORD_OBJ_TYPE_OFFSET),
        payload_len: payload_len as u32,
        lba: 0,
    })
}

fn u32_field(sector: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(sector[offset..offset + 4].try_into().expect("u32 field"))
}

fn u64_field(sector: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(sector[offset..offset + 8].try_into().expect("u64 field"))
}

/// An open object store: validated metadata plus the bounded object index.
pub struct ObjectStore {
    first_lba: u64,
    partition_sectors: u64,
    sequence: u64,
    append_lba: u64,
    active: Slot,
    entries: Vec<Entry>,
}

impl ObjectStore {
    /// Open the store in `partition`: validate both superblock slots, pick
    /// the newest valid root, and scan the committed record area. Records
    /// beyond the committed append offset (interrupted appends) are never
    /// examined. All arithmetic is checked; malformed committed metadata
    /// fails before any out-of-bounds device request.
    pub fn open(io: &mut impl BlockIo, partition: &Partition) -> Result<Self, StoreError> {
        let partition_sectors = partition
            .last_lba
            .checked_sub(partition.first_lba)
            .and_then(|span| span.checked_add(1))
            .ok_or(StoreError::PartitionTooSmall)?;
        if partition_sectors < RECORD_AREA_START + 1 {
            return Err(StoreError::PartitionTooSmall);
        }

        let mut slot_sector = [0u8; SECTOR_SIZE];
        io.read_sector(partition.first_lba + SLOT_A_LBA, &mut slot_sector)?;
        let slot_a = decode_superblock(&slot_sector, partition_sectors).ok();
        io.read_sector(partition.first_lba + SLOT_B_LBA, &mut slot_sector)?;
        let slot_b = decode_superblock(&slot_sector, partition_sectors).ok();

        let (active, superblock) = match (slot_a, slot_b) {
            (Some(a), Some(b)) => {
                if a.sequence >= b.sequence {
                    (Slot::A, a)
                } else {
                    (Slot::B, b)
                }
            }
            (Some(a), None) => (Slot::A, a),
            (None, Some(b)) => (Slot::B, b),
            (None, None) => return Err(StoreError::NoValidSuperblock),
        };

        let mut entries = Vec::new();
        let mut lba = RECORD_AREA_START;
        while lba < superblock.append_lba {
            let mut header_sector = [0u8; SECTOR_SIZE];
            io.read_sector(partition.first_lba + lba, &mut header_sector)?;
            let mut entry = decode_record_header(&header_sector)?;
            let sectors = record_sectors(entry.payload_len as u64)?;
            let end = lba.checked_add(sectors).ok_or(StoreError::CorruptRecord)?;
            if end > superblock.append_lba {
                return Err(StoreError::CorruptRecord);
            }
            if entries.len() >= MAX_OBJECTS {
                return Err(StoreError::TooManyObjects);
            }
            entry.lba = lba;
            entries.push(entry);
            lba = end;
        }
        if entries.len() != superblock.object_count as usize {
            return Err(StoreError::CorruptRecord);
        }

        Ok(Self {
            first_lba: partition.first_lba,
            partition_sectors,
            sequence: superblock.sequence,
            append_lba: superblock.append_lba,
            active,
            entries,
        })
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn object_count(&self) -> usize {
        self.entries.len()
    }

    pub fn append_lba(&self) -> u64 {
        self.append_lba
    }

    /// Look up an object by content hash without touching the device.
    pub fn stat(&self, hash: &[u8; 32]) -> Option<(u32, u32)> {
        self.entries
            .iter()
            .find(|entry| &entry.hash == hash)
            .map(|entry| (entry.obj_type, entry.payload_len))
    }

    /// Retrieve an object's payload. The payload is returned only after its
    /// complete SHA-256 re-verifies against the record's content hash, so a
    /// corrupted object is never handed out as valid.
    pub fn get(
        &self,
        io: &mut impl BlockIo,
        hash: &[u8; 32],
        out: &mut [u8],
    ) -> Result<(u32, usize), StoreError> {
        let entry = *self
            .entries
            .iter()
            .find(|entry| &entry.hash == hash)
            .ok_or(StoreError::NotFound)?;
        let len = entry.payload_len as usize;
        if out.len() < len {
            return Err(StoreError::BufferTooSmall);
        }
        let payload = self.read_payload(io, &entry)?;
        if sha256::digest(&payload) != *hash {
            return Err(StoreError::HashMismatch);
        }
        out[..len].copy_from_slice(&payload);
        Ok((entry.obj_type, len))
    }

    /// Re-read and hash every committed object record. Opening validates the
    /// superblock and record bounds; scrub additionally proves payload
    /// integrity for objects outside the selected state closure.
    pub fn scrub(&self, io: &mut impl BlockIo) -> Result<(), StoreError> {
        for entry in &self.entries {
            let payload = self.read_payload(io, entry)?;
            if sha256::digest(&payload) != entry.hash {
                return Err(StoreError::HashMismatch);
            }
        }
        Ok(())
    }

    /// Append and seal a new object. Identical content already present is an
    /// idempotent no-op returning the existing identity; the same identity
    /// with different payload bytes is rejected. Commit order is record
    /// sectors, flush, superblock into the older slot, flush — an
    /// interruption anywhere leaves the previously committed root intact.
    pub fn put(
        &mut self,
        io: &mut impl BlockIo,
        obj_type: u32,
        payload: &[u8],
    ) -> Result<[u8; 32], StoreError> {
        if payload.len() > MAX_OBJECT_PAYLOAD {
            return Err(StoreError::PayloadTooLarge);
        }
        let hash = sha256::digest(payload);
        if let Some(entry) = self.entries.iter().find(|entry| entry.hash == hash) {
            let existing = self.read_payload(io, entry)?;
            if existing == payload {
                return Ok(hash);
            }
            return Err(StoreError::DuplicateIdentity);
        }
        if self.entries.len() >= MAX_OBJECTS {
            return Err(StoreError::StoreFull);
        }
        let sectors = record_sectors(payload.len() as u64)?;
        let end = self
            .append_lba
            .checked_add(sectors)
            .ok_or(StoreError::StoreFull)?;
        if end > self.partition_sectors {
            return Err(StoreError::StoreFull);
        }
        // Fail before any device write if the monotonic sequence would wrap;
        // a wrapped commit could make a stale slot outrank the new root.
        let next_sequence = self.sequence.checked_add(1).ok_or(StoreError::StoreFull)?;

        let header = encode_record_header(obj_type, payload, &hash);
        let mut record = alloc::vec![0u8; sectors as usize * SECTOR_SIZE];
        record[..RECORD_HEADER].copy_from_slice(&header);
        record[RECORD_HEADER..RECORD_HEADER + payload.len()].copy_from_slice(payload);
        for index in 0..sectors {
            let start = index as usize * SECTOR_SIZE;
            let sector: &[u8; SECTOR_SIZE] = record[start..start + SECTOR_SIZE]
                .try_into()
                .expect("sector-aligned record");
            io.write_sector(self.first_lba + self.append_lba + index, sector)?;
        }
        io.flush()?;

        let target = self.active.other();
        let superblock = Superblock {
            sequence: next_sequence,
            append_lba: end,
            object_count: (self.entries.len() + 1) as u32,
        };
        let sector = encode_superblock(&superblock, self.partition_sectors);
        io.write_sector(self.first_lba + target.lba(), &sector)?;
        io.flush()?;

        self.sequence = superblock.sequence;
        self.append_lba = end;
        self.active = target;
        self.entries.push(Entry {
            hash,
            obj_type,
            payload_len: payload.len() as u32,
            lba: superblock.append_lba - sectors,
        });
        Ok(hash)
    }

    fn read_payload(&self, io: &mut impl BlockIo, entry: &Entry) -> Result<Vec<u8>, StoreError> {
        let sectors = record_sectors(entry.payload_len as u64)?;
        let mut bytes = alloc::vec![0u8; sectors as usize * SECTOR_SIZE];
        for index in 0..sectors {
            let start = index as usize * SECTOR_SIZE;
            let sector: &mut [u8; SECTOR_SIZE] = (&mut bytes[start..start + SECTOR_SIZE])
                .try_into()
                .expect("sector-aligned buffer");
            io.read_sector(self.first_lba + entry.lba + index, sector)?;
        }
        let len = entry.payload_len as usize;
        Ok(bytes[RECORD_HEADER..RECORD_HEADER + len].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_LBA: u64 = 8;
    const SECTORS: u64 = 96;

    /// An in-memory block device that can be told to fail at the Nth write.
    ///
    /// The failure counter is what makes crash consistency testable at all: the
    /// store's commit protocol is write-record, flush, write-superblock, flush,
    /// and interrupting each boundary in turn is exactly the property
    /// `object_store.rs` documents but nothing exercised.
    struct MemoryDisk {
        sectors: alloc::vec::Vec<[u8; SECTOR_SIZE]>,
        writes: usize,
        fail_write_after: Option<usize>,
        flushes: usize,
    }

    impl MemoryDisk {
        fn new() -> Self {
            Self {
                sectors: alloc::vec![[0u8; SECTOR_SIZE]; (FIRST_LBA + SECTORS) as usize],
                writes: 0,
                fail_write_after: None,
                flushes: 0,
            }
        }

        /// Every byte outside the partition, so a test can prove the store never
        /// writes past its own span.
        fn outside_partition(&self) -> alloc::vec::Vec<u8> {
            let mut out = alloc::vec::Vec::new();
            for (lba, sector) in self.sectors.iter().enumerate() {
                if (lba as u64) < FIRST_LBA {
                    out.extend_from_slice(sector);
                }
            }
            out
        }
    }

    impl BlockIo for MemoryDisk {
        fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), IoError> {
            let sector = self.sectors.get(lba as usize).ok_or(IoError::Device)?;
            out.copy_from_slice(sector);
            Ok(())
        }

        fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), IoError> {
            if let Some(limit) = self.fail_write_after
                && self.writes >= limit
            {
                return Err(IoError::Device);
            }
            self.writes += 1;
            let sector = self.sectors.get_mut(lba as usize).ok_or(IoError::Device)?;
            sector.copy_from_slice(data);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), IoError> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn partition() -> Partition {
        Partition {
            first_lba: FIRST_LBA,
            last_lba: FIRST_LBA + SECTORS - 1,
            type_guid: crate::gpt::SLIME_STORE_TYPE_GUID,
        }
    }

    /// A freshly formatted store: slot A carries the genesis root and the record
    /// area is empty.
    fn formatted() -> MemoryDisk {
        let mut disk = MemoryDisk::new();
        let genesis = Superblock {
            sequence: 1,
            append_lba: RECORD_AREA_START,
            object_count: 0,
        };
        let sector = encode_superblock(&genesis, SECTORS);
        disk.sectors[(FIRST_LBA + SLOT_A_LBA) as usize] = sector;
        disk.writes = 0;
        disk
    }

    fn open(disk: &mut MemoryDisk) -> ObjectStore {
        ObjectStore::open(disk, &partition()).expect("store opens")
    }

    /// Without this the refusal corpus below could pass on a store that refuses
    /// everything.
    #[test]
    fn an_object_round_trips_through_a_reopen() {
        let mut disk = formatted();
        let hash = {
            let mut store = open(&mut disk);
            let hash = store.put(&mut disk, 7, b"payload").expect("put");
            assert_eq!(store.object_count(), 1);
            hash
        };
        // Reopened from bytes alone: the entry must be reconstructed from the
        // committed root and the record area, not from in-memory state.
        let store = open(&mut disk);
        assert_eq!(store.object_count(), 1);
        assert_eq!(store.stat(&hash), Some((7, b"payload".len() as u32)));
        let mut out = alloc::vec![0u8; b"payload".len()];
        let (obj_type, read) = store.get(&mut disk, &hash, &mut out).expect("get");
        assert_eq!(obj_type, 7);
        assert_eq!(&out[..read], b"payload");
    }

    /// Content addressing: the same bytes put twice is the same object, not a
    /// second copy and not an error.
    #[test]
    fn putting_identical_content_twice_is_idempotent() {
        let mut disk = formatted();
        let mut store = open(&mut disk);
        let first = store.put(&mut disk, 1, b"same").expect("first put");
        let appended = store.append_lba();
        let second = store.put(&mut disk, 1, b"same").expect("second put");
        assert_eq!(first, second);
        assert_eq!(store.object_count(), 1);
        assert_eq!(store.append_lba(), appended, "no second record was written");
    }

    /// The commit protocol is write-record, flush, write-superblock, flush. A
    /// failure at *any* write must leave the previously committed root intact —
    /// that is the crash-consistency claim, and this is the test that makes it
    /// evidence rather than a comment.
    #[test]
    fn an_interrupted_append_leaves_the_previous_root_committed() {
        let mut disk = formatted();
        {
            let mut store = open(&mut disk);
            store.put(&mut disk, 1, b"first").expect("first put");
        }
        let committed = {
            let store = open(&mut disk);
            (store.sequence(), store.object_count(), store.append_lba())
        };

        // A put issues one write per record sector, then one superblock write.
        // The payload below spans two sectors, so the boundaries are: first
        // record sector, second record sector, superblock. Interrupting each in
        // turn is the whole commit protocol.
        for boundary in 0..3 {
            let mut interrupted = formatted();
            {
                let mut store = open(&mut interrupted);
                store.put(&mut interrupted, 1, b"first").expect("first put");
            }
            interrupted.writes = 0;
            interrupted.fail_write_after = Some(boundary);
            {
                let mut store = open(&mut interrupted);
                let payload = alloc::vec![0xABu8; SECTOR_SIZE + 32];
                let outcome = store.put(&mut interrupted, 2, &payload);
                assert!(outcome.is_err(), "boundary {boundary} should fail");
            }
            interrupted.fail_write_after = None;
            let store = open(&mut interrupted);
            assert_eq!(
                (store.sequence(), store.object_count(), store.append_lba()),
                committed,
                "boundary {boundary} moved the committed root",
            );
        }
    }

    /// Two superblock slots alternate, so the newest valid root wins and a
    /// half-written slot cannot outrank a complete one.
    #[test]
    fn the_newest_valid_slot_wins_and_slots_alternate() {
        let mut disk = formatted();
        let mut sequences = alloc::vec::Vec::new();
        for index in 0..3u32 {
            let mut store = open(&mut disk);
            store
                .put(&mut disk, index, &[index as u8; 16])
                .expect("put");
            sequences.push(store.sequence());
        }
        assert_eq!(sequences, alloc::vec![2, 3, 4], "sequence is monotonic");

        // Both slots must now hold *different* roots. If commits reused one slot
        // the other would still carry genesis, and a torn write to the live slot
        // would lose every object rather than falling back one commit — which is
        // the entire reason two slots exist.
        let slot_a = decode_superblock(&disk.sectors[(FIRST_LBA + SLOT_A_LBA) as usize], SECTORS)
            .expect("slot A is valid");
        let slot_b = decode_superblock(&disk.sectors[(FIRST_LBA + SLOT_B_LBA) as usize], SECTORS)
            .expect("slot B is valid");
        assert_ne!(
            slot_a.sequence, slot_b.sequence,
            "both slots carry the same root, so commits are not alternating",
        );
        let newest = slot_a.sequence.max(slot_b.sequence);
        let older = slot_a.sequence.min(slot_b.sequence);
        assert_eq!(newest, 4, "the newest slot is the last commit");
        assert_eq!(older, 3, "the older slot is the commit before it");

        // Destroying the newest slot must fall back to the one before it, not
        // to genesis and not to a failure.
        let mut damaged = MemoryDisk {
            sectors: disk.sectors.clone(),
            writes: 0,
            fail_write_after: None,
            flushes: 0,
        };
        let newest_lba = if slot_a.sequence > slot_b.sequence {
            SLOT_A_LBA
        } else {
            SLOT_B_LBA
        };
        damaged.sectors[(FIRST_LBA + newest_lba) as usize] = [0u8; SECTOR_SIZE];
        let fallback = ObjectStore::open(&mut damaged, &partition())
            .expect("the older slot still carries a committed root");
        assert_eq!(fallback.sequence(), older);
    }

    /// Both slots unreadable is a refusal, not a silent reformat: a store that
    /// re-genesised here would discard every committed object.
    #[test]
    fn a_store_with_no_valid_superblock_is_refused() {
        let mut disk = formatted();
        {
            let mut store = open(&mut disk);
            store.put(&mut disk, 1, b"data").expect("put");
        }
        disk.sectors[(FIRST_LBA + SLOT_A_LBA) as usize] = [0u8; SECTOR_SIZE];
        disk.sectors[(FIRST_LBA + SLOT_B_LBA) as usize] = [0u8; SECTOR_SIZE];
        assert!(ObjectStore::open(&mut disk, &partition()).is_err());
    }

    /// Content addressing is verified on read, so a record whose payload no
    /// longer hashes to its name is reported rather than returned.
    #[test]
    fn a_corrupted_payload_is_caught_on_read_and_by_scrub() {
        let mut disk = formatted();
        let hash = {
            let mut store = open(&mut disk);
            store.put(&mut disk, 1, b"trustworthy").expect("put")
        };
        let record_lba = (FIRST_LBA + RECORD_AREA_START) as usize;
        disk.sectors[record_lba][RECORD_HEADER] ^= 0xFF;

        let store = open(&mut disk);
        let mut out = alloc::vec![0u8; 64];
        assert!(store.get(&mut disk, &hash, &mut out).is_err());
        assert!(store.scrub(&mut disk).is_err());
    }

    /// A clean store scrubs clean, which is the control for the test above.
    #[test]
    fn an_intact_store_scrubs_clean() {
        let mut disk = formatted();
        {
            let mut store = open(&mut disk);
            store.put(&mut disk, 1, b"alpha").expect("put");
            store.put(&mut disk, 2, b"beta").expect("put");
        }
        let store = open(&mut disk);
        store.scrub(&mut disk).expect("intact store scrubs clean");
    }

    /// An oversized payload is refused before any device write, so a rejected
    /// put cannot leave a partial record behind.
    #[test]
    fn an_oversized_payload_is_refused_without_writing() {
        let mut disk = formatted();
        let mut store = open(&mut disk);
        let writes_before = disk.writes;
        let huge = alloc::vec![0u8; MAX_OBJECT_PAYLOAD + 1];
        assert_eq!(
            store.put(&mut disk, 1, &huge),
            Err(StoreError::PayloadTooLarge)
        );
        assert_eq!(disk.writes, writes_before, "a refused put wrote sectors");
    }

    /// The store never addresses a sector outside its partition. Checked by
    /// bytes rather than by reading the code, because an off-by-one in the
    /// `first_lba` arithmetic is exactly what this would catch.
    #[test]
    fn the_store_never_writes_outside_its_partition() {
        let mut disk = formatted();
        let before = disk.outside_partition();
        {
            let mut store = open(&mut disk);
            for index in 0..4u32 {
                store
                    .put(&mut disk, index, &[index as u8; 200])
                    .expect("put");
            }
        }
        assert_eq!(disk.outside_partition(), before);
    }

    /// GPT resolution and store opening composed, which is the shape a real mount
    /// takes. Tested together because each is correct alone and they still have
    /// to agree on one thing: `validate_store_partition` returns absolute LBAs,
    /// and `ObjectStore::open` adds `partition.first_lba` to every offset. A
    /// partition resolved at a non-zero origin is the case where a double-add or
    /// a missing add would show up, and neither module can catch that by itself.
    #[test]
    fn a_gpt_resolved_partition_opens_as_a_store() {
        use crate::gpt::{SLIME_STORE_TYPE_GUID, validate_store_partition};

        const CAPACITY: u64 = FIRST_LBA + SECTORS + 8;
        const ENTRY_SIZE: u32 = 128;
        const ENTRY_COUNT: u32 = 4;
        const ENTRIES_LBA: u64 = 2;
        const BACKUP_ENTRIES_LBA: u64 = CAPACITY - 2;

        let mut disk = MemoryDisk::new();
        disk.sectors.resize(CAPACITY as usize, [0u8; SECTOR_SIZE]);

        // Protective MBR.
        disk.sectors[0][446 + 4] = 0xEE;
        disk.sectors[0][510..512].copy_from_slice(&[0x55, 0xAA]);

        // One store partition, placed at the same origin the store tests use.
        let mut entries = alloc::vec![0u8; (ENTRY_COUNT * ENTRY_SIZE) as usize];
        entries[..16].copy_from_slice(&SLIME_STORE_TYPE_GUID);
        entries[32..40].copy_from_slice(&FIRST_LBA.to_le_bytes());
        entries[40..48].copy_from_slice(&(FIRST_LBA + SECTORS - 1).to_le_bytes());

        for lba in [ENTRIES_LBA, BACKUP_ENTRIES_LBA] {
            for (index, chunk) in entries.chunks(SECTOR_SIZE).enumerate() {
                disk.sectors[lba as usize + index][..chunk.len()].copy_from_slice(chunk);
            }
        }

        let write_header = |disk: &mut MemoryDisk, my_lba: u64, backup: u64, entries_lba: u64| {
            let mut sector = [0u8; SECTOR_SIZE];
            sector[..8].copy_from_slice(b"EFI PART");
            sector[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
            sector[12..16].copy_from_slice(&92u32.to_le_bytes());
            sector[24..32].copy_from_slice(&my_lba.to_le_bytes());
            sector[32..40].copy_from_slice(&backup.to_le_bytes());
            sector[40..48].copy_from_slice(&FIRST_LBA.to_le_bytes());
            sector[48..56].copy_from_slice(&(CAPACITY - 3).to_le_bytes());
            sector[56..72].copy_from_slice(b"SLIMEDISKGUID!!!");
            sector[72..80].copy_from_slice(&entries_lba.to_le_bytes());
            sector[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
            sector[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
            sector[88..92].copy_from_slice(&crc32(&entries).to_le_bytes());
            let mut covered = sector;
            covered[16..20].fill(0);
            sector[16..20].copy_from_slice(&crc32(&covered[..92]).to_le_bytes());
            disk.sectors[my_lba as usize] = sector;
        };
        write_header(&mut disk, 1, CAPACITY - 1, ENTRIES_LBA);
        write_header(&mut disk, CAPACITY - 1, 1, BACKUP_ENTRIES_LBA);

        // Genesis the store inside the partition GPT will resolve.
        let genesis = Superblock {
            sequence: 1,
            append_lba: RECORD_AREA_START,
            object_count: 0,
        };
        disk.sectors[(FIRST_LBA + SLOT_A_LBA) as usize] = encode_superblock(&genesis, SECTORS);

        let resolved = {
            let snapshot = disk.sectors.clone();
            let mut reader = move |lba: u64, out: &mut [u8; SECTOR_SIZE]| {
                out.copy_from_slice(
                    snapshot
                        .get(lba as usize)
                        .ok_or(crate::gpt::GptError::OutOfBounds)?,
                );
                Ok(())
            };
            validate_store_partition(&mut reader, CAPACITY).expect("GPT resolves the store")
        };
        assert_eq!(resolved.partition.first_lba, FIRST_LBA);

        let mut store =
            ObjectStore::open(&mut disk, &resolved.partition).expect("store opens in it");
        let hash = store.put(&mut disk, 3, b"mounted").expect("put");
        let reopened = ObjectStore::open(&mut disk, &resolved.partition).expect("store reopens");
        assert_eq!(reopened.stat(&hash), Some((3, b"mounted".len() as u32)));
    }

    /// A partition too small to hold two superblocks and one record is refused
    /// rather than opened into an unusable state.
    #[test]
    fn a_partition_too_small_is_refused() {
        let mut disk = formatted();
        let tiny = Partition {
            first_lba: FIRST_LBA,
            last_lba: FIRST_LBA + RECORD_AREA_START - 1,
            type_guid: crate::gpt::SLIME_STORE_TYPE_GUID,
        };
        assert_eq!(
            ObjectStore::open(&mut disk, &tiny).err(),
            Some(StoreError::PartitionTooSmall)
        );
    }
}
