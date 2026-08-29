//! Exact per-ring block authority resource (B83).
//!
//! Authority is an exact tuple: holder, device, ring, and one independently
//! declared right. There is no wildcard holder, no device range, and no rights
//! value meaning "all", so a driver reading this table cannot widen what the
//! generation declared.
//!
//! This is the authenticated replacement for the root's retired
//! `BlockTransact` rights gate. That gate derived the caller from an endpoint
//! badge the kernel supplied and checked *that* task's block capability. A
//! driver serving an IO0 ring has no badge to derive: the ring is shared
//! memory, and a submission carries a request, a slice, and a lease but no
//! rights identity. Declaring the ring's rights here restores the gate without
//! trusting a request byte or a client-side check.

use crate::sha256::Sha256;
include!("generated/block_authority.rs");

pub const MAGIC: [u8; 8] = *b"SLIMEBLK";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_RINGS * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    InvalidEntry,
    Impossible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Right {
    Read,
    Write,
}

impl Right {
    const fn bit(self) -> u16 {
        match self {
            Self::Read => RIGHT_READ,
            Self::Write => RIGHT_WRITE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingAuthority {
    pub holder_identity: [u8; 32],
    pub device: u32,
    pub ring: u32,
    pub rights: u16,
    pub sector_limit: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockAuthority<'a> {
    bytes: &'a [u8],
    ring_count: usize,
}

impl<'a> BlockAuthority<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if u32_at(bytes, OFF_HEADER_FORMAT_VERSION)? != FORMAT_VERSION
            || u32_at(bytes, OFF_HEADER_HEADER_SIZE)? as usize != HEADER_BYTES
        {
            return Err(DecodeError::UnsupportedVersion);
        }
        if u64_at(bytes, OFF_HEADER_REQUIRED_FLAGS)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let count = u32_at(bytes, OFF_HEADER_RING_COUNT)? as usize;
        let total = u32_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize;
        if count > MAX_RINGS || total != HEADER_BYTES + count * ENTRY_BYTES || total != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let mut previous: Option<OrderKey> = None;
        for index in 0..count {
            let entry = decode_entry(bytes, index)?;
            let key = (entry.device, entry.ring);
            // Strictly ascending on `(device, ring)` alone, so one ring can
            // never carry two rows -- not even for two different holders. That
            // is the load-bearing property: a ring is one client's channel, and
            // two holders naming one ring would leave the driver unable to say
            // whose rights a submission carries, which is the exact defect this
            // table exists to close.
            if entry.holder_identity == [0; 32] || previous.is_some_and(|value| key <= value) {
                return Err(DecodeError::BadOrder);
            }
            // Bounded per-holder count. Quadratic over a table of at most
            // `MAX_RINGS`, run once at decode: the ordering is by ring, so a
            // holder's rows are not adjacent and cannot be counted by
            // comparing neighbours.
            let mut per_holder = 0;
            for other in 0..count {
                let candidate = decode_entry(bytes, other)?;
                if candidate.holder_identity == entry.holder_identity {
                    per_holder += 1;
                }
            }
            if per_holder > MAX_RINGS_PER_HOLDER {
                return Err(DecodeError::Impossible);
            }
            previous = Some(key);
        }
        Ok(Self {
            bytes,
            ring_count: count,
        })
    }

    pub const fn ring_count(&self) -> usize {
        self.ring_count
    }

    pub fn ring(&self, index: usize) -> Option<RingAuthority> {
        (index < self.ring_count)
            .then(|| decode_entry(self.bytes, index).expect("validated block authority"))
    }

    /// Canonical bytes for one authenticated entry, used by the root's paged
    /// read without introducing a second encoder for this layout.
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.ring_count {
            return None;
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(offset..offset + ENTRY_BYTES)
    }

    /// The authority bound to one exact `(device, ring)` pair.
    ///
    /// The lookup a driver uses, keyed on the ring rather than on a holder
    /// because the ring *is* the client's channel: the generation grants
    /// exactly one client that ring's shared buffer, and `(device, ring)` is
    /// unique across the whole table, so the ring names its client without the
    /// driver having to be told who is submitting. A driver asking by holder
    /// would have to learn the holder from somewhere, and the only available
    /// source is the submission itself — a client asserting its own authority.
    ///
    /// `None` rather than a permissive default: a ring the table does not name
    /// has no authority at all, which is a different answer from a named ring
    /// whose rights exclude the operation.
    pub fn ring_authority(&self, device: u32, ring: u32) -> Option<RingAuthority> {
        (0..self.ring_count).find_map(|index| {
            let entry = decode_entry(self.bytes, index).expect("validated block authority");
            (entry.device == device && entry.ring == ring).then_some(entry)
        })
    }

    /// Whether one exact ring carries one exact right.
    ///
    /// Fail-closed on every axis: an unnamed device, an unnamed ring, and a
    /// named ring without the bit all answer `false`.
    pub fn authorizes(&self, device: u32, ring: u32, right: Right) -> bool {
        self.ring_authority(device, ring)
            .is_some_and(|entry| entry.rights & right.bit() != 0)
    }

    /// Whether one exact ring carries one exact right *and* may reach every
    /// sector in `lba..lba + count`.
    ///
    /// The range check is here rather than in the driver because `sector_limit`
    /// is the declared bound and an overflowing sum must refuse rather than
    /// wrap into an in-range answer.
    pub fn authorizes_range(
        &self,
        device: u32,
        ring: u32,
        right: Right,
        lba: u64,
        count: u32,
    ) -> bool {
        self.ring_authority(device, ring).is_some_and(|entry| {
            entry.rights & right.bit() != 0
                && lba
                    .checked_add(u64::from(count))
                    .is_some_and(|end| end <= entry.sector_limit)
        })
    }
}

/// The holder identity a declared instance name hashes to.
///
/// Domain-separated from every other holder hash in the tree: the same
/// instance name must not produce the same identity in two resources, or a
/// grant in one would be forgeable from the other's table.
pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-block-authority-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<RingAuthority, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    let device = u32_at(entry, OFF_ENTRY_DEVICE)?;
    let ring = u32_at(entry, OFF_ENTRY_RING)?;
    let rights = u16_at(entry, OFF_ENTRY_RIGHTS)?;
    let sector_limit = u64_at(entry, OFF_ENTRY_SECTOR_LIMIT)?;
    if rights == 0 || rights & !KNOWN_RIGHTS != 0 || sector_limit == 0 {
        return Err(DecodeError::InvalidEntry);
    }
    // Reserved bytes must be zero: a later version giving them meaning must
    // not be satisfiable by a table this version already admitted.
    if entry[OFF_ENTRY_RESERVED..OFF_ENTRY_RESERVED_END]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(DecodeError::InvalidEntry);
    }
    Ok(RingAuthority {
        holder_identity: entry[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
            .try_into()
            .expect("generated block-authority layout"),
        device,
        ring,
        rights,
        sector_limit,
    })
}

/// The total order an authority table must be sorted by.
///
/// Device, then ring — deliberately *not* holder-first. A ring is one client's
/// channel to the driver, so `(device, ring)` identifies it globally, and a
/// strictly ascending sequence makes two rows for one ring unrepresentable
/// rather than merely unlikely. Holder-first ordering would have permitted two
/// holders to name one ring, leaving the driver unable to say whose rights a
/// submission carries — the exact defect this table closes.
type OrderKey = (u32, u32);

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
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
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn entry(
        holder: [u8; 32],
        device: u32,
        ring: u32,
        rights: u16,
        sector_limit: u64,
    ) -> [u8; ENTRY_BYTES] {
        let mut value = [0; ENTRY_BYTES];
        value[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].copy_from_slice(&holder);
        value[OFF_ENTRY_DEVICE..OFF_ENTRY_DEVICE_END].copy_from_slice(&device.to_le_bytes());
        value[OFF_ENTRY_RING..OFF_ENTRY_RING_END].copy_from_slice(&ring.to_le_bytes());
        value[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS_END].copy_from_slice(&rights.to_le_bytes());
        value[OFF_ENTRY_SECTOR_LIMIT..OFF_ENTRY_SECTOR_LIMIT_END]
            .copy_from_slice(&sector_limit.to_le_bytes());
        value
    }

    fn table(entries: &[[u8; ENTRY_BYTES]]) -> Vec<u8> {
        let total = HEADER_BYTES + entries.len() * ENTRY_BYTES;
        let mut bytes = std::vec![0; total];
        bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END].copy_from_slice(&MAGIC);
        bytes[OFF_HEADER_FORMAT_VERSION..OFF_HEADER_FORMAT_VERSION_END]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[OFF_HEADER_HEADER_SIZE..OFF_HEADER_HEADER_SIZE_END]
            .copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[OFF_HEADER_RING_COUNT..OFF_HEADER_RING_COUNT_END]
            .copy_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN_END]
            .copy_from_slice(&(total as u32).to_le_bytes());
        for (index, value) in entries.iter().enumerate() {
            let offset = HEADER_BYTES + index * ENTRY_BYTES;
            bytes[offset..offset + ENTRY_BYTES].copy_from_slice(value);
        }
        bytes
    }

    fn holder(name: &str) -> [u8; 32] {
        holder_identity(name)
    }

    #[test]
    fn a_read_only_ring_refuses_a_write() {
        let probe = holder("sel4-recovery-probe");
        let bytes = table(&[entry(probe, 2, 1, RIGHT_READ, 2048)]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        assert!(authority.authorizes(2, 1, Right::Read));
        assert!(!authority.authorizes(2, 1, Right::Write));
    }

    #[test]
    fn a_read_write_ring_carries_both_rights() {
        let probe = holder("sel4-storage-probe");
        let bytes = table(&[entry(probe, 1, 0, RIGHT_READ | RIGHT_WRITE, 2048)]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        assert!(authority.authorizes(1, 0, Right::Read));
        assert!(authority.authorizes(1, 0, Right::Write));
    }

    #[test]
    fn one_holder_may_hold_two_rings_with_different_rights() {
        // The whole reason rights live on the ring: the recovery probe reaches
        // its primary disk read-write and its guard disk read-only, in one
        // boot, through one driver.
        let probe = holder("sel4-recovery-probe");
        let mut entries = [
            entry(probe, 1, 0, RIGHT_READ | RIGHT_WRITE, 2048),
            entry(probe, 2, 1, RIGHT_READ, 2048),
        ];
        entries.sort_by_key(|value| {
            (
                u32::from_le_bytes(
                    value[OFF_ENTRY_DEVICE..OFF_ENTRY_DEVICE_END]
                        .try_into()
                        .unwrap(),
                ),
                u32::from_le_bytes(
                    value[OFF_ENTRY_RING..OFF_ENTRY_RING_END]
                        .try_into()
                        .unwrap(),
                ),
            )
        });
        let bytes = table(&entries);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        assert!(authority.authorizes(1, 0, Right::Write));
        assert!(!authority.authorizes(2, 1, Right::Write));
        assert!(authority.authorizes(2, 1, Right::Read));
    }

    #[test]
    fn an_unnamed_device_or_ring_is_denied() {
        let probe = holder("sel4-storage-probe");
        let bytes = table(&[entry(probe, 1, 0, RIGHT_READ | RIGHT_WRITE, 2048)]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        // The declared ring carries both rights.
        assert!(authority.authorizes(1, 0, Right::Read));
        assert!(authority.authorizes(1, 0, Right::Write));
        // A ring the table does not name has no authority at all, on either
        // axis, and reports no entry rather than a permissive default.
        assert!(!authority.authorizes(2, 0, Right::Read));
        assert!(!authority.authorizes(1, 1, Right::Read));
        assert!(authority.ring_authority(2, 0).is_none());
        assert!(authority.ring_authority(1, 1).is_none());
        assert_eq!(
            authority.ring_authority(1, 0).unwrap().holder_identity,
            probe
        );
    }

    #[test]
    fn an_empty_table_denies_every_ring() {
        let bytes = table(&[]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        assert_eq!(authority.ring_count(), 0);
        assert!(!authority.authorizes(1, 0, Right::Read));
    }

    #[test]
    fn a_range_past_the_declared_limit_is_refused() {
        let probe = holder("sel4-storage-probe");
        let bytes = table(&[entry(probe, 1, 0, RIGHT_READ, 2048)]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        assert!(authority.authorizes_range(1, 0, Right::Read, 2040, 8));
        assert!(!authority.authorizes_range(1, 0, Right::Read, 2041, 8));
        // An overflowing sum must refuse rather than wrap into range.
        assert!(!authority.authorizes_range(1, 0, Right::Read, u64::MAX, 1));
    }

    #[test]
    fn a_duplicate_or_unsorted_triple_is_refused() {
        let probe = holder("sel4-storage-probe");
        let duplicate = table(&[
            entry(probe, 1, 0, RIGHT_READ, 2048),
            entry(probe, 1, 0, RIGHT_READ | RIGHT_WRITE, 2048),
        ]);
        assert_eq!(
            BlockAuthority::decode(&duplicate).err(),
            Some(DecodeError::BadOrder)
        );
        let unsorted = table(&[
            entry(probe, 2, 0, RIGHT_READ, 2048),
            entry(probe, 1, 0, RIGHT_READ, 2048),
        ]);
        assert_eq!(
            BlockAuthority::decode(&unsorted).err(),
            Some(DecodeError::BadOrder)
        );
    }

    #[test]
    fn two_holders_cannot_share_one_ring() {
        // The load-bearing property. If one ring could carry two holders' rows,
        // a driver reading this table could not say whose rights a submission
        // on that ring carries -- exactly the defect the table closes. Made
        // unrepresentable by ordering on `(device, ring)` alone.
        let probe = holder("sel4-storage-probe");
        let other = holder("sel4-store-probe");
        let bytes = table(&[
            entry(probe, 1, 0, RIGHT_READ, 2048),
            entry(other, 1, 0, RIGHT_READ | RIGHT_WRITE, 2048),
        ]);
        assert_eq!(
            BlockAuthority::decode(&bytes).err(),
            Some(DecodeError::BadOrder)
        );
    }

    #[test]
    fn a_zero_holder_identity_is_refused() {
        let bytes = table(&[entry([0; 32], 1, 0, RIGHT_READ, 2048)]);
        assert_eq!(
            BlockAuthority::decode(&bytes).err(),
            Some(DecodeError::BadOrder)
        );
    }

    #[test]
    fn empty_unknown_rights_or_zero_limit_are_refused() {
        let probe = holder("sel4-storage-probe");
        for (rights, limit) in [(0, 2048), (KNOWN_RIGHTS | 4, 2048), (RIGHT_READ, 0)] {
            let bytes = table(&[entry(probe, 1, 0, rights, limit)]);
            assert_eq!(
                BlockAuthority::decode(&bytes).err(),
                Some(DecodeError::InvalidEntry)
            );
        }
    }

    #[test]
    fn nonzero_reserved_bytes_are_refused() {
        let probe = holder("sel4-storage-probe");
        let mut value = entry(probe, 1, 0, RIGHT_READ, 2048);
        value[OFF_ENTRY_RESERVED_END - 1] = 1;
        let bytes = table(&[value]);
        assert_eq!(
            BlockAuthority::decode(&bytes).err(),
            Some(DecodeError::InvalidEntry)
        );
    }

    #[test]
    fn a_bad_magic_version_or_length_is_refused() {
        let probe = holder("sel4-storage-probe");
        let good = table(&[entry(probe, 1, 0, RIGHT_READ, 2048)]);

        let mut bad_magic = good.clone();
        bad_magic[OFF_HEADER_MAGIC] = b'X';
        assert_eq!(
            BlockAuthority::decode(&bad_magic).err(),
            Some(DecodeError::BadMagic)
        );

        let mut bad_version = good.clone();
        bad_version[OFF_HEADER_FORMAT_VERSION] = FORMAT_VERSION as u8 + 1;
        assert_eq!(
            BlockAuthority::decode(&bad_version).err(),
            Some(DecodeError::UnsupportedVersion)
        );

        let mut bad_flags = good.clone();
        bad_flags[OFF_HEADER_REQUIRED_FLAGS] = 1;
        assert_eq!(
            BlockAuthority::decode(&bad_flags).err(),
            Some(DecodeError::UnknownRequiredFlags)
        );

        let mut bad_count = good.clone();
        bad_count[OFF_HEADER_RING_COUNT..OFF_HEADER_RING_COUNT_END]
            .copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            BlockAuthority::decode(&bad_count).err(),
            Some(DecodeError::BadBounds)
        );

        assert_eq!(
            BlockAuthority::decode(&good[..HEADER_BYTES - 1]).err(),
            Some(DecodeError::Truncated)
        );
    }

    #[test]
    fn more_rings_than_the_per_holder_ceiling_are_refused() {
        let probe = holder("sel4-storage-probe");
        let entries: Vec<[u8; ENTRY_BYTES]> = (0..=MAX_RINGS_PER_HOLDER as u32)
            .map(|ring| entry(probe, 1, ring, RIGHT_READ, 2048))
            .collect();
        let bytes = table(&entries);
        assert_eq!(
            BlockAuthority::decode(&bytes).err(),
            Some(DecodeError::Impossible)
        );
    }

    #[test]
    fn entry_bytes_round_trip_through_the_paged_read() {
        // The root pages canonical entry bytes; a table rebuilt from them must
        // decode to the same authority, or the paged path would be a second
        // encoder that can disagree with this one.
        let probe = holder("sel4-storage-probe");
        let bytes = table(&[entry(probe, 1, 0, RIGHT_READ, 2048)]);
        let authority = BlockAuthority::decode(&bytes).unwrap();
        let row = authority.entry_bytes(0).unwrap();
        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(&bytes[..HEADER_BYTES]);
        rebuilt.extend_from_slice(row);
        let again = BlockAuthority::decode(&rebuilt).unwrap();
        assert_eq!(again.ring(0), authority.ring(0));
        assert!(authority.entry_bytes(1).is_none());
    }

    #[test]
    fn holder_identity_is_domain_separated() {
        // Two resources hashing one instance name to one identity would make a
        // grant in either forgeable from the other's table.
        assert_ne!(
            holder_identity("sel4-storage-probe"),
            crate::network_destination::holder_identity("sel4-storage-probe")
        );
        assert_ne!(holder("a"), holder("b"));
    }
}
