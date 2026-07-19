//! Bounded block request/reply protocol over IPC.
//!
//! M5.1 deliverable: define a bounded block request/reply protocol over IPC
//! and keep payload data in shared memory rather than growing IPC messages
//! into an unbounded data plane.
//!
//! This module defines only the wire format and the request/reply validation
//! the kernel enforces on behalf of the block service. The actual virtio-blk
//! transport arrives in M5.2; here we fix the contract so the M5.1 capability
//! tests can exercise request validation independent of a device.
//!
//! Layout (all little-endian, generated from `contracts/block/v1/schema.zt`):
//!
//! ```text
//! Request (fits in one IPC message, <= MAX_MSG = 64 bytes):
//!   u32 magic        = BLOCK_MAGIC
//!   u32 version      = FORMAT_VERSION
//!   u8  op           = OP_READ | OP_WRITE | OP_FLUSH
//!   u8  flags
//!   u16 reserved
//!   u64 lba
//!   u32 sector_count
//!   u32 buffer_pages
//!   u64 buffer_phys   (0 when the request carries no payload, e.g. OP_FLUSH)
//!   u8[28] padding
//!
//! Reply (fits in one IPC message):
//!   u32 magic        = BLOCK_MAGIC
//!   u32 version      = FORMAT_VERSION
//!   i32 status       = 0 on success, negative BLOCK_E_* on error
//!   u32 sectors_done
//!   u8[48] padding
//! ```

#[path = "block_proto/gen.rs"]
mod generated;

pub use generated::{
    BLOCK_MAGIC, BLOCK_MAGIC_BYTES, FORMAT_VERSION, OP_FLUSH, OP_READ, OP_WRITE, REPLY_LEN,
    REQUEST_LEN, WireBlockReply, WireBlockRequest,
};

pub const BLOCK_E_OK: i32 = 0;
pub const BLOCK_E_BAD_MAGIC: i32 = -1;
pub const BLOCK_E_BAD_OP: i32 = -2;
pub const BLOCK_E_OUT_OF_RANGE: i32 = -3;
pub const BLOCK_E_NO_BUFFER: i32 = -4;
pub const BLOCK_E_BUFFER_TOO_SMALL: i32 = -5;
pub const BLOCK_E_NOT_AUTHORIZED: i32 = -6;
pub const BLOCK_E_DEVICE: i32 = -7;
pub const BLOCK_E_TIMEOUT: i32 = -8;

/// Hard upper bounds. A request outside these is rejected structurally,
/// before any device contact.
pub const MAX_SECTORS_PER_REQUEST: u32 = 256;
pub const SECTOR_SIZE: usize = 512;
pub const MAX_BUFFER_PAGES: u32 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    BadOp,
    OutOfRange,
    NoBuffer,
    BufferTooSmall,
    NotAuthorized,
}

/// Decode a request from an IPC message buffer. Pure: no device contact.
pub fn decode_request(buf: &[u8]) -> Result<BlockRequest, ProtoError> {
    let wire = WireBlockRequest::decode(buf).ok_or(ProtoError::Truncated)?;
    if wire.magic != BLOCK_MAGIC {
        return Err(ProtoError::BadMagic);
    }
    if wire.version != FORMAT_VERSION {
        return Err(ProtoError::UnsupportedVersion);
    }
    if !matches!(wire.op, OP_READ | OP_WRITE | OP_FLUSH) {
        return Err(ProtoError::BadOp);
    }

    if wire.op == OP_FLUSH {
        // Flush carries no payload; lba/sector_count/buffer must be zero.
        if wire.lba != 0
            || wire.sector_count != 0
            || wire.buffer_pages != 0
            || wire.buffer_phys != 0
        {
            return Err(ProtoError::BadOp);
        }
        return Ok(BlockRequest {
            op: wire.op,
            lba: 0,
            sector_count: 0,
            buffer_pages: 0,
            buffer_phys: 0,
        });
    }
    if wire.sector_count == 0 || wire.sector_count > MAX_SECTORS_PER_REQUEST {
        return Err(ProtoError::OutOfRange);
    }
    if wire.buffer_pages > MAX_BUFFER_PAGES {
        return Err(ProtoError::BufferTooSmall);
    }
    if wire.buffer_phys == 0 || wire.buffer_pages == 0 {
        return Err(ProtoError::NoBuffer);
    }
    let needed = wire.sector_count as usize * SECTOR_SIZE;
    let provided = wire.buffer_pages as usize * crate::memory::PAGE_SIZE;
    if provided < needed {
        return Err(ProtoError::BufferTooSmall);
    }
    Ok(BlockRequest {
        op: wire.op,
        lba: wire.lba,
        sector_count: wire.sector_count,
        buffer_pages: wire.buffer_pages,
        buffer_phys: wire.buffer_phys,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub op: u8,
    pub lba: u64,
    pub sector_count: u32,
    pub buffer_pages: u32,
    pub buffer_phys: u64,
}

impl BlockRequest {
    pub fn is_write(&self) -> bool {
        self.op == OP_WRITE
    }
}

/// Encode a reply into a 64-byte buffer.
pub fn encode_reply(buf: &mut [u8; REPLY_LEN], status: i32, sectors_done: u32) {
    *buf = WireBlockReply {
        magic: BLOCK_MAGIC,
        version: FORMAT_VERSION,
        status,
        sectors_done,
    }
    .encode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(op: u8, lba: u64, sectors: u32, pages: u32, phys: u64) -> [u8; REQUEST_LEN] {
        WireBlockRequest {
            magic: BLOCK_MAGIC,
            version: FORMAT_VERSION,
            op,
            flags: 0,
            reserved: 0,
            lba,
            sector_count: sectors,
            buffer_pages: pages,
            buffer_phys: phys,
        }
        .encode()
    }

    #[test_case]
    fn decodes_valid_read() {
        // 1 sector = 512 B, needs 1 page (4096 B).
        let buf = build(OP_READ, 10, 1, 1, 0x1000);
        let r = decode_request(&buf).unwrap();
        assert_eq!(r.op, OP_READ);
        assert_eq!(r.lba, 10);
        assert_eq!(r.sector_count, 1);
    }

    #[test_case]
    fn request_wire_round_trips_byte_identically() {
        let wire = WireBlockRequest {
            magic: BLOCK_MAGIC,
            version: FORMAT_VERSION,
            op: OP_WRITE,
            flags: 0x5a,
            reserved: 0x1234,
            lba: 0x0102_0304_0506_0708,
            sector_count: 16,
            buffer_pages: 2,
            buffer_phys: 0x1000,
        };
        let encoded = wire.encode();
        assert_eq!(WireBlockRequest::decode(&encoded), Some(wire));
        assert!(encoded[36..].iter().all(|byte| *byte == 0));
    }

    #[test_case]
    fn reply_wire_round_trips_byte_identically() {
        let wire = WireBlockReply {
            magic: BLOCK_MAGIC,
            version: FORMAT_VERSION,
            status: BLOCK_E_TIMEOUT,
            sectors_done: 7,
        };
        let encoded = wire.encode();
        assert_eq!(WireBlockReply::decode(&encoded), Some(wire));
        assert!(encoded[16..].iter().all(|byte| *byte == 0));
    }

    #[test_case]
    fn rejects_truncated_request() {
        let buf = [0u8; REQUEST_LEN - 1];
        assert_eq!(decode_request(&buf), Err(ProtoError::Truncated));
    }

    #[test_case]
    fn rejects_unknown_version() {
        let mut wire = WireBlockRequest {
            magic: BLOCK_MAGIC,
            version: FORMAT_VERSION + 1,
            op: OP_READ,
            flags: 0,
            reserved: 0,
            lba: 0,
            sector_count: 1,
            buffer_pages: 1,
            buffer_phys: 0x1000,
        };
        assert_eq!(
            decode_request(&wire.encode()),
            Err(ProtoError::UnsupportedVersion)
        );
        wire.version = FORMAT_VERSION;
        assert!(decode_request(&wire.encode()).is_ok());
    }

    #[test_case]
    fn rejects_missing_buffer() {
        // 8 sectors = 4096 B, needs 1 page. Provide 0 pages.
        let buf = build(OP_READ, 0, 8, 0, 0x1000);
        assert_eq!(decode_request(&buf), Err(ProtoError::NoBuffer));
    }

    #[test_case]
    fn rejects_buffer_too_small() {
        // 256 sectors = 131072 B, needs 32 pages. Provide 1 page.
        let buf = build(OP_READ, 0, 256, 1, 0x1000);
        assert_eq!(decode_request(&buf), Err(ProtoError::BufferTooSmall));
    }

    #[test_case]
    fn rejects_out_of_range_sector_count() {
        let buf = build(OP_READ, 0, MAX_SECTORS_PER_REQUEST + 1, 512, 0x1000);
        assert_eq!(decode_request(&buf), Err(ProtoError::OutOfRange));
    }

    #[test_case]
    fn rejects_bad_magic() {
        let mut buf = build(OP_READ, 0, 1, 1, 0x1000);
        buf[0] = 0;
        assert_eq!(decode_request(&buf), Err(ProtoError::BadMagic));
    }

    #[test_case]
    fn rejects_flush_with_payload() {
        let buf = build(OP_FLUSH, 1, 1, 1, 0x1000);
        assert_eq!(decode_request(&buf), Err(ProtoError::BadOp));
    }

    #[test_case]
    fn decodes_valid_flush() {
        let buf = build(OP_FLUSH, 0, 0, 0, 0);
        let r = decode_request(&buf).unwrap();
        assert_eq!(r.op, OP_FLUSH);
    }
}
