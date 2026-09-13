//! Decoding for the generation-authenticated C9.1 clock-authority resource.
//!
//! This gates Slime's root-brokered clock, timer, and simulated-time services.
//! It does not make AArch64's raw physical counter inaccessible: the current
//! seL4 profiles enable EL0 counter/timer access globally because `slime-root`
//! itself uses that path, and the kernel exposes no per-TCB narrowing.

use crate::generation::{
    RIGHT_CLOCK_MONOTONIC_READ, RIGHT_CLOCK_SIMULATED_ADVANCE, RIGHT_CLOCK_SIMULATED_READ,
    RIGHT_CLOCK_TIMER_USE,
};
use crate::sha256::Sha256;

include!("generated/clock_authority.rs");

pub const MAGIC: [u8; 8] = *b"SLIMECA\0";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_HOLDERS * ENTRY_BYTES;
pub const AUTHORITY_ALL: u64 = RIGHT_CLOCK_MONOTONIC_READ
    | RIGHT_CLOCK_TIMER_USE
    | RIGHT_CLOCK_SIMULATED_READ
    | RIGHT_CLOCK_SIMULATED_ADVANCE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    UnknownAuthority,
    BadTimerDeclaration,
    Impossible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderAuthority {
    pub holder_identity: [u8; 32],
    pub authority_flags: u64,
    pub timer_quota: u32,
    pub notification_grant_identity: u64,
    pub notification_badge: u64,
}

impl HolderAuthority {
    pub const DENY: Self = Self {
        holder_identity: [0; 32],
        authority_flags: 0,
        timer_quota: 0,
        notification_grant_identity: 0,
        notification_badge: 0,
    };

    pub const fn allows(self, authority: u64) -> bool {
        self.authority_flags & authority == authority
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClockAuthority<'a> {
    bytes: &'a [u8],
    holder_count: usize,
}

impl<'a> ClockAuthority<'a> {
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
        let mut timer_total = 0u32;
        for index in 0..holder_count {
            let entry = decode_entry(bytes, index)?;
            if entry.holder_identity == [0; 32] || (index > 0 && entry.holder_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            if u32_at(entry_bytes(bytes, index)?, 44)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if entry.authority_flags == 0 || entry.authority_flags & !AUTHORITY_ALL != 0 {
                return Err(DecodeError::UnknownAuthority);
            }
            let timer_authorized = entry.allows(RIGHT_CLOCK_TIMER_USE);
            if timer_authorized
                != (entry.timer_quota > 0
                    && entry.notification_grant_identity != 0
                    && entry.notification_badge != 0
                    && entry.notification_badge.count_ones() == 1)
            {
                return Err(DecodeError::BadTimerDeclaration);
            }
            if entry.timer_quota as usize > MAX_LIVE_TIMERS_PER_HOLDER {
                return Err(DecodeError::Impossible);
            }
            timer_total = timer_total.saturating_add(entry.timer_quota);
            previous = entry.holder_identity;
        }
        if timer_total as usize > MAX_LIVE_TIMERS {
            return Err(DecodeError::Impossible);
        }
        Ok(Self {
            bytes,
            holder_count,
        })
    }

    pub const fn holder_count(&self) -> usize {
        self.holder_count
    }
    pub fn timer_quota(&self) -> u32 {
        let mut total = 0u32;
        for index in 0..self.holder_count {
            total += decode_entry(self.bytes, index)
                .expect("validated clock authority entry")
                .timer_quota;
        }
        total
    }

    pub fn holder(&self, index: usize) -> Option<HolderAuthority> {
        (index < self.holder_count)
            .then(|| decode_entry(self.bytes, index).expect("validated clock authority entry"))
    }

    pub fn authority_for(&self, identity: &[u8; 32]) -> Option<HolderAuthority> {
        for index in 0..self.holder_count {
            let entry = decode_entry(self.bytes, index).expect("validated clock authority entry");
            if entry.holder_identity == *identity {
                return Some(entry);
            }
        }
        None
    }
}

pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-clock-authority-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// Stable identity of the generation notification grant an authority entry
/// names. The root verifies this against the decoded grant before installing
/// timer state, so an entry cannot redirect expiry to a different object.
pub fn notification_grant_identity(name: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-clock-notification-grant-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap())
}

fn entry_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<HolderAuthority, DecodeError> {
    let entry = entry_bytes(bytes, index)?;
    Ok(HolderAuthority {
        holder_identity: entry[..32].try_into().unwrap(),
        authority_flags: u64_at(entry, 32)?,
        timer_quota: u32_at(entry, 40)?,
        notification_grant_identity: u64_at(entry, 48)?,
        notification_badge: u64_at(entry, 56)?,
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

    fn encoded(entries: &[[u8; ENTRY_BYTES]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes.extend_from_slice(
            &(HEADER_BYTES as u32 + entries.len() as u32 * ENTRY_BYTES as u32).to_le_bytes(),
        );
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    fn entry(name: &str, flags: u64, quota: u32, grant: u64, badge: u64) -> [u8; ENTRY_BYTES] {
        let mut entry = [0u8; ENTRY_BYTES];
        entry[..32].copy_from_slice(&holder_identity(name));
        entry[32..40].copy_from_slice(&flags.to_le_bytes());
        entry[40..44].copy_from_slice(&quota.to_le_bytes());
        entry[48..56].copy_from_slice(&grant.to_le_bytes());
        entry[56..64].copy_from_slice(&badge.to_le_bytes());
        entry
    }

    #[test]
    fn authorities_are_independent_and_absence_denies() {
        let mut entries = [
            entry("timer", RIGHT_CLOCK_TIMER_USE, 2, 7, 1 << 9),
            entry("sim", RIGHT_CLOCK_SIMULATED_READ, 0, 0, 0),
            entry("mono", RIGHT_CLOCK_MONOTONIC_READ, 0, 0, 0),
        ];
        entries.sort_by_key(|entry| entry[..32].to_vec());
        let bytes = encoded(&entries);
        let decoded = ClockAuthority::decode(&bytes).unwrap();
        assert!(
            decoded
                .authority_for(&holder_identity("timer"))
                .unwrap()
                .allows(RIGHT_CLOCK_TIMER_USE)
        );
        assert!(
            !decoded
                .authority_for(&holder_identity("timer"))
                .unwrap()
                .allows(RIGHT_CLOCK_MONOTONIC_READ)
        );
        assert!(decoded.authority_for(&holder_identity("absent")).is_none());
    }

    #[test]
    fn timer_authority_requires_quota_grant_and_one_badge_bit() {
        for invalid in [
            entry("x", RIGHT_CLOCK_TIMER_USE, 0, 7, 1),
            entry("x", RIGHT_CLOCK_TIMER_USE, 1, 0, 1),
            entry("x", RIGHT_CLOCK_TIMER_USE, 1, 7, 0),
            entry("x", RIGHT_CLOCK_TIMER_USE, 1, 7, 3),
            entry("x", RIGHT_CLOCK_MONOTONIC_READ, 1, 7, 1),
        ] {
            assert!(matches!(
                ClockAuthority::decode(&encoded(&[invalid])),
                Err(DecodeError::BadTimerDeclaration)
            ));
        }
    }

    #[test]
    fn timer_bounds_are_admission_properties() {
        let over = entry(
            "x",
            RIGHT_CLOCK_TIMER_USE,
            MAX_LIVE_TIMERS_PER_HOLDER as u32 + 1,
            7,
            1,
        );
        assert!(matches!(
            ClockAuthority::decode(&encoded(&[over])),
            Err(DecodeError::Impossible)
        ));
    }
}
