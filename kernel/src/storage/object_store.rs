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

use crate::block_proto::SECTOR_SIZE;
use crate::crc32::crc32;
use crate::gpt::Partition;
use crate::sha256;

pub use boot_contracts::store_disk::{
    FORMAT_VERSION, MAX_OBJECT_PAYLOAD, MAX_OBJECTS, RECORD_AREA_START, RECORD_HEADER,
    RECORD_MAGIC, SUPERBLOCK_HEADER, SUPERBLOCK_MAGIC,
};
use boot_contracts::store_disk::{
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
