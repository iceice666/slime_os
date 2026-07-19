//! Bounded object store request/reply protocol over IPC.
//!
//! M5.4 deliverable: the userspace surface of the integrity-checked,
//! content-addressed object store. Payloads move through caller-supplied
//! buffers validated by the syscall gate; requests and replies stay inside
//! one IPC message.
//!
//! Layout (all little-endian, generated from `contracts/store/v1/schema.zt`):
//!
//! ```text
//! Request (fits in one IPC message, <= MAX_MSG = 64 bytes):
//!   u32 magic        = STORE_MAGIC
//!   u32 version      = FORMAT_VERSION
//!   u8  op           = OP_STAT | OP_GET | OP_PUT
//!   u8  flags        = 0 (unknown flags are rejected)
//!   u16 reserved
//!   u64 buffer_addr  (payload buffer; 0 when the op carries no payload)
//!   u32 obj_type     (OP_PUT only)
//!   u32 payload_len  (OP_PUT: payload bytes; OP_GET: buffer capacity)
//!   u64 hash0..hash3 (content hash as four little-endian u64 chunks)
//!
//! Reply (fits in one IPC message):
//!   u32 magic        = STORE_MAGIC
//!   u32 version      = FORMAT_VERSION
//!   i32 status       = 0 on success, negative STORE_E_* on error
//!   u32 obj_type     (OP_STAT/OP_GET success)
//!   u32 payload_len  (OP_STAT/OP_GET: actual bytes; OP_GET too-small: needed)
//!   u64 hash0..hash3 (OP_PUT: stored identity; OP_GET: verified identity)
//! ```

#[path = "store_proto/gen.rs"]
mod generated;

pub use generated::{
    FORMAT_VERSION, OP_GET, OP_PUT, OP_STAT, REPLY_LEN, REQUEST_LEN, STORE_MAGIC,
    STORE_MAGIC_BYTES, WireStoreReply, WireStoreRequest,
};

pub const STORE_E_OK: i32 = 0;
pub const STORE_E_BAD_MAGIC: i32 = -1;
pub const STORE_E_BAD_OP: i32 = -2;
pub const STORE_E_NOT_FOUND: i32 = -3;
pub const STORE_E_BUFFER_TOO_SMALL: i32 = -4;
pub const STORE_E_FULL: i32 = -5;
pub const STORE_E_DEVICE: i32 = -6;
pub const STORE_E_CORRUPT: i32 = -7;
pub const STORE_E_CONFLICT: i32 = -8;
pub const STORE_E_TIMEOUT: i32 = -9;

/// Payload bound enforced structurally before any device contact; mirrors
/// the store format's per-object bound.
pub const MAX_OBJECT_PAYLOAD: u32 = crate::object_store::MAX_OBJECT_PAYLOAD as u32;

fn hash_from_chunks(chunks: [u64; 4]) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for (index, chunk) in chunks.iter().enumerate() {
        hash[index * 8..index * 8 + 8].copy_from_slice(&chunk.to_le_bytes());
    }
    hash
}

fn chunks_from_hash(hash: &[u8; 32]) -> [u64; 4] {
    let mut chunks = [0u64; 4];
    for (index, chunk) in chunks.iter_mut().enumerate() {
        *chunk = u64::from_le_bytes(
            hash[index * 8..index * 8 + 8]
                .try_into()
                .expect("hash chunk"),
        );
    }
    chunks
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    BadOp,
    BadFlags,
    PayloadTooLarge,
    MissingBuffer,
}

/// Decode a request from an IPC message buffer. Pure: no device contact.
pub fn decode_request(buf: &[u8]) -> Result<StoreRequest, ProtoError> {
    let wire = WireStoreRequest::decode(buf).ok_or(ProtoError::Truncated)?;
    if wire.magic != STORE_MAGIC {
        return Err(ProtoError::BadMagic);
    }
    if wire.version != FORMAT_VERSION {
        return Err(ProtoError::UnsupportedVersion);
    }
    if !matches!(wire.op, OP_STAT | OP_GET | OP_PUT) {
        return Err(ProtoError::BadOp);
    }
    if wire.flags != 0 {
        return Err(ProtoError::BadFlags);
    }
    if wire.payload_len > MAX_OBJECT_PAYLOAD {
        return Err(ProtoError::PayloadTooLarge);
    }
    let hash = hash_from_chunks([wire.hash0, wire.hash1, wire.hash2, wire.hash3]);
    let request = StoreRequest {
        op: wire.op,
        buffer_addr: wire.buffer_addr,
        obj_type: wire.obj_type,
        payload_len: wire.payload_len,
        hash,
    };
    match wire.op {
        OP_STAT => {
            if wire.buffer_addr != 0 || wire.obj_type != 0 || wire.payload_len != 0 {
                return Err(ProtoError::BadOp);
            }
        }
        OP_GET => {
            if wire.obj_type != 0 || (wire.payload_len > 0 && wire.buffer_addr == 0) {
                return Err(ProtoError::MissingBuffer);
            }
        }
        OP_PUT => {
            if wire.payload_len > 0 && wire.buffer_addr == 0 {
                return Err(ProtoError::MissingBuffer);
            }
        }
        _ => unreachable!("op checked above"),
    }
    Ok(request)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreRequest {
    pub op: u8,
    pub buffer_addr: u64,
    pub obj_type: u32,
    pub payload_len: u32,
    pub hash: [u8; 32],
}

/// Encode a reply into a 64-byte buffer.
pub fn encode_reply(
    buf: &mut [u8; REPLY_LEN],
    status: i32,
    obj_type: u32,
    payload_len: u32,
    hash: &[u8; 32],
) {
    let chunks = chunks_from_hash(hash);
    let wire = WireStoreReply {
        magic: STORE_MAGIC,
        version: FORMAT_VERSION,
        status,
        obj_type,
        payload_len,
        hash0: chunks[0],
        hash1: chunks[1],
        hash2: chunks[2],
        hash3: chunks[3],
    };
    *buf = wire.encode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(op: u8, payload_len: u32, buffer_addr: u64) -> WireStoreRequest {
        WireStoreRequest {
            magic: STORE_MAGIC,
            version: FORMAT_VERSION,
            op,
            flags: 0,
            reserved: 0,
            buffer_addr,
            obj_type: 0,
            payload_len,
            hash0: 0,
            hash1: 0,
            hash2: 0,
            hash3: 0,
        }
    }

    #[test_case]
    fn rejects_bad_magic() {
        let mut wire = request(OP_STAT, 0, 0).encode();
        wire[0] ^= 0xFF;
        assert_eq!(decode_request(&wire), Err(ProtoError::BadMagic));
    }

    #[test_case]
    fn rejects_unknown_version() {
        let mut wire = request(OP_STAT, 0, 0);
        wire.version = 2;
        assert_eq!(
            decode_request(&wire.encode()),
            Err(ProtoError::UnsupportedVersion)
        );
    }

    #[test_case]
    fn rejects_unknown_op_and_flags() {
        let bad_op = request(9, 0, 0);
        assert_eq!(decode_request(&bad_op.encode()), Err(ProtoError::BadOp));
        let mut bad_flags = request(OP_STAT, 0, 0);
        bad_flags.flags = 1;
        assert_eq!(
            decode_request(&bad_flags.encode()),
            Err(ProtoError::BadFlags)
        );
    }

    #[test_case]
    fn rejects_oversized_payload() {
        let wire = request(OP_PUT, MAX_OBJECT_PAYLOAD + 1, 0x1000);
        assert_eq!(
            decode_request(&wire.encode()),
            Err(ProtoError::PayloadTooLarge)
        );
    }

    #[test_case]
    fn rejects_stat_with_payload_fields() {
        let wire = request(OP_STAT, 1, 0x1000);
        assert_eq!(decode_request(&wire.encode()), Err(ProtoError::BadOp));
    }

    #[test_case]
    fn rejects_payload_without_buffer() {
        let wire = request(OP_PUT, 16, 0);
        assert_eq!(
            decode_request(&wire.encode()),
            Err(ProtoError::MissingBuffer)
        );
    }

    #[test_case]
    fn accepts_bounded_requests() {
        let stat = request(OP_STAT, 0, 0);
        assert!(decode_request(&stat.encode()).is_ok());
        let get = request(OP_GET, 512, 0x4000);
        assert!(decode_request(&get.encode()).is_ok());
        let put = request(OP_PUT, 512, 0x4000);
        assert!(decode_request(&put.encode()).is_ok());
        let empty_put = request(OP_PUT, 0, 0);
        assert!(decode_request(&empty_put.encode()).is_ok());
    }
}
