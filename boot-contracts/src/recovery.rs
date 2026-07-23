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
