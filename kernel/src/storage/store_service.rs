//! Capability-gated object store service (M5.4).
//!
//! `SYS_STORE_TRANSACT` validates the `ObjectStore` capability rights and the
//! user payload ranges; this service owns GPT validation, store open/recovery,
//! and bounded dispatch. The store partition is reached only through the
//! shared virtio-blk device and only after GPT validation bounded it; there
//! is no ambient path to store bytes.
//!
//! Lock order (never violated): STAGING, then STORE, then the block device
//! (`block_service::with_device`).

use alloc::vec;
use alloc::vec::Vec;

use spin::{LazyLock, Mutex};

use crate::block_device::{BlockDevice, BlockError};
use crate::block_proto::SECTOR_SIZE;
use crate::block_service;
use crate::gpt::{self, GptError, Recovery};
use crate::object_store::{BlockIo, IoError, MAX_OBJECT_PAYLOAD, ObjectStore, StoreError};
use crate::serial_println;
use crate::store_proto::{
    OP_GET, OP_PUT, OP_STAT, ProtoError, REPLY_LEN, STORE_E_BAD_MAGIC, STORE_E_BAD_OP,
    STORE_E_BUFFER_TOO_SMALL, STORE_E_CONFLICT, STORE_E_CORRUPT, STORE_E_DEVICE, STORE_E_FULL,
    STORE_E_NOT_FOUND, STORE_E_OK, STORE_E_TIMEOUT, decode_request, encode_reply,
};

const ZERO_HASH: [u8; 32] = [0u8; 32];

static STORE: LazyLock<Mutex<Option<ObjectStore>>> = LazyLock::new(|| Mutex::new(None));
// Heap-allocated: a `MAX_OBJECT_PAYLOAD` stack temporary during lazy-init
// would overflow the kernel stack. `vec!` zero-fills directly on the heap.
static STAGING: LazyLock<Mutex<Vec<u8>>> =
    LazyLock::new(|| Mutex::new(vec![0; MAX_OBJECT_PAYLOAD]));

/// Device adapter lending the selected common block backend to the store core.
/// Error mapping keeps timeout distinct so the protocol can report it.
struct DeviceIo<'a>(&'a mut BlockDevice);

impl BlockIo for DeviceIo<'_> {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), IoError> {
        self.0.read_sector(lba, out).map_err(io_status)
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_SIZE]) -> Result<(), IoError> {
        self.0.write_sector(lba, data).map_err(io_status)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.0.flush().map_err(io_status)
    }
}

fn io_status(error: BlockError) -> IoError {
    match error {
        BlockError::Timeout => IoError::Timeout,
        _ => IoError::Device,
    }
}

pub fn transact(request: &[u8], reply: &mut [u8; REPLY_LEN]) {
    let decoded = match decode_request(request) {
        Ok(decoded) => decoded,
        Err(error) => {
            encode_reply(reply, protocol_status(error), 0, 0, &ZERO_HASH);
            return;
        }
    };
    if let Err(status) = ensure_open() {
        encode_reply(reply, status, 0, 0, &ZERO_HASH);
        return;
    }
    match decoded.op {
        OP_STAT => stat(&decoded, reply),
        OP_GET => get(&decoded, reply),
        OP_PUT => put(&decoded, reply),
        _ => encode_reply(reply, STORE_E_BAD_OP, 0, 0, &ZERO_HASH),
    }
}

fn stat(decoded: &crate::store_proto::StoreRequest, reply: &mut [u8; REPLY_LEN]) {
    let store = STORE.lock();
    let store = store.as_ref().expect("store opened");
    match store.stat(&decoded.hash) {
        Some((obj_type, len)) => encode_reply(reply, STORE_E_OK, obj_type, len, &decoded.hash),
        None => encode_reply(reply, STORE_E_NOT_FOUND, 0, 0, &ZERO_HASH),
    }
}

fn get(decoded: &crate::store_proto::StoreRequest, reply: &mut [u8; REPLY_LEN]) {
    let mut staging = STAGING.lock();
    let store = STORE.lock();
    let store = store.as_ref().expect("store opened");
    let Some((obj_type, len)) = store.stat(&decoded.hash) else {
        encode_reply(reply, STORE_E_NOT_FOUND, 0, 0, &ZERO_HASH);
        return;
    };
    if decoded.payload_len < len {
        encode_reply(
            reply,
            STORE_E_BUFFER_TOO_SMALL,
            obj_type,
            len,
            &decoded.hash,
        );
        return;
    }
    let result = block_service::with_device(|device| {
        let mut io = DeviceIo(device);
        Ok(store.get(&mut io, &decoded.hash, &mut staging[..]))
    });
    match result {
        Ok(Ok((obj_type, len))) => {
            if len > 0 {
                // SAFETY: the syscall gate validated the entire writable user
                // range for a nonzero advertised capacity; `len <= capacity`
                // was checked above. Zero-length objects skip the copy so a
                // null user buffer is never dereferenced.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        staging.as_ptr(),
                        decoded.buffer_addr as *mut u8,
                        len,
                    )
                };
            }
            encode_reply(reply, STORE_E_OK, obj_type, len as u32, &decoded.hash);
        }
        Ok(Err(error)) => encode_reply(reply, store_status(error), 0, 0, &ZERO_HASH),
        Err(error) => encode_reply(reply, device_status(error), 0, 0, &ZERO_HASH),
    }
}

fn put(decoded: &crate::store_proto::StoreRequest, reply: &mut [u8; REPLY_LEN]) {
    let len = decoded.payload_len as usize;
    let mut staging = STAGING.lock();
    if len > 0 {
        // SAFETY: the syscall gate validated the entire readable user range.
        unsafe {
            core::ptr::copy_nonoverlapping(
                decoded.buffer_addr as *const u8,
                staging.as_mut_ptr(),
                len,
            )
        };
    }
    let mut store = STORE.lock();
    let store = store.as_mut().expect("store opened");
    let result = block_service::with_device(|device| {
        let mut io = DeviceIo(device);
        Ok(store.put(&mut io, decoded.obj_type, &staging[..len]))
    });
    match result {
        Ok(Ok(hash)) => encode_reply(reply, STORE_E_OK, decoded.obj_type, len as u32, &hash),
        Ok(Err(error)) => encode_reply(reply, store_status(error), 0, 0, &ZERO_HASH),
        Err(error) => encode_reply(reply, device_status(error), 0, 0, &ZERO_HASH),
    }
}

/// Open the store once: validate GPT (with copy recovery), then load the
/// newest valid superblock root. Failures leave STORE empty so the next
/// request retries.
fn ensure_open() -> Result<(), i32> {
    let mut store = STORE.lock();
    if store.is_some() {
        return Ok(());
    }
    let mut outcome = None;
    let init = block_service::with_device(|device| {
        outcome = Some(open_from(device));
        Ok(())
    });
    if let Err(error) = init {
        serial_println!("[store] device init failed: {:?}", error);
        return Err(device_status(error));
    }
    match outcome.expect("device closure ran") {
        Ok(opened) => {
            *store = Some(opened);
            Ok(())
        }
        Err(status) => Err(status),
    }
}

fn open_from(device: &mut BlockDevice) -> Result<ObjectStore, i32> {
    let capacity = device.capacity_sectors();
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_SIZE]| {
        device.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let found = gpt::validate_store_partition(&mut reader, capacity).map_err(|error| {
        serial_println!("[gpt] store partition rejected: {:?}", error);
        gpt_status(error)
    })?;
    match found.recovery {
        Recovery::None => {}
        Recovery::BackupDamaged(error) => {
            serial_println!("[gpt] backup copy rejected ({:?}); using primary", error)
        }
        Recovery::PrimaryDamaged(error) => {
            serial_println!("[gpt] primary copy rejected ({:?}); using backup", error)
        }
    }
    let mut io = DeviceIo(device);
    let opened = ObjectStore::open(&mut io, &found.partition).map_err(|error| {
        serial_println!("[store] open failed: {:?}", error);
        store_status(error)
    })?;
    serial_println!(
        "[store] opened partition first={} last={} seq={} objects={}",
        found.partition.first_lba,
        found.partition.last_lba,
        opened.sequence(),
        opened.object_count()
    );
    Ok(opened)
}

fn protocol_status(error: ProtoError) -> i32 {
    match error {
        ProtoError::Truncated | ProtoError::BadMagic | ProtoError::UnsupportedVersion => {
            STORE_E_BAD_MAGIC
        }
        ProtoError::BadOp
        | ProtoError::BadFlags
        | ProtoError::PayloadTooLarge
        | ProtoError::MissingBuffer => STORE_E_BAD_OP,
    }
}

fn gpt_status(error: GptError) -> i32 {
    match error {
        GptError::Device => STORE_E_DEVICE,
        _ => STORE_E_CORRUPT,
    }
}

fn device_status(error: BlockError) -> i32 {
    match error {
        BlockError::Timeout => STORE_E_TIMEOUT,
        _ => STORE_E_DEVICE,
    }
}

fn store_status(error: StoreError) -> i32 {
    match error {
        StoreError::Io(IoError::Device) => STORE_E_DEVICE,
        StoreError::Io(IoError::Timeout) => STORE_E_TIMEOUT,
        StoreError::NotFound => STORE_E_NOT_FOUND,
        StoreError::BufferTooSmall => STORE_E_BUFFER_TOO_SMALL,
        StoreError::StoreFull => STORE_E_FULL,
        StoreError::DuplicateIdentity => STORE_E_CONFLICT,
        StoreError::PayloadTooLarge => STORE_E_BAD_OP,
        StoreError::PartitionTooSmall
        | StoreError::NoValidSuperblock
        | StoreError::CorruptRecord
        | StoreError::TooManyObjects
        | StoreError::HashMismatch => STORE_E_CORRUPT,
    }
}
