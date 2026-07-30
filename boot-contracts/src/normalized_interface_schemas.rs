pub const MAGIC: [u8; 8] = *b"SLIMENS\0";
include!("generated/normalized_interface_schemas.rs");

/// One normalized interface schema record, sorted by full interface identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedSchema<'a> {
    pub identity: &'a [u8; 32],
    pub normalized: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadLength,
    UnsortedOrDuplicate,
    NonZeroReserved,
}

/// A decoded deterministic normalized-schema corpus.
#[derive(Debug, Clone, Copy)]
pub struct NormalizedInterfaceSchemas<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> NormalizedInterfaceSchemas<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if u32_at(bytes, 8)? != FORMAT_VERSION || u32_at(bytes, 12)? as usize != HEADER_BYTES {
            return Err(DecodeError::UnsupportedVersion);
        }
        if u64_at(bytes, 16)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let count = u32_at(bytes, 24)? as usize;
        let total_len = u32_at(bytes, 28)? as usize;
        if count > MAX_SCHEMAS || total_len != bytes.len() {
            return Err(DecodeError::BadLength);
        }
        if bytes.len() < HEADER_BYTES + count * ENTRY_BYTES {
            return Err(DecodeError::Truncated);
        }
        let mut previous: Option<&[u8; 32]> = None;
        let mut cursor = HEADER_BYTES + count * ENTRY_BYTES;
        for index in 0..count {
            let record = HEADER_BYTES + index * ENTRY_BYTES;
            let identity: &[u8; 32] = bytes[record..record + 32].try_into().unwrap();
            let normalized_len = u32_at(bytes, record + 32)? as usize;
            if u32_at(bytes, record + 36)? != 0 {
                return Err(DecodeError::NonZeroReserved);
            }
            if previous.is_some_and(|prior| identity <= prior) {
                return Err(DecodeError::UnsortedOrDuplicate);
            }
            if normalized_len > bytes.len() - cursor {
                return Err(DecodeError::Truncated);
            }
            cursor += normalized_len;
            previous = Some(identity);
        }
        if cursor != bytes.len() {
            return Err(DecodeError::BadLength);
        }
        Ok(Self { bytes, count })
    }

    pub fn schema_count(&self) -> usize {
        self.count
    }

    pub fn schema(&self, index: usize) -> Option<NormalizedSchema<'a>> {
        if index >= self.count {
            return None;
        }
        let record = HEADER_BYTES + index * ENTRY_BYTES;
        let identity: &[u8; 32] = self.bytes[record..record + 32].try_into().unwrap();
        let normalized_len = u32_at(self.bytes, record + 32).ok()? as usize;
        let payload_offset = self.payload_offset(index, normalized_len).ok()?;
        Some(NormalizedSchema {
            identity,
            normalized: &self.bytes[payload_offset..payload_offset + normalized_len],
        })
    }

    fn payload_offset(&self, index: usize, normalized_len: usize) -> Result<usize, DecodeError> {
        let mut cursor = HEADER_BYTES + self.count * ENTRY_BYTES;
        for prior in 0..index {
            let record = HEADER_BYTES + prior * ENTRY_BYTES;
            cursor += u32_at(self.bytes, record + 32)? as usize;
        }
        if normalized_len > self.bytes.len() - cursor {
            return Err(DecodeError::Truncated);
        }
        Ok(cursor)
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(DecodeError::Truncated)?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or(DecodeError::Truncated)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec::Vec;

    fn artifact() -> Vec<u8> {
        let first = [1u8; 32];
        let second = [2u8; 32];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&((HEADER_BYTES + 2 * ENTRY_BYTES + 5) as u32).to_le_bytes());
        bytes.extend_from_slice(&first);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&second);
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"abxyz");
        bytes
    }

    #[test]
    fn decodes_identity_ordered_schemas() {
        let bytes = artifact();
        let corpus = NormalizedInterfaceSchemas::decode(&bytes).expect("decodes");
        assert_eq!(corpus.schema_count(), 2);
        assert_eq!(corpus.schema(0).expect("first").normalized, b"ab");
        assert_eq!(corpus.schema(1).expect("second").normalized, b"xyz");
        assert!(corpus.schema(2).is_none());
    }

    #[test]
    fn rejects_unsorted_identities() {
        let mut bytes = artifact();
        bytes[HEADER_BYTES..HEADER_BYTES + 32].copy_from_slice(&[3u8; 32]);
        assert!(matches!(
            NormalizedInterfaceSchemas::decode(&bytes),
            Err(DecodeError::UnsortedOrDuplicate)
        ));
    }
}
