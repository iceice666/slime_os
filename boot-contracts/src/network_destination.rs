//! Exact network-destination authority resource (IO4).
//!
//! Authority is an exact tuple: holder, transport, address or DNS name, port,
//! and one independently-declared right. The format has no wildcard field.

use crate::sha256::Sha256;
include!("generated/network_destination.rs");

pub const MAGIC: [u8; 8] = *b"SLIMEND\0";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_DESTINATIONS * ENTRY_BYTES;

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
pub enum Transport {
    Tcp,
    Udp,
}
impl Transport {
    const fn wire(self) -> u8 {
        match self {
            Self::Tcp => TRANSPORT_TCP,
            Self::Udp => TRANSPORT_UDP,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address<'a> {
    Ipv4([u8; 4]),
    Ipv6([u8; 16]),
    Dns(&'a [u8]),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Right {
    Connect,
    Send,
    Recv,
    Listen,
}
impl Right {
    const fn bit(self) -> u16 {
        match self {
            Self::Connect => RIGHT_CONNECT,
            Self::Send => RIGHT_SEND,
            Self::Recv => RIGHT_RECV,
            Self::Listen => RIGHT_LISTEN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination<'a> {
    pub holder_identity: [u8; 32],
    pub transport: Transport,
    pub address: Address<'a>,
    pub port: u16,
    pub rights: u16,
    pub queue_depth: u32,
    pub byte_budget: u32,
    pub timer_budget: u32,
    pub retry_limit: u32,
    pub reconnect_limit: u32,
    pub socket_limit: u32,
    pub listener_limit: u32,
    pub dns_record_limit: u32,
}
#[derive(Debug, Clone, Copy)]
pub struct NetworkDestinations<'a> {
    bytes: &'a [u8],
    destination_count: usize,
}

impl<'a> NetworkDestinations<'a> {
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
        let count = u32_at(bytes, OFF_HEADER_DESTINATION_COUNT)? as usize;
        let total = u32_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize;
        if count > MAX_DESTINATIONS
            || total != HEADER_BYTES + count * ENTRY_BYTES
            || total != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let mut previous: Option<OrderKey> = None;
        let mut previous_holder = [0; 32];
        let mut per_holder = 0;
        for index in 0..count {
            let destination = decode_entry(bytes, index)?;
            let (kind, key_address) = address_key(destination.address);
            let key = (
                destination.holder_identity,
                destination.transport.wire(),
                kind,
                key_address,
                destination.port,
            );
            if destination.holder_identity == [0; 32] || previous.is_some_and(|value| key <= value)
            {
                return Err(DecodeError::BadOrder);
            }
            if destination.holder_identity == previous_holder {
                per_holder += 1;
            } else {
                previous_holder = destination.holder_identity;
                per_holder = 1;
            }
            if per_holder > MAX_DESTINATIONS_PER_HOLDER {
                return Err(DecodeError::Impossible);
            }
            previous = Some(key);
        }
        Ok(Self {
            bytes,
            destination_count: count,
        })
    }
    pub const fn destination_count(&self) -> usize {
        self.destination_count
    }
    pub fn destination(&self, index: usize) -> Option<Destination<'a>> {
        (index < self.destination_count)
            .then(|| decode_entry(self.bytes, index).expect("validated network destination"))
    }
    /// Canonical bytes for one authenticated entry, used by the root's paged
    /// read without introducing a second encoder for this layout.
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.destination_count {
            return None;
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(offset..offset + ENTRY_BYTES)
    }
    pub fn authorizes(
        &self,
        holder: &[u8; 32],
        transport: Transport,
        address: Address<'_>,
        port: u16,
        right: Right,
    ) -> bool {
        (0..self.destination_count).any(|index| {
            let entry = decode_entry(self.bytes, index).expect("validated network destination");
            entry.holder_identity == *holder
                && entry.transport == transport
                && entry.address == address
                && entry.port == port
                && entry.rights & right.bit() != 0
        })
    }
    pub fn authorizes_resolve(&self, holder: &[u8; 32], name: &[u8]) -> bool {
        (0..self.destination_count).any(|index| {
            let entry = decode_entry(self.bytes, index).expect("validated network destination");
            entry.holder_identity == *holder
                && matches!(entry.address, Address::Dns(value) if value == name)
        })
    }
}

pub fn holder_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-network-destination-holder-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<Destination<'_>, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    let transport = match entry[OFF_ENTRY_TRANSPORT] {
        TRANSPORT_TCP => Transport::Tcp,
        TRANSPORT_UDP => Transport::Udp,
        _ => return Err(DecodeError::InvalidEntry),
    };
    let rights = u16_at(entry, OFF_ENTRY_RIGHTS)?;
    let port = u16_at(entry, OFF_ENTRY_PORT)?;
    let name_len = u16_at(entry, OFF_ENTRY_NAME_LEN)? as usize;
    let raw_address: [u8; 16] = entry[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS_END]
        .try_into()
        .expect("generated network-destination layout");
    let raw_name = &entry[OFF_ENTRY_NAME..OFF_ENTRY_NAME_END];
    if rights == 0
        || rights & !KNOWN_RIGHTS != 0
        || port == 0
        || name_len > MAX_NAME_BYTES
        || (rights & RIGHT_LISTEN != 0 && transport != Transport::Tcp)
    {
        return Err(DecodeError::InvalidEntry);
    }
    let address = match entry[OFF_ENTRY_ADDRESS_KIND] {
        ADDRESS_IPV4
            if name_len == 0
                && raw_address[IPV4_BYTES..].iter().all(|byte| *byte == 0)
                && raw_name.iter().all(|byte| *byte == 0) =>
        {
            Address::Ipv4(
                raw_address[..IPV4_BYTES]
                    .try_into()
                    .expect("generated network-destination layout"),
            )
        }
        ADDRESS_IPV6 if name_len == 0 && raw_name.iter().all(|byte| *byte == 0) => {
            Address::Ipv6(raw_address)
        }
        ADDRESS_DNS
            if name_len > 0
                && raw_address == [0; 16]
                && raw_name[name_len..].iter().all(|byte| *byte == 0)
                && valid_dns_name(&raw_name[..name_len]) =>
        {
            Address::Dns(&raw_name[..name_len])
        }
        _ => return Err(DecodeError::InvalidEntry),
    };
    let queue_depth = u32_at(entry, OFF_ENTRY_QUEUE_DEPTH)?;
    let byte_budget = u32_at(entry, OFF_ENTRY_BYTE_BUDGET)?;
    let timer_budget = u32_at(entry, OFF_ENTRY_TIMER_BUDGET)?;
    let retry_limit = u32_at(entry, OFF_ENTRY_RETRY_LIMIT)?;
    let reconnect_limit = u32_at(entry, OFF_ENTRY_RECONNECT_LIMIT)?;
    let socket_limit = u32_at(entry, OFF_ENTRY_SOCKET_LIMIT)?;
    let listener_limit = u32_at(entry, OFF_ENTRY_LISTENER_LIMIT)?;
    let dns_record_limit = u32_at(entry, OFF_ENTRY_DNS_RECORD_LIMIT)?;
    if queue_depth == 0
        || queue_depth > MAX_QUEUE_DEPTH
        || byte_budget == 0
        || byte_budget > MAX_BYTE_BUDGET
        || timer_budget > MAX_TIMER_BUDGET
        || retry_limit > MAX_RETRY_LIMIT
        || reconnect_limit > MAX_RECONNECT_LIMIT
        || socket_limit == 0
        || socket_limit > MAX_SOCKETS
        || listener_limit > MAX_LISTENERS
        || dns_record_limit > MAX_DNS_RECORDS
        || listener_limit > socket_limit
    {
        return Err(DecodeError::Impossible);
    }
    Ok(Destination {
        holder_identity: entry[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
            .try_into()
            .expect("generated network-destination layout"),
        transport,
        address,
        port,
        rights,
        queue_depth,
        byte_budget,
        timer_budget,
        retry_limit,
        reconnect_limit,
        socket_limit,
        listener_limit,
        dns_record_limit,
    })
}
fn valid_dns_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name[0] != b'.'
        && name[name.len() - 1] != b'.'
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'-'))
        && !name.windows(2).any(|pair| pair == b"..")
}
/// The total order a destination table must be sorted by.
///
/// Holder first, then transport, then address kind, then the padded address
/// bytes, then port. Strictly ascending, which is what makes a duplicate
/// destination unrepresentable rather than merely unlikely: two equal keys
/// cannot both appear in an ascending sequence, so `decode` refuses the table
/// instead of a later lookup silently preferring whichever row it reached first.
///
/// Named rather than written inline because the tuple's field order *is* the
/// sort contract, and an inline type invites reordering it by accident.
type OrderKey = ([u8; 32], u8, u8, [u8; 64], u16);

fn address_key(address: Address<'_>) -> (u8, [u8; 64]) {
    let mut key = [0; 64];
    match address {
        Address::Ipv4(value) => {
            key[..4].copy_from_slice(&value);
            (ADDRESS_IPV4, key)
        }
        Address::Ipv6(value) => {
            key[..16].copy_from_slice(&value);
            (ADDRESS_IPV6, key)
        }
        Address::Dns(value) => {
            key[..value.len()].copy_from_slice(value);
            (ADDRESS_DNS, key)
        }
    }
}
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
        transport: u8,
        kind: u8,
        address: &[u8],
        port: u16,
        rights: u16,
    ) -> [u8; ENTRY_BYTES] {
        let mut value = [0; ENTRY_BYTES];
        value[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].copy_from_slice(&holder);
        value[OFF_ENTRY_TRANSPORT] = transport;
        value[OFF_ENTRY_ADDRESS_KIND] = kind;
        value[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS_END].copy_from_slice(&rights.to_le_bytes());
        value[OFF_ENTRY_PORT..OFF_ENTRY_PORT_END].copy_from_slice(&port.to_le_bytes());
        if kind == ADDRESS_DNS {
            value[OFF_ENTRY_NAME_LEN..OFF_ENTRY_NAME_LEN_END]
                .copy_from_slice(&(address.len() as u16).to_le_bytes());
            value[OFF_ENTRY_NAME..OFF_ENTRY_NAME + address.len()].copy_from_slice(address);
        } else {
            value[OFF_ENTRY_ADDRESS..OFF_ENTRY_ADDRESS + address.len()].copy_from_slice(address);
        }
        // Budget fields, addressed by their generated offsets rather than by a
        // second copy of the layout: this encoder is the decoder's only
        // adversary, so a literal here could drift with the schema and still
        // agree with itself.
        for (offset, number) in [
            (OFF_ENTRY_QUEUE_DEPTH, 8u32),
            (OFF_ENTRY_BYTE_BUDGET, 4096),
            (OFF_ENTRY_TIMER_BUDGET, 2),
            (OFF_ENTRY_RETRY_LIMIT, 2),
            (OFF_ENTRY_RECONNECT_LIMIT, 2),
            (OFF_ENTRY_SOCKET_LIMIT, 4),
            (OFF_ENTRY_LISTENER_LIMIT, 1),
            (OFF_ENTRY_DNS_RECORD_LIMIT, 4),
        ] {
            value[offset..offset + 4].copy_from_slice(&number.to_le_bytes());
        }
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
    #[test]
    fn exact_match_and_each_alternate_fail_closed() {
        let holder = holder_identity("client");
        let bytes = object(&[entry(
            holder,
            TRANSPORT_TCP,
            ADDRESS_DNS,
            b"api.example",
            443,
            RIGHT_CONNECT | RIGHT_SEND,
        )]);
        let d = NetworkDestinations::decode(&bytes).unwrap();
        assert!(d.authorizes(
            &holder,
            Transport::Tcp,
            Address::Dns(b"api.example"),
            443,
            Right::Connect
        ));
        assert!(!d.authorizes(
            &holder,
            Transport::Tcp,
            Address::Dns(b"other.example"),
            443,
            Right::Connect
        ));
        assert!(!d.authorizes(
            &holder,
            Transport::Tcp,
            Address::Dns(b"api.example"),
            80,
            Right::Connect
        ));
        assert!(!d.authorizes(
            &holder,
            Transport::Udp,
            Address::Dns(b"api.example"),
            443,
            Right::Connect
        ));
        assert!(!d.authorizes(
            &holder,
            Transport::Tcp,
            Address::Ipv4([127, 0, 0, 1]),
            443,
            Right::Connect
        ));
        assert!(!d.authorizes(
            &holder,
            Transport::Tcp,
            Address::Dns(b"api.example"),
            443,
            Right::Recv
        ));
    }
    #[test]
    fn every_missing_right_is_denied() {
        let holder = holder_identity("rights");
        for (present, missing) in [
            (RIGHT_CONNECT, Right::Send),
            (RIGHT_SEND, Right::Recv),
            (RIGHT_RECV, Right::Connect),
            (RIGHT_LISTEN, Right::Send),
        ] {
            let bytes = object(&[entry(
                holder,
                TRANSPORT_TCP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                8080,
                present,
            )]);
            let d = NetworkDestinations::decode(&bytes).unwrap();
            assert!(!d.authorizes(
                &holder,
                Transport::Tcp,
                Address::Ipv4([10, 0, 0, 1]),
                8080,
                missing
            ));
        }
    }
    #[test]
    fn resolving_one_name_grants_no_other_lookup() {
        let holder = holder_identity("resolver");
        let bytes = object(&[entry(
            holder,
            TRANSPORT_TCP,
            ADDRESS_DNS,
            b"one.example",
            443,
            RIGHT_CONNECT,
        )]);
        let d = NetworkDestinations::decode(&bytes).unwrap();
        assert!(d.authorizes_resolve(&holder, b"one.example"));
        assert!(!d.authorizes_resolve(&holder, b"two.example"));
        assert!(!d.authorizes_resolve(&holder_identity("other"), b"one.example"));
    }
    /// Every header field the decoder refuses on, one mutation each. The
    /// decoder had three positive-path tests and no negative ones, so each
    /// refusal below was unreachable code as far as any check could tell.
    #[test]
    fn every_malformed_header_field_is_refused_with_its_own_error() {
        let valid = object(&[entry(
            holder_identity("client"),
            TRANSPORT_TCP,
            ADDRESS_IPV4,
            &[10, 0, 0, 1],
            443,
            RIGHT_CONNECT,
        )]);
        assert!(NetworkDestinations::decode(&valid).is_ok());

        let truncate = |n: usize| valid[..n].to_vec();
        assert_eq!(
            NetworkDestinations::decode(&truncate(HEADER_BYTES - 1)).err(),
            Some(DecodeError::Truncated)
        );
        // A whole header but a short entry: bounds, not truncation, because the
        // declared `total_len` no longer matches the byte count.
        assert_eq!(
            NetworkDestinations::decode(&truncate(HEADER_BYTES)).err(),
            Some(DecodeError::BadBounds)
        );

        let mutate = |offset: usize, bytes: &[u8]| {
            let mut value = valid.clone();
            value[offset..offset + bytes.len()].copy_from_slice(bytes);
            value
        };
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_MAGIC, b"SLIMEXX\0")).err(),
            Some(DecodeError::BadMagic)
        );
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_FORMAT_VERSION, &2u32.to_le_bytes()))
                .err(),
            Some(DecodeError::UnsupportedVersion)
        );
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_HEADER_SIZE, &64u32.to_le_bytes()))
                .err(),
            Some(DecodeError::UnsupportedVersion)
        );
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_REQUIRED_FLAGS, &1u64.to_le_bytes()))
                .err(),
            Some(DecodeError::UnknownRequiredFlags)
        );
        // A count the byte length cannot support, and a count past the ceiling.
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_DESTINATION_COUNT, &2u32.to_le_bytes()))
                .err(),
            Some(DecodeError::BadBounds)
        );
        assert_eq!(
            NetworkDestinations::decode(&mutate(
                OFF_HEADER_DESTINATION_COUNT,
                &(MAX_DESTINATIONS as u32 + 1).to_le_bytes()
            ))
            .err(),
            Some(DecodeError::BadBounds)
        );
        assert_eq!(
            NetworkDestinations::decode(&mutate(OFF_HEADER_TOTAL_LEN, &0u32.to_le_bytes())).err(),
            Some(DecodeError::BadBounds)
        );
    }

    /// The address-kind arms are mutually exclusive and each demands that every
    /// byte outside its own field be zero. A non-canonical encoding of an
    /// otherwise-valid destination is refused, which is what keeps one
    /// authority from having two representations.
    #[test]
    fn a_non_canonical_address_encoding_is_refused() {
        let holder = holder_identity("canon");
        let with = |offset: usize, bytes: &[u8]| {
            let mut value = entry(
                holder,
                TRANSPORT_TCP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                443,
                RIGHT_CONNECT,
            );
            value[offset..offset + bytes.len()].copy_from_slice(bytes);
            object(&[value])
        };
        // IPv4 with a dirty tail past its four bytes.
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_ADDRESS + IPV4_BYTES, &[1])).err(),
            Some(DecodeError::InvalidEntry)
        );
        // IPv4 carrying name bytes, or a nonzero name length.
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_NAME, b"x")).err(),
            Some(DecodeError::InvalidEntry)
        );
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_NAME_LEN, &1u16.to_le_bytes())).err(),
            Some(DecodeError::InvalidEntry)
        );
        // An undefined address kind and an undefined transport.
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_ADDRESS_KIND, &[9])).err(),
            Some(DecodeError::InvalidEntry)
        );
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_TRANSPORT, &[9])).err(),
            Some(DecodeError::InvalidEntry)
        );
        // Zero port, zero rights, and an undefined rights bit.
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_PORT, &0u16.to_le_bytes())).err(),
            Some(DecodeError::InvalidEntry)
        );
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_RIGHTS, &0u16.to_le_bytes())).err(),
            Some(DecodeError::InvalidEntry)
        );
        assert_eq!(
            NetworkDestinations::decode(&with(OFF_ENTRY_RIGHTS, &(KNOWN_RIGHTS + 1).to_le_bytes()))
                .err(),
            Some(DecodeError::InvalidEntry)
        );
    }

    /// A DNS name is bounded, canonically padded, and syntactically checked.
    /// `name_len` is the field the decoder slices by, so its boundary is the
    /// one that decides whether a later `raw_name[..name_len]` is in range.
    #[test]
    fn dns_name_bounds_and_syntax_are_enforced_at_the_boundary() {
        let holder = holder_identity("dns");
        let named = |name: &[u8]| {
            object(&[entry(
                holder,
                TRANSPORT_TCP,
                ADDRESS_DNS,
                name,
                443,
                RIGHT_CONNECT,
            )])
        };
        // The longest admissible name is accepted, so the bound is not off by one.
        let longest = [b'a'; MAX_NAME_BYTES];
        assert!(NetworkDestinations::decode(&named(&longest)).is_ok());

        // A `name_len` past the field is refused rather than slicing out of range.
        let mut over = entry(
            holder,
            TRANSPORT_TCP,
            ADDRESS_DNS,
            b"api.example",
            443,
            RIGHT_CONNECT,
        );
        over[OFF_ENTRY_NAME_LEN..OFF_ENTRY_NAME_LEN_END]
            .copy_from_slice(&(MAX_NAME_BYTES as u16 + 1).to_le_bytes());
        assert_eq!(
            NetworkDestinations::decode(&object(&[over])).err(),
            Some(DecodeError::InvalidEntry)
        );

        for malformed in [
            &b".leading"[..],
            &b"trailing."[..],
            &b"double..dot"[..],
            &b"under_score"[..],
            &b"sp ace"[..],
        ] {
            assert_eq!(
                NetworkDestinations::decode(&named(malformed)).err(),
                Some(DecodeError::InvalidEntry),
                "accepted a malformed DNS name"
            );
        }

        // Declared length shorter than the bytes present: the tail past
        // `name_len` must be zero, so a hidden suffix cannot ride along.
        let mut dirty = entry(
            holder,
            TRANSPORT_TCP,
            ADDRESS_DNS,
            b"api.example",
            443,
            RIGHT_CONNECT,
        );
        dirty[OFF_ENTRY_NAME_LEN..OFF_ENTRY_NAME_LEN_END].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            NetworkDestinations::decode(&object(&[dirty])).err(),
            Some(DecodeError::InvalidEntry)
        );
    }

    /// Entries must be strictly ascending, which is what makes a duplicate
    /// destination unrepresentable rather than resolved by whichever row a
    /// lookup reaches first.
    #[test]
    fn duplicate_and_descending_entries_are_refused() {
        let holder = holder_identity("order");
        let row = |port: u16| {
            entry(
                holder,
                TRANSPORT_TCP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                port,
                RIGHT_CONNECT,
            )
        };
        assert!(NetworkDestinations::decode(&object(&[row(80), row(443)])).is_ok());
        assert_eq!(
            NetworkDestinations::decode(&object(&[row(443), row(80)])).err(),
            Some(DecodeError::BadOrder)
        );
        assert_eq!(
            NetworkDestinations::decode(&object(&[row(443), row(443)])).err(),
            Some(DecodeError::BadOrder)
        );
        // A zero holder identity names no holder and is refused outright.
        assert_eq!(
            NetworkDestinations::decode(&object(&[entry(
                [0; 32],
                TRANSPORT_TCP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                443,
                RIGHT_CONNECT
            )]))
            .err(),
            Some(DecodeError::BadOrder)
        );
    }

    /// `listen` is TCP-only, and a listener budget cannot exceed the socket
    /// budget that must hold its accepted connections.
    #[test]
    fn impossible_authority_and_budget_combinations_are_refused() {
        let holder = holder_identity("budget");
        assert_eq!(
            NetworkDestinations::decode(&object(&[entry(
                holder,
                TRANSPORT_UDP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                443,
                RIGHT_LISTEN
            )]))
            .err(),
            Some(DecodeError::InvalidEntry)
        );

        let over_budget = |offset: usize, value: u32| {
            let mut row = entry(
                holder,
                TRANSPORT_TCP,
                ADDRESS_IPV4,
                &[10, 0, 0, 1],
                443,
                RIGHT_CONNECT,
            );
            row[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            object(&[row])
        };
        for (offset, value) in [
            (OFF_ENTRY_QUEUE_DEPTH, 0),
            (OFF_ENTRY_QUEUE_DEPTH, MAX_QUEUE_DEPTH + 1),
            (OFF_ENTRY_BYTE_BUDGET, 0),
            (OFF_ENTRY_BYTE_BUDGET, MAX_BYTE_BUDGET + 1),
            (OFF_ENTRY_TIMER_BUDGET, MAX_TIMER_BUDGET + 1),
            (OFF_ENTRY_RETRY_LIMIT, MAX_RETRY_LIMIT + 1),
            (OFF_ENTRY_RECONNECT_LIMIT, MAX_RECONNECT_LIMIT + 1),
            (OFF_ENTRY_SOCKET_LIMIT, 0),
            (OFF_ENTRY_SOCKET_LIMIT, MAX_SOCKETS + 1),
            (OFF_ENTRY_LISTENER_LIMIT, MAX_LISTENERS + 1),
            (OFF_ENTRY_DNS_RECORD_LIMIT, MAX_DNS_RECORDS + 1),
        ] {
            assert_eq!(
                NetworkDestinations::decode(&over_budget(offset, value)).err(),
                Some(DecodeError::Impossible),
                "admitted an out-of-range budget at offset {offset}"
            );
        }
        // Listeners cannot outnumber the sockets that hold their connections.
        let mut row = entry(
            holder,
            TRANSPORT_TCP,
            ADDRESS_IPV4,
            &[10, 0, 0, 1],
            443,
            RIGHT_CONNECT,
        );
        row[OFF_ENTRY_SOCKET_LIMIT..OFF_ENTRY_SOCKET_LIMIT + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        row[OFF_ENTRY_LISTENER_LIMIT..OFF_ENTRY_LISTENER_LIMIT + 4]
            .copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            NetworkDestinations::decode(&object(&[row])).err(),
            Some(DecodeError::Impossible)
        );
    }

    /// The per-holder ceiling binds independently of the table ceiling.
    #[test]
    fn the_per_holder_ceiling_binds_before_the_table_ceiling() {
        let holder = holder_identity("many");
        let rows: Vec<[u8; ENTRY_BYTES]> = (0..=MAX_DESTINATIONS_PER_HOLDER)
            .map(|index| {
                entry(
                    holder,
                    TRANSPORT_TCP,
                    ADDRESS_IPV4,
                    &[10, 0, 0, 1],
                    1000 + index as u16,
                    RIGHT_CONNECT,
                )
            })
            .collect();
        assert_eq!(
            NetworkDestinations::decode(&object(&rows[..MAX_DESTINATIONS_PER_HOLDER]))
                .map(|d| d.destination_count()),
            Ok(MAX_DESTINATIONS_PER_HOLDER)
        );
        assert_eq!(
            NetworkDestinations::decode(&object(&rows)).err(),
            Some(DecodeError::Impossible)
        );
    }
}
