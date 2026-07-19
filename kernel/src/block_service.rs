//! Capability-gated read-only block service.
//!
//! The service is invoked through `SYS_BLOCK_TRANSACT` on a `BlockDevice`
//! capability. Request and reply layouts remain the schema-generated block
//! protocol, while sector payloads are copied only into a validated user buffer.

use spin::{LazyLock, Mutex};

use crate::block_proto::{
    BLOCK_E_BAD_MAGIC, BLOCK_E_BAD_OP, BLOCK_E_BUFFER_TOO_SMALL, BLOCK_E_DEVICE, BLOCK_E_NO_BUFFER,
    BLOCK_E_NOT_AUTHORIZED, BLOCK_E_OK, BLOCK_E_OUT_OF_RANGE, BLOCK_E_TIMEOUT, OP_READ, ProtoError,
    REPLY_LEN, decode_request, encode_reply,
};
use crate::virtio_blk::{VirtioBlkError, VirtioBlock};

static DEVICE: LazyLock<Mutex<Option<VirtioBlock>>> = LazyLock::new(|| Mutex::new(None));

pub fn transact(request: &[u8], reply: &mut [u8; REPLY_LEN]) {
    let decoded = match decode_request(request) {
        Ok(decoded) => decoded,
        Err(error) => {
            encode_reply(reply, protocol_status(error), 0);
            return;
        }
    };
    if decoded.op != OP_READ {
        encode_reply(reply, BLOCK_E_NOT_AUTHORIZED, 0);
        return;
    }

    let bytes = decoded.sector_count as usize * crate::block_proto::SECTOR_SIZE;
    // `decode_request` checked arithmetic bounds and payload size. The syscall
    // gate validated that this user range is writable in the current task.
    let output = unsafe { core::slice::from_raw_parts_mut(decoded.buffer_phys as *mut u8, bytes) };
    let mut device = DEVICE.lock();
    if device.is_none() {
        match VirtioBlock::find_and_init() {
            Ok(found) => *device = Some(found),
            Err(error) => {
                encode_reply(reply, device_status(error), 0);
                return;
            }
        }
    }
    for sector in 0..decoded.sector_count {
        let start = sector as usize * crate::block_proto::SECTOR_SIZE;
        let end = start + crate::block_proto::SECTOR_SIZE;
        let Some(lba) = decoded.lba.checked_add(sector as u64) else {
            encode_reply(reply, BLOCK_E_OUT_OF_RANGE, sector);
            return;
        };
        let result = device
            .as_mut()
            .expect("block device initialized")
            .read_sector(lba, &mut output[start..end]);
        if let Err(error) = result {
            // Any transport error resets the device. Drop it so a later call
            // performs a clean initialization instead of reusing dead queues.
            *device = None;
            encode_reply(reply, device_status(error), sector);
            return;
        }
    }
    encode_reply(reply, BLOCK_E_OK, decoded.sector_count);
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

fn device_status(error: VirtioBlkError) -> i32 {
    match error {
        VirtioBlkError::OutOfRange => BLOCK_E_OUT_OF_RANGE,
        VirtioBlkError::BufferSize => BLOCK_E_BUFFER_TOO_SMALL,
        VirtioBlkError::Timeout => BLOCK_E_TIMEOUT,
        _ => BLOCK_E_DEVICE,
    }
}
