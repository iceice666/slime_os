use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMETR\0";
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_LEN: usize = 320;
pub const OBJECT_LEN: usize = 64;
pub const STATE_LEN: usize = 80;
pub const MAX_TRANSFER_BYTES: usize = 32 * 1024 * 1024;
pub const OBJECT_FLAG_PAYLOAD: u32 = 1;
pub const STATE_FLAG_TRAVEL: u32 = 1;
pub const STATE_FLAG_READ_ONLY: u32 = 2;
pub const HASH_OFFSET: usize = 248;
pub const HASH_END: usize = 280;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownFlags,
    BadBounds,
    BadHash,
    BadEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferObject<'a> {
    pub digest: [u8; 32],
    pub length: usize,
    pub kind: u32,
    pub payload: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferState {
    pub binding: [u8; 32],
    pub state_root: [u8; 32],
    pub schema_version: u32,
    pub policy: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct TransferManifest<'a> {
    bytes: &'a [u8],
    pub generation: [u8; 32],
    pub parent: Option<[u8; 32]>,
    pub source_state_root: [u8; 32],
    pub authority_manifest: [u8; 32],
    pub release_sequence: u64,
    pub generation_len: usize,
    object_count: usize,
    state_count: usize,
    object_offset: usize,
    state_offset: usize,
    release_offset: usize,
    metadata_offset: usize,
    metadata_len: usize,
}

impl<'a> TransferManifest<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, TransferError> {
        if bytes.len() < HEADER_LEN {
            return Err(TransferError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(TransferError::BadMagic);
        }
        if u32_at(bytes, 8)? != FORMAT_VERSION || u32_at(bytes, 12)? as usize != HEADER_LEN {
            return Err(TransferError::UnsupportedVersion);
        }
        if u64_at(bytes, 16)? != 0 || bytes[280..HEADER_LEN].iter().any(|byte| *byte != 0) {
            return Err(TransferError::UnknownFlags);
        }
        let object_count = u32_at(bytes, 176)? as usize;
        let state_count = u32_at(bytes, 180)? as usize;
        if object_count > crate::generation::MAX_OBJECTS
            || state_count > crate::generation::MAX_STATES
        {
            return Err(TransferError::BadBounds);
        }
        let object_offset = u64_at(bytes, 184)? as usize;
        let state_offset = u64_at(bytes, 192)? as usize;
        let release_offset = u64_at(bytes, 200)? as usize;
        let metadata_offset = u64_at(bytes, 208)? as usize;
        let metadata_len = u64_at(bytes, 216)? as usize;
        let payload_offset = u64_at(bytes, 224)? as usize;
        let total_len = u64_at(bytes, 232)? as usize;
        if total_len != bytes.len()
            || total_len > MAX_TRANSFER_BYTES
            || object_offset != HEADER_LEN
            || state_offset != object_offset.checked_add(object_count.checked_mul(OBJECT_LEN).ok_or(TransferError::BadBounds)?).ok_or(TransferError::BadBounds)?
            || release_offset != state_offset.checked_add(state_count.checked_mul(STATE_LEN).ok_or(TransferError::BadBounds)?).ok_or(TransferError::BadBounds)?
            || metadata_offset != release_offset.checked_add(crate::release::RELEASE_BYTES).ok_or(TransferError::BadBounds)?
            || payload_offset != metadata_offset.checked_add(metadata_len).ok_or(TransferError::BadBounds)?
            || payload_offset > total_len
        {
            return Err(TransferError::BadBounds);
        }
        let expected: [u8; 32] = bytes[HASH_OFFSET..HASH_END].try_into().unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&bytes[..HASH_OFFSET]);
        hasher.update(&[0; 32]);
        hasher.update(&bytes[HASH_END..]);
        if hasher.finalize() != expected {
            return Err(TransferError::BadHash);
        }
        let parent: [u8; 32] = bytes[56..88].try_into().unwrap();
        Ok(Self {
            bytes,
            generation: bytes[24..56].try_into().unwrap(),
            parent: (parent != [0; 32]).then_some(parent),
            source_state_root: bytes[88..120].try_into().unwrap(),
            authority_manifest: bytes[120..152].try_into().unwrap(),
            release_sequence: u64_at(bytes, 152)?,
            generation_len: u64_at(bytes, 160)? as usize,
            object_count,
            state_count,
            object_offset,
            state_offset,
            release_offset,
            metadata_offset,
            metadata_len,
        })
    }

    pub fn object_count(&self) -> usize {
        self.object_count
    }

    pub fn state_count(&self) -> usize {
        self.state_count
    }

    pub fn objects(&self) -> impl Iterator<Item = Result<TransferObject<'a>, TransferError>> + '_ {
        (0..self.object_count).map(|index| self.object(index))
    }

    pub fn object(&self, index: usize) -> Result<TransferObject<'a>, TransferError> {
        if index >= self.object_count {
            return Err(TransferError::BadEntry);
        }
        let offset = self.object_offset + index * OBJECT_LEN;
        if self.bytes[offset + 56..offset + OBJECT_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(TransferError::BadEntry);
        }
        let length = u64_at(self.bytes, offset + 32)? as usize;
        let payload_offset = u64_at(self.bytes, offset + 40)? as usize;
        let flags = u32_at(self.bytes, offset + 52)?;
        if flags & !OBJECT_FLAG_PAYLOAD != 0
            || (flags == 0 && payload_offset != 0)
            || (flags == OBJECT_FLAG_PAYLOAD && payload_offset == 0)
        {
            return Err(TransferError::BadEntry);
        }
        let payload = if flags == OBJECT_FLAG_PAYLOAD {
            Some(
                self.bytes
                    .get(payload_offset..payload_offset.checked_add(length).ok_or(TransferError::BadBounds)?)
                    .ok_or(TransferError::BadBounds)?,
            )
        } else {
            None
        };
        Ok(TransferObject {
            digest: self.bytes[offset..offset + 32].try_into().unwrap(),
            length,
            kind: u32_at(self.bytes, offset + 48)?,
            payload,
        })
    }

    pub fn state(&self, index: usize) -> Result<TransferState, TransferError> {
        if index >= self.state_count {
            return Err(TransferError::BadEntry);
        }
        let offset = self.state_offset + index * STATE_LEN;
        if u32_at(self.bytes, offset + 76)? != 0 {
            return Err(TransferError::BadEntry);
        }
        let flags = u32_at(self.bytes, offset + 72)?;
        if flags & !(STATE_FLAG_TRAVEL | STATE_FLAG_READ_ONLY) != 0
            || flags & STATE_FLAG_TRAVEL == 0
        {
            return Err(TransferError::BadEntry);
        }
        Ok(TransferState {
            binding: self.bytes[offset..offset + 32].try_into().unwrap(),
            state_root: self.bytes[offset + 32..offset + 64].try_into().unwrap(),
            schema_version: u32_at(self.bytes, offset + 64)?,
            policy: u32_at(self.bytes, offset + 68)?,
            flags,
        })
    }

    pub fn release(&self) -> &'a [u8] {
        &self.bytes[self.release_offset..self.metadata_offset]
    }

    pub fn metadata(&self) -> &'a [u8] {
        &self.bytes[self.metadata_offset..self.metadata_offset + self.metadata_len]
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, TransferError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(TransferError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, TransferError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(TransferError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
