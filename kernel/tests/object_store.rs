//! M5.4 GPT + object store tests: the kernel validators accept well-formed
//! metadata and reject every malformed class with a structured error, the
//! store preserves the previously committed root across interruptions at
//! every append/commit boundary, and duplicate identities follow the
//! documented idempotent/conflicting rules. All device access is a bounded
//! in-memory mock; the QEMU device path is covered by `storage_store_check`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use slime_os_kernel::block_proto::SECTOR_SIZE;
use slime_os_kernel::crc32::crc32;
use slime_os_kernel::gpt::{
    self, GptError, Partition, Recovery, SLIME_STORE_TYPE_GUID, validate_store_partition,
};
use slime_os_kernel::object_store::{
    BlockIo, IoError, MAX_OBJECT_PAYLOAD, MAX_OBJECTS, ObjectStore, RECORD_AREA_START,
    RECORD_HEADER, StoreError, Superblock, SuperblockError, decode_superblock,
    encode_record_header, encode_superblock,
};
use slime_os_kernel::sha256;
use slime_os_kernel::{gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    gdt::init();
    interrupts::init();
    memory::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

// --- Mock device -----------------------------------------------------------

const CAPACITY: usize = 2048;
const STORE_FIRST: u64 = 40;
const STORE_LAST: u64 = 2014;
const PARTITION_SECTORS: u64 = STORE_LAST - STORE_FIRST + 1;

fn store_partition() -> Partition {
    Partition {
        first_lba: STORE_FIRST,
        last_lba: STORE_LAST,
        type_guid: SLIME_STORE_TYPE_GUID,
    }
}

struct MockDisk {
    sectors: Vec<[u8; SECTOR_SIZE]>,
    writes: usize,
    flushes: usize,
    fail_write_at: Option<usize>,
    fail_flush_at: Option<usize>,
}

impl MockDisk {
    fn new() -> Self {
        Self {
            sectors: vec![[0u8; SECTOR_SIZE]; CAPACITY],
            writes: 0,
            flushes: 0,
            fail_write_at: None,
            fail_flush_at: None,
        }
    }

    /// Genesis state: slot B carries sequence 1 with an empty record area;
    /// slot A is zeroed (never valid).
    fn genesis() -> Self {
        let mut disk = Self::new();
        disk.sectors[(STORE_FIRST + 1) as usize] = encode_superblock(
            &Superblock {
                sequence: 1,
                append_lba: RECORD_AREA_START,
                object_count: 0,
            },
            PARTITION_SECTORS,
        );
        disk
    }

    fn open(&mut self) -> ObjectStore {
        ObjectStore::open(self, &store_partition()).expect("store should open")
    }
}

impl BlockIo for MockDisk {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), IoError> {
        let sector = self.sectors.get(lba as usize).ok_or(IoError::Device)?;
        *out = *sector;
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), IoError> {
        let index = self.writes;
        self.writes += 1;
        if self.fail_write_at == Some(index) {
            return Err(IoError::Device);
        }
        let sector = self.sectors.get_mut(lba as usize).ok_or(IoError::Device)?;
        *sector = *data;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let index = self.flushes;
        self.flushes += 1;
        if self.fail_flush_at == Some(index) {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

// --- Mock GPT disk ---------------------------------------------------------

fn gpt_header(
    current_lba: u64,
    backup_lba: u64,
    entries_lba: u64,
    entries_crc: u32,
    disk_guid: &[u8; 16],
) -> [u8; SECTOR_SIZE] {
    let mut sector = [0u8; SECTOR_SIZE];
    sector[..8].copy_from_slice(b"EFI PART");
    sector[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    sector[12..16].copy_from_slice(&92u32.to_le_bytes());
    sector[24..32].copy_from_slice(&current_lba.to_le_bytes());
    sector[32..40].copy_from_slice(&backup_lba.to_le_bytes());
    sector[40..48].copy_from_slice(&34u64.to_le_bytes());
    sector[48..56].copy_from_slice(&(STORE_LAST as u64).to_le_bytes());
    sector[56..72].copy_from_slice(disk_guid);
    sector[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    sector[80..84].copy_from_slice(&128u32.to_le_bytes());
    sector[84..88].copy_from_slice(&128u32.to_le_bytes());
    sector[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&sector[..92]);
    sector[16..20].copy_from_slice(&crc.to_le_bytes());
    sector
}

fn fix_header_crc(sector: &mut [u8; SECTOR_SIZE]) {
    sector[16..20].fill(0);
    let crc = crc32(&sector[..92]);
    sector[16..20].copy_from_slice(&crc.to_le_bytes());
}

fn gpt_entries(partitions: &[(u64, u64, [u8; 16])]) -> [u8; 128 * 128] {
    let mut table = [0u8; 128 * 128];
    for (index, (first, last, guid)) in partitions.iter().enumerate() {
        let base = index * 128;
        table[base..base + 16].copy_from_slice(guid);
        table[base + 32..base + 40].copy_from_slice(&first.to_le_bytes());
        table[base + 40..base + 48].copy_from_slice(&last.to_le_bytes());
    }
    table
}

const DISK_GUID: &[u8; 16] = b"SLIMEOSDISKGUID!";
const OTHER_GUID: &[u8; 16] = b"SLIMEOSOTHERGUID";
const DATA_GUID: &[u8; 16] = b"SLIMEOSDATAGUID!";

struct GptDisk {
    sectors: Vec<[u8; SECTOR_SIZE]>,
}

impl GptDisk {
    fn new(partitions: &[(u64, u64, [u8; 16])]) -> Self {
        let mut sectors = vec![[0u8; SECTOR_SIZE]; CAPACITY];
        sectors[0][446 + 4] = 0xEE;
        sectors[0][446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
        sectors[0][446 + 12..446 + 16].copy_from_slice(&2047u32.to_le_bytes());
        sectors[0][510] = 0x55;
        sectors[0][511] = 0xAA;
        let entries = gpt_entries(partitions);
        let crc = crc32(&entries);
        for slot in 0..32usize {
            sectors[2 + slot] = entries[slot * SECTOR_SIZE..(slot + 1) * SECTOR_SIZE]
                .try_into()
                .expect("entry sector");
            sectors[2015 + slot] = entries[slot * SECTOR_SIZE..(slot + 1) * SECTOR_SIZE]
                .try_into()
                .expect("entry sector");
        }
        sectors[1] = gpt_header(1, 2047, 2, crc, DISK_GUID);
        sectors[2047] = gpt_header(2047, 1, 2015, crc, DISK_GUID);
        Self { sectors }
    }

    fn validate(&mut self) -> Result<gpt::StorePartition, GptError> {
        let mut reader = |lba: u64, out: &mut [u8; SECTOR_SIZE]| {
            let sector = self.sectors.get(lba as usize).ok_or(GptError::Device)?;
            *out = *sector;
            Ok(())
        };
        validate_store_partition(&mut reader, CAPACITY as u64)
    }
}

fn store_only_partitions() -> [(u64, u64, [u8; 16]); 1] {
    [(STORE_FIRST, STORE_LAST, SLIME_STORE_TYPE_GUID)]
}

// --- GPT validation --------------------------------------------------------

#[test_case]
fn valid_gpt_resolves_store_partition() {
    let mut disk = GptDisk::new(&store_only_partitions());
    let found = disk.validate().expect("valid GPT");
    assert_eq!(found.recovery, Recovery::None);
    assert_eq!(found.partition.first_lba, STORE_FIRST);
    assert_eq!(found.partition.last_lba, STORE_LAST);
}

#[test_case]
fn missing_pmbr_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    disk.sectors[0][446 + 4] = 0;
    assert_eq!(disk.validate(), Err(GptError::ProtectiveMbr));
}

#[test_case]
fn damaged_primary_recovers_via_backup() {
    let mut disk = GptDisk::new(&store_only_partitions());
    disk.sectors[1][0] ^= 0xFF;
    let found = disk.validate().expect("backup copy usable");
    assert_eq!(found.recovery, Recovery::PrimaryDamaged(GptError::BadMagic));
    assert_eq!(found.partition.first_lba, STORE_FIRST);
}

#[test_case]
fn damaged_backup_tolerated() {
    let mut disk = GptDisk::new(&store_only_partitions());
    disk.sectors[2047][0] ^= 0xFF;
    let found = disk.validate().expect("primary copy usable");
    assert_eq!(found.recovery, Recovery::BackupDamaged(GptError::BadMagic));
}

#[test_case]
fn both_copies_damaged_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    disk.sectors[1][0] ^= 0xFF;
    disk.sectors[2047][0] ^= 0xFF;
    assert_eq!(disk.validate(), Err(GptError::NoValidCopy));
}

#[test_case]
fn conflicting_valid_copies_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    let crc = crc32(&gpt_entries(&store_only_partitions()));
    disk.sectors[2047] = gpt_header(2047, 1, 2015, crc, OTHER_GUID);
    assert_eq!(disk.validate(), Err(GptError::ConflictingCopies));
}

#[test_case]
fn overlapping_partitions_rejected() {
    let mut disk = GptDisk::new(&[(40, 100, *DATA_GUID), (90, 200, *OTHER_GUID)]);
    assert_eq!(disk.validate(), Err(GptError::Overlap));
}

#[test_case]
fn entry_outside_usable_range_rejected() {
    let mut disk = GptDisk::new(&[(40, 3000, SLIME_STORE_TYPE_GUID)]);
    assert_eq!(disk.validate(), Err(GptError::OutOfBounds));
}

#[test_case]
fn unsupported_version_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    for lba in [1usize, 2047] {
        disk.sectors[lba][8..12].copy_from_slice(&0x0002_0000u32.to_le_bytes());
        fix_header_crc(&mut disk.sectors[lba]);
    }
    assert_eq!(disk.validate(), Err(GptError::NoValidCopy));
}

#[test_case]
fn damaged_primary_entries_recover_via_backup() {
    let mut disk = GptDisk::new(&store_only_partitions());
    disk.sectors[2][0] ^= 0xFF;
    let found = disk.validate().expect("backup entries usable");
    assert_eq!(
        found.recovery,
        Recovery::PrimaryDamaged(GptError::BadEntriesCrc)
    );
}

#[test_case]
fn bad_header_crc_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    for lba in [1usize, 2047] {
        disk.sectors[lba][24] ^= 0xFF;
    }
    assert_eq!(disk.validate(), Err(GptError::NoValidCopy));
}

#[test_case]
fn excessive_entry_count_rejected() {
    let mut disk = GptDisk::new(&store_only_partitions());
    for lba in [1usize, 2047] {
        disk.sectors[lba][80..84].copy_from_slice(&4096u32.to_le_bytes());
        fix_header_crc(&mut disk.sectors[lba]);
    }
    assert_eq!(disk.validate(), Err(GptError::NoValidCopy));
}

#[test_case]
fn tiny_capacity_rejected() {
    let disk = GptDisk::new(&store_only_partitions());
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_SIZE]| {
        let sector = disk.sectors.get(lba as usize).ok_or(GptError::Device)?;
        *out = *sector;
        Ok(())
    };
    assert_eq!(
        validate_store_partition(&mut reader, 2),
        Err(GptError::OutOfBounds)
    );
}

#[test_case]
fn missing_store_partition_rejected() {
    let mut disk = GptDisk::new(&[]);
    assert_eq!(disk.validate(), Err(GptError::NoStorePartition));
}

#[test_case]
fn ambiguous_store_partition_rejected() {
    let mut disk = GptDisk::new(&[
        (40, 100, SLIME_STORE_TYPE_GUID),
        (200, 300, SLIME_STORE_TYPE_GUID),
    ]);
    assert_eq!(disk.validate(), Err(GptError::AmbiguousStorePartition));
}

// --- Superblock codec ------------------------------------------------------

#[test_case]
fn superblock_roundtrip() {
    let superblock = Superblock {
        sequence: 7,
        append_lba: 9,
        object_count: 3,
    };
    let sector = encode_superblock(&superblock, PARTITION_SECTORS);
    assert_eq!(
        decode_superblock(&sector, PARTITION_SECTORS),
        Ok(superblock)
    );
}

#[test_case]
fn superblock_rejects_corruption() {
    let superblock = Superblock {
        sequence: 1,
        append_lba: RECORD_AREA_START,
        object_count: 0,
    };
    let mut sector = encode_superblock(&superblock, PARTITION_SECTORS);
    sector[60] ^= 0xFF;
    assert!(decode_superblock(&sector, PARTITION_SECTORS).is_err());
    let mut sector = encode_superblock(&superblock, PARTITION_SECTORS);
    sector[0] ^= 0xFF;
    assert!(decode_superblock(&sector, PARTITION_SECTORS).is_err());
    let mut sector = encode_superblock(&superblock, PARTITION_SECTORS);
    sector[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert!(decode_superblock(&sector, PARTITION_SECTORS).is_err());
}

// --- Store behavior --------------------------------------------------------

#[test_case]
fn genesis_opens_empty() {
    let mut disk = MockDisk::genesis();
    let store = disk.open();
    assert_eq!(store.sequence(), 1);
    assert_eq!(store.object_count(), 0);
    assert_eq!(store.append_lba(), RECORD_AREA_START);
    assert_eq!(store.stat(&[0u8; 32]), None);
}

#[test_case]
fn put_get_roundtrip() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = [0xABu8; 512];
    let hash = store.put(&mut disk, 7, &payload).expect("put");
    assert_eq!(hash, sha256::digest(&payload));
    assert_eq!(store.object_count(), 1);
    assert_eq!(store.sequence(), 2);
    assert_eq!(store.stat(&hash), Some((7, 512)));

    let mut out = [0u8; 512];
    let (obj_type, len) = store.get(&mut disk, &hash, &mut out).expect("get");
    assert_eq!((obj_type, len), (7, 512));
    assert_eq!(out, payload);
}

#[test_case]
fn committed_object_survives_reopen() {
    let mut disk = MockDisk::genesis();
    let payload = [0x5Au8; 512];
    let hash = disk.open().put(&mut disk, 1, &payload).expect("put");
    let reopened = disk.open();
    assert_eq!(reopened.sequence(), 2);
    assert_eq!(reopened.object_count(), 1);
    let mut out = [0u8; 512];
    assert!(reopened.get(&mut disk, &hash, &mut out).is_ok());
    assert_eq!(out, payload);
}

#[test_case]
fn duplicate_put_is_idempotent() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = [0x11u8; 512];
    let first = store.put(&mut disk, 1, &payload).expect("put");
    let append = store.append_lba();
    let second = store.put(&mut disk, 1, &payload).expect("dedup put");
    assert_eq!(first, second);
    assert_eq!(store.object_count(), 1);
    assert_eq!(store.append_lba(), append);
    assert_eq!(store.sequence(), 2);
}

#[test_case]
fn conflicting_identity_rejected() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = [0x22u8; 512];
    let hash = store.put(&mut disk, 1, &payload).expect("put");
    // Corrupt the committed payload on disk: the record's identity now
    // collides with different contents.
    let record_lba = (STORE_FIRST + RECORD_AREA_START) as usize;
    disk.sectors[record_lba][RECORD_HEADER] ^= 0xFF;
    assert_eq!(
        store.put(&mut disk, 1, &payload),
        Err(StoreError::DuplicateIdentity)
    );
    assert_eq!(
        store.get(&mut disk, &hash, &mut [0u8; 512]),
        Err(StoreError::HashMismatch)
    );
}

#[test_case]
fn corrupted_payload_never_returned() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = [0x33u8; 512];
    let hash = store.put(&mut disk, 1, &payload).expect("put");
    let record_lba = (STORE_FIRST + RECORD_AREA_START) as usize;
    disk.sectors[record_lba][RECORD_HEADER + 10] ^= 0xFF;
    let mut out = [0xEEu8; 512];
    assert_eq!(
        store.get(&mut disk, &hash, &mut out),
        Err(StoreError::HashMismatch)
    );
    assert_eq!(out, [0xEEu8; 512]);
}

#[test_case]
fn truncated_committed_record_rejected() {
    let mut disk = MockDisk::genesis();
    // Commit a superblock whose append offset lands mid-record.
    disk.sectors[STORE_FIRST as usize] = encode_superblock(
        &Superblock {
            sequence: 2,
            append_lba: RECORD_AREA_START + 1,
            object_count: 1,
        },
        PARTITION_SECTORS,
    );
    let payload = [0x44u8; 512];
    let header = encode_record_header(1, &payload, &sha256::digest(&payload));
    disk.sectors[(STORE_FIRST + RECORD_AREA_START) as usize][..RECORD_HEADER]
        .copy_from_slice(&header);
    assert!(matches!(
        ObjectStore::open(&mut disk, &store_partition()),
        Err(StoreError::CorruptRecord)
    ));
}

#[test_case]
fn interruption_at_every_commit_boundary_preserves_root() {
    // After any injected write/flush failure, reopening must yield a valid
    // root that is EITHER the pre-put state (seq 1, empty) or the fully
    // committed post-put state (seq 2, one verifying object) — never a torn
    // intermediate and never zero roots. The genesis root (slot B) is the
    // fallback and must survive every interruption.
    let payload = [0x55u8; 512];
    let expected_hash = sha256::digest(&payload);
    let assert_valid_root = |disk: &mut MockDisk, label: &str| {
        let reopened = ObjectStore::open(disk, &store_partition())
            .unwrap_or_else(|error| panic!("{label}: reopen failed: {error:?}"));
        match (reopened.sequence(), reopened.object_count()) {
            (1, 0) => {}
            (2, 1) => {
                assert_eq!(reopened.stat(&expected_hash), Some((1, 512)), "{label}");
                let mut out = [0u8; 512];
                assert!(
                    reopened.get(disk, &expected_hash, &mut out).is_ok(),
                    "{label}"
                );
                assert_eq!(out, payload, "{label}");
            }
            (seq, count) => panic!("{label}: torn root seq={seq} count={count}"),
        }
    };
    for fail_write in 0..3usize {
        let mut disk = MockDisk::genesis();
        disk.fail_write_at = Some(fail_write);
        assert!(disk.open().put(&mut disk, 1, &payload).is_err());
        assert_valid_root(&mut disk, "write failure");
    }
    for fail_flush in 0..2usize {
        let mut disk = MockDisk::genesis();
        disk.fail_flush_at = Some(fail_flush);
        assert!(disk.open().put(&mut disk, 1, &payload).is_err());
        assert_valid_root(&mut disk, "flush failure");
    }
}

#[test_case]
fn payload_bounds_enforced() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let oversized = vec![0x66u8; MAX_OBJECT_PAYLOAD + 1];
    assert_eq!(
        store.put(&mut disk, 1, &oversized),
        Err(StoreError::PayloadTooLarge)
    );
    assert_eq!(disk.writes, 0);
}

#[test_case]
fn store_full_rejected() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    for index in 0..MAX_OBJECTS {
        let mut payload = [0u8; 512];
        payload[0] = index as u8;
        store.put(&mut disk, 1, &payload).expect("put within bound");
    }
    let payload = [0xFFu8; 512];
    assert_eq!(
        store.put(&mut disk, 1, &payload),
        Err(StoreError::StoreFull)
    );
}

#[test_case]
fn record_area_bound_rejected() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = vec![0x77u8; MAX_OBJECT_PAYLOAD];
    let mut fills = 0;
    loop {
        match store.put(&mut disk, 1, &payload[..payload.len() - fills]) {
            Ok(_) => fills += 1,
            Err(StoreError::StoreFull) => break,
            Err(other) => panic!("unexpected store error: {other:?}"),
        }
        assert!(fills < MAX_OBJECTS, "object bound should hit first");
    }
}

#[test_case]
fn get_missing_and_small_buffer() {
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let payload = [0x88u8; 512];
    let hash = store.put(&mut disk, 1, &payload).expect("put");
    assert_eq!(
        store.get(&mut disk, &[0x99u8; 32], &mut [0u8; 512]),
        Err(StoreError::NotFound)
    );
    assert_eq!(
        store.get(&mut disk, &hash, &mut [0u8; 256]),
        Err(StoreError::BufferTooSmall)
    );
}

#[test_case]
fn no_valid_superblock_rejected() {
    let mut disk = MockDisk::new();
    assert!(matches!(
        ObjectStore::open(&mut disk, &store_partition()),
        Err(StoreError::NoValidSuperblock)
    ));
}

#[test_case]
fn maxed_sequence_superblock_rejected() {
    // A valid-looking superblock whose sequence is u64::MAX is treated as
    // corrupt so the next put cannot wrap the monotonic counter and let a
    // stale slot outrank a fresh commit.
    let sector = encode_superblock(
        &Superblock {
            sequence: u64::MAX,
            append_lba: RECORD_AREA_START,
            object_count: 0,
        },
        PARTITION_SECTORS,
    );
    assert_eq!(
        decode_superblock(&sector, PARTITION_SECTORS),
        Err(SuperblockError::BadBounds)
    );
}

#[test_case]
fn zero_length_object_roundtrips() {
    // An empty payload is a legal object: it hashes deterministically, stats
    // as length 0, and gets back as an empty slice.
    let mut disk = MockDisk::genesis();
    let mut store = disk.open();
    let hash = store.put(&mut disk, 3, &[]).expect("empty put");
    assert_eq!(hash, sha256::digest(&[]));
    assert_eq!(store.stat(&hash), Some((3, 0)));
    let (obj_type, len) = store.get(&mut disk, &hash, &mut []).expect("empty get");
    assert_eq!((obj_type, len), (3, 0));
}
