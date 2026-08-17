#![no_std]

// Protocol modules are generated from contracts/*/v1 schemas.
pub mod block;
pub mod capability_transfer;
pub mod component;
pub mod fabric_call;
pub mod fabric_operation;
pub mod fabric_qos;
pub mod fabric_ring;
pub mod fabric_stream;
pub mod fabric_time;
pub mod fabric_trace;
pub mod fabric_visibility;
pub mod fs;
pub mod generation;
pub mod interface_schema;
pub mod powerbox;
pub mod ring;
pub mod sample_descriptor;
pub mod spawn;
pub mod store;
pub mod syscall_abi;
pub mod trace_sink;

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
    if request.flags == spawn::REQUEST_FLAG_SHUTDOWN {
        return request.command_len == 0
            && request.argument_count == 0
            && request.environment_count == 0
            && request.capability_roles == 0
            && request.command.iter().all(|byte| *byte == 0)
            && request.arguments.iter().all(|byte| *byte == 0)
            && request.environment.iter().all(|byte| *byte == 0)
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
/// Structural validity of one C8.8 introspection page request. The cursor is a
/// bounded `u8` by construction; the service filters it against the caller's
/// generation-derived view and returns the same terminal record for an empty
/// view and an out-of-range cursor.
pub fn valid_visibility_request(request: &fabric_visibility::WireVisibilityRequest) -> bool {
    request.magic == fabric_visibility::VISIBILITY_REQUEST_MAGIC
        && request.version == fabric_visibility::FORMAT_VERSION
        && request.flags & !fabric_visibility::KNOWN_REQUEST_FLAGS == 0
        && request.reserved.iter().all(|byte| *byte == 0)
}

/// Validate a route page returned by the read-only graph service. An end page
/// carries no graph-dependent bytes; this is what prevents an ungranted caller
/// from learning protected counts or identities through error detail.
pub fn valid_visibility_route_record(
    record: &fabric_visibility::WireVisibilityRouteRecord,
) -> bool {
    if record.magic != fabric_visibility::VISIBILITY_ROUTE_MAGIC
        || record.version != fabric_visibility::FORMAT_VERSION
        || record.flags & !fabric_visibility::KNOWN_ROUTE_FLAGS != 0
        || record.reserved0.iter().any(|byte| *byte != 0)
    {
        return false;
    }
    if record.status == fabric_visibility::STATUS_END {
        return record.contract_kind == 0
            && record.route_name_len == 0
            && record.route_name.iter().all(|byte| *byte == 0)
            && record.schema_identity.iter().all(|byte| *byte == 0)
            && record.flags == 0;
    }
    let name_len = record.route_name_len as usize;
    record.status == fabric_visibility::STATUS_RECORD
        && matches!(record.contract_kind, 1..=3)
        && name_len > 0
        && name_len <= fabric_visibility::ROUTE_NAME_BYTES
        && record.route_name[name_len..].iter().all(|byte| *byte == 0)
        && record.route_name[..name_len]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
        && record.schema_identity.iter().any(|byte| *byte != 0)
}

/// Validate the complete QoS half of one introspection page.
pub fn valid_visibility_qos_record(record: &fabric_visibility::WireVisibilityQosRecord) -> bool {
    record.magic == fabric_visibility::VISIBILITY_QOS_MAGIC
        && record.version == fabric_visibility::FORMAT_VERSION
        && record.status == fabric_visibility::STATUS_RECORD
        && record.flags & !fabric_visibility::KNOWN_QOS_FLAGS == 0
        && fixed_name_valid(&record.route_name)
        && matches!(record.reliability, 1 | 2)
        && matches!(record.durability, 1 | 2)
        && matches!(record.liveliness, 1 | 2)
        && record.matched <= 1
        && record.event_mask & !fabric_visibility::EVENT_PROXY_LOST == 0
}

/// Validate a trace record before accepting it from an authenticated interposer
/// or delivering the resulting route event.
pub fn valid_interposition_trace(record: &fabric_visibility::WireInterpositionTrace) -> bool {
    record.magic == fabric_visibility::INTERPOSITION_TRACE_MAGIC
        && record.version == fabric_visibility::FORMAT_VERSION
        && matches!(
            record.event,
            fabric_visibility::TRACE_RELAYED | fabric_visibility::TRACE_PROXY_LOST
        )
        && record.flags & !fabric_visibility::KNOWN_TRACE_FLAGS == 0
        && record.route_identity.iter().any(|byte| *byte != 0)
        && record.sequence != 0
        && record.reserved.iter().all(|byte| *byte == 0)
}

fn fixed_name_valid(name: &[u8]) -> bool {
    let length = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    length > 0
        && name[length..].iter().all(|byte| *byte == 0)
        && name[..length]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
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

/// Structural validity of a ring header (B46).
///
/// A ring is shared memory, so every field here was written by a peer this
/// reader does not trust to be correct — a publisher with a wild pointer or a
/// stale mapping produces bytes, not an error. The reader's own bound comes
/// from `expected_slots`, which the fabric fixed at provisioning; a header
/// claiming more is refused rather than believed, because believing it is how
/// a reader walks off the end of its own mapping.
pub fn valid_ring_header(
    header: &fabric_ring::WireRingHeader,
    expected_type: u64,
    expected_slots: usize,
) -> bool {
    if header.magic != fabric_ring::RING_MAGIC || header.version != fabric_ring::FORMAT_VERSION {
        return false;
    }
    if header.slot_len as usize != fabric_ring::RING_SLOT_LEN {
        return false;
    }
    let slots = header.slot_count as usize;
    if slots != expected_slots
        || !(fabric_ring::MIN_RING_SLOTS..=fabric_ring::MAX_RING_SLOTS).contains(&slots)
        || !slots.is_power_of_two()
    {
        return false;
    }
    // `head - tail` is the occupancy, so `tail` past `head` is not a full ring
    // or an empty one — it is a header no correct writer produces, and a
    // reader that subtracted anyway would get a huge count and try to consume
    // it.
    if header.tail > header.head || header.head - header.tail > slots as u64 {
        return false;
    }
    if !matches!(
        header.producer_state,
        fabric_ring::PRODUCER_ACTIVE | fabric_ring::PRODUCER_FINISHED | fabric_ring::PRODUCER_DEAD
    ) {
        return false;
    }
    header.type_identity != 0
        && header.type_identity == expected_type
        && header.reserved.iter().all(|byte| *byte == 0)
}

/// Structural validity of one ring slot (B46).
///
/// `sequence` is absolute, and `expected_sequence` is the one this reader is
/// owed. A slot carrying anything else is a wrap the reader fell behind on,
/// not a sample it may consume — which is the distinction that lets a lagging
/// subscriber count drops instead of silently reading stale bytes as new.
///
/// Only `SLOT_READY` is admissible. `SLOT_CLAIMED` means the publisher is
/// mid-copy: refusing it is what makes a torn write unobservable rather than
/// merely unlikely.
pub fn valid_ring_slot(
    slot: &fabric_ring::WireRingSlot,
    expected_type: u64,
    expected_sequence: u64,
) -> bool {
    if slot.magic != fabric_ring::SLOT_MAGIC || slot.state != fabric_ring::SLOT_READY {
        return false;
    }
    if slot.flags & !fabric_ring::KNOWN_SLOT_FLAGS != 0 {
        return false;
    }
    if slot.sequence == 0 || slot.sequence != expected_sequence {
        return false;
    }
    if slot.type_identity == 0 || slot.type_identity != expected_type {
        return false;
    }
    let length = slot.payload_len as usize;
    if length == 0 || length > fabric_ring::MAX_INLINE_BYTES {
        return false;
    }
    // As with v1's inline sample: padding past the declared length must be
    // zero, so two byte-distinct slots cannot decode to the same payload.
    slot.payload[length..].iter().all(|byte| *byte == 0)
}

/// Which slot a sequence occupies, given a validated `slot_count`.
///
/// Masking rather than a remainder, which is why the count is required to be a
/// power of two. Callers must validate the header first: this cannot be
/// checked here without making every read pay for it.
pub fn ring_slot_index(sequence: u64, slot_count: usize) -> usize {
    (sequence as usize) & (slot_count - 1)
}

/// Whether a badge word carries only bits this version defines (B46).
///
/// A notification word is OR-ed, so an unknown bit means a peer signalling
/// something this reader cannot interpret. Refusing it is the same discipline
/// as an unknown message label: the alternative is treating it as one of the
/// bits that *is* known.
pub fn valid_ring_badge(badge: u64) -> bool {
    badge != 0 && badge & !fabric_ring::KNOWN_BADGE_BITS == 0
}

pub fn valid_time_advance(value: &fabric_time::WireTimeAdvance) -> bool {
    value.magic == fabric_time::TIME_ADVANCE_MAGIC
        && value.version == fabric_time::FORMAT_VERSION
        && value.flags == 0
        && value.reserved0 == 0
        && value.reserved.iter().all(|byte| *byte == 0)
}

pub fn valid_qos_event(value: &fabric_qos::WireQosEvent, expected_type: u64) -> bool {
    use fabric_qos::*;
    value.magic == QOS_EVENT_MAGIC
        && value.version == FORMAT_VERSION
        && value.flags == 0
        && value.type_identity == expected_type
        && value.reserved.iter().all(|byte| *byte == 0)
        && matches!(
            value.event,
            EVENT_MATCHED
                | EVENT_UNMATCHED
                | EVENT_INCOMPATIBLE_QOS
                | EVENT_LIFESPAN_EXPIRED
                | EVENT_RETRY_EXHAUSTED
                | EVENT_DEADLINE_MISSED
                | EVENT_LIVELINESS_LOST
                | EVENT_PEER_DEAD
        )
}

/// Structural validity of one C8.6 inline call envelope. Correlation/session
/// checks that require live broker state stay in the fabric service; this
/// rejects malformed bytes before they can allocate an in-flight entry or
/// steer data. Shared payloads use `SampleDescriptor`, never this envelope.
pub fn valid_call_envelope(value: &fabric_call::WireCallEnvelope, expected_type: u64) -> bool {
    use fabric_call::*;
    if value.magic != CALL_MAGIC
        || value.version != FORMAT_VERSION
        || value.flags & !FLAG_NON_IDEMPOTENT != 0
        || value.session == 0
        || value.request_id == 0
        || value.type_identity == 0
        || value.type_identity != expected_type
        || (!matches!(value.kind, KIND_TERMINAL | KIND_TERMINAL_ACK)
            && value.payload_len as usize > INLINE_BYTES)
        || (matches!(value.kind, KIND_TERMINAL | KIND_TERMINAL_ACK) && value.payload_len != 0)
        || value.payload[value.payload_len.min(value.payload.len() as u32) as usize..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return false;
    }
    match value.kind {
        KIND_REQUEST => value.status == STATUS_SUCCESS,
        KIND_REPLY => matches!(
            value.status,
            STATUS_SUCCESS | STATUS_REJECTED | STATUS_CANCELLED
        ),
        KIND_CANCEL => {
            value.flags == 0 && value.status == STATUS_CANCELLED && value.payload_len == 0
        }
        KIND_TERMINAL => {
            value.flags == 0
                && matches!(
                    value.status,
                    STATUS_SUCCESS
                        | STATUS_REJECTED
                        | STATUS_TIMEOUT
                        | STATUS_CANCELLED
                        | STATUS_RETRY_EXHAUSTED
                        | STATUS_MALFORMED_REPLY
                        | STATUS_PEER_DEAD
                        | STATUS_DUPLICATE
                        | STATUS_STALE
                )
                && value.payload_len == 0
        }
        // The client's word that it took a terminal, naming the request that
        // terminal settled. It carries the settled status back so a broker
        // cannot retire a record on an ack for some other outcome.
        KIND_TERMINAL_ACK => {
            value.flags == 0
                && matches!(
                    value.status,
                    STATUS_SUCCESS
                        | STATUS_REJECTED
                        | STATUS_TIMEOUT
                        | STATUS_CANCELLED
                        | STATUS_RETRY_EXHAUSTED
                        | STATUS_MALFORMED_REPLY
                        | STATUS_PEER_DEAD
                        | STATUS_DUPLICATE
                        | STATUS_STALE
                )
                && value.payload_len == 0
        }
        _ => false,
    }
}

pub fn valid_call_time_advance(value: &fabric_call::WireCallTimeAdvance) -> bool {
    value.magic == fabric_call::CALL_TIME_MAGIC
        && value.version == fabric_call::FORMAT_VERSION
        && value.flags == 0
        && value.reserved0 == 0
        && value.reserved.iter().all(|byte| *byte == 0)
}

/// Structural validity of one C8.7 operation envelope, before it can allocate
/// an active-operation entry, a feedback slot, or a retained result.
///
/// C8.7 composes an operation from a start-goal call, an operation-keyed
/// feedback stream, a result call, and a cancellation request. Every leg rides
/// this one record, so the checks are per-kind: a field that means nothing on a
/// leg must be zero there rather than merely ignored, or a peer could smuggle a
/// feedback sequence into a cancellation and have it survive into broker state.
///
/// Correlation, authority, and duplicate/stale suppression need live broker
/// state and stay in the fabric; this rejects bytes that are not a well-formed
/// leg at all. Shared payloads travel as `SampleDescriptor`, never here.
pub fn valid_operation_envelope(
    value: &fabric_operation::WireOperationEnvelope,
    expected_type: u64,
) -> bool {
    use fabric_operation::*;
    if value.magic != OPERATION_MAGIC
        || value.version != FORMAT_VERSION
        || value.session == 0
        || value.operation_id == 0
        || value.type_identity == 0
        || value.type_identity != expected_type
        || value.payload_len as usize > INLINE_BYTES
        || value.payload[value.payload_len as usize..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return false;
    }
    // Only feedback is ordered within an operation; a sequence anywhere else is
    // a field the sender had no business setting. The server-idle fence carries
    // no payload, but names the handled request so the broker can match it.
    if value.kind != KIND_FEEDBACK && value.sequence != 0 {
        return false;
    }
    match value.kind {
        KIND_GOAL => value.status == STATUS_SUCCESS,
        // The transport's answer to a goal: refused, running, or winding down.
        KIND_ACCEPTED => {
            value.payload_len == 0
                && matches!(
                    value.status,
                    STATUS_SUCCESS | STATUS_REJECTED | STATUS_ACTIVE | STATUS_CANCEL_REQUESTED
                )
        }
        // Feedback is progress, never an outcome, and is numbered from one.
        KIND_FEEDBACK => value.status == STATUS_ACTIVE && value.sequence != 0,
        KIND_RESULT => matches!(
            value.status,
            STATUS_SUCCESS | STATUS_REJECTED | STATUS_CANCELLED
        ),
        KIND_RESULT_REQUEST | KIND_CANCEL => {
            value.payload_len == 0 && value.status == STATUS_SUCCESS
        }
        KIND_TERMINAL => {
            value.payload_len == 0
                && matches!(
                    value.status,
                    STATUS_SUCCESS
                        | STATUS_REJECTED
                        | STATUS_CANCELLED
                        | STATUS_EXPIRED
                        | STATUS_TIMEOUT
                        | STATUS_PEER_DEAD
                        | STATUS_DUPLICATE
                        | STATUS_STALE
                        | STATUS_MALFORMED
                        | STATUS_RETRY_EXHAUSTED
                )
        }
        KIND_SERVER_IDLE => value.payload_len == 0 && value.status == STATUS_SUCCESS,
        _ => false,
    }
}

/// Structural validity of one C8.11 semantic-trace record.
///
/// The record is the deterministic evidence stream the repeated-boot comparison
/// reads, so "structural" is stricter here than on a transport envelope: a
/// field that means nothing for a family must be zero in that family rather
/// than merely ignored. A trace whose unused fields carried whatever the
/// emitter happened to hold would not be byte-comparable across two runs of
/// the same inputs, which is the whole property C8.11 claims.
///
/// What is deliberately *not* checked here is anything requiring live state:
/// whether a route identity is admitted, whether a correlation is outstanding,
/// or whether the clock actually reached `now_ns`. Those belong to the emitting
/// worker, which holds the graph. This rejects bytes that are not a well-formed
/// record of any family.
pub fn valid_trace_record(value: &fabric_trace::WireTraceRecord) -> bool {
    use fabric_trace::*;
    if value.magic != TRACE_MAGIC
        || value.version != FORMAT_VERSION
        || value.kind == 0
        || value.kind > MAX_KIND
        || value.flags & !KNOWN_FLAGS != 0
        || value.order_class == 0
        || u32::from(value.order_class) > MAX_ORDER_CLASS
        || value.reserved.iter().any(|byte| *byte != 0)
    {
        return false;
    }
    // The two flags answer different questions and cannot both be true: a
    // saturation report is not the record that ends the stream.
    if value.flags == KNOWN_FLAGS {
        return false;
    }
    // A saturation report counts refused records; that count is what makes the
    // loss observable rather than silent, so zero would defeat the record.
    if value.flags & FLAG_DROPPED != 0 && value.high_water == 0 {
        return false;
    }
    // The time class closes an instant rather than occurring within it, so a
    // record claiming it must name neither an edge nor a correlation: it is not
    // an event *on* anything. Equivalently, an edge-bearing record can never
    // sort after everything at its own instant, which is what makes the tie
    // order meaningful. Two families may close an instant — the clock advance
    // itself, and the sink's own accounting at that boundary.
    let time_class = u32::from(value.order_class) == ORDER_TIME;
    if time_class && (value.route_identity != 0 || value.correlation != 0) {
        return false;
    }
    if time_class && !matches!(value.kind, KIND_QOS | KIND_RESOURCE) {
        return false;
    }
    match value.kind {
        // Schema admission is a per-generation fact: it names no route edge, no
        // correlation, and no outcome, only the sequence it was admitted in.
        KIND_SCHEMA => {
            value.route_identity == 0
                && value.correlation == 0
                && value.status == 0
                && value.event == 0
        }
        // Route provisioning names its edge and reports no correlation.
        KIND_ROUTE => value.route_identity != 0 && value.correlation == 0 && value.event == 0,
        // A QoS record is either the clock advance that closes an instant or a
        // policy event on a declared edge. The two are told apart by the edge,
        // not by the order class: an advance names no edge, and a policy event
        // always does. Deriving it from the fields rather than the class keeps
        // the class free to be checked independently above.
        KIND_QOS => {
            if value.route_identity == 0 {
                // A clock advance. It carries the emitting worker's sequence like
                // any other record -- that is the sort's tie-break key, and two
                // records closing one instant must not share it -- but it names
                // no correlation, no outcome, and no event, because it is not an
                // event on anything.
                u32::from(value.order_class) == ORDER_TIME
                    && value.correlation == 0
                    && value.status == 0
                    && value.event == 0
            } else {
                u32::from(value.order_class) != ORDER_TIME && value.event != 0
            }
        }
        // Request/response families correlate a request with its outcome, so
        // both the edge and the correlation are load-bearing.
        KIND_CALL | KIND_OPERATION => value.route_identity != 0 && value.correlation != 0,
        // Visibility and interposition are graph-shaped: an edge, no outcome
        // code, and an event naming what was observed or traversed.
        //
        // The event is bounded, for the same reason a resource counter is: it
        // is the only field on these families carrying meaning, so a number
        // outside the declared vocabulary is not evidence a reader can compare
        // across runs.
        KIND_VISIBILITY | KIND_INTERPOSITION => {
            value.route_identity != 0
                && value.correlation == 0
                && value.event != 0
                && value.event <= fabric_trace::MAX_GRAPH_EVENT
        }
        // A denial is a refusal, and it names nothing.
        //
        // Enforced by the format rather than left to each call site. A refusal
        // that carried the route identity would confirm to the caller that the
        // edge exists — the protected graph metadata the refusal is there to
        // withhold — and one that echoed the caller's correlation would
        // republish an identity the broker just rejected, which on a shared
        // route may belong to a different client. What a denial carries is the
        // fact that something was refused, and the status saying what kind of
        // refusal it was.
        KIND_DENIAL => {
            value.route_identity == 0
                && value.correlation == 0
                && value.event == 0
                && value.status < 0
        }
        // A fault is attributed to an edge and reports a failure status.
        KIND_FAULT => value.route_identity != 0 && value.status < 0,
        // A resource record is a count, not an event on an edge, and its
        // `event` names *which* count. A bare number with no counter identity
        // is not evidence: a reader could not tell frames from operations, and
        // two runs reporting different counters would compare as equal.
        KIND_RESOURCE => {
            value.route_identity == 0
                && value.correlation == 0
                && value.status == 0
                && value.event != 0
                && value.event <= MAX_RESOURCE_COUNTER
        }
        _ => false,
    }
}

/// Whether `later` may follow `earlier` under C8.11's declared total order.
///
/// The rule is lexicographic on `(now_ns, order_class, sequence)`. It is a
/// separate function from [`valid_trace_record`] because ordering is a property
/// of a *pair* of records: a reader validates each record on arrival and the
/// order as it walks the sink, and a sink that mixed the two checks could not
/// report which of them a bad trace violated.
pub fn trace_records_ordered(
    earlier: &fabric_trace::WireTraceRecord,
    later: &fabric_trace::WireTraceRecord,
) -> bool {
    (earlier.now_ns, earlier.order_class, earlier.sequence)
        <= (later.now_ns, later.order_class, later.sequence)
}
