//! Capability-gated block service with deterministic request recording.
//!
//! `SYS_BLOCK_TRANSACT` validates operation-specific capability rights and user
//! mappings. This service owns bounded device dispatch, durability ordering,
//! transport recovery, and the M5.3 single-entry IPC flight recorder.

use spin::{LazyLock, Mutex};

use crate::block_device::{BlockDevice, BlockError};
use crate::block_proto::{
    BLOCK_E_BAD_MAGIC, BLOCK_E_BAD_OP, BLOCK_E_BUFFER_TOO_SMALL, BLOCK_E_DEVICE, BLOCK_E_NO_BUFFER,
    BLOCK_E_NOT_AUTHORIZED, BLOCK_E_OK, BLOCK_E_OUT_OF_RANGE, BLOCK_E_TIMEOUT, BlockRequest,
    FLAG_INJECT_FLUSH_FAILURE, FLAG_INJECT_INTERRUPTED, FLAG_INJECT_REQUEST_FAILURE,
    FLAG_INJECT_RESET, FLAG_INJECT_TIMEOUT, FLAG_REPLAY_LAST, OP_FLUSH, OP_READ, OP_WRITE,
    ProtoError, REPLY_LEN, SECTOR_SIZE, decode_request, encode_reply,
};
use crate::capability::PciFunctionInfo;
use crate::serial_println;

static DEVICE: LazyLock<Mutex<Option<(PciFunctionInfo, BlockDevice)>>> =
    LazyLock::new(|| Mutex::new(None));
static LAST_PAYLOAD: LazyLock<Mutex<[u8; SECTOR_SIZE]>> =
    LazyLock::new(|| Mutex::new([0; SECTOR_SIZE]));
static LAST_REQUEST: LazyLock<Mutex<Option<BlockRequest>>> = LazyLock::new(|| Mutex::new(None));

pub fn transact(function: PciFunctionInfo, request: &[u8], reply: &mut [u8; REPLY_LEN]) {
    let decoded = match decode_request(request) {
        Ok(decoded) => decoded,
        Err(error) => {
            encode_reply(reply, protocol_status(error), 0);
            return;
        }
    };
    let decoded = if decoded.flags == FLAG_REPLAY_LAST {
        let Some(mut recorded) = *LAST_REQUEST.lock() else {
            encode_reply(reply, BLOCK_E_DEVICE, 0);
            return;
        };
        if recorded.op != OP_FLUSH {
            recorded.buffer_phys = LAST_PAYLOAD.lock().as_ptr() as u64;
        }
        serial_println!(
            "[block-flight] replay op={} flags={} lba={} sectors={}",
            recorded.op,
            recorded.flags,
            recorded.lba,
            recorded.sector_count
        );
        recorded
    } else {
        let mut recorded = decoded;
        if decoded.op == OP_WRITE {
            let bytes = decoded.sector_count as usize * SECTOR_SIZE;
            let snapshot_len = bytes.min(SECTOR_SIZE);
            let mut payload = LAST_PAYLOAD.lock();
            let source = decoded.buffer_phys as *const u8;
            unsafe { core::ptr::copy_nonoverlapping(source, payload.as_mut_ptr(), snapshot_len) };
            recorded.buffer_phys = payload.as_ptr() as u64;
        }
        serial_println!(
            "[block-flight] record op={} flags={} lba={} sectors={}",
            decoded.op,
            decoded.flags,
            decoded.lba,
            decoded.sector_count
        );
        *LAST_REQUEST.lock() = Some(recorded);
        decoded
    };

    let mut device = DEVICE.lock();
    if device
        .as_ref()
        .is_none_or(|(current, _)| *current != function)
    {
        match BlockDevice::init(function) {
            Ok(found) => *device = Some((function, found)),
            Err(error) => {
                encode_reply(reply, device_status(error), 0);
                serial_println!("[block-flight] init error {:?}", error);
                return;
            }
        }
    }
    let result = execute(
        &mut device.as_mut().expect("block device initialized").1,
        decoded,
    );
    if result.is_ok() && decoded.op == OP_READ {
        let mut payload = LAST_PAYLOAD.lock();
        unsafe {
            core::ptr::copy_nonoverlapping(
                decoded.buffer_phys as *const u8,
                payload.as_mut_ptr(),
                SECTOR_SIZE,
            )
        };
        let mut recorded = decoded;
        recorded.buffer_phys = payload.as_ptr() as u64;
        *LAST_REQUEST.lock() = Some(recorded);
    }
    if let Err(error) = result {
        if error.requires_reinitialize() {
            *device = None;
        }
        encode_reply(reply, device_status(error), 0);
        serial_println!(
            "[block-flight] error {:?} status={}",
            error,
            device_status(error)
        );
        return;
    }
    encode_reply(reply, BLOCK_E_OK, decoded.sector_count);
}

/// Run `f` against the lazily initialized block device, reinitializing on
/// transport-fatal errors. Internal services and capability-gated clients
/// share this one backend; transport selection does not create a second path.
pub fn with_device<R>(
    f: impl FnOnce(&mut BlockDevice) -> Result<R, BlockError>,
) -> Result<R, BlockError> {
    let mut device = DEVICE.lock();
    if device.is_none() {
        match BlockDevice::find_and_init() {
            Ok(found) => *device = Some(found),
            Err(error) => {
                crate::serial_println!("[block-service] init error {:?}", error);
                return Err(error);
            }
        }
    }
    let result = f(&mut device.as_mut().expect("block device initialized").1);
    if let Err(error) = &result
        && error.requires_reinitialize()
    {
        *device = None;
    }
    result
}

fn execute(device: &mut BlockDevice, request: BlockRequest) -> Result<(), BlockError> {
    match request.flags {
        FLAG_INJECT_REQUEST_FAILURE => return device.inject_failure(),
        FLAG_INJECT_TIMEOUT => return device.inject_timeout(),
        FLAG_INJECT_RESET => return device.inject_reset(),
        FLAG_INJECT_FLUSH_FAILURE => return device.inject_flush_failure(),
        _ => {}
    }

    match request.op {
        OP_READ => {
            let bytes = request.sector_count as usize * SECTOR_SIZE;
            // SAFETY: the syscall gate validated the entire writable user range.
            let output =
                unsafe { core::slice::from_raw_parts_mut(request.buffer_phys as *mut u8, bytes) };
            for sector in 0..request.sector_count {
                let start = sector as usize * SECTOR_SIZE;
                let end = start + SECTOR_SIZE;
                let lba = request
                    .lba
                    .checked_add(sector as u64)
                    .ok_or(BlockError::OutOfRange)?;
                device.read_sector(lba, &mut output[start..end])?;
            }
        }
        OP_WRITE => {
            let bytes = request.sector_count as usize * SECTOR_SIZE;
            // SAFETY: the syscall gate validated the entire readable user range.
            let input =
                unsafe { core::slice::from_raw_parts(request.buffer_phys as *const u8, bytes) };
            if request.flags == FLAG_INJECT_INTERRUPTED {
                return device.inject_interrupted_write(request.lba, &input[..SECTOR_SIZE]);
            }
            for sector in 0..request.sector_count {
                let start = sector as usize * SECTOR_SIZE;
                let end = start + SECTOR_SIZE;
                let lba = request
                    .lba
                    .checked_add(sector as u64)
                    .ok_or(BlockError::OutOfRange)?;
                device.write_sector(lba, &input[start..end])?;
            }
        }
        OP_FLUSH => device.flush()?,
        _ => return Err(BlockError::Device),
    }
    Ok(())
}

fn protocol_status(error: ProtoError) -> i32 {
    match error {
        ProtoError::Truncated | ProtoError::BadMagic | ProtoError::UnsupportedVersion => {
            BLOCK_E_BAD_MAGIC
        }
        ProtoError::BadOp => BLOCK_E_BAD_OP,
        ProtoError::OutOfRange => BLOCK_E_OUT_OF_RANGE,
        ProtoError::NoBuffer => BLOCK_E_NO_BUFFER,
        ProtoError::BufferTooSmall => BLOCK_E_BUFFER_TOO_SMALL,
        ProtoError::NotAuthorized => BLOCK_E_NOT_AUTHORIZED,
    }
}

fn device_status(error: BlockError) -> i32 {
    match error {
        BlockError::OutOfRange => BLOCK_E_OUT_OF_RANGE,
        BlockError::BufferSize => BLOCK_E_BUFFER_TOO_SMALL,
        BlockError::Timeout => BLOCK_E_TIMEOUT,
        BlockError::ReadOnly => BLOCK_E_NOT_AUTHORIZED,
        _ => BLOCK_E_DEVICE,
    }
}
