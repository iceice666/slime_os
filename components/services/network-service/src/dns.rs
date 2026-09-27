//! Allocation-free RFC 1035 A queries and bounded, fail-closed responses.
//!
//! The caller must match the UDP source to the exact declared resolver address
//! and port on the pending query's socket before parsing. IDs are supplied by
//! the caller's entropy policy; parsing neither authenticates DNS nor grants
//! destination authority. Bind every result to its original holder/name/port,
//! enforce address policy on every result, and refuse use at or after expiry.

pub const MAX_PACKET_BYTES: usize = 512;
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_ADDRESSES: usize = 4;
pub const MAX_RECORDS: usize = 32;
pub const MAX_ALIASES: usize = 4;
const MAX_POINTER_HOPS: usize = 16;
const HEADER_BYTES: usize = 12;
const TYPE_A: u16 = 1;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Bounds,
    Mismatch,
    Truncated,
    NxDomain,
    ResponseCode,
    NoAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub addresses: [[u8; 4]; MAX_ADDRESSES],
    pub count: usize,
    /// Minimum TTL across the complete alias chain and returned A RRset.
    /// Zero is valid DNS, but grants no reusable resolution lifetime.
    pub ttl_seconds: u32,
}

impl Answer {
    /// `now` must be the service's monotonic receive time, not a wall clock.
    /// Overflow returns no expiry rather than extending authority indefinitely.
    pub fn expires_at_millis(&self, now: u64) -> Option<u64> {
        now.checked_add(u64::from(self.ttl_seconds) * 1000)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Name {
    bytes: [u8; MAX_NAME_BYTES],
    len: usize,
}

impl Name {
    const EMPTY: Self = Self {
        bytes: [0; MAX_NAME_BYTES],
        len: 0,
    };

    fn from_host(host: &[u8]) -> Result<Self, Error> {
        if host.is_empty() || host.len() > MAX_NAME_BYTES {
            return Err(Error::Bounds);
        }
        let mut value = Self::EMPTY;
        for label in host.split(|byte| *byte == b'.') {
            value.label(label)?;
        }
        Ok(value)
    }

    fn label(&mut self, label: &[u8]) -> Result<(), Error> {
        if label.is_empty()
            || label.len() > 63
            || !label
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(Error::Malformed);
        }
        let separator = usize::from(self.len != 0);
        if self.len + separator + label.len() > MAX_NAME_BYTES {
            return Err(Error::Bounds);
        }
        if separator != 0 {
            self.bytes[self.len] = b'.';
            self.len += 1;
        }
        for byte in label {
            self.bytes[self.len] = byte.to_ascii_lowercase();
            self.len += 1;
        }
        Ok(())
    }
}

/// Encode one recursive IN/A question. Names have no trailing dot and obey the
/// authority contract's 64-byte bound, plus DNS's 63-byte per-label bound.
pub fn encode_query(name: &[u8], id: u16, output: &mut [u8]) -> Result<usize, Error> {
    let name = Name::from_host(name)?;
    let len = HEADER_BYTES + name.len + 2 + 4;
    let output = output.get_mut(..len).ok_or(Error::Bounds)?;
    output.fill(0);
    output[..2].copy_from_slice(&id.to_be_bytes());
    output[2] = 1; // RD, RFC 1035 section 4.1.1.
    output[5] = 1;
    let mut cursor = HEADER_BYTES;
    for label in name.bytes[..name.len].split(|byte| *byte == b'.') {
        output[cursor] = label.len() as u8;
        cursor += 1;
        output[cursor..cursor + label.len()].copy_from_slice(label);
        cursor += label.len();
    }
    cursor += 1; // The root label was zeroed above.
    output[cursor..cursor + 2].copy_from_slice(&TYPE_A.to_be_bytes());
    output[cursor + 2..cursor + 4].copy_from_slice(&CLASS_IN.to_be_bytes());
    Ok(len)
}

#[derive(Clone, Copy)]
struct Record {
    owner: usize,
    kind: u16,
    data: usize,
    ttl: u32,
}

impl Record {
    const EMPTY: Self = Self {
        owner: 0,
        kind: 0,
        data: 0,
        ttl: 0,
    };
}

/// Only answer-section records reachable from the original question contribute
/// addresses or TTLs. Authority/additional records never extend name authority.
/// A truncated UDP answer fails explicitly; there is no implicit TCP fallback.
pub fn parse_response(packet: &[u8], id: u16, question: &[u8]) -> Result<Answer, Error> {
    let question = Name::from_host(question)?;
    if packet.len() < HEADER_BYTES || packet.len() > MAX_PACKET_BYTES {
        return Err(Error::Bounds);
    }
    if word(packet, 0)? != id {
        return Err(Error::Mismatch);
    }
    let flags = word(packet, 2)?;
    if flags & 0x8000 == 0 || flags & 0x7840 != 0 || word(packet, 4)? != 1 {
        return Err(Error::Malformed);
    }
    let mut cursor = HEADER_BYTES;
    if read_name(packet, &mut cursor)? != question
        || word(packet, cursor)? != TYPE_A
        || word(packet, cursor + 2)? != CLASS_IN
    {
        return Err(Error::Mismatch);
    }
    cursor += 4;
    if flags & 0x0200 != 0 {
        return Err(Error::Truncated);
    }
    match flags & 0xf {
        0 => (),
        3 => return Err(Error::NxDomain),
        _ => return Err(Error::ResponseCode),
    }
    let answers = usize::from(word(packet, 6)?);
    let total = answers + usize::from(word(packet, 8)?) + usize::from(word(packet, 10)?);
    if total > MAX_RECORDS {
        return Err(Error::Bounds);
    }
    let mut records = [Record::EMPTY; MAX_RECORDS];
    for (index, record) in records.iter_mut().enumerate().take(total) {
        let owner = cursor;
        read_name(packet, &mut cursor)?;
        let kind = word(packet, cursor)?;
        let class = word(packet, cursor + 2)?;
        let ttl = long(packet, cursor + 4)?;
        let len = usize::from(word(packet, cursor + 8)?);
        cursor += 10;
        let end = cursor.checked_add(len).ok_or(Error::Bounds)?;
        if end > packet.len() {
            return Err(Error::Malformed);
        }
        if kind == TYPE_A || kind == TYPE_CNAME {
            if class != CLASS_IN {
                return Err(Error::Malformed);
            }
            if kind == TYPE_A && len != 4 {
                return Err(Error::Malformed);
            }
            if kind == TYPE_CNAME {
                let mut name_end = cursor;
                let alias = read_name(packet, &mut name_end)?;
                if alias.len == 0 || name_end != end {
                    return Err(Error::Malformed);
                }
            }
        }
        if index < answers {
            *record = Record {
                owner,
                kind,
                data: cursor,
                // RFC 2181 section 8 treats a received high-bit TTL as zero.
                ttl: if ttl & 0x8000_0000 == 0 { ttl } else { 0 },
            };
        }
        cursor = end;
    }
    if cursor != packet.len() {
        return Err(Error::Malformed);
    }
    resolve(packet, &records[..answers], question)
}

fn resolve(packet: &[u8], records: &[Record], mut name: Name) -> Result<Answer, Error> {
    let mut visited = [Name::EMPTY; MAX_ALIASES + 1];
    let mut answer = Answer {
        addresses: [[0; 4]; MAX_ADDRESSES],
        count: 0,
        ttl_seconds: u32::MAX,
    };
    for depth in 0..=MAX_ALIASES {
        if visited[..depth].contains(&name) {
            return Err(Error::Malformed);
        }
        visited[depth] = name;
        let mut alias = None;
        for record in records {
            let mut owner = record.owner;
            if read_name(packet, &mut owner)? != name {
                continue;
            }
            match record.kind {
                TYPE_A => {
                    let address: [u8; 4] = packet[record.data..record.data + 4]
                        .try_into()
                        .map_err(|_| Error::Malformed)?;
                    if !answer.addresses[..answer.count].contains(&address) {
                        if answer.count == MAX_ADDRESSES {
                            return Err(Error::Bounds);
                        }
                        answer.addresses[answer.count] = address;
                        answer.count += 1;
                    }
                    answer.ttl_seconds = answer.ttl_seconds.min(record.ttl);
                }
                TYPE_CNAME => {
                    let mut data = record.data;
                    let next = read_name(packet, &mut data)?;
                    if alias.is_some_and(|previous| previous != next) {
                        return Err(Error::Malformed);
                    }
                    alias = Some(next);
                    answer.ttl_seconds = answer.ttl_seconds.min(record.ttl);
                }
                _ => (),
            }
        }
        if let Some(next) = alias {
            if answer.count != 0 {
                return Err(Error::Malformed);
            }
            if depth == MAX_ALIASES {
                return Err(Error::Bounds);
            }
            name = next;
        } else if answer.count != 0 {
            return Ok(answer);
        } else {
            return Err(Error::NoAddress);
        }
    }
    Err(Error::Bounds)
}

fn read_name(packet: &[u8], cursor: &mut usize) -> Result<Name, Error> {
    let mut name = Name::EMPTY;
    let mut position = *cursor;
    let mut jumped = false;
    let mut hops = 0;
    loop {
        let tag = *packet.get(position).ok_or(Error::Malformed)?;
        if tag & 0xc0 == 0xc0 {
            let pointer = usize::from(word(packet, position)? & 0x3fff);
            // RFC 1035 pointers refer to prior name occurrences, not headers or
            // forward bytes. The hop cap also bounds cycles involving labels.
            if pointer < HEADER_BYTES || pointer >= position {
                return Err(Error::Malformed);
            }
            hops += 1;
            if hops > MAX_POINTER_HOPS {
                return Err(Error::Bounds);
            }
            if !jumped {
                *cursor = position + 2;
                jumped = true;
            }
            position = pointer;
        } else if tag & 0xc0 != 0 {
            return Err(Error::Malformed);
        } else {
            position += 1;
            if tag == 0 {
                if !jumped {
                    *cursor = position;
                }
                return Ok(name);
            }
            let end = position + usize::from(tag);
            name.label(packet.get(position..end).ok_or(Error::Malformed)?)?;
            position = end;
        }
    }
}

fn word(packet: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_be_bytes(
        packet
            .get(offset..offset + 2)
            .ok_or(Error::Malformed)?
            .try_into()
            .map_err(|_| Error::Malformed)?,
    ))
}

fn long(packet: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        packet
            .get(offset..offset + 4)
            .ok_or(Error::Malformed)?
            .try_into()
            .map_err(|_| Error::Malformed)?,
    ))
}

/// Conservative public IPv4 unicast policy. A controlled test profile needs a
/// separate exact-address declaration; it must not turn this into allow-private.
pub fn is_public_address(address: [u8; 4]) -> bool {
    let [a, b, c, _] = address;
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const ID: u16 = 0x7813;
    const HOST: &[u8] = b"example.com";
    const PUBLIC: [u8; 4] = [93, 184, 215, 14];

    fn name(value: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for label in value.split(|byte| *byte == b'.') {
            encoded.push(label.len() as u8);
            encoded.extend_from_slice(label);
        }
        encoded.push(0);
        encoded
    }

    fn response() -> Vec<u8> {
        let mut buffer = [0; MAX_PACKET_BYTES];
        let len = encode_query(HOST, ID, &mut buffer).unwrap();
        buffer[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        buffer[..len].to_vec()
    }

    fn record(packet: &mut Vec<u8>, owner: &[u8], kind: u16, ttl: u32, data: &[u8]) {
        packet.extend_from_slice(owner);
        packet.extend_from_slice(&kind.to_be_bytes());
        packet.extend_from_slice(&CLASS_IN.to_be_bytes());
        packet.extend_from_slice(&ttl.to_be_bytes());
        packet.extend_from_slice(&(data.len() as u16).to_be_bytes());
        packet.extend_from_slice(data);
    }

    fn answer(packet: &mut Vec<u8>, owner: &[u8], kind: u16, ttl: u32, data: &[u8]) {
        packet[7] += 1;
        record(packet, owner, kind, ttl, data);
    }

    fn direct() -> Vec<u8> {
        let mut packet = response();
        answer(&mut packet, &[0xc0, 12], TYPE_A, 90, &PUBLIC);
        packet
    }

    fn parse(packet: &[u8]) -> Result<Answer, Error> {
        parse_response(packet, ID, HOST)
    }

    #[test]
    fn encodes_recursive_a_query_and_bounds_output() {
        let mut bytes = [0xff; 40];
        let size = encode_query(b"EXAMPLE.com", ID, &mut bytes).unwrap();
        assert_eq!(size, 29);
        assert_eq!(&bytes[..12], &[0x78, 0x13, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[12..size], b"\x07example\x03com\0\0\x01\0\x01");
        assert_eq!(bytes[size], 0xff);
        assert_eq!(encode_query(HOST, ID, &mut bytes[..28]), Err(Error::Bounds));
    }

    #[test]
    fn host_syntax_and_label_bounds() {
        for host in [b"".as_slice(), b".a", b"a.", b"a..b", b"a_b", b"a\0b"] {
            assert!(encode_query(host, ID, &mut [0; 512]).is_err());
        }
        assert!(encode_query(&[b'a'; 63], ID, &mut [0; 512]).is_ok());
        assert!(encode_query(&[b'a'; 64], ID, &mut [0; 512]).is_err());
        let mut host = [b'a'; 64];
        host[1] = b'.';
        assert!(encode_query(&host, ID, &mut [0; 512]).is_ok());
        assert!(encode_query(&[b'a'; 65], ID, &mut [0; 512]).is_err());
    }

    #[test]
    fn direct_a_and_checked_expiry() {
        let result = parse(&direct()).unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.addresses[0], PUBLIC);
        assert_eq!(result.ttl_seconds, 90);
        assert_eq!(result.expires_at_millis(100), Some(90100));
        assert_eq!(result.expires_at_millis(u64::MAX), None);
    }

    #[test]
    fn case_insensitive_question_and_owner() {
        let mut packet = response();
        packet[13..20].copy_from_slice(b"ExAmPlE");
        answer(&mut packet, &name(b"eXamPLE.COM"), TYPE_A, 4, &PUBLIC);
        assert_eq!(
            parse_response(&packet, ID, b"EXAMPLE.COM").unwrap().count,
            1
        );
    }

    #[test]
    fn validates_id_question_type_class_and_header() {
        assert_eq!(
            parse_response(&direct(), ID + 1, HOST),
            Err(Error::Mismatch)
        );
        assert_eq!(
            parse_response(&direct(), ID, b"other.com"),
            Err(Error::Mismatch)
        );
        for (offset, value) in [
            (25, 1),
            (26, 28),
            (28, 3),
            (2, 1),
            (2, 0x89),
            (3, 0xc0),
            (5, 2),
        ] {
            let mut packet = direct();
            packet[offset] = value;
            assert!(parse(&packet).is_err(), "offset {offset}");
        }
    }

    #[test]
    fn explicit_negative_responses() {
        for (flags, expected) in [
            (0x8380u16, Error::Truncated),
            (0x8183, Error::NxDomain),
            (0x8182, Error::ResponseCode),
        ] {
            let mut packet = response();
            packet[2..4].copy_from_slice(&flags.to_be_bytes());
            assert_eq!(parse(&packet), Err(expected));
        }
        assert_eq!(parse(&response()), Err(Error::NoAddress));
    }

    #[test]
    fn every_packet_truncation_fails() {
        let packet = direct();
        for end in 0..packet.len() {
            assert!(parse(&packet[..end]).is_err(), "end {end}");
        }
        let mut padded = packet;
        padded.push(0);
        assert_eq!(parse(&padded), Err(Error::Malformed));
        assert_eq!(parse(&[0; 513]), Err(Error::Bounds));
    }

    #[test]
    fn malformed_a_length_and_class() {
        for data in [vec![], vec![1, 2, 3], vec![1, 2, 3, 4, 5]] {
            let mut packet = response();
            answer(&mut packet, &[0xc0, 12], TYPE_A, 4, &data);
            assert_eq!(parse(&packet), Err(Error::Malformed));
        }
        let mut packet = direct();
        packet[34] = 3;
        assert_eq!(parse(&packet), Err(Error::Malformed));
    }

    #[test]
    fn multiple_addresses_deduplicate_and_minimize_ttl() {
        let mut packet = direct();
        answer(&mut packet, &[0xc0, 12], TYPE_A, 40, &PUBLIC);
        for n in 1..4 {
            answer(&mut packet, &[0xc0, 12], TYPE_A, 80, &[8, 8, 8, n]);
        }
        let parsed = parse(&packet).unwrap();
        assert_eq!(parsed.count, 4);
        assert_eq!(parsed.ttl_seconds, 40);
        answer(&mut packet, &[0xc0, 12], TYPE_A, 80, &[8, 8, 8, 4]);
        assert_eq!(parse(&packet), Err(Error::Bounds));
    }

    #[test]
    fn zero_and_high_bit_ttls_never_extend_authority() {
        for ttl in [0, 0x8000_0001, u32::MAX] {
            let mut packet = response();
            answer(&mut packet, &[0xc0, 12], TYPE_A, ttl, &PUBLIC);
            let result = parse(&packet).unwrap();
            assert_eq!(result.ttl_seconds, 0);
            assert_eq!(result.expires_at_millis(123), Some(123));
        }
    }

    #[test]
    fn compressed_cname_chain_can_be_out_of_order() {
        let mut packet = response();
        answer(&mut packet, &name(b"www.example.com"), TYPE_A, 50, &PUBLIC);
        answer(
            &mut packet,
            &[0xc0, 12],
            TYPE_CNAME,
            20,
            &[3, b'w', b'w', b'w', 0xc0, 12],
        );
        let result = parse(&packet).unwrap();
        assert_eq!(result.addresses[0], PUBLIC);
        assert_eq!(result.ttl_seconds, 20);
    }

    #[test]
    fn ignores_unrelated_answer_authority_and_additional_addresses() {
        let mut packet = response();
        answer(&mut packet, &name(b"unrelated.com"), TYPE_A, 1, &PUBLIC);
        packet[9] = 1;
        record(&mut packet, &[0xc0, 12], TYPE_A, 1, &PUBLIC);
        packet[11] = 1;
        record(&mut packet, &[0xc0, 12], TYPE_A, 1, &PUBLIC);
        assert_eq!(parse(&packet), Err(Error::NoAddress));
    }

    #[test]
    fn additional_cname_cannot_redirect_authority() {
        let mut packet = response();
        answer(&mut packet, &name(b"other.com"), TYPE_A, 3, &PUBLIC);
        packet[11] = 1;
        record(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &name(b"other.com"));
        assert_eq!(parse(&packet), Err(Error::NoAddress));
    }

    #[test]
    fn cname_loops_conflicts_and_a_coexistence_fail() {
        let mut packet = response();
        answer(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &[0xc0, 12]);
        assert_eq!(parse(&packet), Err(Error::Malformed));
        let mut packet = direct();
        answer(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &name(b"other.com"));
        assert_eq!(parse(&packet), Err(Error::Malformed));
        let mut packet = response();
        answer(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &name(b"first.com"));
        answer(
            &mut packet,
            &[0xc0, 12],
            TYPE_CNAME,
            3,
            &name(b"second.com"),
        );
        assert_eq!(parse(&packet), Err(Error::Malformed));
    }

    #[test]
    fn duplicate_same_cname_retains_smallest_ttl() {
        let mut packet = response();
        for ttl in [4, 8] {
            answer(
                &mut packet,
                &[0xc0, 12],
                TYPE_CNAME,
                ttl,
                &name(b"other.com"),
            );
        }
        answer(&mut packet, &name(b"other.com"), TYPE_A, 20, &PUBLIC);
        assert_eq!(parse(&packet).unwrap().ttl_seconds, 4);
    }

    #[test]
    fn cname_requires_terminal_answer_and_exact_rdata() {
        for data in [vec![], vec![0], vec![0xc0, 12, 0], vec![0xc0]] {
            let mut packet = response();
            answer(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &data);
            assert!(parse(&packet).is_err());
        }
        let mut packet = response();
        answer(&mut packet, &[0xc0, 12], TYPE_CNAME, 3, &name(b"other.com"));
        assert_eq!(parse(&packet), Err(Error::NoAddress));
    }

    #[test]
    fn alias_depth_is_bounded() {
        let names: [&[u8]; 6] = [HOST, b"a.com", b"b.com", b"c.com", b"d.com", b"e.com"];
        for count in [MAX_ALIASES, MAX_ALIASES + 1] {
            let mut packet = response();
            for index in 0..count {
                answer(
                    &mut packet,
                    &name(names[index]),
                    TYPE_CNAME,
                    20,
                    &name(names[index + 1]),
                );
            }
            answer(&mut packet, &name(names[count]), TYPE_A, 20, &PUBLIC);
            if count == MAX_ALIASES {
                assert!(parse(&packet).is_ok());
            } else {
                assert_eq!(parse(&packet), Err(Error::Bounds));
            }
        }
    }

    #[test]
    fn bounds_sum_of_all_record_sections() {
        for section in [6, 8, 10] {
            let mut packet = response();
            packet[section..section + 2].copy_from_slice(&33u16.to_be_bytes());
            assert_eq!(parse(&packet), Err(Error::Bounds));
        }
        let mut packet = response();
        packet[7] = 12;
        packet[9] = 12;
        packet[11] = 12;
        assert_eq!(parse(&packet), Err(Error::Bounds));
    }

    #[test]
    fn refuses_pointer_loops_forward_header_and_out_of_range_offsets() {
        for pointer in [0, 29, 30, 500, 0x3fff] {
            let mut packet = response();
            answer(
                &mut packet,
                &(0xc000u16 | pointer).to_be_bytes(),
                TYPE_A,
                3,
                &PUBLIC,
            );
            assert!(parse(&packet).is_err(), "pointer {pointer}");
        }
        let mut packet = vec![0; 12];
        packet.extend_from_slice(&[1, b'a', 0xc0, 12]);
        assert!(read_name(&packet, &mut 12).is_err());
    }

    #[test]
    fn pointer_hops_and_expanded_names_are_bounded() {
        let mut packet = vec![0; 12];
        packet.extend_from_slice(&name(b"a"));
        let mut previous = 12;
        for _ in 0..MAX_POINTER_HOPS {
            let current = packet.len();
            packet.extend_from_slice(&(0xc000u16 | previous as u16).to_be_bytes());
            previous = current;
        }
        let mut cursor = previous;
        assert!(read_name(&packet, &mut cursor).is_ok());
        let mut current = packet.len();
        packet.extend_from_slice(&(0xc000u16 | previous as u16).to_be_bytes());
        assert_eq!(read_name(&packet, &mut current).err(), Some(Error::Bounds));
        let mut oversized = vec![0; 12];
        oversized.push(63);
        oversized.extend_from_slice(&[b'a'; 63]);
        oversized.extend_from_slice(&[1, b'b', 0]);
        assert_eq!(read_name(&oversized, &mut 12).err(), Some(Error::Bounds));
    }

    #[test]
    fn reserved_label_encodings_and_truncated_labels_fail() {
        for label in [vec![0x40], vec![0x80], vec![63, b'a'], vec![1, 0xff, 0]] {
            let mut packet = response();
            answer(&mut packet, &label, TYPE_A, 3, &PUBLIC);
            assert!(parse(&packet).is_err());
        }
    }

    #[test]
    fn public_address_policy_rejects_sensitive_ranges() {
        for address in [
            [0, 1, 2, 3],
            [10, 2, 3, 4],
            [100, 64, 0, 1],
            [100, 127, 255, 254],
            [127, 0, 0, 1],
            [169, 254, 169, 254],
            [172, 16, 0, 1],
            [172, 31, 255, 254],
            [192, 168, 1, 2],
            [192, 0, 0, 9],
            [192, 0, 2, 1],
            [192, 88, 99, 1],
            [198, 18, 0, 1],
            [198, 19, 255, 254],
            [198, 51, 100, 1],
            [203, 0, 113, 1],
            [224, 0, 0, 1],
            [239, 255, 255, 255],
            [240, 0, 0, 1],
            [255, 255, 255, 255],
        ] {
            assert!(!is_public_address(address), "{address:?}");
        }
        for address in [
            PUBLIC,
            [8, 8, 8, 8],
            [1, 1, 1, 1],
            [100, 63, 255, 255],
            [100, 128, 0, 1],
            [172, 15, 255, 255],
            [172, 32, 0, 1],
        ] {
            assert!(is_public_address(address), "{address:?}");
        }
    }

    #[test]
    fn parser_is_total_under_single_byte_mutations() {
        let packet = direct();
        for index in 0..packet.len() {
            for byte in 0..=255 {
                let mut mutation = packet.clone();
                mutation[index] = byte;
                let _ = parse(&mutation);
            }
        }
    }
}
