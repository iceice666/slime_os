//! On-disk object-store layout (M5.4), and the superblock validator that
//! decides whether a sector read off a real disk may be believed.
//!
//! The layout is pinned by `contracts/store/disk/v1/schema.zt`; every constant
//! below is generated from it. The append/commit machinery — which sector to
//! write next, which slot to overwrite, how to recover the previously
//! committed root — is a *device* concern and stays with whichever kernel owns
//! the block device.
//!
//! Validation is not a device concern. A superblock is a fixed 64-byte header
//! in a 512-byte sector, and whether it is well formed is a question about
//! bytes, answerable with no disk, no allocator, and no architecture. The
//! retired kernel answered it in `kernel/src/storage/object_store.rs`, so the
//! rules were reachable only from a `no_std` x86 test binary that no named
//! gate ran (P5.4.1 recorded `object_store.rs` as thirty-two ungated
//! assertions). Here the same rules are host-testable and Miri-checkable, and
//! any root on any architecture can refuse a bad superblock before trusting a
//! single offset inside it.

include!("generated/store_disk.rs");

use crate::crc32::crc32;

/// Why a superblock sector was refused.
///
/// Ordered as the checks run, which is also strongest-to-weakest: a bad magic
/// says this is not a superblock at all, while bad bounds say it is one whose
/// fields disagree with the partition it was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperblockError {
    /// Not a Slime superblock.
    BadMagic,
    /// A Slime superblock from a format this reader does not implement.
    UnsupportedVersion,
    /// The header declares a size other than the one this format fixes.
    BadHeaderSize,
    /// The recorded CRC-32 does not cover the bytes present.
    BadCrc,
    /// Internally consistent, but the fields contradict the partition
    /// geometry or this format's ceilings.
    BadBounds,
}

/// The mutable state one committed superblock records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    /// Commit counter. The higher of the two slots is the live one.
    pub sequence: u64,
    /// First free LBA in the record area.
    pub append_lba: u64,
    /// Committed objects in the index.
    pub object_count: u32,
}

fn u32_field(sector: &[u8; SECTOR_BYTES], offset: usize) -> u32 {
    u32::from_le_bytes([
        sector[offset],
        sector[offset + 1],
        sector[offset + 2],
        sector[offset + 3],
    ])
}

fn u64_field(sector: &[u8; SECTOR_BYTES], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&sector[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

/// Render a superblock into the sector that records it.
///
/// Present so the validator has an exact inverse to be tested against: a
/// corpus that hand-assembles sectors would be testing the corpus.
pub fn encode_superblock(superblock: &Superblock, partition_sectors: u64) -> [u8; SECTOR_BYTES] {
    let mut sector = [0u8; SECTOR_BYTES];
    sector[..8].copy_from_slice(&SUPERBLOCK_MAGIC);
    sector[SUPERBLOCK_FORMAT_VERSION_OFFSET..SUPERBLOCK_HEADER_SIZE_OFFSET]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    sector[SUPERBLOCK_HEADER_SIZE_OFFSET..SUPERBLOCK_SEQUENCE_OFFSET]
        .copy_from_slice(&(SUPERBLOCK_HEADER as u32).to_le_bytes());
    sector[SUPERBLOCK_SEQUENCE_OFFSET..SUPERBLOCK_APPEND_LBA_OFFSET]
        .copy_from_slice(&superblock.sequence.to_le_bytes());
    sector[SUPERBLOCK_APPEND_LBA_OFFSET..SUPERBLOCK_OBJECT_COUNT_OFFSET]
        .copy_from_slice(&superblock.append_lba.to_le_bytes());
    sector[SUPERBLOCK_OBJECT_COUNT_OFFSET..SUPERBLOCK_FLAGS_OFFSET]
        .copy_from_slice(&superblock.object_count.to_le_bytes());
    sector[SUPERBLOCK_FLAGS_OFFSET..SUPERBLOCK_RECORD_AREA_START_OFFSET]
        .copy_from_slice(&0u32.to_le_bytes());
    sector[SUPERBLOCK_RECORD_AREA_START_OFFSET..SUPERBLOCK_PARTITION_SECTORS_OFFSET]
        .copy_from_slice(&RECORD_AREA_START.to_le_bytes());
    sector[SUPERBLOCK_PARTITION_SECTORS_OFFSET..SUPERBLOCK_RESERVED_OFFSET]
        .copy_from_slice(&partition_sectors.to_le_bytes());
    let crc = crc32(&sector[..SUPERBLOCK_CRC32_OFFSET]);
    sector[SUPERBLOCK_CRC32_OFFSET..SUPERBLOCK_HEADER].copy_from_slice(&crc.to_le_bytes());
    sector
}

/// Decide whether a sector is a superblock this reader may act on.
///
/// `partition_sectors` is the geometry the *caller* established by validating
/// the GPT entry, not something the sector may claim for itself: the recorded
/// value must match it. That is the check that stops a superblock copied from
/// a larger disk from authorising appends past the end of this partition.
pub fn decode_superblock(
    sector: &[u8; SECTOR_BYTES],
    partition_sectors: u64,
) -> Result<Superblock, SuperblockError> {
    if sector[..8] != SUPERBLOCK_MAGIC {
        return Err(SuperblockError::BadMagic);
    }
    if u32_field(sector, SUPERBLOCK_FORMAT_VERSION_OFFSET) != FORMAT_VERSION {
        return Err(SuperblockError::UnsupportedVersion);
    }
    if u32_field(sector, SUPERBLOCK_HEADER_SIZE_OFFSET) != SUPERBLOCK_HEADER as u32 {
        return Err(SuperblockError::BadHeaderSize);
    }
    // Before any field is read as a number: the CRC is what makes the rest of
    // this function's arithmetic meaningful rather than a reading of noise.
    let stored_crc = u32_field(sector, SUPERBLOCK_CRC32_OFFSET);
    if crc32(&sector[..SUPERBLOCK_CRC32_OFFSET]) != stored_crc {
        return Err(SuperblockError::BadCrc);
    }
    let superblock = Superblock {
        sequence: u64_field(sector, SUPERBLOCK_SEQUENCE_OFFSET),
        append_lba: u64_field(sector, SUPERBLOCK_APPEND_LBA_OFFSET),
        object_count: u32_field(sector, SUPERBLOCK_OBJECT_COUNT_OFFSET),
    };
    let record_area_start = u64_field(sector, SUPERBLOCK_RECORD_AREA_START_OFFSET);
    let recorded_partition = u64_field(sector, SUPERBLOCK_PARTITION_SECTORS_OFFSET);
    // `sequence == u64::MAX` is refused rather than saturated: the next commit
    // must produce a strictly higher number to win the slot comparison, and a
    // store that cannot commit again is not a store this reader should open.
    if record_area_start != RECORD_AREA_START
        || recorded_partition != partition_sectors
        || superblock.append_lba < RECORD_AREA_START
        || superblock.append_lba > partition_sectors
        || superblock.object_count as usize > MAX_OBJECTS
        || superblock.sequence == u64::MAX
    {
        return Err(SuperblockError::BadBounds);
    }
    Ok(superblock)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARTITION_SECTORS: u64 = 2048;

    fn valid() -> Superblock {
        Superblock {
            sequence: 7,
            append_lba: 9,
            object_count: 3,
        }
    }

    /// The encoder and validator are exact inverses. Without this the refusal
    /// corpus below could pass on a validator that refuses everything.
    #[test]
    fn a_well_formed_superblock_round_trips() {
        let sector = encode_superblock(&valid(), PARTITION_SECTORS);
        assert_eq!(decode_superblock(&sector, PARTITION_SECTORS), Ok(valid()));
    }

    /// Each refusal is attributed to its own cause, one mutation at a time.
    ///
    /// The oracle's `superblock_rejects_corruption` asserted only `is_err()`
    /// for three mutations, which cannot tell a CRC failure from a bounds
    /// failure — and those are different operational events: one is a damaged
    /// sector, the other a superblock that does not belong to this partition.
    #[test]
    fn every_malformed_class_is_refused_with_its_own_error() {
        let cases: [(&str, fn(&mut [u8; SECTOR_BYTES]), SuperblockError); 4] = [
            ("magic", |s| s[0] ^= 0xFF, SuperblockError::BadMagic),
            (
                "version",
                |s| s[SUPERBLOCK_FORMAT_VERSION_OFFSET..][..4].copy_from_slice(&2u32.to_le_bytes()),
                SuperblockError::UnsupportedVersion,
            ),
            (
                "header size",
                |s| s[SUPERBLOCK_HEADER_SIZE_OFFSET..][..4].copy_from_slice(&8u32.to_le_bytes()),
                SuperblockError::BadHeaderSize,
            ),
            (
                "payload byte covered by the crc",
                |s| s[SUPERBLOCK_SEQUENCE_OFFSET] ^= 0xFF,
                SuperblockError::BadCrc,
            ),
        ];
        for (name, mutate, expected) in cases {
            let mut sector = encode_superblock(&valid(), PARTITION_SECTORS);
            mutate(&mut sector);
            assert_eq!(
                decode_superblock(&sector, PARTITION_SECTORS),
                Err(expected),
                "mutating the {name} must be refused as {expected:?}"
            );
        }
    }

    /// A flipped CRC field is refused, not merely a flipped payload byte.
    ///
    /// Distinct from the case above: this proves the stored CRC is *compared*
    /// rather than recomputed and written back, which a validator that
    /// re-derived it would silently pass.
    #[test]
    fn a_corrupted_checksum_field_is_refused() {
        let mut sector = encode_superblock(&valid(), PARTITION_SECTORS);
        sector[SUPERBLOCK_CRC32_OFFSET] ^= 0xFF;
        assert_eq!(
            decode_superblock(&sector, PARTITION_SECTORS),
            Err(SuperblockError::BadCrc)
        );
    }

    /// Geometry the sector claims must match the geometry the caller
    /// established from the GPT entry.
    ///
    /// This is the check that stops a superblock lifted from a larger disk
    /// from authorising appends past the end of this partition — the CRC is
    /// intact in every case here, so nothing else would catch it.
    #[test]
    fn a_superblock_from_a_different_partition_is_refused() {
        let sector = encode_superblock(&valid(), PARTITION_SECTORS);
        assert_eq!(
            decode_superblock(&sector, PARTITION_SECTORS * 2),
            Err(SuperblockError::BadBounds)
        );
        // …and the same sector is still good against its own geometry, so the
        // refusal is about the mismatch rather than about the sector.
        assert!(decode_superblock(&sector, PARTITION_SECTORS).is_ok());
    }

    /// Every out-of-range field is refused, each isolated from the others.
    ///
    /// Re-encoded per case rather than mutated in place, so the CRC stays
    /// valid and `BadBounds` is reached — a mutated sector would be refused as
    /// `BadCrc` and prove nothing about the bounds.
    #[test]
    fn each_out_of_range_field_is_refused_on_its_own() {
        let cases: [(&str, Superblock); 4] = [
            (
                "append_lba below the record area",
                Superblock {
                    append_lba: RECORD_AREA_START - 1,
                    ..valid()
                },
            ),
            (
                "append_lba past the partition",
                Superblock {
                    append_lba: PARTITION_SECTORS + 1,
                    ..valid()
                },
            ),
            (
                "object_count over the ceiling",
                Superblock {
                    object_count: MAX_OBJECTS as u32 + 1,
                    ..valid()
                },
            ),
            (
                "sequence with no successor",
                Superblock {
                    sequence: u64::MAX,
                    ..valid()
                },
            ),
        ];
        for (name, superblock) in cases {
            let sector = encode_superblock(&superblock, PARTITION_SECTORS);
            assert_eq!(
                decode_superblock(&sector, PARTITION_SECTORS),
                Err(SuperblockError::BadBounds),
                "{name} must be refused"
            );
        }
    }

    /// The boundary values on either side of the bounds are *accepted*.
    ///
    /// Without this the bounds test above would pass on a validator that
    /// refused every append_lba, and an off-by-one that rejected a legal
    /// full-partition store would look like correct strictness.
    #[test]
    fn the_extreme_legal_values_are_admitted() {
        for (name, superblock) in [
            (
                "first record LBA",
                Superblock {
                    append_lba: RECORD_AREA_START,
                    ..valid()
                },
            ),
            (
                "last LBA in the partition",
                Superblock {
                    append_lba: PARTITION_SECTORS,
                    ..valid()
                },
            ),
            (
                "a full index",
                Superblock {
                    object_count: MAX_OBJECTS as u32,
                    ..valid()
                },
            ),
            (
                "the highest committable sequence",
                Superblock {
                    sequence: u64::MAX - 1,
                    ..valid()
                },
            ),
        ] {
            let sector = encode_superblock(&superblock, PARTITION_SECTORS);
            assert_eq!(
                decode_superblock(&sector, PARTITION_SECTORS),
                Ok(superblock),
                "{name} is legal and must be admitted"
            );
        }
    }

    /// An all-zero sector — an erased or never-written slot — is refused as
    /// "not a superblock" rather than decoded as a zeroed one.
    ///
    /// The oracle's `no_valid_superblock_rejected` covers this through the
    /// store's open path; here it is the validator's own answer, which is what
    /// a root without that store machinery would call.
    #[test]
    fn a_blank_sector_is_not_a_superblock() {
        assert_eq!(
            decode_superblock(&[0u8; SECTOR_BYTES], PARTITION_SECTORS),
            Err(SuperblockError::BadMagic)
        );
    }

    /// The header fits inside a sector with the CRC as its last field.
    ///
    /// Generated constants, so this is a check on the *schema*: a layout
    /// change that pushed the CRC past the header, or the header past the
    /// sector, would make every offset above silently wrong.
    #[test]
    fn the_generated_layout_is_self_consistent() {
        assert_eq!(SUPERBLOCK_CRC32_OFFSET + 4, SUPERBLOCK_HEADER);
        assert!(SUPERBLOCK_HEADER <= SECTOR_BYTES);
        assert!(RECORD_HEADER <= SECTOR_BYTES);
        assert_eq!(MAX_OBJECT_PAYLOAD % SECTOR_BYTES, 0);
        assert!(SLOT_A_LBA < RECORD_AREA_START && SLOT_B_LBA < RECORD_AREA_START);
        assert_ne!(SLOT_A_LBA, SLOT_B_LBA);
    }
}
