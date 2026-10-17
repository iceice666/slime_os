//! Entropy authority resource (entropy-authority/v1).
//!
//! One entry per holder that may draw from the generation's entropy service:
//! its name, whether its stream is instantiated from the hardware source or
//! from a declared seed, and the total bytes it may draw. A hardware row's
//! seed is all zero, so a hardware holder's stream can never be replayed from
//! generation data.

use crate::sha256::Sha256;
include!("generated/entropy_authority.rs");

pub const MAGIC: [u8; 8] = *b"SLIMEEA\0";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_HOLDERS * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    InvalidEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Hardware,
    Seeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holder {
    pub holder_identity: [u8; 32],
    name: [u8; HOLDER_NAME_BYTES],
    name_len: u8,
    pub source: Source,
    pub byte_budget: u32,
    /// All zero for a hardware holder.
    pub seed: [u8; SEED_BYTES],
}

impl Holder {
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len)]
    }
    pub fn name(&self) -> &str {
        core::str::from_utf8(self.name_bytes()).expect("validated ASCII holder name")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntropyAuthority<'a> {
    bytes: &'a [u8],
    holder_count: usize,
}

impl<'a> EntropyAuthority<'a> {
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
        let count = u32_at(bytes, OFF_HEADER_HOLDER_COUNT)? as usize;
        let total = u32_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize;
        if count > MAX_HOLDERS
            || total != HEADER_BYTES + count * ENTRY_BYTES
            || total != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        // Strictly ascending identities make a second row for one holder
        // unrepresentable rather than a choice the service has to make.
        let mut previous: Option<[u8; 32]> = None;
        for index in 0..count {
            let holder = decode_entry(bytes, index)?;
            if previous.is_some_and(|value| holder.holder_identity <= value) {
                return Err(DecodeError::BadOrder);
            }
            previous = Some(holder.holder_identity);
        }
        Ok(Self {
            bytes,
            holder_count: count,
        })
    }
    pub const fn holder_count(&self) -> usize {
        self.holder_count
    }
    pub fn holder(&self, index: usize) -> Option<Holder> {
        (index < self.holder_count)
            .then(|| decode_entry(self.bytes, index).expect("validated entropy authority"))
    }
    /// Canonical bytes for one authenticated entry, used by the root's paged
    /// read without introducing a second encoder for this layout.
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.holder_count {
            return None;
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(offset..offset + ENTRY_BYTES)
    }
    pub fn for_holder(&self, name: &str) -> Option<Holder> {
        let identity = holder_identity(name);
        (0..self.holder_count)
            .filter_map(|index| self.holder(index))
            .find(|holder| holder.holder_identity == identity)
    }
}

/// The header a reader places in front of `count` entries it received one
/// page at a time, so the assembled object decodes under the same rules as
/// the authenticated one.
pub fn header(count: usize) -> Option<[u8; HEADER_BYTES]> {
    if count > MAX_HOLDERS {
        return None;
    }
    let mut value = [0u8; HEADER_BYTES];
    value[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END].copy_from_slice(&MAGIC);
    value[OFF_HEADER_FORMAT_VERSION..OFF_HEADER_FORMAT_VERSION_END]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    value[OFF_HEADER_HEADER_SIZE..OFF_HEADER_HEADER_SIZE_END]
        .copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    value[OFF_HEADER_HOLDER_COUNT..OFF_HEADER_HOLDER_COUNT_END]
        .copy_from_slice(&(count as u32).to_le_bytes());
    value[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN_END]
        .copy_from_slice(&((HEADER_BYTES + count * ENTRY_BYTES) as u32).to_le_bytes());
    Some(value)
}

/// Stable per-holder identity. Its own domain tag: an identity computed for
/// another holder table must not be replayable as an entropy holder.
pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-entropy-authority-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<Holder, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    let name: [u8; HOLDER_NAME_BYTES] = entry[OFF_ENTRY_HOLDER_NAME..OFF_ENTRY_HOLDER_NAME_END]
        .try_into()
        .expect("generated entropy-authority layout");
    let name_len = entry[OFF_ENTRY_HOLDER_NAME_LEN];
    let used = usize::from(name_len);
    if used == 0
        || used > HOLDER_NAME_BYTES
        || !name[..used].iter().all(u8::is_ascii_graphic)
        || name[used..].iter().any(|byte| *byte != 0)
    {
        return Err(DecodeError::InvalidEntry);
    }
    let holder_identity: [u8; 32] = entry[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
        .try_into()
        .expect("generated entropy-authority layout");
    let text = core::str::from_utf8(&name[..used]).map_err(|_| DecodeError::InvalidEntry)?;
    if holder_identity != self::holder_identity(text) {
        return Err(DecodeError::InvalidEntry);
    }
    let seed: [u8; SEED_BYTES] = entry[OFF_ENTRY_SEED..OFF_ENTRY_SEED_END]
        .try_into()
        .expect("generated entropy-authority layout");
    let source = match entry[OFF_ENTRY_SOURCE] {
        SOURCE_HARDWARE if seed == [0; SEED_BYTES] => Source::Hardware,
        SOURCE_SEEDED => Source::Seeded,
        _ => return Err(DecodeError::InvalidEntry),
    };
    let byte_budget = u32::from_le_bytes(
        entry[OFF_ENTRY_BYTE_BUDGET..OFF_ENTRY_BYTE_BUDGET_END]
            .try_into()
            .expect("generated entropy-authority layout"),
    );
    if entry[OFF_ENTRY_RESERVED..OFF_ENTRY_RESERVED_END] != [0, 0]
        || !(1..=MAX_BYTE_BUDGET).contains(&byte_budget)
    {
        return Err(DecodeError::InvalidEntry);
    }
    Ok(Holder {
        holder_identity,
        name,
        name_len,
        source,
        byte_budget,
        seed,
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
    extern crate std;
    use super::*;
    use std::vec::Vec;

    const SEED: [u8; SEED_BYTES] = [0x5a; SEED_BYTES];

    fn entry(name: &str, source: u8, budget: u32, seed: [u8; SEED_BYTES]) -> [u8; ENTRY_BYTES] {
        let mut value = [0; ENTRY_BYTES];
        value[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
            .copy_from_slice(&holder_identity(name));
        value[OFF_ENTRY_HOLDER_NAME..OFF_ENTRY_HOLDER_NAME + name.len()]
            .copy_from_slice(name.as_bytes());
        value[OFF_ENTRY_HOLDER_NAME_LEN] = name.len() as u8;
        value[OFF_ENTRY_SOURCE] = source;
        value[OFF_ENTRY_BYTE_BUDGET..OFF_ENTRY_BYTE_BUDGET_END]
            .copy_from_slice(&budget.to_le_bytes());
        value[OFF_ENTRY_SEED..OFF_ENTRY_SEED_END].copy_from_slice(&seed);
        value
    }
    fn hardware(name: &str) -> [u8; ENTRY_BYTES] {
        entry(name, SOURCE_HARDWARE, 96, [0; SEED_BYTES])
    }
    fn seeded(name: &str) -> [u8; ENTRY_BYTES] {
        entry(name, SOURCE_SEEDED, 96, SEED)
    }
    fn sorted(mut entries: Vec<[u8; ENTRY_BYTES]>) -> Vec<[u8; ENTRY_BYTES]> {
        entries.sort_by(|left, right| {
            left[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
                .cmp(&right[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END])
        });
        entries
    }
    fn object(entries: &[[u8; ENTRY_BYTES]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&header(entries.len()).unwrap());
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    #[test]
    fn decodes_hardware_and_seeded_holders() {
        let entries = sorted(std::vec![
            hardware("entropy-hw-a"),
            hardware("entropy-hw-b"),
            seeded("entropy-seeded"),
        ]);
        let bytes = object(&entries);
        let table = EntropyAuthority::decode(&bytes).unwrap();
        assert_eq!(table.holder_count(), 3);
        let a = table.for_holder("entropy-hw-a").unwrap();
        assert_eq!(a.name(), "entropy-hw-a");
        assert_eq!(a.source, Source::Hardware);
        assert_eq!(a.byte_budget, 96);
        assert_eq!(a.seed, [0; SEED_BYTES]);
        let seeded = table.for_holder("entropy-seeded").unwrap();
        assert_eq!(seeded.source, Source::Seeded);
        assert_eq!(seeded.seed, SEED);
        assert_eq!(seeded.name_bytes(), b"entropy-seeded");
        assert_eq!(table.for_holder("entropy-intruder"), None);
        assert_eq!(
            table.entry_bytes(0),
            Some(&bytes[HEADER_BYTES..HEADER_BYTES + ENTRY_BYTES])
        );
        assert_eq!(table.entry_bytes(3), None);
        assert_eq!(table.holder(3), None);
        let names: Vec<_> = (0..table.holder_count())
            .map(|index| table.holder(index).unwrap().holder_identity)
            .collect();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert_ne!(
            holder_identity("network-service"),
            crate::network_interface::holder_identity("network-service")
        );
    }

    #[test]
    fn budget_bounds_are_inclusive() {
        for budget in [1, MAX_BYTE_BUDGET] {
            let bytes = object(&[entry("h", SOURCE_HARDWARE, budget, [0; SEED_BYTES])]);
            assert!(EntropyAuthority::decode(&bytes).is_ok(), "budget {budget}");
        }
        for budget in [0, MAX_BYTE_BUDGET + 1, u32::MAX] {
            assert_eq!(
                EntropyAuthority::decode(&object(&[entry("h", SOURCE_SEEDED, budget, SEED)])),
                Err(DecodeError::InvalidEntry),
                "budget {budget}"
            );
        }
    }

    #[test]
    fn each_entry_arm_fails_closed() {
        let good = hardware("entropy-hw-a");
        assert!(EntropyAuthority::decode(&object(&[good])).is_ok());
        let longest = "h".repeat(HOLDER_NAME_BYTES);
        assert!(EntropyAuthority::decode(&object(&[hardware(&longest)])).is_ok());
        assert!(
            EntropyAuthority::decode(&object(&[entry("s", SOURCE_SEEDED, 1, [0; 32])])).is_ok()
        );

        let mut empty_name = good;
        empty_name[OFF_ENTRY_HOLDER_NAME..OFF_ENTRY_HOLDER_NAME_END].fill(0);
        empty_name[OFF_ENTRY_HOLDER_NAME_LEN] = 0;
        let mut long_name = good;
        long_name[OFF_ENTRY_HOLDER_NAME_LEN] = HOLDER_NAME_BYTES as u8 + 1;
        let mut short_len = good;
        short_len[OFF_ENTRY_HOLDER_NAME_LEN] -= 1;
        let mut padding = good;
        padding[OFF_ENTRY_HOLDER_NAME_END - 1] = b'x';
        let non_ascii = hardware("entropy-\u{e9}");
        let mut embedded_nul = hardware("ab");
        embedded_nul[OFF_ENTRY_HOLDER_NAME] = 0;
        let space = hardware("a b");
        let mut identity = good;
        identity[OFF_ENTRY_HOLDER_IDENTITY] ^= 1;
        let mut renamed = good;
        renamed[OFF_ENTRY_HOLDER_NAME] = b'f';
        let mut reserved_low = good;
        reserved_low[OFF_ENTRY_RESERVED] = 1;
        let mut reserved_high = good;
        reserved_high[OFF_ENTRY_RESERVED + 1] = 1;
        let mut hardware_seed = good;
        hardware_seed[OFF_ENTRY_SEED_END - 1] = 1;
        for (label, bad) in [
            ("empty name", empty_name),
            ("name length above field", long_name),
            ("name length short of name", short_len),
            ("nonzero padding", padding),
            ("non-ASCII name", non_ascii),
            ("embedded NUL", embedded_nul),
            ("space in name", space),
            ("identity mismatch", identity),
            ("name mismatch", renamed),
            ("reserved low", reserved_low),
            ("reserved high", reserved_high),
            ("hardware seed", hardware_seed),
            ("source zero", entry("h", 0, 96, [0; SEED_BYTES])),
            ("source unknown", entry("h", 3, 96, [0; SEED_BYTES])),
            ("source unknown seeded", entry("h", 0xff, 96, SEED)),
        ] {
            assert_eq!(
                EntropyAuthority::decode(&object(&[bad])),
                Err(DecodeError::InvalidEntry),
                "{label}"
            );
        }
    }

    #[test]
    fn holders_are_unique_and_ordered() {
        let entries = sorted(std::vec![
            hardware("entropy-hw-a"),
            seeded("entropy-seeded")
        ]);
        assert!(EntropyAuthority::decode(&object(&entries)).is_ok());
        assert_eq!(
            EntropyAuthority::decode(&object(&[entries[1], entries[0]])),
            Err(DecodeError::BadOrder)
        );
        assert_eq!(
            EntropyAuthority::decode(&object(&[entries[0], entries[0]])),
            Err(DecodeError::BadOrder)
        );
        let mut twin = hardware("entropy-hw-a");
        twin[OFF_ENTRY_SOURCE] = SOURCE_SEEDED;
        let pair = sorted(std::vec![hardware("entropy-hw-a"), twin]);
        assert_eq!(
            EntropyAuthority::decode(&object(&pair)),
            Err(DecodeError::BadOrder)
        );
    }

    #[test]
    fn header_arms_fail_closed() {
        let good = object(&[hardware("entropy-hw-a")]);
        assert!(EntropyAuthority::decode(&good).is_ok());
        let mut magic = good.clone();
        magic[OFF_HEADER_MAGIC_END - 1] = b'X';
        assert_eq!(EntropyAuthority::decode(&magic), Err(DecodeError::BadMagic));
        let mut version = good.clone();
        version[OFF_HEADER_FORMAT_VERSION] = 2;
        assert_eq!(
            EntropyAuthority::decode(&version),
            Err(DecodeError::UnsupportedVersion)
        );
        let mut header_size = good.clone();
        header_size[OFF_HEADER_HEADER_SIZE] = 40;
        assert_eq!(
            EntropyAuthority::decode(&header_size),
            Err(DecodeError::UnsupportedVersion)
        );
        for byte in OFF_HEADER_REQUIRED_FLAGS..OFF_HEADER_REQUIRED_FLAGS_END {
            let mut flags = good.clone();
            flags[byte] = 0x80;
            assert_eq!(
                EntropyAuthority::decode(&flags),
                Err(DecodeError::UnknownRequiredFlags)
            );
        }
        let mut count_high = good.clone();
        count_high[OFF_HEADER_HOLDER_COUNT] = 2;
        assert_eq!(
            EntropyAuthority::decode(&count_high),
            Err(DecodeError::BadBounds)
        );
        let mut count_low = good.clone();
        count_low[OFF_HEADER_HOLDER_COUNT] = 0;
        assert_eq!(
            EntropyAuthority::decode(&count_low),
            Err(DecodeError::BadBounds)
        );
        let mut total = good.clone();
        total[OFF_HEADER_TOTAL_LEN] += 1;
        assert_eq!(
            EntropyAuthority::decode(&total),
            Err(DecodeError::BadBounds)
        );
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(
            EntropyAuthority::decode(&trailing),
            Err(DecodeError::BadBounds)
        );
        assert_eq!(
            EntropyAuthority::decode(&good[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(
            EntropyAuthority::decode(&good[..good.len() - 1]),
            Err(DecodeError::BadBounds)
        );
        let empty = object(&[]);
        assert_eq!(EntropyAuthority::decode(&empty).unwrap().holder_count(), 0);
    }

    #[test]
    fn holder_count_is_bounded() {
        let names: Vec<std::string::String> = (0..=MAX_HOLDERS)
            .map(|index| std::format!("holder-{index}"))
            .collect();
        let full = sorted(
            names[..MAX_HOLDERS]
                .iter()
                .map(|name| hardware(name))
                .collect(),
        );
        assert_eq!(
            EntropyAuthority::decode(&object(&full))
                .unwrap()
                .holder_count(),
            MAX_HOLDERS
        );
        assert_eq!(header(MAX_HOLDERS + 1), None);
        let over = sorted(names.iter().map(|name| hardware(name)).collect());
        let mut bytes = object(&full);
        bytes[OFF_HEADER_HOLDER_COUNT..OFF_HEADER_HOLDER_COUNT_END]
            .copy_from_slice(&((MAX_HOLDERS + 1) as u32).to_le_bytes());
        bytes[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN_END].copy_from_slice(
            &((HEADER_BYTES + (MAX_HOLDERS + 1) * ENTRY_BYTES) as u32).to_le_bytes(),
        );
        bytes.truncate(HEADER_BYTES);
        for entry in &over {
            bytes.extend_from_slice(entry);
        }
        assert_eq!(
            EntropyAuthority::decode(&bytes),
            Err(DecodeError::Truncated)
        );
    }
}
