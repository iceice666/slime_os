//! Network interface declaration resource (IO11).
//!
//! One static IPv4 interface per holder: address, prefix, gateway, and MAC.
//! The format carries no lease, discovery, or negotiation field; what a
//! component's stack answers to is generation data, like the destinations it
//! may reach.

use crate::sha256::Sha256;
include!("generated/network_interface.rs");

pub const MAGIC: [u8; 8] = *b"SLIMENI\0";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_INTERFACES * ENTRY_BYTES;

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
pub struct Interface {
    pub holder_identity: [u8; 32],
    pub address: [u8; IPV4_BYTES],
    pub prefix_len: u8,
    /// `None` when the declaration is all zero: an on-link-only interface.
    pub gateway: Option<[u8; IPV4_BYTES]>,
    pub mac: [u8; MAC_BYTES],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkInterfaces<'a> {
    bytes: &'a [u8],
    interface_count: usize,
}

impl<'a> NetworkInterfaces<'a> {
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
        let count = u32_at(bytes, OFF_HEADER_INTERFACE_COUNT)? as usize;
        let total = u32_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize;
        if count > MAX_INTERFACES
            || total != HEADER_BYTES + count * ENTRY_BYTES
            || total != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        // Strictly ascending holder identities: one interface per holder, and
        // a duplicate is unrepresentable rather than merely unlikely.
        let mut previous: Option<[u8; 32]> = None;
        for index in 0..count {
            let interface = decode_entry(bytes, index)?;
            if interface.holder_identity == [0; 32]
                || previous.is_some_and(|value| interface.holder_identity <= value)
            {
                return Err(DecodeError::BadOrder);
            }
            previous = Some(interface.holder_identity);
        }
        Ok(Self {
            bytes,
            interface_count: count,
        })
    }
    pub const fn interface_count(&self) -> usize {
        self.interface_count
    }
    pub fn interface(&self, index: usize) -> Option<Interface> {
        (index < self.interface_count)
            .then(|| decode_entry(self.bytes, index).expect("validated network interface"))
    }
    /// Canonical bytes for one authenticated entry, used by the root's paged
    /// read without introducing a second encoder for this layout.
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.interface_count {
            return None;
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(offset..offset + ENTRY_BYTES)
    }
    pub fn for_holder(&self, holder: &[u8; 32]) -> Option<Interface> {
        (0..self.interface_count)
            .map(|index| decode_entry(self.bytes, index).expect("validated network interface"))
            .find(|interface| interface.holder_identity == *holder)
    }
}

/// Stable per-holder identity. Its own domain tag: an identity computed for a
/// network destination must not be replayable as an interface identity.
pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-network-interface-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// A unicast host address inside its prefix: not the network address, not
/// the directed broadcast, not multicast, not the unspecified or limited
/// broadcast address. `/31` and `/32` are outside the declared prefix range,
/// so both host-bit patterns are always excluded.
fn valid_host(address: [u8; IPV4_BYTES], prefix_len: u8) -> bool {
    let value = u32::from_be_bytes(address);
    let host_bits = 32 - u32::from(prefix_len);
    let host_mask = (1u32 << host_bits) - 1;
    let host = value & host_mask;
    address[0] < 224 && address[0] != 0 && host != 0 && host != host_mask
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<Interface, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    let prefix_len = entry[OFF_ENTRY_PREFIX_LEN];
    let address: [u8; IPV4_BYTES] = entry[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS_END]
        .try_into()
        .expect("generated network-interface layout");
    let raw_gateway: [u8; IPV4_BYTES] = entry[OFF_ENTRY_GATEWAY..OFF_ENTRY_GATEWAY_END]
        .try_into()
        .expect("generated network-interface layout");
    let mac: [u8; MAC_BYTES] = entry[OFF_ENTRY_MAC..OFF_ENTRY_MAC_END]
        .try_into()
        .expect("generated network-interface layout");
    if !(MIN_PREFIX_LEN..=MAX_PREFIX_LEN).contains(&prefix_len)
        || entry[OFF_ENTRY_RESERVED] != 0
        || !valid_host(address, prefix_len)
        || mac == [0; MAC_BYTES]
        || mac[0] & 1 != 0
    {
        return Err(DecodeError::InvalidEntry);
    }
    let gateway = if raw_gateway == [0; IPV4_BYTES] {
        None
    } else {
        let network_mask = u32::MAX << (32 - u32::from(prefix_len));
        if raw_gateway == address
            || !valid_host(raw_gateway, prefix_len)
            || u32::from_be_bytes(raw_gateway) & network_mask
                != u32::from_be_bytes(address) & network_mask
        {
            return Err(DecodeError::InvalidEntry);
        }
        Some(raw_gateway)
    };
    Ok(Interface {
        holder_identity: entry[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
            .try_into()
            .expect("generated network-interface layout"),
        address,
        prefix_len,
        gateway,
        mac,
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

    const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x53, 0x4c, 0x01];

    fn entry(
        holder: [u8; 32],
        address: [u8; 4],
        prefix_len: u8,
        gateway: [u8; 4],
    ) -> [u8; ENTRY_BYTES] {
        let mut value = [0; ENTRY_BYTES];
        value[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].copy_from_slice(&holder);
        value[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS_END].copy_from_slice(&address);
        value[OFF_ENTRY_GATEWAY..OFF_ENTRY_GATEWAY_END].copy_from_slice(&gateway);
        value[OFF_ENTRY_MAC..OFF_ENTRY_MAC_END].copy_from_slice(&MAC);
        value[OFF_ENTRY_PREFIX_LEN] = prefix_len;
        value
    }
    fn object(entries: &[[u8; ENTRY_BYTES]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes.extend_from_slice(
            &((HEADER_BYTES + entries.len() * ENTRY_BYTES) as u32).to_le_bytes(),
        );
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }
    fn holder(byte: u8) -> [u8; 32] {
        let mut value = [0; 32];
        value[0] = byte;
        value
    }

    #[test]
    fn decodes_one_interface_per_holder() {
        let service = holder_identity("network-service");
        let bytes = object(&[entry(service, [10, 0, 0, 1], 24, [0; 4])]);
        let table = NetworkInterfaces::decode(&bytes).unwrap();
        assert_eq!(table.interface_count(), 1);
        let interface = table.for_holder(&service).unwrap();
        assert_eq!(interface.address, [10, 0, 0, 1]);
        assert_eq!(interface.prefix_len, 24);
        assert_eq!(interface.gateway, None);
        assert_eq!(interface.mac, MAC);
        assert_eq!(table.entry_bytes(0), Some(&bytes[HEADER_BYTES..]));
        assert_eq!(table.for_holder(&holder_identity("io-tcp-probe")), None);
        assert_ne!(
            holder_identity("network-service"),
            crate::network_destination::holder_identity("network-service")
        );
    }

    #[test]
    fn gateway_must_be_a_distinct_host_on_the_prefix() {
        let table = object(&[entry(holder(1), [10, 0, 0, 1], 24, [10, 0, 0, 254])]);
        assert_eq!(
            NetworkInterfaces::decode(&table)
                .unwrap()
                .interface(0)
                .unwrap()
                .gateway,
            Some([10, 0, 0, 254])
        );
        for gateway in [[10, 0, 0, 1], [10, 0, 1, 1], [10, 0, 0, 255], [10, 0, 0, 0]] {
            assert_eq!(
                NetworkInterfaces::decode(&object(&[entry(holder(1), [10, 0, 0, 1], 24, gateway)])),
                Err(DecodeError::InvalidEntry),
                "gateway {gateway:?}"
            );
        }
    }

    #[test]
    fn each_entry_arm_fails_closed() {
        let good = entry(holder(1), [10, 0, 0, 1], 24, [0; 4]);
        assert!(NetworkInterfaces::decode(&object(&[good])).is_ok());
        let mut prefix_short = good;
        prefix_short[OFF_ENTRY_PREFIX_LEN] = 7;
        let mut prefix_long = good;
        prefix_long[OFF_ENTRY_PREFIX_LEN] = 31;
        let mut reserved = good;
        reserved[OFF_ENTRY_RESERVED] = 1;
        let mut multicast_mac = good;
        multicast_mac[OFF_ENTRY_MAC] |= 1;
        let mut zero_mac = good;
        zero_mac[OFF_ENTRY_MAC..OFF_ENTRY_MAC_END].fill(0);
        for (label, bad) in [
            ("prefix below 8", prefix_short),
            ("prefix above 30", prefix_long),
            ("reserved byte", reserved),
            ("multicast mac", multicast_mac),
            ("zero mac", zero_mac),
            (
                "network address",
                entry(holder(1), [10, 0, 0, 0], 24, [0; 4]),
            ),
            (
                "broadcast address",
                entry(holder(1), [10, 0, 0, 255], 24, [0; 4]),
            ),
            (
                "multicast address",
                entry(holder(1), [224, 0, 0, 1], 24, [0; 4]),
            ),
            (
                "zero-net address",
                entry(holder(1), [0, 0, 0, 1], 24, [0; 4]),
            ),
        ] {
            assert_eq!(
                NetworkInterfaces::decode(&object(&[bad])),
                Err(DecodeError::InvalidEntry),
                "{label}"
            );
        }
    }

    #[test]
    fn holders_are_unique_and_ordered() {
        let first = entry(holder(1), [10, 0, 0, 1], 24, [0; 4]);
        let second = entry(holder(2), [10, 0, 1, 1], 24, [0; 4]);
        assert!(NetworkInterfaces::decode(&object(&[first, second])).is_ok());
        assert_eq!(
            NetworkInterfaces::decode(&object(&[second, first])),
            Err(DecodeError::BadOrder)
        );
        assert_eq!(
            NetworkInterfaces::decode(&object(&[first, first])),
            Err(DecodeError::BadOrder)
        );
        assert_eq!(
            NetworkInterfaces::decode(&object(&[entry([0; 32], [10, 0, 0, 1], 24, [0; 4])])),
            Err(DecodeError::BadOrder)
        );
    }

    #[test]
    fn header_arms_fail_closed() {
        let good = object(&[entry(holder(1), [10, 0, 0, 1], 24, [0; 4])]);
        let mut magic = good.clone();
        magic[0] ^= 1;
        assert_eq!(
            NetworkInterfaces::decode(&magic),
            Err(DecodeError::BadMagic)
        );
        let mut version = good.clone();
        version[OFF_HEADER_FORMAT_VERSION] = 2;
        assert_eq!(
            NetworkInterfaces::decode(&version),
            Err(DecodeError::UnsupportedVersion)
        );
        let mut flags = good.clone();
        flags[OFF_HEADER_REQUIRED_FLAGS] = 1;
        assert_eq!(
            NetworkInterfaces::decode(&flags),
            Err(DecodeError::UnknownRequiredFlags)
        );
        let mut count = good.clone();
        count[OFF_HEADER_INTERFACE_COUNT] = 2;
        assert_eq!(
            NetworkInterfaces::decode(&count),
            Err(DecodeError::BadBounds)
        );
        assert_eq!(
            NetworkInterfaces::decode(&good[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        );
        let empty = object(&[]);
        assert_eq!(
            NetworkInterfaces::decode(&empty).unwrap().interface_count(),
            0
        );
    }
}
