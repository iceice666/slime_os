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
//! Layout (all little-endian):
//!
//! ```text
//! Request (fits in one IPC message, <= MAX_MSG = 64 bytes):
//!   u32 magic        = BLOCK_MAGIC
//!   u8  op           = OP_READ | OP_WRITE | OP_FLUSH
//!   u8  flags
//!   u16 reserved
//!   u64 lba
//!   u32 sector_count
//!   u32 buffer_pages
//!   u64 buffer_phys   (0 when the request carries no payload, e.g. OP_FLUSH)
//!   u8[24] padding
//!
//! Reply (fits in one IPC message):
//!   u32 magic        = BLOCK_MAGIC
//!   i32 status       = 0 on success, negative BLOCK_E_* on error
//!   u32 sectors_done
//!   u8[52] padding
//! ```

pub const BLOCK_MAGIC: u32 = u32::from_le_bytes(*b"BLKE");
pub const BLOCK_MAGIC_BYTES: [u8; 4] = *b"BLKE";

pub const OP_READ: u8 = 1;
pub const OP_WRITE: u8 = 2;
pub const OP_FLUSH: u8 = 3;

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

pub const REQUEST_LEN: usize = 64;
pub const REPLY_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    Truncated,
    BadMagic,
    BadOp,
    OutOfRange,
    NoBuffer,
    BufferTooSmall,
    NotAuthorized,
}

/// Decode a request from an IPC message buffer. Pure: no device contact.
pub fn decode_request(buf: &[u8]) -> Result<BlockRequest, ProtoError> {
    if buf.len() < REQUEST_LEN {
        return Err(ProtoError::Truncated);
    }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != u32::from_le_bytes(BLOCK_MAGIC_BYTES) {
        return Err(ProtoError::BadMagic);
    }
    let op = buf[4];
    if !matches!(op, OP_READ | OP_WRITE | OP_FLUSH) {
        return Err(ProtoError::BadOp);
    }
    let lba = u64::from_le_bytes(buf[12..20].try_into().expect("request len"));
    let sector_count = u32::from_le_bytes(buf[20..24].try_into().expect("request len"));
    let buffer_pages = u32::from_le_bytes(buf[24..28].try_into().expect("request len"));
    let buffer_phys = u64::from_le_bytes(buf[28..36].try_into().expect("request len"));

    if op == OP_FLUSH {
        // Flush carries no payload; lba/sector_count/buffer must be zero.
        if sector_count != 0 || buffer_pages != 0 || buffer_phys != 0 {
            return Err(ProtoError::BadOp);
        }
        return Ok(BlockRequest {
            op,
            lba: 0,
            sector_count: 0,
            buffer_pages: 0,
            buffer_phys: 0,
        });
    }
    if sector_count == 0 || sector_count > MAX_SECTORS_PER_REQUEST {
        return Err(ProtoError::OutOfRange);
    }
    if buffer_pages > MAX_BUFFER_PAGES {
        return Err(ProtoError::BufferTooSmall);
    }
    if buffer_phys == 0 || buffer_pages == 0 {
        return Err(ProtoError::NoBuffer);
    }
    let needed = sector_count as usize * SECTOR_SIZE;
    let provided = buffer_pages as usize * crate::memory::PAGE_SIZE;
    if provided < needed {
        return Err(ProtoError::BufferTooSmall);
    }
    Ok(BlockRequest {
        op,
        lba,
        sector_count,
        buffer_pages,
        buffer_phys,
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
    buf[0..4].copy_from_slice(&u32::from_le_bytes(BLOCK_MAGIC_BYTES).to_le_bytes());
    buf[4..8].copy_from_slice(&status.to_le_bytes());
    buf[8..12].copy_from_slice(&sectors_done.to_le_bytes());
    for byte in &mut buf[12..] {
        *byte = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(op: u8, lba: u64, sectors: u32, pages: u32, phys: u64) -> [u8; REQUEST_LEN] {
        let mut buf = [0u8; REQUEST_LEN];
        buf[0..4].copy_from_slice(&u32::from_le_bytes(BLOCK_MAGIC_BYTES).to_le_bytes());
        buf[4] = op;
        buf[12..20].copy_from_slice(&lba.to_le_bytes());
        buf[20..24].copy_from_slice(&sectors.to_le_bytes());
        buf[24..28].copy_from_slice(&pages.to_le_bytes());
        buf[28..36].copy_from_slice(&phys.to_le_bytes());
        buf
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
    fn rejects_short_buffer() {
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
