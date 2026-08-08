use crate::sha256::Sha256;

include!("generated/transfer.rs");

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
        if u32_at(bytes, HEADER_FORMAT_VERSION_OFFSET)? != FORMAT_VERSION
            || u32_at(bytes, HEADER_HEADER_SIZE_OFFSET)? as usize != HEADER_LEN
        {
            return Err(TransferError::UnsupportedVersion);
        }
        if u64_at(bytes, HEADER_REQUIRED_FLAGS_OFFSET)? != 0
            || bytes[HEADER_FIELDS_END..HEADER_LEN]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(TransferError::UnknownFlags);
        }
        let object_count = u32_at(bytes, HEADER_OBJECT_COUNT_OFFSET)? as usize;
        let state_count = u32_at(bytes, HEADER_STATE_COUNT_OFFSET)? as usize;
        if object_count > crate::generation::MAX_OBJECTS
            || state_count > crate::generation::MAX_STATES
        {
            return Err(TransferError::BadBounds);
        }
        let object_offset = u64_at(bytes, HEADER_OBJECT_OFFSET_OFFSET)? as usize;
        let state_offset = u64_at(bytes, HEADER_STATE_OFFSET_OFFSET)? as usize;
        let release_offset = u64_at(bytes, HEADER_RELEASE_OFFSET_OFFSET)? as usize;
        let metadata_offset = u64_at(bytes, HEADER_METADATA_OFFSET_OFFSET)? as usize;
        let metadata_len = u64_at(bytes, HEADER_METADATA_LEN_OFFSET)? as usize;
        let payload_offset = u64_at(bytes, HEADER_PAYLOAD_OFFSET_OFFSET)? as usize;
        let total_len = u64_at(bytes, HEADER_TOTAL_LEN_OFFSET)? as usize;
        if total_len != bytes.len()
            || total_len > MAX_TRANSFER_BYTES
            || object_offset != HEADER_LEN
            || state_offset
                != object_offset
                    .checked_add(
                        object_count
                            .checked_mul(OBJECT_LEN)
                            .ok_or(TransferError::BadBounds)?,
                    )
                    .ok_or(TransferError::BadBounds)?
            || release_offset
                != state_offset
                    .checked_add(
                        state_count
                            .checked_mul(STATE_LEN)
                            .ok_or(TransferError::BadBounds)?,
                    )
                    .ok_or(TransferError::BadBounds)?
            || metadata_offset
                != release_offset
                    .checked_add(crate::release::RELEASE_BYTES)
                    .ok_or(TransferError::BadBounds)?
            || payload_offset
                != metadata_offset
                    .checked_add(metadata_len)
                    .ok_or(TransferError::BadBounds)?
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
        let parent: [u8; 32] = bytes[HEADER_PARENT_OFFSET..HEADER_PARENT_OFFSET + 32]
            .try_into()
            .unwrap();
        Ok(Self {
            bytes,
            generation: bytes[HEADER_GENERATION_OFFSET..HEADER_GENERATION_OFFSET + 32]
                .try_into()
                .unwrap(),
            parent: (parent != [0; 32]).then_some(parent),
            source_state_root: bytes
                [HEADER_SOURCE_STATE_ROOT_OFFSET..HEADER_SOURCE_STATE_ROOT_OFFSET + 32]
                .try_into()
                .unwrap(),
            authority_manifest: bytes
                [HEADER_AUTHORITY_MANIFEST_OFFSET..HEADER_AUTHORITY_MANIFEST_OFFSET + 32]
                .try_into()
                .unwrap(),
            release_sequence: u64_at(bytes, HEADER_RELEASE_SEQUENCE_OFFSET)?,
            generation_len: u64_at(bytes, HEADER_GENERATION_LEN_OFFSET)? as usize,
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
        if self.bytes[offset + OBJECT_PADDING_OFFSET..offset + OBJECT_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(TransferError::BadEntry);
        }
        let length = u64_at(self.bytes, offset + OBJECT_LENGTH_OFFSET)? as usize;
        let payload_offset = u64_at(self.bytes, offset + OBJECT_PAYLOAD_OFFSET_OFFSET)? as usize;
        let flags = u32_at(self.bytes, offset + OBJECT_FLAGS_OFFSET)?;
        if flags & !OBJECT_FLAG_PAYLOAD != 0
            || (flags == 0 && payload_offset != 0)
            || (flags == OBJECT_FLAG_PAYLOAD && payload_offset == 0)
        {
            return Err(TransferError::BadEntry);
        }
        let payload = if flags == OBJECT_FLAG_PAYLOAD {
            Some(
                self.bytes
                    .get(
                        payload_offset
                            ..payload_offset
                                .checked_add(length)
                                .ok_or(TransferError::BadBounds)?,
                    )
                    .ok_or(TransferError::BadBounds)?,
            )
        } else {
            None
        };
        Ok(TransferObject {
            digest: self.bytes[offset..offset + 32].try_into().unwrap(),
            length,
            kind: u32_at(self.bytes, offset + OBJECT_KIND_OFFSET)?,
            payload,
        })
    }

    pub fn state(&self, index: usize) -> Result<TransferState, TransferError> {
        if index >= self.state_count {
            return Err(TransferError::BadEntry);
        }
        let offset = self.state_offset + index * STATE_LEN;
        if u32_at(self.bytes, offset + STATE_PADDING_OFFSET)? != 0 {
            return Err(TransferError::BadEntry);
        }
        let flags = u32_at(self.bytes, offset + STATE_FLAGS_OFFSET)?;
        if flags & !(STATE_FLAG_TRAVEL | STATE_FLAG_READ_ONLY) != 0
            || flags & STATE_FLAG_TRAVEL == 0
        {
            return Err(TransferError::BadEntry);
        }
        Ok(TransferState {
            binding: self.bytes[offset..offset + 32].try_into().unwrap(),
            state_root: self.bytes
                [offset + STATE_STATE_ROOT_OFFSET..offset + STATE_STATE_ROOT_OFFSET + 32]
                .try_into()
                .unwrap(),
            schema_version: u32_at(self.bytes, offset + STATE_SCHEMA_VERSION_OFFSET)?,
            policy: u32_at(self.bytes, offset + STATE_POLICY_OFFSET)?,
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

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;

    /// Rebuild the self-excluding digest the decoder checks: everything before
    /// the hash field, thirty-two zeros in its place, then everything after.
    /// Kept here rather than in the builder so a test can corrupt a byte
    /// *after* sealing and get `BadHash` instead of a stale-hash false pass.
    fn seal(bytes: &mut [u8]) {
        let mut hasher = Sha256::new();
        hasher.update(&bytes[..HASH_OFFSET]);
        hasher.update(&[0; 32]);
        hasher.update(&bytes[HASH_END..]);
        let digest = hasher.finalize();
        bytes[HASH_OFFSET..HASH_END].copy_from_slice(&digest);
    }

    /// One object carrying an inline payload and one travelling state, which is
    /// the smallest manifest that exercises every offset in the chain the
    /// decoder verifies: objects, then states, then the release record, then
    /// metadata, then payload bytes.
    fn valid() -> alloc::vec::Vec<u8> {
        const OBJECT_COUNT: usize = 1;
        const STATE_COUNT: usize = 1;
        const METADATA: &[u8] = b"transfer-metadata";
        const PAYLOAD: &[u8] = b"object-payload";

        let object_offset = HEADER_LEN;
        let state_offset = object_offset + OBJECT_COUNT * OBJECT_LEN;
        let release_offset = state_offset + STATE_COUNT * STATE_LEN;
        let metadata_offset = release_offset + crate::release::RELEASE_BYTES;
        let payload_offset = metadata_offset + METADATA.len();
        let total_len = payload_offset + PAYLOAD.len();

        let mut bytes = alloc::vec![0u8; total_len];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[HEADER_FORMAT_VERSION_OFFSET..HEADER_FORMAT_VERSION_OFFSET + 4]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[HEADER_HEADER_SIZE_OFFSET..HEADER_HEADER_SIZE_OFFSET + 4]
            .copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        bytes[HEADER_GENERATION_OFFSET..HEADER_GENERATION_OFFSET + 32].fill(0x11);
        bytes[HEADER_SOURCE_STATE_ROOT_OFFSET..HEADER_SOURCE_STATE_ROOT_OFFSET + 32].fill(0x22);
        bytes[HEADER_AUTHORITY_MANIFEST_OFFSET..HEADER_AUTHORITY_MANIFEST_OFFSET + 32].fill(0x33);
        bytes[HEADER_RELEASE_SEQUENCE_OFFSET..HEADER_RELEASE_SEQUENCE_OFFSET + 8]
            .copy_from_slice(&9u64.to_le_bytes());
        bytes[HEADER_GENERATION_LEN_OFFSET..HEADER_GENERATION_LEN_OFFSET + 8]
            .copy_from_slice(&64u64.to_le_bytes());
        bytes[HEADER_OBJECT_COUNT_OFFSET..HEADER_OBJECT_COUNT_OFFSET + 4]
            .copy_from_slice(&(OBJECT_COUNT as u32).to_le_bytes());
        bytes[HEADER_STATE_COUNT_OFFSET..HEADER_STATE_COUNT_OFFSET + 4]
            .copy_from_slice(&(STATE_COUNT as u32).to_le_bytes());
        for (offset, value) in [
            (HEADER_OBJECT_OFFSET_OFFSET, object_offset),
            (HEADER_STATE_OFFSET_OFFSET, state_offset),
            (HEADER_RELEASE_OFFSET_OFFSET, release_offset),
            (HEADER_METADATA_OFFSET_OFFSET, metadata_offset),
            (HEADER_METADATA_LEN_OFFSET, METADATA.len()),
            (HEADER_PAYLOAD_OFFSET_OFFSET, payload_offset),
            (HEADER_TOTAL_LEN_OFFSET, total_len),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
        }

        bytes[object_offset..object_offset + 32].fill(0x44);
        bytes[object_offset + OBJECT_LENGTH_OFFSET..object_offset + OBJECT_LENGTH_OFFSET + 8]
            .copy_from_slice(&(PAYLOAD.len() as u64).to_le_bytes());
        bytes[object_offset + OBJECT_PAYLOAD_OFFSET_OFFSET
            ..object_offset + OBJECT_PAYLOAD_OFFSET_OFFSET + 8]
            .copy_from_slice(&(payload_offset as u64).to_le_bytes());
        bytes[object_offset + OBJECT_KIND_OFFSET..object_offset + OBJECT_KIND_OFFSET + 4]
            .copy_from_slice(&7u32.to_le_bytes());
        bytes[object_offset + OBJECT_FLAGS_OFFSET..object_offset + OBJECT_FLAGS_OFFSET + 4]
            .copy_from_slice(&OBJECT_FLAG_PAYLOAD.to_le_bytes());

        bytes[state_offset..state_offset + 32].fill(0x55);
        bytes[state_offset + STATE_STATE_ROOT_OFFSET..state_offset + STATE_STATE_ROOT_OFFSET + 32]
            .fill(0x66);
        bytes[state_offset + STATE_SCHEMA_VERSION_OFFSET
            ..state_offset + STATE_SCHEMA_VERSION_OFFSET + 4]
            .copy_from_slice(&3u32.to_le_bytes());
        bytes[state_offset + STATE_POLICY_OFFSET..state_offset + STATE_POLICY_OFFSET + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        bytes[state_offset + STATE_FLAGS_OFFSET..state_offset + STATE_FLAGS_OFFSET + 4]
            .copy_from_slice(&(STATE_FLAG_TRAVEL | STATE_FLAG_READ_ONLY).to_le_bytes());

        bytes[metadata_offset..metadata_offset + METADATA.len()].copy_from_slice(METADATA);
        bytes[payload_offset..payload_offset + PAYLOAD.len()].copy_from_slice(PAYLOAD);
        seal(&mut bytes);
        bytes
    }

    /// Every field the decoder promises to surface, read back from a sealed
    /// manifest. Without this the refusal corpus below could pass against a
    /// decoder that rejects everything.
    #[test]
    fn a_well_formed_manifest_decodes_with_every_field() {
        let bytes = valid();
        let manifest = TransferManifest::decode(&bytes).expect("valid manifest");
        assert_eq!(manifest.generation, [0x11; 32]);
        assert_eq!(manifest.source_state_root, [0x22; 32]);
        assert_eq!(manifest.authority_manifest, [0x33; 32]);
        assert_eq!(manifest.release_sequence, 9);
        assert_eq!(manifest.generation_len, 64);
        assert_eq!(manifest.object_count(), 1);
        assert_eq!(manifest.state_count(), 1);
        assert_eq!(manifest.metadata(), b"transfer-metadata");
        assert_eq!(manifest.release().len(), crate::release::RELEASE_BYTES);

        let object = manifest.object(0).expect("object 0");
        assert_eq!(object.digest, [0x44; 32]);
        assert_eq!(object.kind, 7);
        assert_eq!(object.length, b"object-payload".len());
        assert_eq!(object.payload, Some(&b"object-payload"[..]));

        let state = manifest.state(0).expect("state 0");
        assert_eq!(state.binding, [0x55; 32]);
        assert_eq!(state.state_root, [0x66; 32]);
        assert_eq!(state.schema_version, 3);
        assert_eq!(state.policy, 1);
        assert_eq!(state.flags, STATE_FLAG_TRAVEL | STATE_FLAG_READ_ONLY);
    }

    /// An all-zero parent is *absent*, not a real ancestor hash. The rollback
    /// chain reads this to decide whether a transfer has a predecessor at all,
    /// so conflating the two would invent one.
    #[test]
    fn a_zero_parent_decodes_as_absent_and_a_set_one_as_present() {
        let bytes = valid();
        assert_eq!(
            TransferManifest::decode(&bytes).expect("valid").parent,
            None
        );

        let mut bytes = valid();
        bytes[HEADER_PARENT_OFFSET..HEADER_PARENT_OFFSET + 32].fill(0x77);
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes).expect("valid").parent,
            Some([0x77; 32])
        );
    }

    /// The digest covers the whole manifest with its own field zeroed, so any
    /// single flipped byte outside that field must be caught. Re-sealing is
    /// deliberately *not* done here — that is the point.
    #[test]
    fn one_flipped_payload_byte_fails_the_digest() {
        let mut bytes = valid();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert_eq!(
            TransferManifest::decode(&bytes).err(),
            Some(TransferError::BadHash)
        );
    }

    #[test]
    fn a_header_shorter_than_one_header_is_truncated() {
        let bytes = valid();
        assert_eq!(
            TransferManifest::decode(&bytes[..HEADER_LEN - 1]).err(),
            Some(TransferError::Truncated)
        );
    }

    #[test]
    fn a_foreign_magic_is_refused_before_anything_else() {
        let mut bytes = valid();
        bytes[0] = b'X';
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes).err(),
            Some(TransferError::BadMagic)
        );
    }

    /// A future format is refused rather than read with this version's offsets,
    /// and so is a header claiming a different size than this build compiled.
    #[test]
    fn a_wrong_version_or_header_size_is_unsupported() {
        for (offset, value) in [
            (HEADER_FORMAT_VERSION_OFFSET, FORMAT_VERSION + 1),
            (HEADER_HEADER_SIZE_OFFSET, HEADER_LEN as u32 + 8),
        ] {
            let mut bytes = valid();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            seal(&mut bytes);
            assert_eq!(
                TransferManifest::decode(&bytes).err(),
                Some(TransferError::UnsupportedVersion),
                "offset {offset} value {value}",
            );
        }
    }

    /// Reserved space is the extension point: a decoder that ignored it would
    /// silently accept a manifest written by a newer producer that means
    /// something by those bytes.
    #[test]
    fn a_nonzero_required_flag_or_reserved_byte_is_refused() {
        let mut bytes = valid();
        bytes[HEADER_REQUIRED_FLAGS_OFFSET] = 1;
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes).err(),
            Some(TransferError::UnknownFlags)
        );

        let mut bytes = valid();
        bytes[HEADER_FIELDS_END] = 1;
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes).err(),
            Some(TransferError::UnknownFlags)
        );
    }

    /// The section offsets are a chain, each pinned to the end of the one
    /// before, and `total_len` must equal the slice actually handed over. Both
    /// are what stop a manifest from naming bytes outside itself.
    #[test]
    fn a_broken_offset_chain_or_length_is_out_of_bounds() {
        for offset in [
            HEADER_OBJECT_OFFSET_OFFSET,
            HEADER_STATE_OFFSET_OFFSET,
            HEADER_RELEASE_OFFSET_OFFSET,
            HEADER_METADATA_OFFSET_OFFSET,
            HEADER_PAYLOAD_OFFSET_OFFSET,
            HEADER_TOTAL_LEN_OFFSET,
        ] {
            let mut bytes = valid();
            let shifted =
                u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("eight bytes")) + 8;
            bytes[offset..offset + 8].copy_from_slice(&shifted.to_le_bytes());
            seal(&mut bytes);
            assert_eq!(
                TransferManifest::decode(&bytes).err(),
                Some(TransferError::BadBounds),
                "offset {offset}",
            );
        }
    }

    /// A count past the generation's own ceiling is refused at the header,
    /// before any per-entry offset is computed from it.
    #[test]
    fn a_count_over_the_generation_ceiling_is_out_of_bounds() {
        for (offset, over) in [
            (
                HEADER_OBJECT_COUNT_OFFSET,
                crate::generation::MAX_OBJECTS + 1,
            ),
            (HEADER_STATE_COUNT_OFFSET, crate::generation::MAX_STATES + 1),
        ] {
            let mut bytes = valid();
            bytes[offset..offset + 4].copy_from_slice(&(over as u32).to_le_bytes());
            seal(&mut bytes);
            assert_eq!(
                TransferManifest::decode(&bytes).err(),
                Some(TransferError::BadBounds),
                "offset {offset}",
            );
        }
    }

    /// An index past the declared count is an error rather than a wrapped or
    /// zeroed entry, on both tables.
    #[test]
    fn an_index_past_the_declared_count_is_a_bad_entry() {
        let bytes = valid();
        let manifest = TransferManifest::decode(&bytes).expect("valid");
        assert_eq!(manifest.object(1), Err(TransferError::BadEntry));
        assert_eq!(manifest.state(1), Err(TransferError::BadEntry));
    }

    /// The payload flag and the payload offset must agree: a flag with no
    /// offset names nothing, and an offset with no flag would be read by a
    /// producer that set one and not the other.
    #[test]
    fn a_payload_flag_disagreeing_with_its_offset_is_a_bad_entry() {
        let object_offset = HEADER_LEN;

        let mut bytes = valid();
        bytes[object_offset + OBJECT_PAYLOAD_OFFSET_OFFSET
            ..object_offset + OBJECT_PAYLOAD_OFFSET_OFFSET + 8]
            .fill(0);
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes)
                .expect("header still valid")
                .object(0),
            Err(TransferError::BadEntry),
        );

        let mut bytes = valid();
        bytes[object_offset + OBJECT_FLAGS_OFFSET..object_offset + OBJECT_FLAGS_OFFSET + 4].fill(0);
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes)
                .expect("header still valid")
                .object(0),
            Err(TransferError::BadEntry),
        );

        let mut bytes = valid();
        bytes[object_offset + OBJECT_FLAGS_OFFSET..object_offset + OBJECT_FLAGS_OFFSET + 4]
            .copy_from_slice(&(OBJECT_FLAG_PAYLOAD << 1).to_le_bytes());
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes)
                .expect("header still valid")
                .object(0),
            Err(TransferError::BadEntry),
        );
    }

    /// A state that does not travel has no business in a transfer, and an
    /// undefined flag bit means a producer this build cannot interpret.
    #[test]
    fn a_state_that_does_not_travel_is_a_bad_entry() {
        let state_offset = HEADER_LEN + OBJECT_LEN;

        for flags in [0, STATE_FLAG_READ_ONLY, STATE_FLAG_TRAVEL | 0b1000] {
            let mut bytes = valid();
            bytes[state_offset + STATE_FLAGS_OFFSET..state_offset + STATE_FLAGS_OFFSET + 4]
                .copy_from_slice(&flags.to_le_bytes());
            seal(&mut bytes);
            assert_eq!(
                TransferManifest::decode(&bytes)
                    .expect("header still valid")
                    .state(0),
                Err(TransferError::BadEntry),
                "flags {flags:#x}",
            );
        }
    }

    /// Per-entry padding is reserved for the same reason the header's is.
    #[test]
    fn nonzero_entry_padding_is_a_bad_entry() {
        let mut bytes = valid();
        bytes[HEADER_LEN + OBJECT_PADDING_OFFSET] = 1;
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes)
                .expect("header still valid")
                .object(0),
            Err(TransferError::BadEntry),
        );

        let state_offset = HEADER_LEN + OBJECT_LEN;
        let mut bytes = valid();
        bytes[state_offset + STATE_PADDING_OFFSET] = 1;
        seal(&mut bytes);
        assert_eq!(
            TransferManifest::decode(&bytes)
                .expect("header still valid")
                .state(0),
            Err(TransferError::BadEntry),
        );
    }
}
