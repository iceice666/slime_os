#![no_std]

// Protocol modules are generated from contracts/*/v1 schemas.
pub mod block;
pub mod capability_transfer;
pub mod component;
pub mod fabric_stream;
pub mod fs;
pub mod generation;
pub mod interface_schema;
pub mod powerbox;
pub mod sample_descriptor;
pub mod spawn;
pub mod store;

pub fn valid_fs_request(request: &fs::WireFsRequest) -> bool {
    let name_len = request.name_len as usize;
    let base_valid = request.magic == fs::FS_MAGIC
        && request.version == fs::FORMAT_VERSION
        && matches!(
            request.op,
            fs::OP_LIST | fs::OP_READ | fs::OP_WRITE | fs::OP_DERIVE
        )
        && request.flags == 0
        && request.reserved0 == 0
        && name_len <= fs::MAX_NAME_BYTES
        && request.name[name_len..].iter().all(|byte| *byte == 0)
        && valid_name(&request.name[..name_len], request.op == fs::OP_LIST);
    if !base_valid {
        return false;
    }
    let zero_hash =
        request.hash0 == 0 && request.hash1 == 0 && request.hash2 == 0 && request.hash3 == 0;
    match request.op {
        fs::OP_LIST | fs::OP_READ | fs::OP_DERIVE => request.payload_len == 0 && zero_hash,
        fs::OP_WRITE => request.payload_len <= 32 * 1024 && !zero_hash,
        _ => false,
    }
}

pub fn valid_fs_reply(reply: &fs::WireFsReply) -> bool {
    reply.magic == fs::FS_MAGIC
        && reply.version == fs::FORMAT_VERSION
        && reply.entry_count as usize <= fs::MAX_ENTRIES
        && reply.reserved == 0
}
fn valid_name(name: &[u8], allow_empty: bool) -> bool {
    if name.is_empty() {
        return allow_empty;
    }
    name != b"."
        && name != b".."
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

pub fn valid_powerbox_request(request: &powerbox::WirePowerboxRequest) -> bool {
    let purpose_len = request.purpose_len as usize;
    request.magic == powerbox::POWERBOX_MAGIC
        && request.version == powerbox::FORMAT_VERSION
        && request.object_kind == powerbox::OBJECT_KIND_FILE
        && request.reserved0 == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && purpose_len > 0
        && purpose_len <= powerbox::MAX_PURPOSE_BYTES
        && request.purpose[purpose_len..].iter().all(|byte| *byte == 0)
        && request.purpose[..purpose_len]
            .iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        && request.requested_rights != 0
}

pub fn valid_powerbox_reply(reply: &powerbox::WirePowerboxReply) -> bool {
    if reply.magic != powerbox::POWERBOX_MAGIC
        || reply.version != powerbox::FORMAT_VERSION
        || reply.object_kind != powerbox::OBJECT_KIND_FILE
        || reply.reserved0 != 0
        || reply.purpose_len as usize > reply.purpose.len()
        || reply.purpose[reply.purpose_len as usize..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return false;
    }
    match reply.flags {
        powerbox::REPLY_FLAG_SELECTED => {
            reply.status == 0 && reply.granted_rights != 0 && reply.event_id != 0
        }
        powerbox::REPLY_FLAG_CANCELLED => {
            reply.status == 0 && reply.granted_rights == 0 && reply.event_id == 0
        }
        _ => reply.status < 0 && reply.granted_rights == 0 && reply.event_id == 0,
    }
}

pub fn valid_spawn_request(request: &spawn::WireSpawnRequest) -> bool {
    if request.magic != spawn::SPAWN_MAGIC || request.version != spawn::FORMAT_VERSION {
        return false;
    }
    if request.flags == spawn::REQUEST_FLAG_WAIT {
        return request.command_len == 0
            && request.argument_count == 0
            && request.environment_count == 0
            && request.capability_roles == 0
            && request.command.iter().all(|byte| *byte == 0)
            && request.environment.iter().all(|byte| *byte == 0)
            && u64::from_le_bytes(request.arguments) != 0
            && request.grant_rights == 0
            && request.reserved.iter().all(|byte| *byte == 0);
    }
    request.flags == 0
        && request.command_len > 0
        && request.command_len as usize <= spawn::MAX_COMMAND_BYTES
        && request.argument_count as usize <= spawn::MAX_ARGUMENTS
        && request.environment_count as usize <= spawn::MAX_ENVIRONMENT
        && packed_fields_valid(&request.arguments, request.argument_count as usize)
        && packed_fields_valid(&request.environment, request.environment_count as usize)
}

fn packed_fields_valid<const N: usize>(bytes: &[u8; N], count: usize) -> bool {
    let mut offset = 0;
    for _ in 0..count {
        let Some(length) = bytes.get(offset).copied().map(usize::from) else {
            return false;
        };
        if length == 0 || offset + 1 + length > bytes.len() {
            return false;
        }
        offset += 1 + length;
    }
    bytes[offset..].iter().all(|byte| *byte == 0)
}

pub fn valid_spawn_reply(reply: &spawn::WireSpawnReply) -> bool {
    reply.magic == spawn::SPAWN_MAGIC && reply.version == spawn::FORMAT_VERSION
}

/// Validate a versioned sample descriptor before a receiver maps the loaned
/// bytes or allocates receiver state. Every field that could steer a mapping or
/// allocation is bounded here: version, known flags, capability kind, a live
/// loan identity, a page-aligned in-bounds offset/length within `MAX_SAMPLE_BYTES`,
/// a non-zero type identity, and zeroed reserved bytes. `expected_type` binds the
/// descriptor to the receiver's declared type; `expected_loan` binds it to the
/// exact transferred loan the receiver holds.
pub fn valid_sample_descriptor(
    descriptor: &sample_descriptor::WireSampleDescriptor,
    expected_loan: u64,
    expected_type: u64,
    page_size: u64,
) -> bool {
    if descriptor.magic != sample_descriptor::SAMPLE_DESCRIPTOR_MAGIC
        || descriptor.version != sample_descriptor::FORMAT_VERSION
    {
        return false;
    }
    if descriptor.capability_kind != sample_descriptor::CAPABILITY_KIND_LOAN {
        return false;
    }
    if descriptor.flags & !sample_descriptor::KNOWN_FLAGS != 0 {
        return false;
    }
    if descriptor.reserved.iter().any(|byte| *byte != 0) {
        return false;
    }
    if descriptor.loan_id == 0 || descriptor.loan_id != expected_loan {
        return false;
    }
    if descriptor.type_identity == 0 || descriptor.type_identity != expected_type {
        return false;
    }
    if page_size == 0 || !page_size.is_power_of_two() {
        return false;
    }
    let Some(end) = descriptor.offset.checked_add(descriptor.length) else {
        return false;
    };
    descriptor.length != 0
        && descriptor.offset.is_multiple_of(page_size)
        && descriptor.length.is_multiple_of(page_size)
        && end <= sample_descriptor::MAX_SAMPLE_BYTES as u64
}

/// Validate a capability-transfer descriptor a component received alongside a
/// moved capability (C8.3), binding it to the role the receiver expects.
///
/// The kernel already enforced the structural rules and installed exactly
/// `rights_mask` (minus `RIGHT_TRANSFER` without `FLAG_RETAIN_TRANSFER`), so a
/// descriptor cannot overstate what arrived. What the kernel cannot check is
/// the fabric's own role binding: it knows nothing of routes or directions.
/// This checks that half — the descriptor names the exact
/// (route identity, direction) edge the receiver was provisioned for, and
/// carries the object kind that role implies.
pub fn valid_capability_transfer(
    descriptor: &capability_transfer::WireCapabilityTransfer,
    expected_route: &[u8; 32],
    expected_direction: u32,
    expected_kind: u32,
) -> bool {
    descriptor.magic == capability_transfer::CAPABILITY_TRANSFER_MAGIC
        && descriptor.version == capability_transfer::FORMAT_VERSION
        && descriptor.status == 0
        && descriptor.flags & !capability_transfer::KNOWN_FLAGS == 0
        && descriptor.rights_mask != 0
        && descriptor.object_kind == expected_kind
        && descriptor.direction == expected_direction
        && descriptor.route_identity == *expected_route
        && *expected_route != [0; 32]
}

/// Structural validity of a fabric provisioning request, before the service
/// looks at anything it claims.
///
/// Deliberately shallow: the route name, direction, and type identity a
/// request carries are caller-supplied and grant nothing. The fabric
/// authenticates by the control endpoint the request arrived on and answers
/// from the generation graph, so this only rejects bytes that are not a
/// request at all.
pub fn valid_fabric_request(request: &capability_transfer::WireFabricRequest) -> bool {
    request.magic == capability_transfer::FABRIC_REQUEST_MAGIC
        && request.version == capability_transfer::FORMAT_VERSION
        && request.flags == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && (request.route_name_len as usize) <= capability_transfer::MAX_ROUTE_NAME_BYTES
        && request.route_name[request.route_name_len as usize..]
            .iter()
            .all(|byte| *byte == 0)
}

/// Structural validity of an inline stream sample, before a subscriber reads
/// its payload or a broker forwards it (C8.4).
///
/// Inline delivery is the path a control-bound sample takes; anything larger
/// travels as a C7.6 descriptor over a receiver-bound loan. So this bounds
/// every field that could steer a copy: version, known flags, a payload length
/// inside `MAX_INLINE_BYTES`, zeroed padding past that length, and the exact
/// admitted type identity. `expected_type` binds the sample to the route's
/// declared interface, so a publisher cannot inject another type's bytes into
/// a route that carries this one.
pub fn valid_stream_sample(
    sample: &fabric_stream::WireStreamSample,
    expected_type: u64,
    max_inline: usize,
) -> bool {
    if sample.magic != fabric_stream::STREAM_SAMPLE_MAGIC
        || sample.version != fabric_stream::FORMAT_VERSION
    {
        return false;
    }
    if sample.flags & !fabric_stream::KNOWN_SAMPLE_FLAGS != 0 {
        return false;
    }
    if sample.type_identity == 0 || sample.type_identity != expected_type {
        return false;
    }
    let bound = max_inline.min(fabric_stream::MAX_INLINE_BYTES);
    let length = sample.payload_len as usize;
    if length == 0 || length > bound {
        return false;
    }
    // Padding past the declared length must be zero: two byte-distinct samples
    // that decode to the same payload would otherwise both be admissible, and
    // a KEEP_LAST ring comparing stored bytes could not treat them as one.
    sample.payload[length..].iter().all(|byte| *byte == 0)
}

/// Structural validity of a delivery-slot release (C8.4).
///
/// An ack names the sequence it releases, so the fabric can reject a
/// subscriber that tries to free a slot it never consumed. That check needs the
/// fabric's own delivery state and lives there; this bounds only the bytes:
/// version, no unknown flags, zeroed padding, the route's exact type identity,
/// and a nonzero sequence, since publishers number from one.
pub fn valid_stream_ack(ack: &fabric_stream::WireStreamAck, expected_type: u64) -> bool {
    ack.magic == fabric_stream::STREAM_ACK_MAGIC
        && ack.version == fabric_stream::FORMAT_VERSION
        && ack.flags & !fabric_stream::KNOWN_ACK_FLAGS == 0
        && ack.reserved0 == 0
        && ack.reserved.iter().all(|byte| *byte == 0)
        && ack.sequence != 0
        && ack.type_identity != 0
        && ack.type_identity == expected_type
}

/// Structural validity of a stream event (C8.4). An event never carries data,
/// so it must never be mistaken for a sample: the kind is checked against the
/// three this version defines, and each is bound to the fields it may name.
pub fn valid_stream_event(event: &fabric_stream::WireStreamEvent, expected_type: u64) -> bool {
    if event.magic != fabric_stream::STREAM_EVENT_MAGIC
        || event.version != fabric_stream::FORMAT_VERSION
    {
        return false;
    }
    if event.flags & !fabric_stream::KNOWN_EVENT_FLAGS != 0 {
        return false;
    }
    if event.reserved.iter().any(|byte| *byte != 0) {
        return false;
    }
    if event.type_identity == 0 || event.type_identity != expected_type {
        return false;
    }
    match event.event {
        // A loss report must name a loss and the oldest sequence it covers.
        fabric_stream::EVENT_SAMPLE_LOST => event.lost != 0 && event.sequence != 0,
        // A terminal notice covers the route, not a sample, so it names neither.
        fabric_stream::EVENT_STREAM_END => event.lost == 0 && event.sequence == 0,
        // A credit settles one exact sample, so it names that sequence.
        fabric_stream::EVENT_SAMPLE_TAKEN => event.lost == 0 && event.sequence != 0,
        _ => false,
    }
}
