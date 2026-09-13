use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMERC\0";
include!("generated/recovery.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_STATE_OBJECTS * STATE_ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    NonZeroReserved,
    BadOrder,
    BadStateRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateEntry {
    pub binding_identity: [u8; 32],
    pub object_identity: [u8; 32],
    pub schema_version: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct RecoveryIndex<'a> {
    pub target_generation: [u8; 32],
    pub generation_root: [u8; 32],
    pub state_root: [u8; 32],
    pub accepted_release_sequence: u64,
    pub target_pci_bdf: u32,
    pub state_first_lba: u64,
    pub state_last_lba: u64,
    bytes: &'a [u8],
    state_count: usize,
}

impl<'a> RecoveryIndex<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
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
        let state_count = u32_at(bytes, 132)? as usize;
        let total_len = u32_at(bytes, 136)? as usize;
        if state_count > MAX_STATE_OBJECTS
            || total_len != HEADER_BYTES + state_count * STATE_ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        if bytes[156..HEADER_BYTES].iter().any(|byte| *byte != 0) {
            return Err(DecodeError::NonZeroReserved);
        }
        let mut previous = [0u8; 32];
        let mut hasher = Sha256::new();
        for position in 0..state_count {
            let entry = decode_state(bytes, position)?;
            if entry.binding_identity == [0; 32]
                || entry.object_identity == [0; 32]
                || entry.schema_version == 0
                || (position > 0 && entry.binding_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            hasher.update(&entry.binding_identity);
            hasher.update(&entry.object_identity);
            hasher.update(&entry.schema_version.to_le_bytes());
            previous = entry.binding_identity;
        }
        let state_root: [u8; 32] = bytes[88..120].try_into().unwrap();
        if hasher.finalize() != state_root {
            return Err(DecodeError::BadStateRoot);
        }
        let target_generation: [u8; 32] = bytes[24..56].try_into().unwrap();
        let generation_root: [u8; 32] = bytes[56..88].try_into().unwrap();
        let state_first_lba = u64_at(bytes, 140)?;
        let state_last_lba = u64_at(bytes, 148)?;
        if target_generation == [0; 32]
            || generation_root == [0; 32]
            || state_first_lba > state_last_lba
        {
            return Err(DecodeError::BadBounds);
        }
        Ok(Self {
            target_generation,
            generation_root,
            state_root,
            accepted_release_sequence: u64_at(bytes, 120)?,
            target_pci_bdf: u32_at(bytes, 128)?,
            state_first_lba,
            state_last_lba,
            bytes,
            state_count,
        })
    }

    pub fn state_count(&self) -> usize {
        self.state_count
    }

    pub fn state(&self, index: usize) -> Option<StateEntry> {
        (index < self.state_count)
            .then(|| decode_state(self.bytes, index).expect("validated recovery state entry"))
    }
}

pub fn binding_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-state-binding-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_state(bytes: &[u8], index: usize) -> Result<StateEntry, DecodeError> {
    let offset = HEADER_BYTES + index * STATE_ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + STATE_ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    if entry[68..72].iter().any(|byte| *byte != 0) {
        return Err(DecodeError::NonZeroReserved);
    }
    Ok(StateEntry {
        binding_identity: entry[..32].try_into().unwrap(),
        object_identity: entry[32..64].try_into().unwrap(),
        schema_version: u32_at(entry, 64)?,
    })
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;

    /// Recompute the state root the decoder checks: SHA-256 over every entry's
    /// binding, object, and schema version in table order. Kept out of the
    /// builder so a test can corrupt an entry *after* sealing and get
    /// `BadStateRoot` rather than passing against a root recomputed to match.
    fn seal(bytes: &mut [u8], state_count: usize) {
        let mut hasher = Sha256::new();
        for position in 0..state_count {
            let offset = HEADER_BYTES + position * STATE_ENTRY_BYTES;
            hasher.update(&bytes[offset..offset + 32]);
            hasher.update(&bytes[offset + 32..offset + 64]);
            hasher.update(&bytes[offset + 64..offset + 68]);
        }
        let root = hasher.finalize();
        bytes[88..120].copy_from_slice(&root);
    }

    /// Two state entries in strictly ascending binding order, which is the
    /// smallest index that exercises the ordering rule as well as the root.
    fn valid() -> alloc::vec::Vec<u8> {
        const STATE_COUNT: usize = 2;
        let total_len = HEADER_BYTES + STATE_COUNT * STATE_ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8; total_len];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..56].fill(0xA1);
        bytes[56..88].fill(0xB2);
        bytes[120..128].copy_from_slice(&11u64.to_le_bytes());
        bytes[128..132].copy_from_slice(&0x0000_0100u32.to_le_bytes());
        bytes[132..136].copy_from_slice(&(STATE_COUNT as u32).to_le_bytes());
        bytes[136..140].copy_from_slice(&(total_len as u32).to_le_bytes());
        bytes[140..148].copy_from_slice(&64u64.to_le_bytes());
        bytes[148..156].copy_from_slice(&96u64.to_le_bytes());

        for (position, binding) in [0x10u8, 0x20].into_iter().enumerate() {
            let offset = HEADER_BYTES + position * STATE_ENTRY_BYTES;
            bytes[offset..offset + 32].fill(binding);
            bytes[offset + 32..offset + 64].fill(binding | 0x01);
            bytes[offset + 64..offset + 68].copy_from_slice(&(position as u32 + 1).to_le_bytes());
        }
        seal(&mut bytes, STATE_COUNT);
        bytes
    }

    fn entry_offset(position: usize) -> usize {
        HEADER_BYTES + position * STATE_ENTRY_BYTES
    }

    /// Every field the decoder promises, read back from a sealed index. Without
    /// this the refusal corpus below could pass on a decoder that refuses
    /// everything.
    #[test]
    fn a_well_formed_index_decodes_with_every_field() {
        let bytes = valid();
        let index = RecoveryIndex::decode(&bytes).expect("valid index");
        assert_eq!(index.target_generation, [0xA1; 32]);
        assert_eq!(index.generation_root, [0xB2; 32]);
        assert_eq!(index.accepted_release_sequence, 11);
        assert_eq!(index.target_pci_bdf, 0x0000_0100);
        assert_eq!(index.state_first_lba, 64);
        assert_eq!(index.state_last_lba, 96);
        assert_eq!(index.state_count(), 2);

        assert_eq!(
            index.state(0),
            Some(StateEntry {
                binding_identity: [0x10; 32],
                object_identity: [0x11; 32],
                schema_version: 1,
            })
        );
        assert_eq!(
            index.state(1),
            Some(StateEntry {
                binding_identity: [0x20; 32],
                object_identity: [0x21; 32],
                schema_version: 2,
            })
        );
        assert_eq!(index.state(2), None);
    }

    /// An index with no state objects is legitimate — a generation that binds
    /// nothing still needs a recovery target — and its root is the hash of no
    /// input, not zero.
    #[test]
    fn an_empty_state_table_is_valid_and_roots_the_empty_hash() {
        let mut bytes = alloc::vec![0u8; HEADER_BYTES];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..56].fill(0xA1);
        bytes[56..88].fill(0xB2);
        bytes[132..136].copy_from_slice(&0u32.to_le_bytes());
        bytes[136..140].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        seal(&mut bytes, 0);
        let index = RecoveryIndex::decode(&bytes).expect("empty index is valid");
        assert_eq!(index.state_count(), 0);
        assert_eq!(index.state(0), None);
        assert_ne!(index.state_root, [0; 32]);
    }

    /// The state root is the integrity link between the index and the objects
    /// it names. A flipped byte in any covered field must break it, which is
    /// why `seal` is not re-run here.
    #[test]
    fn a_corrupted_entry_fails_the_state_root() {
        for field in [0usize, 32, 64] {
            let mut bytes = valid();
            bytes[entry_offset(1) + field] ^= 0x01;
            assert_eq!(
                RecoveryIndex::decode(&bytes).err(),
                Some(DecodeError::BadStateRoot),
                "field at +{field}",
            );
        }
    }

    /// Strictly ascending binding order makes lookup decidable and duplicates
    /// structurally impossible. Equal is as wrong as descending.
    #[test]
    fn out_of_order_or_duplicate_bindings_are_refused() {
        for second in [0x10u8, 0x05] {
            let mut bytes = valid();
            bytes[entry_offset(1)..entry_offset(1) + 32].fill(second);
            seal(&mut bytes, 2);
            assert_eq!(
                RecoveryIndex::decode(&bytes).err(),
                Some(DecodeError::BadOrder),
                "second binding {second:#x}",
            );
        }
    }

    /// A zero identity names nothing and a zero schema version is not a
    /// version, so neither can stand in for a real value.
    #[test]
    fn a_zero_identity_or_schema_version_is_refused() {
        for (offset, len) in [(0usize, 32usize), (32, 32), (64, 4)] {
            let mut bytes = valid();
            bytes[entry_offset(0) + offset..entry_offset(0) + offset + len].fill(0);
            seal(&mut bytes, 2);
            assert_eq!(
                RecoveryIndex::decode(&bytes).err(),
                Some(DecodeError::BadOrder),
                "zeroed field at +{offset}",
            );
        }
    }

    /// Without a target generation or its root there is nothing to recover to,
    /// so an all-zero value is refused rather than treated as absent.
    #[test]
    fn a_missing_target_generation_or_root_is_out_of_bounds() {
        for range in [24..56, 56..88] {
            let mut bytes = valid();
            bytes[range.clone()].fill(0);
            assert_eq!(
                RecoveryIndex::decode(&bytes).err(),
                Some(DecodeError::BadBounds),
                "zeroed {range:?}",
            );
        }
    }

    /// An inverted LBA span would describe a region the reader cannot walk. An
    /// equal pair is a single sector and stays legal.
    #[test]
    fn an_inverted_lba_span_is_out_of_bounds_but_a_single_sector_is_not() {
        let mut bytes = valid();
        bytes[140..148].copy_from_slice(&97u64.to_le_bytes());
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::BadBounds)
        );

        let mut bytes = valid();
        bytes[140..148].copy_from_slice(&96u64.to_le_bytes());
        RecoveryIndex::decode(&bytes).expect("first == last is one sector");
    }

    #[test]
    fn a_short_or_oversized_index_is_truncated() {
        let bytes = valid();
        assert_eq!(
            RecoveryIndex::decode(&bytes[..HEADER_BYTES - 1]).err(),
            Some(DecodeError::Truncated)
        );
        assert_eq!(
            RecoveryIndex::decode(&alloc::vec![0u8; MAX_BYTES + 1]).err(),
            Some(DecodeError::Truncated)
        );
    }

    #[test]
    fn a_foreign_magic_is_refused_before_anything_else() {
        let mut bytes = valid();
        bytes[0] = b'X';
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::BadMagic)
        );
    }

    /// A future format is refused rather than read with this version's offsets,
    /// and so is a header claiming a size this build did not compile.
    #[test]
    fn a_wrong_version_or_header_size_is_unsupported() {
        for (offset, value) in [(8usize, FORMAT_VERSION + 1), (12, HEADER_BYTES as u32 + 8)] {
            let mut bytes = valid();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(
                RecoveryIndex::decode(&bytes).err(),
                Some(DecodeError::UnsupportedVersion),
                "offset {offset}",
            );
        }
    }

    /// Required flags and reserved space are the extension points: a decoder
    /// ignoring them would silently accept an index whose newer producer means
    /// something by those bytes.
    #[test]
    fn a_required_flag_or_reserved_byte_is_refused_with_its_own_error() {
        let mut bytes = valid();
        bytes[16] = 1;
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::UnknownRequiredFlags)
        );

        let mut bytes = valid();
        bytes[156] = 1;
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::NonZeroReserved)
        );

        let mut bytes = valid();
        bytes[entry_offset(0) + 68] = 1;
        seal(&mut bytes, 2);
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::NonZeroReserved)
        );
    }

    /// `total_len` is checked against both the declared count and the slice
    /// handed over, so an index cannot claim entries it does not carry.
    #[test]
    fn a_count_or_length_disagreement_is_out_of_bounds() {
        let mut bytes = valid();
        bytes[132..136].copy_from_slice(&(MAX_STATE_OBJECTS as u32 + 1).to_le_bytes());
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::BadBounds)
        );

        let mut bytes = valid();
        bytes[132..136].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::BadBounds)
        );

        let mut bytes = valid();
        let claimed = bytes.len() as u32 + STATE_ENTRY_BYTES as u32;
        bytes[136..140].copy_from_slice(&claimed.to_le_bytes());
        assert_eq!(
            RecoveryIndex::decode(&bytes).err(),
            Some(DecodeError::BadBounds)
        );
    }

    /// The binding identity is domain-separated and length-prefixed, so two
    /// different names cannot collide and a name is not a bare hash of itself.
    #[test]
    fn binding_identity_is_domain_separated_and_length_prefixed() {
        assert_ne!(binding_identity("store"), binding_identity("state"));
        assert_ne!(binding_identity("ab"), binding_identity("abc"));
        assert_eq!(binding_identity("store"), binding_identity("store"));

        let mut bare = Sha256::new();
        bare.update(b"store");
        assert_ne!(binding_identity("store"), bare.finalize());
    }
}
