//! Instance lifetime resource object.
//!
//! A generation resource object naming the instances declared resident. A
//! resident instance runs for the life of its graph, so the root treats its
//! exit as a failure rather than as completion. Every instance the table does
//! not name is bounded, and a generation without the object has every instance
//! bounded.
//!
//! It is embedded as a `KIND_RESOURCE` object and authenticated by the
//! generation's per-object digest table. Decoding here assumes integrity has
//! already been verified and enforces only structure. Whether each row names an
//! admitted instance is the root's admission check, because only the root holds
//! the instance table the identities are derived from.

use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMELT\0";
include!("generated/instance_lifetime.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_ROWS * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    /// A row declares a lifetime other than resident, or carries nonzero
    /// reserved bytes. A bounded instance is expressed by its absence.
    BadEntry,
}

/// One instance's declared lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    Bounded,
    Resident,
}

impl Lifetime {
    /// The wire id the root answers a lifetime query with.
    pub const fn id(self) -> u8 {
        match self {
            Self::Bounded => LIFETIME_BOUNDED,
            Self::Resident => LIFETIME_RESIDENT,
        }
    }

    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            LIFETIME_BOUNDED => Some(Self::Bounded),
            LIFETIME_RESIDENT => Some(Self::Resident),
            _ => None,
        }
    }
}

/// A decoded, structurally validated lifetime table. Rows are sorted by
/// identity and unique, so lookup is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct InstanceLifetime<'a> {
    bytes: &'a [u8],
    row_count: usize,
}

impl<'a> InstanceLifetime<'a> {
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
        let row_count = u32_at(bytes, 24)? as usize;
        let total_len = u32_at(bytes, 28)? as usize;
        if row_count > MAX_ROWS
            || total_len != HEADER_BYTES + row_count * ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let mut previous = [0u8; 32];
        for index in 0..row_count {
            let entry = entry(bytes, index)?;
            if entry[32] != LIFETIME_RESIDENT || entry[33..].iter().any(|byte| *byte != 0) {
                return Err(DecodeError::BadEntry);
            }
            let identity: [u8; 32] = entry[..32].try_into().unwrap();
            if identity == [0; 32] || (index > 0 && identity <= previous) {
                return Err(DecodeError::BadOrder);
            }
            previous = identity;
        }
        Ok(Self { bytes, row_count })
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// The identity of the `index`th resident row.
    pub fn resident(&self, index: usize) -> Option<[u8; 32]> {
        (index < self.row_count).then(|| {
            entry(self.bytes, index).expect("validated lifetime row")[..32]
                .try_into()
                .unwrap()
        })
    }

    /// The declared lifetime of the instance with `identity`.
    pub fn lifetime_of(&self, identity: &[u8; 32]) -> Lifetime {
        if (0..self.row_count).any(|index| self.resident(index).as_ref() == Some(identity)) {
            Lifetime::Resident
        } else {
            Lifetime::Bounded
        }
    }
}

/// Stable identity derived from an instance name, in this contract's own
/// domain, so no other resource's identity can name a row here.
pub fn instance_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-instance-lifetime-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn entry(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)
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
    use alloc::vec::Vec;

    fn build(rows: &[[u8; 32]]) -> Vec<u8> {
        let total = HEADER_BYTES + rows.len() * ENTRY_BYTES;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(total as u32).to_le_bytes());
        for row in rows {
            bytes.extend_from_slice(row);
            bytes.push(LIFETIME_RESIDENT);
            bytes.extend_from_slice(&[0; 3]);
        }
        bytes
    }

    #[test]
    fn named_instances_are_resident_and_every_other_is_bounded() {
        let bytes = build(&[[0x11; 32], [0x22; 32]]);
        let table = InstanceLifetime::decode(&bytes).expect("decodes");
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.lifetime_of(&[0x11; 32]), Lifetime::Resident);
        assert_eq!(table.lifetime_of(&[0x22; 32]), Lifetime::Resident);
        assert_eq!(table.lifetime_of(&[0x33; 32]), Lifetime::Bounded);
        assert_eq!(table.resident(1), Some([0x22; 32]));
        assert_eq!(table.resident(2), None);
    }

    #[test]
    fn unsorted_duplicate_or_zero_rows_fail_closed() {
        for rows in [
            &[[0x22; 32], [0x11; 32]][..],
            &[[0x11; 32], [0x11; 32]][..],
            &[[0x00; 32]][..],
        ] {
            assert_eq!(
                InstanceLifetime::decode(&build(rows)).unwrap_err(),
                DecodeError::BadOrder
            );
        }
    }

    #[test]
    fn a_bounded_or_unknown_lifetime_row_and_nonzero_reserved_fail_closed() {
        let offset = HEADER_BYTES + 32;
        for (position, value) in [(offset, LIFETIME_BOUNDED), (offset, 2), (offset + 1, 1)] {
            let mut bytes = build(&[[0x11; 32]]);
            bytes[position] = value;
            assert_eq!(
                InstanceLifetime::decode(&bytes).unwrap_err(),
                DecodeError::BadEntry
            );
        }
    }

    #[test]
    fn wrong_magic_version_flags_and_bounds_fail_closed() {
        let mut bytes = build(&[[0x11; 32]]);
        bytes[0] = b'X';
        assert_eq!(
            InstanceLifetime::decode(&bytes).unwrap_err(),
            DecodeError::BadMagic
        );
        let mut bytes = build(&[[0x11; 32]]);
        bytes[8] = 2;
        assert_eq!(
            InstanceLifetime::decode(&bytes).unwrap_err(),
            DecodeError::UnsupportedVersion
        );
        let mut bytes = build(&[[0x11; 32]]);
        bytes[16] = 1;
        assert_eq!(
            InstanceLifetime::decode(&bytes).unwrap_err(),
            DecodeError::UnknownRequiredFlags
        );
        let mut bytes = build(&[[0x11; 32]]);
        bytes[24] = 2;
        assert_eq!(
            InstanceLifetime::decode(&bytes).unwrap_err(),
            DecodeError::BadBounds
        );
        let mut bytes = build(&[[0x11; 32]]);
        bytes.push(0);
        assert_eq!(
            InstanceLifetime::decode(&bytes).unwrap_err(),
            DecodeError::BadBounds
        );
        let bytes = build(&[[0x11; 32]]);
        assert_eq!(
            InstanceLifetime::decode(&bytes[..HEADER_BYTES - 1]).unwrap_err(),
            DecodeError::Truncated
        );
    }

    #[test]
    fn identity_is_domain_separated_and_distinguishes_names() {
        for name in ["init", "network-service", ""] {
            assert_ne!(
                instance_identity(name),
                crate::scheduling_class::instance_identity(name)
            );
        }
        assert_ne!(
            instance_identity("lifetime-holder"),
            instance_identity("lifetime-leaver")
        );
    }

    #[test]
    fn lifetime_ids_round_trip() {
        for lifetime in [Lifetime::Bounded, Lifetime::Resident] {
            assert_eq!(Lifetime::from_id(lifetime.id()), Some(lifetime));
        }
        assert_eq!(Lifetime::from_id(2), None);
    }
}
