//! Private-memory budget resource object (C10.2).
//!
//! A generation resource object declaring how many pages of task-private
//! working memory each named component may hold. It is embedded as a
//! `KIND_RESOURCE` object in a generation and authenticated by the generation's
//! existing per-object digest table, so decoding here assumes integrity already
//! verified and enforces only structural validity and the deterministic,
//! globally-possible bounds every launch must satisfy.
//!
//! A holder is named by a stable identity derived from its component name
//! ([`holder_identity`]); the root matches a launching component to its declared
//! quota by the same derivation. A holder absent from the budget has no quota
//! and cannot grow a single page — deny by default.
//!
//! # Why a sibling of [`crate::shared_buffer_budget`] rather than a column in it
//!
//! The two bound unrelated mechanisms. A shared buffer is a nameable,
//! transferable object the root retypes under a root-wide page ceiling; private
//! memory is one task's own window no other component can ever see or receive.
//! Widening the C7.3 contract would make every existing holder entry restate a
//! quota for a mechanism it does not use, and would tie the two formats'
//! versions together so a private-memory change rewrote every shared-buffer
//! budget on disk. The identity domain is distinct for the same reason: an
//! identity computed for one budget must never be replayable as a valid
//! identity in the other.

use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMEPM\0";
include!("generated/private_memory_budget.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_HOLDERS * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    /// A declared quota cannot ever be satisfied under the fixed root ceilings
    /// passed to [`PrivateMemoryBudget::validate_against`].
    Impossible,
}

/// One holder's declared private-memory quota: the absolute page ceiling its
/// region may ever be grown to, not a rate and not an initial size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderQuota {
    pub holder_identity: [u8; 32],
    pub page_quota: u32,
}

impl HolderQuota {
    /// The deny-by-default answer: a holder the budget does not name grows
    /// nothing. Its identity is all-zero, which [`PrivateMemoryBudget::decode`]
    /// refuses as an entry, so this value can never collide with a real holder.
    pub const DENY: Self = Self {
        holder_identity: [0; 32],
        page_quota: 0,
    };
}

/// A decoded, structurally validated private-memory budget. Holder entries are
/// sorted by identity and unique, so lookup is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct PrivateMemoryBudget<'a> {
    bytes: &'a [u8],
    holder_count: usize,
}

impl<'a> PrivateMemoryBudget<'a> {
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

    /// Reject any budget the root can never honour, so a declared quota is the
    /// live ceiling rather than a hope. Validated before any component
    /// launches. A zero quota is legal — a holder that may grow nothing, which
    /// is the same state an absent holder is in; it is impossibility, not
    /// smallness, that is rejected.
    ///
    /// Two classes, matching [`crate::shared_buffer_budget`]. Per-holder: a
    /// ceiling above what one task's reserved window can physically hold, which
    /// no growth could ever reach because the base cannot move. Aggregate
    /// (B8): a budget whose holders, all at their declared ceilings at once,
    /// would exceed the root-wide page ceiling. The aggregate rule is what makes
    /// a validating budget one the root can honour in full — without it the
    /// declaration degrades into first-come-first-served, and a late-growing
    /// component would be refused a quota the generation promised it.
    pub fn validate_against(
        &self,
        max_region_pages: u32,
        max_total_pages: u32,
    ) -> Result<(), DecodeError> {
        let mut total_pages: u32 = 0;
        for index in 0..self.holder_count {
            let entry = decode_entry(self.bytes, index).expect("validated budget entry");
            // The per-task reservation is structural: the window's address
            // space and translation tables are sized for it when the child
            // VSpace is built, so a ceiling above it is unreachable rather than
            // merely expensive.
            if entry.page_quota > max_region_pages {
                return Err(DecodeError::Impossible);
            }
            // Sum with saturation: an overflowing total is over-committed by
            // construction, and saturating keeps the comparison below honest
            // instead of wrapping into a passing value.
            total_pages = total_pages.saturating_add(entry.page_quota);
        }
        if total_pages > max_total_pages {
            return Err(DecodeError::Impossible);
        }
        Ok(())
    }
}

/// Stable holder identity derived from a component name. The root derives the
/// same value for a launching component to find its declared quota.
///
/// Its domain tag is this contract's own. An identity computed here must never
/// be a valid identity in [`crate::shared_buffer_budget`], and vice versa: two
/// budgets bounding unrelated mechanisms must not be able to name each other's
/// holders.
pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-private-memory-holder-v1");
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
        page_quota: u32_at(entry, 32)?,
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
    use alloc::vec::Vec;

    fn quota(identity: u8, pages: u32) -> HolderQuota {
        HolderQuota {
            holder_identity: [identity; 32],
            page_quota: pages,
        }
    }

    fn build(holders: &[HolderQuota]) -> Vec<u8> {
        let total = HEADER_BYTES + holders.len() * ENTRY_BYTES;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(holders.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(total as u32).to_le_bytes());
        for holder in holders {
            bytes.extend_from_slice(&holder.holder_identity);
            bytes.extend_from_slice(&holder.page_quota.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_sorted_holders_and_looks_up_quota() {
        let a = quota(0x11, 4);
        let b = quota(0x22, 16);
        let bytes = build(&[a, b]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert_eq!(budget.holder_count(), 2);
        assert_eq!(budget.quota_for(&[0x11; 32]), Some(a));
        assert_eq!(budget.quota_for(&[0x22; 32]), Some(b));
        // Deny by default: a holder the budget does not name has no quota.
        assert_eq!(budget.quota_for(&[0x33; 32]), None);
        assert_eq!(budget.holder(0), Some(a));
        assert_eq!(budget.holder(2), None);
    }

    #[test]
    fn unsorted_or_duplicate_holders_fail_closed() {
        let bytes = build(&[quota(0x22, 4), quota(0x11, 4)]);
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
        let bytes = build(&[quota(0x11, 4), quota(0x11, 8)]);
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    #[test]
    fn zero_holder_identity_fails_closed() {
        let bytes = build(&[quota(0x00, 4)]);
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    #[test]
    fn wrong_magic_version_and_flags_fail_closed() {
        let mut bytes = build(&[quota(0x11, 4)]);
        bytes[0] = b'X';
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadMagic)
        ));
        let mut bytes = build(&[quota(0x11, 4)]);
        bytes[8] = 9;
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::UnsupportedVersion)
        ));
        // A reserved flag bit is a requirement this reader does not understand,
        // so it fails closed rather than being ignored.
        let mut bytes = build(&[quota(0x11, 4)]);
        bytes[16] = 1;
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::UnknownRequiredFlags)
        ));
    }

    #[test]
    fn truncated_and_bad_count_fail_closed() {
        let bytes = build(&[quota(0x11, 4)]);
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        ));
        // A holder count the byte length cannot back.
        let mut bytes = build(&[quota(0x11, 4)]);
        bytes[24] = 4;
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
        // A count above the format's own holder bound.
        let mut bytes = build(&[quota(0x11, 4)]);
        bytes[24] = (MAX_HOLDERS + 1) as u8;
        assert!(matches!(
            PrivateMemoryBudget::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
    }

    /// The root's real ceilings: `private_memory::MAX_REGION_PAGES` and
    /// `MAX_TOTAL_PAGES`. Named once so every case below reads against the same
    /// pair.
    fn check(budget: &PrivateMemoryBudget<'_>) -> Result<(), DecodeError> {
        budget.validate_against(512, 2048)
    }

    #[test]
    fn a_quota_above_the_reservation_is_rejected() {
        let bytes = build(&[quota(0x11, 513)]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(matches!(check(&budget), Err(DecodeError::Impossible)));
        // Exactly the reservation is satisfiable: the window holds it.
        let bytes = build(&[quota(0x11, 512)]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(check(&budget).is_ok());
        // A zero quota is legal — a holder that may grow nothing.
        let bytes = build(&[quota(0x11, 0)]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(check(&budget).is_ok());
    }

    #[test]
    fn aggregate_over_commitment_is_rejected() {
        // Five holders at the full per-task reservation sum to 2560 > 2048, so
        // they cannot all peak at once even though each is individually
        // satisfiable (B8).
        let holders: Vec<HolderQuota> = (1..=5).map(|index| quota(index, 512)).collect();
        let bytes = build(&holders);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(matches!(check(&budget), Err(DecodeError::Impossible)));

        // Four at the same ceiling total exactly 2048 and pass, so the rejection
        // above is the sum and not the holder count.
        let holders: Vec<HolderQuota> = (1..=4).map(|index| quota(index, 512)).collect();
        let bytes = build(&holders);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(check(&budget).is_ok());
    }

    #[test]
    fn an_overflowing_aggregate_saturates_rather_than_wrapping() {
        // Two holders whose quotas sum past `u32::MAX`. Each is refused by the
        // per-holder arm, but the saturating sum is what keeps the aggregate
        // comparison honest if that arm ever moved.
        let bytes = build(&[quota(0x11, u32::MAX), quota(0x22, u32::MAX)]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert!(matches!(
            budget.validate_against(u32::MAX, 2048),
            Err(DecodeError::Impossible)
        ));
    }

    #[test]
    fn holder_identity_is_domain_separated_from_the_shared_buffer_budget() {
        // The same component name must not produce the same identity in both
        // budgets: an identity computed for one must never be replayable in the
        // other.
        for name in ["init", "dango", "spawn-service", ""] {
            assert_ne!(
                holder_identity(name),
                crate::shared_buffer_budget::holder_identity(name),
                "{name} collides across budget identity domains"
            );
        }
        // And it still distinguishes names within its own domain.
        assert_ne!(holder_identity("init"), holder_identity("inits"));
    }

    #[test]
    fn an_empty_budget_denies_every_holder() {
        let bytes = build(&[]);
        let budget = PrivateMemoryBudget::decode(&bytes).expect("decodes");
        assert_eq!(budget.holder_count(), 0);
        assert_eq!(budget.quota_for(&holder_identity("init")), None);
        assert!(check(&budget).is_ok());
    }
}
