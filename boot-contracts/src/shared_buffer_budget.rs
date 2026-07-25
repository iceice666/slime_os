//! Shared-buffer budget resource object (C7.3).
//!
//! A generation resource object declaring per-holder shared-buffer quotas. It
//! is embedded as a `KIND_RESOURCE` object in a generation and authenticated by
//! the generation's existing per-object digest table, so decoding here assumes
//! integrity already verified and enforces only structural validity and the
//! deterministic, globally-possible bounds every launch must satisfy.
//!
//! A holder is named by a stable identity derived from its component name
//! ([`holder_identity`]); the kernel matches a launching component to its
//! declared quota by the same derivation. A holder absent from the budget has
//! no quota and cannot allocate any shared buffer — deny by default.

use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMESB\0";
include!("generated/shared_buffer_budget.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_HOLDERS * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    NonZeroReserved,
    BadOrder,
    /// A declared quota cannot ever be satisfied under the fixed kernel
    /// ceilings passed to [`SharedBufferBudget::validate_against`].
    Impossible,
}

/// One holder's declared shared-buffer quota. Every field is an absolute
/// live-resource ceiling, not a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderQuota {
    pub holder_identity: [u8; 32],
    pub byte_pages: u32,
    pub buffer_count: u32,
    pub mapping_count: u32,
    pub loan_count: u32,
}

/// A decoded, structurally validated shared-buffer budget. Holder entries are
/// sorted by identity and unique, so lookup is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct SharedBufferBudget<'a> {
    bytes: &'a [u8],
    holder_count: usize,
}

impl<'a> SharedBufferBudget<'a> {
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
        let holder_count = u32_at(bytes, 24)? as usize;
        let total_len = u32_at(bytes, 28)? as usize;
        if holder_count > MAX_HOLDERS
            || total_len != HEADER_BYTES + holder_count * ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let mut previous = [0u8; 32];
        for index in 0..holder_count {
            let entry = decode_entry(bytes, index)?;
            if entry.holder_identity == [0; 32] || (index > 0 && entry.holder_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            previous = entry.holder_identity;
        }
        Ok(Self {
            bytes,
            holder_count,
        })
    }

    pub fn holder_count(&self) -> usize {
        self.holder_count
    }

    pub fn holder(&self, index: usize) -> Option<HolderQuota> {
        (index < self.holder_count)
            .then(|| decode_entry(self.bytes, index).expect("validated budget entry"))
    }

    /// Return the quota declared for `identity`, or `None` when the holder is
    /// absent (deny by default).
    pub fn quota_for(&self, identity: &[u8; 32]) -> Option<HolderQuota> {
        for index in 0..self.holder_count {
            let entry = decode_entry(self.bytes, index).expect("validated budget entry");
            if entry.holder_identity == *identity {
                return Some(entry);
            }
        }
        None
    }

    /// Reject any quota that can never be satisfied under the fixed kernel
    /// ceilings. This is validated before component launch, so a manifest that
    /// promises more than the kernel can ever grant fails closed rather than
    /// silently capping at runtime. A zero quota is legal (a holder that may
    /// hold nothing); it is impossibility, not smallness, that is rejected.
    pub fn validate_against(
        &self,
        max_buffer_pages: u32,
        max_total_pages: u32,
        max_shared_buffers: u32,
    ) -> Result<(), DecodeError> {
        for index in 0..self.holder_count {
            let entry = decode_entry(self.bytes, index).expect("validated budget entry");
            // A page ceiling above the global page budget, or a per-holder
            // buffer count above the whole table, can never be reached.
            if entry.byte_pages > max_total_pages || entry.buffer_count > max_shared_buffers {
                return Err(DecodeError::Impossible);
            }
            // Every declared buffer must be able to hold at least one page
            // without exceeding the per-holder page ceiling; a holder promised
            // more buffers than its page ceiling can back is impossible.
            if entry.buffer_count > entry.byte_pages {
                return Err(DecodeError::Impossible);
            }
            // A mapping references pages already charged to the holder, so the
            // holder cannot ever have more live mappings than pages; likewise a
            // loan references at most one live buffer.
            if entry.mapping_count > entry.byte_pages || entry.loan_count > entry.buffer_count {
                return Err(DecodeError::Impossible);
            }
            // A single buffer never exceeds the per-run contiguous ceiling, but
            // that is a per-buffer, not per-holder, cap; nothing to check here
            // beyond the above. `max_buffer_pages` is retained for symmetry with
            // the kernel bounds and future per-buffer descriptor checks.
            let _ = max_buffer_pages;
        }
        Ok(())
    }
}

/// Stable holder identity derived from a component name. The kernel derives the
/// same value for a launching component to find its declared quota.
pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-shared-buffer-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<HolderQuota, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    Ok(HolderQuota {
        holder_identity: entry[..32].try_into().unwrap(),
        byte_pages: u32_at(entry, 32)?,
        buffer_count: u32_at(entry, 36)?,
        mapping_count: u32_at(entry, 40)?,
        loan_count: u32_at(entry, 44)?,
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

    fn build(entries: &[HolderQuota]) -> alloc::vec::Vec<u8> {
        let total_len = HEADER_BYTES + entries.len() * ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8; total_len];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..28].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes[28..32].copy_from_slice(&(total_len as u32).to_le_bytes());
        for (index, quota) in entries.iter().enumerate() {
            let offset = HEADER_BYTES + index * ENTRY_BYTES;
            bytes[offset..offset + 32].copy_from_slice(&quota.holder_identity);
            bytes[offset + 32..offset + 36].copy_from_slice(&quota.byte_pages.to_le_bytes());
            bytes[offset + 36..offset + 40].copy_from_slice(&quota.buffer_count.to_le_bytes());
            bytes[offset + 40..offset + 44].copy_from_slice(&quota.mapping_count.to_le_bytes());
            bytes[offset + 44..offset + 48].copy_from_slice(&quota.loan_count.to_le_bytes());
        }
        bytes
    }

    fn quota(id: u8, pages: u32, buffers: u32) -> HolderQuota {
        HolderQuota {
            holder_identity: [id; 32],
            byte_pages: pages,
            buffer_count: buffers,
            mapping_count: buffers,
            loan_count: buffers,
        }
    }

    #[test]
    fn decodes_sorted_holders_and_looks_up_quota() {
        let a = quota(0x11, 8, 2);
        let b = quota(0x22, 4, 1);
        let bytes = build(&[a, b]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert_eq!(budget.holder_count(), 2);
        assert_eq!(budget.quota_for(&[0x11; 32]), Some(a));
        assert_eq!(budget.quota_for(&[0x22; 32]), Some(b));
        // Absent holder has no quota (deny by default).
        assert_eq!(budget.quota_for(&[0x33; 32]), None);
    }

    #[test]
    fn unsorted_or_duplicate_holders_fail_closed() {
        let bytes = build(&[quota(0x22, 4, 1), quota(0x11, 4, 1)]);
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
        let bytes = build(&[quota(0x11, 4, 1), quota(0x11, 4, 1)]);
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    #[test]
    fn zero_holder_identity_fails_closed() {
        let bytes = build(&[quota(0x00, 4, 1)]);
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    #[test]
    fn wrong_magic_and_version_fail_closed() {
        let mut bytes = build(&[quota(0x11, 4, 1)]);
        bytes[0] = b'X';
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::BadMagic)
        ));
        let mut bytes = build(&[quota(0x11, 4, 1)]);
        bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::UnsupportedVersion)
        ));
    }

    #[test]
    fn truncated_and_bad_count_fail_closed() {
        let bytes = build(&[quota(0x11, 4, 1)]);
        assert!(matches!(
            SharedBufferBudget::decode(&bytes[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        ));
        let mut bytes = build(&[quota(0x11, 4, 1)]);
        // Claim two holders but supply one entry of bytes.
        bytes[24..28].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(
            SharedBufferBudget::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
    }

    #[test]
    fn impossible_quotas_rejected_against_ceilings() {
        // Page ceiling above the global budget.
        let bytes = build(&[quota(0x11, 300, 1)]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert!(matches!(
            budget.validate_against(64, 256, 32),
            Err(DecodeError::Impossible)
        ));
        // More buffers than the whole table.
        let bytes = build(&[quota(0x11, 64, 40)]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert!(matches!(
            budget.validate_against(64, 256, 32),
            Err(DecodeError::Impossible)
        ));
        // More buffers than pages: cannot back each buffer with a page.
        let bytes = build(&[quota(0x11, 2, 3)]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert!(matches!(
            budget.validate_against(64, 256, 32),
            Err(DecodeError::Impossible)
        ));
        // A satisfiable budget passes.
        let bytes = build(&[quota(0x11, 8, 2)]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert!(budget.validate_against(64, 256, 32).is_ok());
        // A zero quota is legal (holder may hold nothing).
        let bytes = build(&[quota(0x11, 0, 0)]);
        let budget = SharedBufferBudget::decode(&bytes).expect("decodes");
        assert!(budget.validate_against(64, 256, 32).is_ok());
    }
}
