//! Exact application bindings and bounded loopback-listener authority.

pub use crate::network_destination::holder_identity;
include!("generated/network_application.rs");

pub const MAX_BYTES: usize = HEADER_BYTES + MAX_APPLICATIONS * ENTRY_BYTES;

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
    InvalidPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Listener,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    External,
    Loopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Application<'a> {
    pub holder_identity: [u8; 32],
    pub control_binding: &'a [u8],
    pub provision_binding: &'a [u8],
    pub supervision_binding: &'a [u8],
    pub request_notification: &'a [u8],
    pub completion_notification: &'a [u8],
    pub allowed_peer_identity: [u8; 32],
    pub local_ipv4: [u8; 4],
    pub local_port: u16,
    pub rights: u16,
    pub role: Role,
    pub backend: Backend,
    pub backlog: u32,
    pub accepted_socket_limit: u32,
    pub byte_budget: u32,
    pub timer_budget: u32,
    pub queue_depth: u32,
    pub retry_limit: u32,
    pub reconnect_limit: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct NetworkApplications<'a> {
    bytes: &'a [u8],
    application_count: usize,
}

impl<'a> NetworkApplications<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if u32_at(bytes, OFF_HEADER_FORMAT_VERSION) != FORMAT_VERSION
            || u32_at(bytes, OFF_HEADER_HEADER_SIZE) as usize != HEADER_BYTES
        {
            return Err(DecodeError::UnsupportedVersion);
        }
        if bytes[OFF_HEADER_REQUIRED_FLAGS..OFF_HEADER_REQUIRED_FLAGS_END]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let count = u32_at(bytes, OFF_HEADER_APPLICATION_COUNT) as usize;
        if count > MAX_APPLICATIONS
            || bytes.len() != HEADER_BYTES + count * ENTRY_BYTES
            || u32_at(bytes, OFF_HEADER_TOTAL_LEN) as usize != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let table = Self {
            bytes,
            application_count: count,
        };
        let mut previous = None;
        for index in 0..count {
            let entry = decode_entry(table.entry_bytes(index).ok_or(DecodeError::Truncated)?)?;
            if previous.is_some_and(|holder| entry.holder_identity <= holder) {
                return Err(DecodeError::BadOrder);
            }
            previous = Some(entry.holder_identity);
        }
        for index in 0..count {
            let entry = table.application(index).ok_or(DecodeError::Truncated)?;
            if entry.role == Role::Listener {
                let peer = table
                    .by_holder(&entry.allowed_peer_identity)
                    .ok_or(DecodeError::InvalidPeer)?;
                if peer.role != Role::Client || peer.backend != Backend::Loopback {
                    return Err(DecodeError::InvalidPeer);
                }
                if (0..index)
                    .filter_map(|prior| table.application(prior))
                    .any(|prior| {
                        prior.role == Role::Listener
                            && prior.local_ipv4 == entry.local_ipv4
                            && prior.local_port == entry.local_port
                    })
                {
                    return Err(DecodeError::InvalidEntry);
                }
            }
        }
        Ok(table)
    }

    pub const fn application_count(&self) -> usize {
        self.application_count
    }
    pub fn application(&self, index: usize) -> Option<Application<'a>> {
        self.entry_bytes(index)
            .map(|bytes| decode_entry(bytes).expect("validated network application"))
    }
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.application_count {
            return None;
        }
        let offset = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(offset..offset + ENTRY_BYTES)
    }
    pub fn by_holder(&self, holder: &[u8; 32]) -> Option<Application<'a>> {
        (0..self.application_count)
            .filter_map(|index| self.application(index))
            .find(|entry| entry.holder_identity == *holder)
    }
}

fn decode_entry(bytes: &[u8]) -> Result<Application<'_>, DecodeError> {
    let role = match bytes[OFF_ENTRY_ROLE] {
        ROLE_CLIENT => Role::Client,
        ROLE_LISTENER => Role::Listener,
        _ => return Err(DecodeError::InvalidEntry),
    };
    let backend = match bytes[OFF_ENTRY_BACKEND] {
        BACKEND_EXTERNAL => Backend::External,
        BACKEND_LOOPBACK => Backend::Loopback,
        _ => return Err(DecodeError::InvalidEntry),
    };
    if bytes[OFF_ENTRY_RESERVED0..OFF_ENTRY_RESERVED0_END]
        .iter()
        .any(|byte| *byte != 0)
        || bytes[OFF_ENTRY_RESERVED..OFF_ENTRY_RESERVED_END]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(DecodeError::InvalidEntry);
    }
    let entry = Application {
        holder_identity: bytes[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END]
            .try_into()
            .unwrap(),
        control_binding: binding(
            &bytes[OFF_ENTRY_CONTROL_BINDING..OFF_ENTRY_CONTROL_BINDING_END],
            true,
        )?,
        provision_binding: binding(
            &bytes[OFF_ENTRY_PROVISION_BINDING..OFF_ENTRY_PROVISION_BINDING_END],
            true,
        )?,
        supervision_binding: binding(
            &bytes[OFF_ENTRY_SUPERVISION_BINDING..OFF_ENTRY_SUPERVISION_BINDING_END],
            false,
        )?,
        request_notification: binding(
            &bytes[OFF_ENTRY_REQUEST_NOTIFICATION..OFF_ENTRY_REQUEST_NOTIFICATION_END],
            false,
        )?,
        completion_notification: binding(
            &bytes[OFF_ENTRY_COMPLETION_NOTIFICATION..OFF_ENTRY_COMPLETION_NOTIFICATION_END],
            false,
        )?,
        allowed_peer_identity: bytes
            [OFF_ENTRY_ALLOWED_PEER_IDENTITY..OFF_ENTRY_ALLOWED_PEER_IDENTITY_END]
            .try_into()
            .unwrap(),
        local_ipv4: bytes[OFF_ENTRY_LOCAL_IPV4..OFF_ENTRY_LOCAL_IPV4_END]
            .try_into()
            .unwrap(),
        local_port: u16_at(bytes, OFF_ENTRY_LOCAL_PORT),
        rights: u16_at(bytes, OFF_ENTRY_RIGHTS),
        role,
        backend,
        backlog: u32_at(bytes, OFF_ENTRY_BACKLOG),
        accepted_socket_limit: u32_at(bytes, OFF_ENTRY_ACCEPTED_SOCKET_LIMIT),
        byte_budget: u32_at(bytes, OFF_ENTRY_BYTE_BUDGET),
        timer_budget: u32_at(bytes, OFF_ENTRY_TIMER_BUDGET),
        queue_depth: u32_at(bytes, OFF_ENTRY_QUEUE_DEPTH),
        retry_limit: u32_at(bytes, OFF_ENTRY_RETRY_LIMIT),
        reconnect_limit: u32_at(bytes, OFF_ENTRY_RECONNECT_LIMIT),
    };
    if entry.holder_identity == [0; 32]
        || entry.control_binding == entry.provision_binding
        || entry.request_notification.is_empty() != entry.completion_notification.is_empty()
        || (!entry.request_notification.is_empty()
            && entry.request_notification == entry.completion_notification)
        || entry.rights & !KNOWN_RIGHTS != 0
    {
        return Err(DecodeError::InvalidEntry);
    }
    match role {
        Role::Client => {
            if entry.local_ipv4 != [0; 4]
                || entry.local_port != 0
                || entry.allowed_peer_identity != [0; 32]
                || entry.rights != 0
                || entry.backlog != 0
                || entry.accepted_socket_limit != 0
                || entry.byte_budget != 0
                || entry.timer_budget != 0
                || entry.queue_depth != 0
                || entry.retry_limit != 0
                || entry.reconnect_limit != 0
            {
                return Err(DecodeError::InvalidEntry);
            }
        }
        Role::Listener => {
            if backend != Backend::Loopback
                || entry.local_ipv4 != [127, 0, 0, 1]
                || entry.local_port == 0
                || entry.allowed_peer_identity == [0; 32]
                || entry.allowed_peer_identity == entry.holder_identity
                || entry.rights & RIGHT_LISTEN == 0
            {
                return Err(DecodeError::InvalidEntry);
            }
            if entry.backlog != 1
                || entry.accepted_socket_limit == 0
                || entry.accepted_socket_limit > MAX_ACCEPTED_SOCKETS
                || entry.backlog > entry.accepted_socket_limit
                || entry.byte_budget > MAX_BYTE_BUDGET
                || entry.byte_budget < BYTES_PER_ACCEPTED_SOCKET * entry.accepted_socket_limit
                || entry.timer_budget == 0
                || entry.timer_budget > MAX_TIMER_BUDGET
                || !(MIN_QUEUE_DEPTH..=MAX_QUEUE_DEPTH).contains(&entry.queue_depth)
                || !entry.queue_depth.is_power_of_two()
                || entry.retry_limit > MAX_RETRY_LIMIT
                || entry.reconnect_limit > MAX_RECONNECT_LIMIT
            {
                return Err(DecodeError::Impossible);
            }
        }
    }
    Ok(entry)
}

fn binding(bytes: &[u8], required: bool) -> Result<&[u8], DecodeError> {
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if (required && length == 0)
        || bytes[length..].iter().any(|byte| *byte != 0)
        || !bytes[..length].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(*byte, b'-' | b'_')
        })
    {
        return Err(DecodeError::InvalidEntry);
    }
    Ok(&bytes[..length])
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn name(bytes: &mut [u8], offset: usize, value: &[u8]) {
        bytes[offset..offset + value.len()].copy_from_slice(value);
    }
    fn client(holder: u8, backend: u8) -> [u8; ENTRY_BYTES] {
        let mut bytes = [0; ENTRY_BYTES];
        bytes[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].fill(holder);
        name(&mut bytes, OFF_ENTRY_CONTROL_BINDING, b"client-control");
        name(&mut bytes, OFF_ENTRY_PROVISION_BINDING, b"client-provision");
        bytes[OFF_ENTRY_ROLE] = ROLE_CLIENT;
        bytes[OFF_ENTRY_BACKEND] = backend;
        bytes
    }
    fn listener() -> [u8; ENTRY_BYTES] {
        let mut bytes = client(2, BACKEND_LOOPBACK);
        bytes[OFF_ENTRY_ROLE] = ROLE_LISTENER;
        bytes[OFF_ENTRY_ALLOWED_PEER_IDENTITY..OFF_ENTRY_ALLOWED_PEER_IDENTITY_END].fill(1);
        bytes[OFF_ENTRY_LOCAL_IPV4..OFF_ENTRY_LOCAL_IPV4_END].copy_from_slice(&[127, 0, 0, 1]);
        bytes[OFF_ENTRY_LOCAL_PORT..OFF_ENTRY_LOCAL_PORT_END]
            .copy_from_slice(&8080u16.to_le_bytes());
        bytes[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS_END]
            .copy_from_slice(&(RIGHT_LISTEN | RIGHT_SEND | RIGHT_RECV).to_le_bytes());
        for (offset, value) in [
            (OFF_ENTRY_BACKLOG, 1),
            (OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, 1),
            (OFF_ENTRY_BYTE_BUDGET, 4096),
            (OFF_ENTRY_TIMER_BUDGET, 1),
            (OFF_ENTRY_QUEUE_DEPTH, 4),
        ] {
            put32(&mut bytes, offset, value);
        }
        bytes
    }
    fn object(entries: &[[u8; ENTRY_BYTES]]) -> Vec<u8> {
        let mut bytes = std::vec![0; HEADER_BYTES];
        bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END].copy_from_slice(&MAGIC);
        put32(&mut bytes, OFF_HEADER_FORMAT_VERSION, FORMAT_VERSION);
        put32(&mut bytes, OFF_HEADER_HEADER_SIZE, HEADER_BYTES as u32);
        put32(
            &mut bytes,
            OFF_HEADER_APPLICATION_COUNT,
            entries.len() as u32,
        );
        put32(
            &mut bytes,
            OFF_HEADER_TOTAL_LEN,
            (HEADER_BYTES + entries.len() * ENTRY_BYTES) as u32,
        );
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }
    #[test]
    fn exact_client_and_listener_are_decoded_with_shared_holder_identity_domain() {
        assert_eq!(INCARNATION_PARAMETER_KEY, 1);
        assert_eq!(
            holder_identity("app"),
            crate::network_destination::holder_identity("app")
        );
        let bytes = object(&[client(1, BACKEND_LOOPBACK), listener()]);
        let table = NetworkApplications::decode(&bytes).unwrap();
        assert_eq!(table.application_count(), 2);
        let listener = table.by_holder(&[2; 32]).unwrap();
        assert_eq!(listener.role, Role::Listener);
        assert_eq!(listener.local_port, 8080);
        assert_eq!(listener.allowed_peer_identity, [1; 32]);
        assert_eq!(listener.control_binding, b"client-control");
        assert_eq!(table.entry_bytes(1).unwrap().len(), ENTRY_BYTES);
        assert!(table.entry_bytes(2).is_none());
        assert!(table.application(2).is_none());
        assert!(table.by_holder(&[3; 32]).is_none());
        assert!(NetworkApplications::decode(&object(&[])).is_ok());
        assert!(NetworkApplications::decode(&object(&[client(1, BACKEND_EXTERNAL)])).is_ok());
    }
    #[test]
    fn header_bounds_versions_flags_and_order_are_checked() {
        let valid = object(&[client(1, BACKEND_EXTERNAL)]);
        assert_eq!(
            NetworkApplications::decode(&valid[..HEADER_BYTES - 1]).err(),
            Some(DecodeError::Truncated)
        );
        for (offset, value, expected) in [
            (
                OFF_HEADER_FORMAT_VERSION,
                2,
                DecodeError::UnsupportedVersion,
            ),
            (OFF_HEADER_HEADER_SIZE, 0, DecodeError::UnsupportedVersion),
            (OFF_HEADER_APPLICATION_COUNT, 5, DecodeError::BadBounds),
            (OFF_HEADER_TOTAL_LEN, 0, DecodeError::BadBounds),
        ] {
            let mut bytes = valid.clone();
            put32(&mut bytes, offset, value);
            assert_eq!(NetworkApplications::decode(&bytes).err(), Some(expected));
        }
        for (offset, expected) in [
            (OFF_HEADER_MAGIC, DecodeError::BadMagic),
            (OFF_HEADER_REQUIRED_FLAGS, DecodeError::UnknownRequiredFlags),
        ] {
            let mut bytes = valid.clone();
            bytes[offset] ^= 1;
            assert_eq!(NetworkApplications::decode(&bytes).err(), Some(expected));
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert_eq!(
            NetworkApplications::decode(&trailing).err(),
            Some(DecodeError::BadBounds)
        );
        for entries in [
            [client(1, BACKEND_EXTERNAL); 2],
            [client(2, BACKEND_EXTERNAL), client(1, BACKEND_EXTERNAL)],
        ] {
            assert_eq!(
                NetworkApplications::decode(&object(&entries)).err(),
                Some(DecodeError::BadOrder)
            );
        }
        assert_eq!(
            NetworkApplications::decode(&object(
                &[client(1, BACKEND_EXTERNAL); MAX_APPLICATIONS + 1]
            ))
            .err(),
            Some(DecodeError::Truncated)
        );
    }
    #[test]
    fn malformed_names_discriminants_reserved_and_client_authority_are_rejected() {
        let valid = client(1, BACKEND_EXTERNAL);
        for offset in [
            OFF_ENTRY_ROLE,
            OFF_ENTRY_BACKEND,
            OFF_ENTRY_RESERVED0,
            OFF_ENTRY_RESERVED,
            OFF_ENTRY_RIGHTS,
            OFF_ENTRY_LOCAL_IPV4,
            OFF_ENTRY_LOCAL_PORT,
            OFF_ENTRY_ALLOWED_PEER_IDENTITY,
            OFF_ENTRY_BACKLOG,
            OFF_ENTRY_ACCEPTED_SOCKET_LIMIT,
            OFF_ENTRY_BYTE_BUDGET,
            OFF_ENTRY_TIMER_BUDGET,
            OFF_ENTRY_QUEUE_DEPTH,
            OFF_ENTRY_RETRY_LIMIT,
            OFF_ENTRY_RECONNECT_LIMIT,
        ] {
            let mut entry = valid;
            entry[offset] = 255;
            assert!(
                NetworkApplications::decode(&object(&[entry])).is_err(),
                "offset={offset}"
            );
        }
        for value in [b'!', b'A', 255] {
            let mut entry = valid;
            entry[OFF_ENTRY_CONTROL_BINDING] = value;
            assert!(NetworkApplications::decode(&object(&[entry])).is_err());
        }
        let mut zero = valid;
        zero[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].fill(0);
        assert!(NetworkApplications::decode(&object(&[zero])).is_err());
        for offset in [OFF_ENTRY_CONTROL_BINDING, OFF_ENTRY_PROVISION_BINDING] {
            let mut entry = valid;
            entry[offset..offset + BINDING_BYTES].fill(0);
            assert!(NetworkApplications::decode(&object(&[entry])).is_err());
        }
        let mut padded = valid;
        padded[OFF_ENTRY_CONTROL_BINDING_END - 1] = b'x';
        assert!(NetworkApplications::decode(&object(&[padded])).is_err());
        let mut equal = valid;
        equal.copy_within(
            OFF_ENTRY_CONTROL_BINDING..OFF_ENTRY_CONTROL_BINDING_END,
            OFF_ENTRY_PROVISION_BINDING,
        );
        assert!(NetworkApplications::decode(&object(&[equal])).is_err());
        let mut pair = valid;
        name(&mut pair, OFF_ENTRY_REQUEST_NOTIFICATION, b"request");
        assert!(NetworkApplications::decode(&object(&[pair])).is_err());
        name(&mut pair, OFF_ENTRY_COMPLETION_NOTIFICATION, b"complete");
        name(&mut pair, OFF_ENTRY_SUPERVISION_BINDING, b"supervisor");
        assert!(NetworkApplications::decode(&object(&[pair])).is_ok());
    }
    #[test]
    fn exact_capacity_names_and_listener_discriminants_are_bounded() {
        let entries = core::array::from_fn::<_, MAX_APPLICATIONS, _>(|index| {
            client(index as u8 + 1, BACKEND_EXTERNAL)
        });
        assert!(NetworkApplications::decode(&object(&entries)).is_ok());
        let mut full_name = entries[0];
        full_name[OFF_ENTRY_CONTROL_BINDING..OFF_ENTRY_CONTROL_BINDING_END].fill(b'x');
        assert_eq!(
            NetworkApplications::decode(&object(&[full_name]))
                .unwrap()
                .application(0)
                .unwrap()
                .control_binding
                .len(),
            BINDING_BYTES
        );
        let peer = client(1, BACKEND_LOOPBACK);
        for backend in [BACKEND_EXTERNAL, 0, 3] {
            let mut entry = listener();
            entry[OFF_ENTRY_BACKEND] = backend;
            assert!(NetworkApplications::decode(&object(&[peer, entry])).is_err());
        }
        for rights in [0, RIGHT_SEND | RIGHT_RECV, RIGHT_LISTEN | 1, u16::MAX] {
            let mut entry = listener();
            entry[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS_END].copy_from_slice(&rights.to_le_bytes());
            assert!(NetworkApplications::decode(&object(&[peer, entry])).is_err());
        }
        let mut duplicate_notifications = entries[0];
        name(
            &mut duplicate_notifications,
            OFF_ENTRY_REQUEST_NOTIFICATION,
            b"same",
        );
        name(
            &mut duplicate_notifications,
            OFF_ENTRY_COMPLETION_NOTIFICATION,
            b"same",
        );
        assert!(NetworkApplications::decode(&object(&[duplicate_notifications])).is_err());
    }

    #[test]
    fn distinct_holders_cannot_claim_the_same_listener_endpoint() {
        let mut second = listener();
        second[OFF_ENTRY_HOLDER_IDENTITY..OFF_ENTRY_HOLDER_IDENTITY_END].fill(3);
        let entries = [client(1, BACKEND_LOOPBACK), listener(), second];
        assert_eq!(
            NetworkApplications::decode(&object(&entries)).err(),
            Some(DecodeError::InvalidEntry)
        );
        second[OFF_ENTRY_LOCAL_PORT..OFF_ENTRY_LOCAL_PORT_END]
            .copy_from_slice(&8081u16.to_le_bytes());
        assert!(NetworkApplications::decode(&object(&[entries[0], entries[1], second])).is_ok());
    }

    #[test]
    fn listener_requires_exact_loopback_peer_and_independent_bounded_rights() {
        let peer = client(1, BACKEND_LOOPBACK);
        let valid = listener();
        for offset in [
            OFF_ENTRY_BACKEND,
            OFF_ENTRY_LOCAL_IPV4,
            OFF_ENTRY_LOCAL_PORT,
            OFF_ENTRY_ALLOWED_PEER_IDENTITY,
            OFF_ENTRY_RIGHTS,
        ] {
            let mut entry = valid;
            entry[offset] = 0;
            if offset == OFF_ENTRY_LOCAL_PORT {
                entry[OFF_ENTRY_LOCAL_PORT..OFF_ENTRY_LOCAL_PORT_END].fill(0);
            }
            assert!(
                NetworkApplications::decode(&object(&[peer, entry])).is_err(),
                "offset={offset}"
            );
        }
        for (offset, value) in [
            (OFF_ENTRY_BACKLOG, 0),
            (OFF_ENTRY_BACKLOG, 2),
            (OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, 0),
            (OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, MAX_ACCEPTED_SOCKETS + 1),
            (OFF_ENTRY_BYTE_BUDGET, 4095),
            (OFF_ENTRY_BYTE_BUDGET, MAX_BYTE_BUDGET + 1),
            (OFF_ENTRY_TIMER_BUDGET, 0),
            (OFF_ENTRY_TIMER_BUDGET, MAX_TIMER_BUDGET + 1),
            (OFF_ENTRY_QUEUE_DEPTH, 1),
            (OFF_ENTRY_QUEUE_DEPTH, 3),
            (OFF_ENTRY_QUEUE_DEPTH, MAX_QUEUE_DEPTH + 1),
            (OFF_ENTRY_RETRY_LIMIT, MAX_RETRY_LIMIT + 1),
            (OFF_ENTRY_RECONNECT_LIMIT, MAX_RECONNECT_LIMIT + 1),
        ] {
            let mut entry = valid;
            put32(&mut entry, offset, value);
            assert_eq!(
                NetworkApplications::decode(&object(&[peer, entry])).err(),
                Some(DecodeError::Impossible),
                "offset={offset}"
            );
        }
        let mut maximum = valid;
        for (offset, value) in [
            (OFF_ENTRY_BACKLOG, 1),
            (OFF_ENTRY_ACCEPTED_SOCKET_LIMIT, MAX_ACCEPTED_SOCKETS),
            (OFF_ENTRY_BYTE_BUDGET, MAX_BYTE_BUDGET),
            (OFF_ENTRY_TIMER_BUDGET, MAX_TIMER_BUDGET),
            (OFF_ENTRY_RETRY_LIMIT, MAX_RETRY_LIMIT),
            (OFF_ENTRY_RECONNECT_LIMIT, MAX_RECONNECT_LIMIT),
        ] {
            put32(&mut maximum, offset, value);
        }
        assert!(NetworkApplications::decode(&object(&[peer, maximum])).is_ok());
        for backlog in 2..=MAX_ACCEPTED_SOCKETS {
            let mut unsupported = maximum;
            put32(&mut unsupported, OFF_ENTRY_BACKLOG, backlog);
            assert_eq!(
                NetworkApplications::decode(&object(&[peer, unsupported])).err(),
                Some(DecodeError::Impossible)
            );
        }
        assert_eq!(
            NetworkApplications::decode(&object(&[valid])).err(),
            Some(DecodeError::InvalidPeer)
        );
        assert_eq!(
            NetworkApplications::decode(&object(&[client(1, BACKEND_EXTERNAL), valid])).err(),
            Some(DecodeError::InvalidPeer)
        );
        let mut self_peer = valid;
        self_peer[OFF_ENTRY_ALLOWED_PEER_IDENTITY..OFF_ENTRY_ALLOWED_PEER_IDENTITY_END].fill(2);
        assert!(NetworkApplications::decode(&object(&[peer, self_peer])).is_err());
        for rights in [
            RIGHT_LISTEN,
            RIGHT_LISTEN | RIGHT_SEND,
            RIGHT_LISTEN | RIGHT_RECV,
        ] {
            let mut entry = valid;
            entry[OFF_ENTRY_RIGHTS..OFF_ENTRY_RIGHTS_END].copy_from_slice(&rights.to_le_bytes());
            assert!(NetworkApplications::decode(&object(&[peer, entry])).is_ok());
        }
    }
}
