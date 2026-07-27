//! Native fabric-graph resource object (C8.2).
//!
//! A generation resource object that fixes every native interface, graph edge,
//! direction, QoS policy, visibility grant, interposition hop, and resource
//! ceiling of one fabric instance. It is embedded as a `KIND_RESOURCE` object
//! in a generation and authenticated by the generation's existing per-object
//! digest table, so decoding here assumes integrity already verified and
//! enforces only structural validity plus the deterministic, globally-possible
//! bounds every launch must satisfy.
//!
//! Route authority is the exact tuple (route name, full interface identity,
//! contract kind, component identity, direction). The first three fold into a
//! [`RouteEntry::route_identity`] via [`route_identity`]; the whole tuple folds
//! into a [`ParticipantEntry::grant_identity`] via [`grant_identity`]. Two
//! alternate names sharing one interface produce distinct route identities, and
//! two conflicting interfaces sharing one name likewise do — so a name, a type
//! string, or a graph observation grants nothing on its own. Only a grant
//! identity present in this table is authority.

use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMEFG\0";
include!("generated/fabric_graph.rs");

/// Largest structurally admissible graph: every table at its ceiling.
pub const MAX_BYTES: usize = HEADER_BYTES
    + MAX_SCHEMAS * SCHEMA_ENTRY_BYTES
    + MAX_ROUTES * ROUTE_ENTRY_BYTES
    + MAX_PARTICIPANTS * PARTICIPANT_ENTRY_BYTES
    + MAX_INTERPOSITION_HOPS * INTERPOSITION_ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    NonZeroReserved,
    /// A table count, declared limit, or section offset is out of range.
    BadBounds,
    /// A schema, route, or participant table is unsorted or has a duplicate
    /// identity. Sorted-and-unique is part of the format, not a convenience:
    /// it makes lookup deterministic and duplicate grants structurally
    /// impossible.
    BadOrder,
    /// An enumerated field (contract kind, direction, visibility, or a QoS
    /// policy) carries a value this version does not define.
    UnknownEnum,
    /// An index field names a table slot that does not exist.
    MissingReference,
    /// A participant's declared identity does not match the tuple it claims,
    /// or a route's does not match its (name, interface, kind) triple.
    IdentityMismatch,
    /// A declared QoS policy combination this version does not admit.
    UnsupportedQos,
    /// An interposition chain revisits a hop or exceeds the hop ceiling.
    InterpositionCycle,
    /// A declared per-graph limit, or the aggregate demand of every admitted
    /// route and participant, exceeds what the kernel could ever grant.
    Impossible,
}

/// One admitted native interface: the C8.1 full identity, the collision-checked
/// generation-local tag, and the contract kind and encoded bound they imply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaEntry {
    pub identity: [u8; 32],
    pub type_tag: u64,
    pub contract_kind: u32,
    pub max_encoded_bytes: u32,
}

/// One graph edge: a route name folded with its interface identity and
/// contract kind, plus the number of participants bound to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteEntry {
    pub route_identity: [u8; 32],
    pub schema_index: u32,
    pub contract_kind: u32,
    pub participant_count: u32,
}

/// One component's exact role on one route, with the QoS it offers or requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantEntry {
    pub grant_identity: [u8; 32],
    pub component_identity: [u8; 32],
    pub route_index: u32,
    pub direction: u32,
    pub visibility: u32,
    /// Index of the first interposition hop, or [`INTERPOSITION_NONE`].
    pub interposition_head: u32,
    pub qos: TransportQos,
}

/// One declared interposition hop and the next hop in its chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterpositionEntry {
    pub component_identity: [u8; 32],
    pub next_hop: u32,
}

/// Delivery policy for one participant on one route. Every duration is
/// nanoseconds; zero means the policy is not armed, never "unbounded".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportQos {
    pub deadline_ns: u64,
    pub lifespan_ns: u64,
    pub lease_ns: u64,
    /// KEEP_LAST depth. Always finite and at least one.
    pub history_depth: u32,
    /// Retained (durable) sample depth. Zero exactly when durability is
    /// `DURABILITY_VOLATILE`.
    pub retained_depth: u32,
    pub reliability: u8,
    pub durability: u8,
    pub liveliness: u8,
}

impl TransportQos {
    /// Fixed offered/requested compatibility truth table. There are no implicit
    /// defaults and no ROS/DDS policy: a request is satisfied only when the
    /// offer is at least as strong on every axis this version defines.
    ///
    /// - reliability: RELIABLE satisfies both; BEST_EFFORT satisfies only
    ///   BEST_EFFORT.
    /// - durability: RETAINED satisfies both; VOLATILE satisfies only VOLATILE,
    ///   and a RETAINED request additionally needs the offer's retained depth to
    ///   cover it.
    /// - liveliness: MANUAL (an explicit assertion) satisfies both; AUTOMATIC
    ///   satisfies only AUTOMATIC.
    /// - deadline / lifespan / lease: a zero request is unarmed and always
    ///   satisfied; a nonzero request needs a nonzero offer no slower than it.
    pub fn offer_satisfies(offered: &Self, requested: &Self) -> bool {
        if requested.reliability == RELIABILITY_RELIABLE as u8
            && offered.reliability != RELIABILITY_RELIABLE as u8
        {
            return false;
        }
        if requested.durability == DURABILITY_RETAINED as u8
            && (offered.durability != DURABILITY_RETAINED as u8
                || offered.retained_depth < requested.retained_depth)
        {
            return false;
        }
        if requested.liveliness == LIVELINESS_MANUAL as u8
            && offered.liveliness != LIVELINESS_MANUAL as u8
        {
            return false;
        }
        deadline_satisfied(offered.deadline_ns, requested.deadline_ns)
            && deadline_satisfied(offered.lifespan_ns, requested.lifespan_ns)
            && deadline_satisfied(offered.lease_ns, requested.lease_ns)
    }
}

/// A nonzero request needs an offer that is armed and no slower. Zero request
/// means unarmed, which any offer satisfies.
fn deadline_satisfied(offered: u64, requested: u64) -> bool {
    requested == 0 || (offered != 0 && offered <= requested)
}

/// Every per-graph resource ceiling the generation declares. These are the
/// numbers a fabric instance and its clients are admitted against; nothing at
/// runtime may raise them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphLimits {
    pub routes: u32,
    pub ingress_sources: u32,
    pub publishers: u32,
    pub subscribers: u32,
    pub clients: u32,
    pub servers: u32,
    pub sample_bytes: u32,
    pub queue_depth: u32,
    pub history_depth: u32,
    pub event_depth: u32,
    pub retained_samples: u32,
    pub retries: u32,
    pub in_flight_calls: u32,
    pub in_flight_operations: u32,
    pub buffer_pages: u32,
    pub mappings: u32,
    pub loans: u32,
    pub capability_slots: u32,
}

/// A decoded, structurally validated fabric graph. Schema, route, and
/// participant tables are sorted by identity and unique, so lookup is
/// deterministic.
#[derive(Debug, Clone, Copy)]
pub struct FabricGraph<'a> {
    bytes: &'a [u8],
    schema_count: usize,
    route_count: usize,
    participant_count: usize,
    interposition_count: usize,
    fabric_component_identity: [u8; 32],
    limits: GraphLimits,
}

impl<'a> FabricGraph<'a> {
    /// Decode and structurally validate a fabric-graph resource payload.
    ///
    /// Everything checkable without kernel ceilings happens here: header shape,
    /// exact section sizing, table ordering and uniqueness, enum admissibility,
    /// index resolution, per-entry QoS coherence, interposition acyclicity, and
    /// the declared limits against the format's own structural ceilings.
    /// [`validate_against`](Self::validate_against) then adds the kernel-ceiling
    /// and aggregate-demand arms.
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
        let total_len = u32_at(bytes, 24)? as usize;
        let schema_count = u32_at(bytes, 28)? as usize;
        let route_count = u32_at(bytes, 32)? as usize;
        let participant_count = u32_at(bytes, 36)? as usize;
        let interposition_count = u32_at(bytes, 40)? as usize;
        if u32_at(bytes, 44)? != 0 {
            return Err(DecodeError::NonZeroReserved);
        }
        if schema_count > MAX_SCHEMAS
            || route_count > MAX_ROUTES
            || participant_count > MAX_PARTICIPANTS
            || interposition_count > MAX_INTERPOSITION_HOPS
        {
            return Err(DecodeError::BadBounds);
        }
        let expected = HEADER_BYTES
            + schema_count * SCHEMA_ENTRY_BYTES
            + route_count * ROUTE_ENTRY_BYTES
            + participant_count * PARTICIPANT_ENTRY_BYTES
            + interposition_count * INTERPOSITION_ENTRY_BYTES;
        if total_len != expected || total_len != bytes.len() {
            return Err(DecodeError::BadBounds);
        }

        let fabric_component_identity: [u8; 32] = bytes[48..80].try_into().unwrap();
        if fabric_component_identity == [0; 32] {
            return Err(DecodeError::MissingReference);
        }
        let limits = GraphLimits {
            routes: u32_at(bytes, 80)?,
            ingress_sources: u32_at(bytes, 84)?,
            publishers: u32_at(bytes, 88)?,
            subscribers: u32_at(bytes, 92)?,
            clients: u32_at(bytes, 96)?,
            servers: u32_at(bytes, 100)?,
            sample_bytes: u32_at(bytes, 104)?,
            queue_depth: u32_at(bytes, 108)?,
            history_depth: u32_at(bytes, 112)?,
            event_depth: u32_at(bytes, 116)?,
            retained_samples: u32_at(bytes, 120)?,
            retries: u32_at(bytes, 124)?,
            in_flight_calls: u32_at(bytes, 128)?,
            in_flight_operations: u32_at(bytes, 132)?,
            buffer_pages: u32_at(bytes, 136)?,
            mappings: u32_at(bytes, 140)?,
            loans: u32_at(bytes, 144)?,
            capability_slots: u32_at(bytes, 148)?,
        };

        let graph = Self {
            bytes,
            schema_count,
            route_count,
            participant_count,
            interposition_count,
            fabric_component_identity,
            limits,
        };
        graph.validate_structure()?;
        Ok(graph)
    }

    fn validate_structure(&self) -> Result<(), DecodeError> {
        self.validate_declared_limits()?;
        self.validate_reserved()?;
        self.validate_schemas()?;
        self.validate_routes()?;
        self.validate_participants()?;
        self.validate_interposition()?;
        Ok(())
    }

    /// Every reserved and padding byte in every entry must be zero.
    ///
    /// The resource is authenticated by the generation's per-object digest, so
    /// a field the decoder skips would let two byte-distinct resources with
    /// different digests decode to the identical graph. Requiring canonical
    /// zeros keeps "one authenticated resource fixes the graph" literally true
    /// and preserves the headroom these fields were declared for.
    fn validate_reserved(&self) -> Result<(), DecodeError> {
        let ranges = [
            (self.route_offset(), self.route_count, ROUTE_ENTRY_BYTES, 44),
            (
                self.participant_offset(),
                self.participant_count,
                PARTICIPANT_ENTRY_BYTES,
                115,
            ),
            (
                self.interposition_offset(),
                self.interposition_count,
                INTERPOSITION_ENTRY_BYTES,
                36,
            ),
        ];
        for (base, count, stride, reserved_start) in ranges {
            for index in 0..count {
                let offset = base + index * stride + reserved_start;
                let end = base + index * stride + stride;
                if self
                    .bytes
                    .get(offset..end)
                    .ok_or(DecodeError::Truncated)?
                    .iter()
                    .any(|byte| *byte != 0)
                {
                    return Err(DecodeError::NonZeroReserved);
                }
            }
        }
        Ok(())
    }

    /// A declared limit above the format's own structural ceiling can never be
    /// honoured and would let a graph name a table no decoder can size.
    fn validate_declared_limits(&self) -> Result<(), DecodeError> {
        let limits = &self.limits;
        if limits.routes as usize > MAX_ROUTES
            || limits.ingress_sources as usize > MAX_INGRESS_SOURCES
            || limits.publishers as usize > MAX_PARTICIPANTS
            || limits.subscribers as usize > MAX_PARTICIPANTS
            || limits.clients as usize > MAX_PARTICIPANTS
            || limits.servers as usize > MAX_PARTICIPANTS
            || limits.sample_bytes > LIMIT_SAMPLE_BYTES
            || limits.queue_depth > LIMIT_QUEUE_DEPTH
            || limits.history_depth > LIMIT_HISTORY_DEPTH
            || limits.event_depth > LIMIT_EVENT_DEPTH
            || limits.retained_samples > LIMIT_RETAINED_SAMPLES
            || limits.retries > LIMIT_RETRIES
            || limits.in_flight_calls > LIMIT_IN_FLIGHT
            || limits.in_flight_operations > LIMIT_IN_FLIGHT
            || limits.capability_slots > LIMIT_CAPABILITY_SLOTS
        {
            return Err(DecodeError::Impossible);
        }
        // The graph's own tables must fit inside the limits it declares: a
        // graph that admits more routes than it budgets for is over-committed
        // at rest, before a single participant launches.
        if self.route_count > limits.routes as usize {
            return Err(DecodeError::Impossible);
        }
        Ok(())
    }

    fn validate_schemas(&self) -> Result<(), DecodeError> {
        let mut previous = [0u8; 32];
        for index in 0..self.schema_count {
            let entry = self.schema(index).ok_or(DecodeError::Truncated)?;
            if entry.identity == [0; 32] || (index > 0 && entry.identity <= previous) {
                return Err(DecodeError::BadOrder);
            }
            if !is_contract_kind(entry.contract_kind) {
                return Err(DecodeError::UnknownEnum);
            }
            // A zero tag is the "absent" value in the retained C7 descriptor,
            // so an admitted schema may never derive one.
            if entry.type_tag == 0 {
                return Err(DecodeError::IdentityMismatch);
            }
            if entry.max_encoded_bytes == 0 {
                return Err(DecodeError::BadBounds);
            }
            previous = entry.identity;
        }
        // Distinct full identities may not share one generation-local tag:
        // the tag is a lookup key, and a collision would make it ambiguous.
        for index in 0..self.schema_count {
            let entry = self.schema(index).ok_or(DecodeError::Truncated)?;
            for other in (index + 1)..self.schema_count {
                let candidate = self.schema(other).ok_or(DecodeError::Truncated)?;
                if candidate.type_tag == entry.type_tag {
                    return Err(DecodeError::IdentityMismatch);
                }
            }
        }
        Ok(())
    }

    fn validate_routes(&self) -> Result<(), DecodeError> {
        let mut previous = [0u8; 32];
        let mut bound_participants: u32 = 0;
        for index in 0..self.route_count {
            let entry = self.route(index).ok_or(DecodeError::Truncated)?;
            if entry.route_identity == [0; 32] || (index > 0 && entry.route_identity <= previous) {
                return Err(DecodeError::BadOrder);
            }
            let schema = self
                .schema(entry.schema_index as usize)
                .ok_or(DecodeError::MissingReference)?;
            if !is_contract_kind(entry.contract_kind) {
                return Err(DecodeError::UnknownEnum);
            }
            // The route's contract kind is authority, so it may not disagree
            // with the interface it names.
            if entry.contract_kind != schema.contract_kind {
                return Err(DecodeError::IdentityMismatch);
            }
            if entry.participant_count == 0 {
                return Err(DecodeError::BadBounds);
            }
            bound_participants = bound_participants
                .checked_add(entry.participant_count)
                .ok_or(DecodeError::BadBounds)?;
            previous = entry.route_identity;
        }
        // Every declared participant belongs to exactly one route, and every
        // route's count is honest: the sum must equal the table length.
        if bound_participants as usize != self.participant_count {
            return Err(DecodeError::BadBounds);
        }
        Ok(())
    }

    fn validate_participants(&self) -> Result<(), DecodeError> {
        let mut previous = [0u8; 32];
        let mut per_route = [0u32; MAX_ROUTES];
        for index in 0..self.participant_count {
            let entry = self.participant(index).ok_or(DecodeError::Truncated)?;
            // Sorted-and-unique by grant identity is what makes a duplicate
            // grant structurally impossible rather than merely discouraged.
            if entry.grant_identity == [0; 32] || (index > 0 && entry.grant_identity <= previous) {
                return Err(DecodeError::BadOrder);
            }
            if entry.component_identity == [0; 32] {
                return Err(DecodeError::MissingReference);
            }
            let route = self
                .route(entry.route_index as usize)
                .ok_or(DecodeError::MissingReference)?;
            if !is_visibility(entry.visibility) {
                return Err(DecodeError::UnknownEnum);
            }
            if !direction_admits(route.contract_kind, entry.direction) {
                return Err(DecodeError::UnknownEnum);
            }
            // The declared grant identity must be exactly the fold of the
            // authority tuple. A participant cannot claim one route's identity
            // while binding to another route, direction, or component.
            let expected = grant_identity(
                &route.route_identity,
                &entry.component_identity,
                entry.direction,
            );
            if entry.grant_identity != expected {
                return Err(DecodeError::IdentityMismatch);
            }
            validate_qos(&entry.qos, &self.limits)?;
            if entry.interposition_head != INTERPOSITION_NONE
                && entry.interposition_head as usize >= self.interposition_count
            {
                return Err(DecodeError::MissingReference);
            }
            per_route[entry.route_index as usize] += 1;
            previous = entry.grant_identity;
        }
        for (index, counted) in per_route.iter().enumerate().take(self.route_count) {
            let route = self.route(index).ok_or(DecodeError::Truncated)?;
            if *counted != route.participant_count {
                return Err(DecodeError::BadBounds);
            }
        }
        Ok(())
    }

    /// Walk each participant's interposition chain. A chain must terminate
    /// within the hop ceiling, may not revisit a hop, and may not name the
    /// participant's own component — a self-hop is a bypass dressed as a proxy.
    fn validate_interposition(&self) -> Result<(), DecodeError> {
        for index in 0..self.interposition_count {
            let hop = self.interposition(index).ok_or(DecodeError::Truncated)?;
            if hop.component_identity == [0; 32] {
                return Err(DecodeError::MissingReference);
            }
            if hop.next_hop != INTERPOSITION_NONE
                && hop.next_hop as usize >= self.interposition_count
            {
                return Err(DecodeError::MissingReference);
            }
        }
        for index in 0..self.participant_count {
            let entry = self.participant(index).ok_or(DecodeError::Truncated)?;
            let mut visited = [false; MAX_INTERPOSITION_HOPS];
            let mut cursor = entry.interposition_head;
            let mut steps = 0usize;
            while cursor != INTERPOSITION_NONE {
                let slot = cursor as usize;
                if slot >= self.interposition_count || visited[slot] {
                    return Err(DecodeError::InterpositionCycle);
                }
                visited[slot] = true;
                steps += 1;
                if steps > MAX_INTERPOSITION_HOPS {
                    return Err(DecodeError::InterpositionCycle);
                }
                let hop = self.interposition(slot).ok_or(DecodeError::Truncated)?;
                if hop.component_identity == entry.component_identity {
                    return Err(DecodeError::InterpositionCycle);
                }
                cursor = hop.next_hop;
            }
        }
        Ok(())
    }

    pub fn schema_count(&self) -> usize {
        self.schema_count
    }

    pub fn route_count(&self) -> usize {
        self.route_count
    }

    pub fn participant_count(&self) -> usize {
        self.participant_count
    }

    pub fn interposition_count(&self) -> usize {
        self.interposition_count
    }

    /// The component that runs this fabric instance. Only it receives the
    /// control plane; every other participant gets its exact route role.
    pub fn fabric_component_identity(&self) -> [u8; 32] {
        self.fabric_component_identity
    }

    pub fn limits(&self) -> GraphLimits {
        self.limits
    }

    pub fn schema(&self, index: usize) -> Option<SchemaEntry> {
        (index < self.schema_count).then(|| {
            let offset = HEADER_BYTES + index * SCHEMA_ENTRY_BYTES;
            let entry = &self.bytes[offset..offset + SCHEMA_ENTRY_BYTES];
            SchemaEntry {
                identity: entry[..32].try_into().unwrap(),
                type_tag: u64::from_le_bytes(entry[32..40].try_into().unwrap()),
                contract_kind: u32::from_le_bytes(entry[40..44].try_into().unwrap()),
                max_encoded_bytes: u32::from_le_bytes(entry[44..48].try_into().unwrap()),
            }
        })
    }

    pub fn route(&self, index: usize) -> Option<RouteEntry> {
        (index < self.route_count).then(|| {
            let offset = self.route_offset() + index * ROUTE_ENTRY_BYTES;
            let entry = &self.bytes[offset..offset + ROUTE_ENTRY_BYTES];
            RouteEntry {
                route_identity: entry[..32].try_into().unwrap(),
                schema_index: u32::from_le_bytes(entry[32..36].try_into().unwrap()),
                contract_kind: u32::from_le_bytes(entry[36..40].try_into().unwrap()),
                participant_count: u32::from_le_bytes(entry[40..44].try_into().unwrap()),
            }
        })
    }

    pub fn participant(&self, index: usize) -> Option<ParticipantEntry> {
        (index < self.participant_count).then(|| {
            let offset = self.participant_offset() + index * PARTICIPANT_ENTRY_BYTES;
            let entry = &self.bytes[offset..offset + PARTICIPANT_ENTRY_BYTES];
            ParticipantEntry {
                grant_identity: entry[..32].try_into().unwrap(),
                component_identity: entry[32..64].try_into().unwrap(),
                route_index: u32::from_le_bytes(entry[64..68].try_into().unwrap()),
                direction: u32::from_le_bytes(entry[68..72].try_into().unwrap()),
                visibility: u32::from_le_bytes(entry[72..76].try_into().unwrap()),
                interposition_head: u32::from_le_bytes(entry[76..80].try_into().unwrap()),
                qos: TransportQos {
                    deadline_ns: u64::from_le_bytes(entry[80..88].try_into().unwrap()),
                    lifespan_ns: u64::from_le_bytes(entry[88..96].try_into().unwrap()),
                    lease_ns: u64::from_le_bytes(entry[96..104].try_into().unwrap()),
                    history_depth: u32::from_le_bytes(entry[104..108].try_into().unwrap()),
                    retained_depth: u32::from_le_bytes(entry[108..112].try_into().unwrap()),
                    reliability: entry[112],
                    durability: entry[113],
                    liveliness: entry[114],
                },
            }
        })
    }

    pub fn interposition(&self, index: usize) -> Option<InterpositionEntry> {
        (index < self.interposition_count).then(|| {
            let offset = self.interposition_offset() + index * INTERPOSITION_ENTRY_BYTES;
            let entry = &self.bytes[offset..offset + INTERPOSITION_ENTRY_BYTES];
            InterpositionEntry {
                component_identity: entry[..32].try_into().unwrap(),
                next_hop: u32::from_le_bytes(entry[32..36].try_into().unwrap()),
            }
        })
    }

    /// Return the participant entry for an exact authority tuple, or `None`
    /// when the graph declares no such edge (deny by default).
    pub fn participant_for(&self, grant_identity: &[u8; 32]) -> Option<ParticipantEntry> {
        (0..self.participant_count)
            .filter_map(|index| self.participant(index))
            .find(|entry| entry.grant_identity == *grant_identity)
    }

    fn route_offset(&self) -> usize {
        HEADER_BYTES + self.schema_count * SCHEMA_ENTRY_BYTES
    }

    fn participant_offset(&self) -> usize {
        self.route_offset() + self.route_count * ROUTE_ENTRY_BYTES
    }

    fn interposition_offset(&self) -> usize {
        self.participant_offset() + self.participant_count * PARTICIPANT_ENTRY_BYTES
    }

    /// Reject any graph that can never be satisfied under the fixed kernel
    /// ceilings, and any graph whose participants, all live at once, would
    /// exceed a limit the generation itself declared.
    ///
    /// Two classes, mirroring the C7.3 budget validator. Per-limit: a declared
    /// ceiling no kernel table could ever reach. Aggregate: the demand of every
    /// admitted route and participant summed at its declared peak. The
    /// aggregate rule means a graph that validates is one the fabric can honour
    /// in full — every declared participant can peak at once — rather than
    /// first-come-first-served, where a late-starting subscriber would fail at
    /// runtime despite holding a route the generation promised it.
    pub fn validate_against(
        &self,
        max_wait_sources: u32,
        max_capability_slots: u32,
        max_total_pages: u32,
        max_mappings: u32,
        max_loans: u32,
        max_message_bytes: u32,
    ) -> Result<(), DecodeError> {
        let limits = &self.limits;
        if limits.ingress_sources > max_wait_sources
            || limits.capability_slots > max_capability_slots
            || limits.buffer_pages > max_total_pages
            || limits.mappings > max_mappings
            || limits.loans > max_loans
        {
            return Err(DecodeError::Impossible);
        }
        // The fabric brokers one downstream loan per matched subscriber and one
        // mapping per live loan, so a graph promising more subscribers than it
        // budgets loans or mappings for cannot deliver its own fan-out.
        if limits.subscribers > limits.loans || limits.subscribers > limits.mappings {
            return Err(DecodeError::Impossible);
        }
        // A sample larger than the control-message bound must travel as a C7
        // shared-buffer loan, so the page budget has to be able to hold one.
        if limits.sample_bytes > max_message_bytes && limits.buffer_pages == 0 {
            return Err(DecodeError::Impossible);
        }

        let mut publishers: u32 = 0;
        let mut subscribers: u32 = 0;
        let mut clients: u32 = 0;
        let mut servers: u32 = 0;
        let mut ingress: u32 = 0;
        for index in 0..self.participant_count {
            let entry = self.participant(index).ok_or(DecodeError::Truncated)?;
            match entry.direction {
                d if d == DIRECTION_PUBLISH => publishers = publishers.saturating_add(1),
                d if d == DIRECTION_SUBSCRIBE => subscribers = subscribers.saturating_add(1),
                d if d == DIRECTION_CLIENT => clients = clients.saturating_add(1),
                d if d == DIRECTION_SERVER => servers = servers.saturating_add(1),
                _ => return Err(DecodeError::UnknownEnum),
            }
            // Every edge that delivers *into* the fabric is a live wake source
            // it must register. A graph the fabric cannot block on would have
            // to poll, which B2 removed from the system.
            if is_fabric_ingress(entry.direction) {
                ingress = ingress.saturating_add(1);
            }
            if entry.qos.retained_depth > limits.retained_samples
                || entry.qos.history_depth > limits.history_depth
            {
                return Err(DecodeError::Impossible);
            }
        }
        if publishers > limits.publishers
            || subscribers > limits.subscribers
            || clients > limits.clients
            || servers > limits.servers
        {
            return Err(DecodeError::Impossible);
        }
        if ingress > limits.ingress_sources || ingress > max_wait_sources {
            return Err(DecodeError::Impossible);
        }
        // Each admitted schema must fit the declared sample bound; otherwise a
        // valid message of an admitted type could never be carried.
        for index in 0..self.schema_count {
            let schema = self.schema(index).ok_or(DecodeError::Truncated)?;
            if schema.max_encoded_bytes > limits.sample_bytes {
                return Err(DecodeError::Impossible);
            }
        }
        Ok(())
    }

    /// Whether every matched publisher/subscriber and client/server pair on
    /// every route has a compatible offered/requested QoS. An incompatible pair
    /// is admissible data — C8.5 reports it as a structured event — so this is
    /// a query, not a decode error.
    pub fn all_pairs_qos_compatible(&self) -> bool {
        for route in 0..self.route_count {
            for left in 0..self.participant_count {
                let Some(offer) = self.participant(left) else {
                    return false;
                };
                if offer.route_index as usize != route || !is_offering(offer.direction) {
                    continue;
                }
                for right in 0..self.participant_count {
                    let Some(request) = self.participant(right) else {
                        return false;
                    };
                    if request.route_index as usize != route || is_offering(request.direction) {
                        continue;
                    }
                    if !TransportQos::offer_satisfies(&offer.qos, &request.qos) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// A route's authority identity: the fold of its name, the full C8.1 interface
/// identity, and the contract kind. Alternate names over one interface and
/// conflicting interfaces under one name are therefore distinct routes.
pub fn route_identity(name: &str, interface_identity: &[u8; 32], contract_kind: u32) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ROUTE_DOMAIN);
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update(interface_identity);
    hasher.update(&contract_kind.to_le_bytes());
    hasher.finalize()
}

/// A participant's authority identity: the fold of its route identity, its
/// component identity, and its direction. Possession of the route name, the
/// type, or a graph observation derives nothing without the component identity.
pub fn grant_identity(
    route_identity: &[u8; 32],
    component_identity: &[u8; 32],
    direction: u32,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(GRANT_DOMAIN);
    hasher.update(route_identity);
    hasher.update(component_identity);
    hasher.update(&direction.to_le_bytes());
    hasher.finalize()
}

/// Stable component identity derived from a component name, matching the
/// builder's derivation. Distinct from the C7.3 shared-buffer holder identity:
/// the two authority domains are separate on purpose.
pub fn component_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(COMPONENT_DOMAIN);
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn is_contract_kind(value: u32) -> bool {
    matches!(
        value,
        CONTRACT_KIND_STREAM | CONTRACT_KIND_CALL | CONTRACT_KIND_OPERATION
    )
}

fn is_visibility(value: u32) -> bool {
    matches!(value, VISIBILITY_PRIVATE | VISIBILITY_GRAPH)
}

/// Which directions a contract kind admits. A stream has publishers and
/// subscribers; a call and an operation have clients and servers. Mixing them
/// is not a policy choice, it is a malformed graph.
fn direction_admits(contract_kind: u32, direction: u32) -> bool {
    match contract_kind {
        CONTRACT_KIND_STREAM => matches!(direction, DIRECTION_PUBLISH | DIRECTION_SUBSCRIBE),
        CONTRACT_KIND_CALL | CONTRACT_KIND_OPERATION => {
            matches!(direction, DIRECTION_CLIENT | DIRECTION_SERVER)
        }
        _ => false,
    }
}

/// A direction that offers data (the QoS-offering side of a match).
fn is_offering(direction: u32) -> bool {
    direction == DIRECTION_PUBLISH || direction == DIRECTION_SERVER
}

/// A direction that delivers into the fabric, consuming one live wait source.
fn is_fabric_ingress(direction: u32) -> bool {
    direction == DIRECTION_PUBLISH || direction == DIRECTION_CLIENT
}

fn validate_qos(qos: &TransportQos, limits: &GraphLimits) -> Result<(), DecodeError> {
    if qos.reliability as u32 != RELIABILITY_BEST_EFFORT
        && qos.reliability as u32 != RELIABILITY_RELIABLE
    {
        return Err(DecodeError::UnknownEnum);
    }
    if qos.durability as u32 != DURABILITY_VOLATILE && qos.durability as u32 != DURABILITY_RETAINED
    {
        return Err(DecodeError::UnknownEnum);
    }
    if qos.liveliness as u32 != LIVELINESS_AUTOMATIC && qos.liveliness as u32 != LIVELINESS_MANUAL {
        return Err(DecodeError::UnknownEnum);
    }
    // KEEP_LAST is the only history policy this version defines, so the depth
    // is always finite and at least one; "keep all" would be unbounded.
    if qos.history_depth == 0 || qos.history_depth > limits.history_depth {
        return Err(DecodeError::UnsupportedQos);
    }
    // Retained depth and durability are one fact stated twice; disagreement is
    // a malformed policy, not a defaulted one.
    let retained = qos.durability as u32 == DURABILITY_RETAINED;
    if retained == (qos.retained_depth == 0) {
        return Err(DecodeError::UnsupportedQos);
    }
    if qos.retained_depth > limits.retained_samples {
        return Err(DecodeError::UnsupportedQos);
    }
    // A lifespan shorter than the deadline expires every sample before its
    // deadline can be met: the two policies would permanently contradict.
    if qos.deadline_ns != 0 && qos.lifespan_ns != 0 && qos.lifespan_ns < qos.deadline_ns {
        return Err(DecodeError::UnsupportedQos);
    }
    // MANUAL liveliness is an explicit periodic assertion, so it needs a lease
    // to assert against; AUTOMATIC is peer-death driven and must not carry one.
    if (qos.liveliness as u32 == LIVELINESS_MANUAL) == (qos.lease_ns == 0) {
        return Err(DecodeError::UnsupportedQos);
    }
    Ok(())
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

    /// A graph builder that packs exactly what the decoder reads, so a test can
    /// mutate one field without hand-computing offsets.
    struct Builder {
        fabric: [u8; 32],
        limits: GraphLimits,
        schemas: Vec<SchemaEntry>,
        routes: Vec<RouteEntry>,
        participants: Vec<ParticipantEntry>,
        hops: Vec<InterpositionEntry>,
    }

    fn base_limits() -> GraphLimits {
        GraphLimits {
            routes: 8,
            ingress_sources: 4,
            publishers: 4,
            subscribers: 4,
            clients: 4,
            servers: 4,
            sample_bytes: 65536,
            queue_depth: 8,
            history_depth: 8,
            event_depth: 8,
            retained_samples: 8,
            retries: 4,
            in_flight_calls: 4,
            in_flight_operations: 4,
            buffer_pages: 16,
            mappings: 8,
            loans: 8,
            capability_slots: 16,
        }
    }

    fn volatile_qos() -> TransportQos {
        TransportQos {
            deadline_ns: 0,
            lifespan_ns: 0,
            lease_ns: 0,
            history_depth: 1,
            retained_depth: 0,
            reliability: RELIABILITY_BEST_EFFORT as u8,
            durability: DURABILITY_VOLATILE as u8,
            liveliness: LIVELINESS_AUTOMATIC as u8,
        }
    }

    fn reliable_qos() -> TransportQos {
        TransportQos {
            reliability: RELIABILITY_RELIABLE as u8,
            ..volatile_qos()
        }
    }

    impl Builder {
        fn new() -> Self {
            Self {
                fabric: component_identity("fabric"),
                limits: base_limits(),
                schemas: Vec::new(),
                routes: Vec::new(),
                participants: Vec::new(),
                hops: Vec::new(),
            }
        }

        fn schema(&mut self, identity: [u8; 32], tag: u64, kind: u32) -> usize {
            self.schemas.push(SchemaEntry {
                identity,
                type_tag: tag,
                contract_kind: kind,
                max_encoded_bytes: 1024,
            });
            self.schemas.sort_by_key(|entry| entry.identity);
            self.schemas
                .iter()
                .position(|entry| entry.identity == identity)
                .unwrap()
        }

        fn route(&mut self, name: &str, schema_index: usize) -> usize {
            let schema = self.schemas[schema_index];
            self.routes.push(RouteEntry {
                route_identity: route_identity(name, &schema.identity, schema.contract_kind),
                schema_index: schema_index as u32,
                contract_kind: schema.contract_kind,
                participant_count: 0,
            });
            self.routes.sort_by_key(|entry| entry.route_identity);
            self.routes.len() - 1
        }

        fn participant(
            &mut self,
            route_index: usize,
            component: &str,
            direction: u32,
            qos: TransportQos,
        ) {
            let route = self.routes[route_index];
            let component_identity = component_identity(component);
            self.routes[route_index].participant_count += 1;
            self.participants.push(ParticipantEntry {
                grant_identity: grant_identity(
                    &route.route_identity,
                    &component_identity,
                    direction,
                ),
                component_identity,
                route_index: route_index as u32,
                direction,
                visibility: VISIBILITY_GRAPH,
                interposition_head: INTERPOSITION_NONE,
                qos,
            });
        }

        fn encode(&self) -> Vec<u8> {
            let mut schemas = self.schemas.clone();
            schemas.sort_by_key(|entry| entry.identity);
            let mut participants = self.participants.clone();
            participants.sort_by_key(|entry| entry.grant_identity);

            let total_len = HEADER_BYTES
                + schemas.len() * SCHEMA_ENTRY_BYTES
                + self.routes.len() * ROUTE_ENTRY_BYTES
                + participants.len() * PARTICIPANT_ENTRY_BYTES
                + self.hops.len() * INTERPOSITION_ENTRY_BYTES;
            let mut bytes = alloc::vec![0u8; total_len];
            bytes[..8].copy_from_slice(&MAGIC);
            bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
            bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
            bytes[24..28].copy_from_slice(&(total_len as u32).to_le_bytes());
            bytes[28..32].copy_from_slice(&(schemas.len() as u32).to_le_bytes());
            bytes[32..36].copy_from_slice(&(self.routes.len() as u32).to_le_bytes());
            bytes[36..40].copy_from_slice(&(participants.len() as u32).to_le_bytes());
            bytes[40..44].copy_from_slice(&(self.hops.len() as u32).to_le_bytes());
            bytes[48..80].copy_from_slice(&self.fabric);
            let limits = [
                self.limits.routes,
                self.limits.ingress_sources,
                self.limits.publishers,
                self.limits.subscribers,
                self.limits.clients,
                self.limits.servers,
                self.limits.sample_bytes,
                self.limits.queue_depth,
                self.limits.history_depth,
                self.limits.event_depth,
                self.limits.retained_samples,
                self.limits.retries,
                self.limits.in_flight_calls,
                self.limits.in_flight_operations,
                self.limits.buffer_pages,
                self.limits.mappings,
                self.limits.loans,
                self.limits.capability_slots,
            ];
            for (index, value) in limits.iter().enumerate() {
                let offset = 80 + index * 4;
                bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }

            let mut cursor = HEADER_BYTES;
            for entry in &schemas {
                bytes[cursor..cursor + 32].copy_from_slice(&entry.identity);
                bytes[cursor + 32..cursor + 40].copy_from_slice(&entry.type_tag.to_le_bytes());
                bytes[cursor + 40..cursor + 44].copy_from_slice(&entry.contract_kind.to_le_bytes());
                bytes[cursor + 44..cursor + 48]
                    .copy_from_slice(&entry.max_encoded_bytes.to_le_bytes());
                cursor += SCHEMA_ENTRY_BYTES;
            }
            for entry in &self.routes {
                bytes[cursor..cursor + 32].copy_from_slice(&entry.route_identity);
                bytes[cursor + 32..cursor + 36].copy_from_slice(&entry.schema_index.to_le_bytes());
                bytes[cursor + 36..cursor + 40].copy_from_slice(&entry.contract_kind.to_le_bytes());
                bytes[cursor + 40..cursor + 44]
                    .copy_from_slice(&entry.participant_count.to_le_bytes());
                cursor += ROUTE_ENTRY_BYTES;
            }
            for entry in &participants {
                bytes[cursor..cursor + 32].copy_from_slice(&entry.grant_identity);
                bytes[cursor + 32..cursor + 64].copy_from_slice(&entry.component_identity);
                bytes[cursor + 64..cursor + 68].copy_from_slice(&entry.route_index.to_le_bytes());
                bytes[cursor + 68..cursor + 72].copy_from_slice(&entry.direction.to_le_bytes());
                bytes[cursor + 72..cursor + 76].copy_from_slice(&entry.visibility.to_le_bytes());
                bytes[cursor + 76..cursor + 80]
                    .copy_from_slice(&entry.interposition_head.to_le_bytes());
                bytes[cursor + 80..cursor + 88]
                    .copy_from_slice(&entry.qos.deadline_ns.to_le_bytes());
                bytes[cursor + 88..cursor + 96]
                    .copy_from_slice(&entry.qos.lifespan_ns.to_le_bytes());
                bytes[cursor + 96..cursor + 104].copy_from_slice(&entry.qos.lease_ns.to_le_bytes());
                bytes[cursor + 104..cursor + 108]
                    .copy_from_slice(&entry.qos.history_depth.to_le_bytes());
                bytes[cursor + 108..cursor + 112]
                    .copy_from_slice(&entry.qos.retained_depth.to_le_bytes());
                bytes[cursor + 112] = entry.qos.reliability;
                bytes[cursor + 113] = entry.qos.durability;
                bytes[cursor + 114] = entry.qos.liveliness;
                cursor += PARTICIPANT_ENTRY_BYTES;
            }
            for entry in &self.hops {
                bytes[cursor..cursor + 32].copy_from_slice(&entry.component_identity);
                bytes[cursor + 32..cursor + 36].copy_from_slice(&entry.next_hop.to_le_bytes());
                cursor += INTERPOSITION_ENTRY_BYTES;
            }
            assert_eq!(cursor, total_len);
            bytes
        }
    }

    /// One stream route with a publisher and a subscriber: the smallest graph
    /// that exercises matching, ingress accounting, and both directions.
    fn stream_graph() -> Builder {
        let mut builder = Builder::new();
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        let route = builder.route("telemetry", schema);
        builder.participant(route, "producer", DIRECTION_PUBLISH, volatile_qos());
        builder.participant(route, "consumer", DIRECTION_SUBSCRIBE, volatile_qos());
        builder
    }

    /// Kernel ceilings the live path passes: SYS_WAIT sources, MAX_CAPS,
    /// MAX_TOTAL_PAGES, MAX_MAPPINGS, MAX_LOANS, MAX_MSG.
    fn check(graph: &FabricGraph<'_>) -> Result<(), DecodeError> {
        graph.validate_against(8, 64, 256, 64, 64, 64)
    }

    #[test]
    fn decodes_a_well_formed_graph_and_resolves_authority() {
        let bytes = stream_graph().encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert_eq!(graph.schema_count(), 1);
        assert_eq!(graph.route_count(), 1);
        assert_eq!(graph.participant_count(), 2);
        assert_eq!(
            graph.fabric_component_identity(),
            component_identity("fabric")
        );
        check(&graph).expect("satisfiable under kernel ceilings");

        let route = graph.route(0).expect("route");
        let publisher = grant_identity(
            &route.route_identity,
            &component_identity("producer"),
            DIRECTION_PUBLISH,
        );
        let entry = graph.participant_for(&publisher).expect("publisher edge");
        assert_eq!(entry.direction, DIRECTION_PUBLISH);
        // The same component in the other direction is a different authority.
        let flipped = grant_identity(
            &route.route_identity,
            &component_identity("producer"),
            DIRECTION_SUBSCRIBE,
        );
        assert_eq!(graph.participant_for(&flipped), None);
        // An ungranted component derives nothing from the route name.
        let outsider = grant_identity(
            &route.route_identity,
            &component_identity("intruder"),
            DIRECTION_SUBSCRIBE,
        );
        assert_eq!(graph.participant_for(&outsider), None);
    }

    #[test]
    fn alternate_names_and_conflicting_types_stay_distinct_authority() {
        // Same interface, two route names: distinct route identities.
        let left = route_identity("telemetry", &[0x11; 32], CONTRACT_KIND_STREAM);
        let right = route_identity("diagnostics", &[0x11; 32], CONTRACT_KIND_STREAM);
        assert_ne!(left, right);
        // Same name, two interfaces: also distinct.
        let other = route_identity("telemetry", &[0x22; 32], CONTRACT_KIND_STREAM);
        assert_ne!(left, other);
        // Same name and interface, different contract kind: also distinct.
        let as_call = route_identity("telemetry", &[0x11; 32], CONTRACT_KIND_CALL);
        assert_ne!(left, as_call);
    }

    #[test]
    fn missing_references_fail_closed() {
        // Route naming a schema slot that does not exist.
        let mut builder = stream_graph();
        builder.routes[0].schema_index = 7;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::MissingReference)
        ));

        // Participant naming a route slot that does not exist. The route index
        // is re-read by the decoder, so an out-of-range value must be caught
        // before it is used to index the route table.
        let mut builder = stream_graph();
        builder.participants[0].route_index = 4;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::MissingReference)
        ));

        // A zero fabric component identity names no component at all.
        let mut builder = stream_graph();
        builder.fabric = [0; 32];
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::MissingReference)
        ));
    }

    #[test]
    fn duplicate_and_unsorted_grants_fail_closed() {
        let mut builder = stream_graph();
        // A duplicate grant would let one component hold one edge twice.
        let duplicate = builder.participants[0];
        builder.participants.push(duplicate);
        builder.routes[0].participant_count += 1;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));

        // Unsorted participants: the builder sorts, so write the table by hand.
        let builder = stream_graph();
        let mut bytes = builder.encode();
        let base = HEADER_BYTES + SCHEMA_ENTRY_BYTES + ROUTE_ENTRY_BYTES;
        let (first, second) = (base, base + PARTICIPANT_ENTRY_BYTES);
        for index in 0..PARTICIPANT_ENTRY_BYTES {
            bytes.swap(first + index, second + index);
        }
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    #[test]
    fn forged_grant_identity_fails_closed() {
        // A participant claiming an identity it did not derive from its own
        // tuple: the exact "possession of a name grants authority" attack.
        let mut builder = stream_graph();
        builder.participants[1].grant_identity = [0xFF; 32];
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::IdentityMismatch)
        ));
    }

    #[test]
    fn contract_kind_and_direction_must_agree() {
        // A stream route cannot host a client.
        let mut builder = Builder::new();
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        let route = builder.route("telemetry", schema);
        builder.participant(route, "producer", DIRECTION_PUBLISH, volatile_qos());
        builder.participant(route, "caller", DIRECTION_CLIENT, volatile_qos());
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnknownEnum)
        ));

        // A route whose kind disagrees with the interface it names.
        let mut builder = stream_graph();
        builder.routes[0].contract_kind = CONTRACT_KIND_CALL;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::IdentityMismatch)
        ));

        // An undefined direction value.
        let mut builder = stream_graph();
        builder.participants[0].direction = 9;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnknownEnum)
        ));
    }

    #[test]
    fn a_call_route_admits_client_and_server() {
        let mut builder = Builder::new();
        let schema = builder.schema([0x33; 32], 0xBBBB, CONTRACT_KIND_CALL);
        let route = builder.route("parameters", schema);
        builder.participant(route, "caller", DIRECTION_CLIENT, volatile_qos());
        builder.participant(route, "server", DIRECTION_SERVER, volatile_qos());
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        check(&graph).expect("satisfiable");
        assert!(graph.all_pairs_qos_compatible());
    }

    #[test]
    fn unsupported_qos_fails_closed() {
        // An undefined reliability value.
        let mut builder = stream_graph();
        builder.participants[0].qos.reliability = 7;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnknownEnum)
        ));

        // Zero KEEP_LAST depth would be an unbounded history.
        let mut builder = stream_graph();
        builder.participants[0].qos.history_depth = 0;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));

        // VOLATILE durability with a retained depth states two facts at once.
        let mut builder = stream_graph();
        builder.participants[0].qos.retained_depth = 4;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));

        // RETAINED durability without a depth is equally incoherent.
        let mut builder = stream_graph();
        builder.participants[0].qos.durability = DURABILITY_RETAINED as u8;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));

        // MANUAL liveliness needs a lease to assert against.
        let mut builder = stream_graph();
        builder.participants[0].qos.liveliness = LIVELINESS_MANUAL as u8;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));

        // AUTOMATIC liveliness must not carry one.
        let mut builder = stream_graph();
        builder.participants[0].qos.lease_ns = 1_000;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));

        // A lifespan shorter than the deadline permanently contradicts it.
        let mut builder = stream_graph();
        builder.participants[0].qos.deadline_ns = 2_000;
        builder.participants[0].qos.lifespan_ns = 1_000;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::UnsupportedQos)
        ));
    }

    #[test]
    fn offered_requested_compatibility_is_a_fixed_truth_table() {
        let best_effort = volatile_qos();
        let reliable = reliable_qos();
        // RELIABLE satisfies both requests; BEST_EFFORT satisfies only itself.
        assert!(TransportQos::offer_satisfies(&reliable, &best_effort));
        assert!(TransportQos::offer_satisfies(&reliable, &reliable));
        assert!(TransportQos::offer_satisfies(&best_effort, &best_effort));
        assert!(!TransportQos::offer_satisfies(&best_effort, &reliable));

        // RETAINED satisfies a VOLATILE request; VOLATILE cannot satisfy a
        // RETAINED one, and a retained offer must cover the requested depth.
        let retained = TransportQos {
            durability: DURABILITY_RETAINED as u8,
            retained_depth: 4,
            ..volatile_qos()
        };
        let deeper = TransportQos {
            retained_depth: 8,
            ..retained
        };
        assert!(TransportQos::offer_satisfies(&retained, &best_effort));
        assert!(!TransportQos::offer_satisfies(&best_effort, &retained));
        assert!(TransportQos::offer_satisfies(&deeper, &retained));
        assert!(!TransportQos::offer_satisfies(&retained, &deeper));

        // MANUAL liveliness satisfies both; AUTOMATIC satisfies only itself.
        let manual = TransportQos {
            liveliness: LIVELINESS_MANUAL as u8,
            lease_ns: 1_000,
            ..volatile_qos()
        };
        assert!(TransportQos::offer_satisfies(&manual, &best_effort));
        assert!(!TransportQos::offer_satisfies(&best_effort, &manual));

        // An unarmed request is always satisfied; an armed one needs an armed
        // offer no slower than it.
        let fast = TransportQos {
            deadline_ns: 1_000,
            lifespan_ns: 2_000,
            ..volatile_qos()
        };
        let slow = TransportQos {
            deadline_ns: 5_000,
            lifespan_ns: 6_000,
            ..volatile_qos()
        };
        assert!(TransportQos::offer_satisfies(&fast, &slow));
        assert!(!TransportQos::offer_satisfies(&slow, &fast));
        assert!(TransportQos::offer_satisfies(&slow, &best_effort));
        assert!(!TransportQos::offer_satisfies(&best_effort, &fast));
    }

    #[test]
    fn incompatible_pair_is_reported_without_failing_decode() {
        // A BEST_EFFORT publisher against a RELIABLE subscriber is admissible
        // data; C8.5 reports it as an event rather than refusing the graph.
        let mut builder = Builder::new();
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        let route = builder.route("telemetry", schema);
        builder.participant(route, "producer", DIRECTION_PUBLISH, volatile_qos());
        builder.participant(route, "consumer", DIRECTION_SUBSCRIBE, reliable_qos());
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(!graph.all_pairs_qos_compatible());

        let bytes = stream_graph().encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(graph.all_pairs_qos_compatible());
    }

    #[test]
    fn interposition_cycles_and_bypasses_fail_closed() {
        // A two-hop chain that closes on itself.
        let mut builder = stream_graph();
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("proxy-a"),
            next_hop: 1,
        });
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("proxy-b"),
            next_hop: 0,
        });
        builder.participants[0].interposition_head = 0;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::InterpositionCycle)
        ));

        // A hop naming the participant's own component is a bypass: the
        // participant would proxy to itself and reach the route directly.
        let mut builder = stream_graph();
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("producer"),
            next_hop: INTERPOSITION_NONE,
        });
        builder.participants[0].interposition_head = 0;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::InterpositionCycle)
        ));

        // A head pointing past the hop table.
        let mut builder = stream_graph();
        builder.participants[0].interposition_head = 3;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::MissingReference)
        ));

        // A well-formed two-hop chain is admitted.
        let mut builder = stream_graph();
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("proxy-a"),
            next_hop: 1,
        });
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("proxy-b"),
            next_hop: INTERPOSITION_NONE,
        });
        builder.participants[0].interposition_head = 0;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert_eq!(graph.interposition_count(), 2);
        check(&graph).expect("satisfiable");
    }

    #[test]
    fn more_than_eight_live_ingress_sources_fails_closed() {
        // Nine publishers, each a live wake source the fabric must register.
        let mut builder = Builder::new();
        builder.limits.publishers = 16;
        builder.limits.ingress_sources = MAX_INGRESS_SOURCES as u32;
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        let route = builder.route("telemetry", schema);
        for index in 0..9 {
            let name = match index {
                0 => "p0",
                1 => "p1",
                2 => "p2",
                3 => "p3",
                4 => "p4",
                5 => "p5",
                6 => "p6",
                7 => "p7",
                _ => "p8",
            };
            builder.participant(route, name, DIRECTION_PUBLISH, volatile_qos());
        }
        let bytes = builder.encode();
        // Structurally admissible; the aggregate arm counts nine live sources
        // against the eight the graph declared and the kernel can register.
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));

        // Eight is the boundary and passes.
        let mut builder = Builder::new();
        builder.limits.publishers = 16;
        builder.limits.ingress_sources = 8;
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        let route = builder.route("telemetry", schema);
        for name in ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"] {
            builder.participant(route, name, DIRECTION_PUBLISH, volatile_qos());
        }
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        check(&graph).expect("eight sources is admissible");
    }

    #[test]
    fn declared_ingress_limit_above_the_wait_bound_fails_closed() {
        let mut builder = stream_graph();
        builder.limits.ingress_sources = MAX_INGRESS_SOURCES as u32 + 1;
        let bytes = builder.encode();
        // The format's own ceiling catches it before any kernel comparison.
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::Impossible)
        ));
    }

    #[test]
    fn impossible_aggregate_limits_fail_closed() {
        // More participants of a direction than the graph budgets for.
        let mut builder = stream_graph();
        builder.limits.subscribers = 0;
        // Keep the loan/mapping coherence rule from firing first.
        builder.limits.loans = 0;
        builder.limits.mappings = 0;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));

        // A subscriber budget the loan budget cannot back: the fabric owes one
        // receiver-bound downstream loan per matched subscriber.
        let mut builder = stream_graph();
        builder.limits.subscribers = 4;
        builder.limits.loans = 2;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));

        // A page budget above the kernel's global ceiling.
        let mut builder = stream_graph();
        builder.limits.buffer_pages = 4096;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));

        // A capability-slot budget above MAX_CAPS.
        let mut builder = stream_graph();
        builder.limits.capability_slots = LIMIT_CAPABILITY_SLOTS;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        check(&graph).expect("at the ceiling is admissible");
        assert!(matches!(
            graph.validate_against(8, 32, 256, 64, 64, 64),
            Err(DecodeError::Impossible)
        ));

        // A >MAX_MSG sample with no page budget can never be carried.
        let mut builder = stream_graph();
        builder.limits.sample_bytes = 4096;
        builder.limits.buffer_pages = 0;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));

        // A schema whose encoded bound exceeds the declared sample bound.
        let mut builder = stream_graph();
        builder.limits.sample_bytes = 128;
        let bytes = builder.encode();
        let graph = FabricGraph::decode(&bytes).expect("decodes");
        assert!(matches!(check(&graph), Err(DecodeError::Impossible)));
    }

    #[test]
    fn declared_limit_above_the_structural_ceiling_fails_closed() {
        let mut builder = stream_graph();
        builder.limits.sample_bytes = LIMIT_SAMPLE_BYTES + 1;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::Impossible)
        ));

        // A graph admitting more routes than it budgets is over-committed at
        // rest, before any participant launches.
        let mut builder = stream_graph();
        builder.limits.routes = 0;
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::Impossible)
        ));
    }

    #[test]
    fn duplicate_type_tag_between_distinct_identities_fails_closed() {
        let mut builder = stream_graph();
        let existing = builder.schemas[0];
        builder.schemas.push(SchemaEntry {
            identity: [0x99; 32],
            type_tag: existing.type_tag,
            contract_kind: CONTRACT_KIND_STREAM,
            max_encoded_bytes: 1024,
        });
        let bytes = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bytes),
            Err(DecodeError::IdentityMismatch)
        ));
    }

    #[test]
    fn non_canonical_reserved_bytes_fail_closed() {
        // The resource is authenticated by its digest, so a byte the decoder
        // skips would let two distinct resources decode identically.
        let bytes = stream_graph().encode();
        let route_base = HEADER_BYTES + SCHEMA_ENTRY_BYTES;
        let participant_base = route_base + ROUTE_ENTRY_BYTES;
        for offset in [
            route_base + 44,
            participant_base + 115,
            participant_base + 120,
        ] {
            let mut bad = bytes.clone();
            bad[offset] = 1;
            assert!(matches!(
                FabricGraph::decode(&bad),
                Err(DecodeError::NonZeroReserved)
            ));
        }

        let mut builder = stream_graph();
        builder.hops.push(InterpositionEntry {
            component_identity: component_identity("proxy-a"),
            next_hop: INTERPOSITION_NONE,
        });
        builder.participants[0].interposition_head = 0;
        let bytes = builder.encode();
        let hop_base = bytes.len() - INTERPOSITION_ENTRY_BYTES;
        let mut bad = bytes.clone();
        bad[hop_base + 36] = 1;
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::NonZeroReserved)
        ));
    }

    #[test]
    fn header_shape_violations_fail_closed() {
        let bytes = stream_graph().encode();
        assert!(matches!(
            FabricGraph::decode(&bytes[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        ));

        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::BadMagic)
        ));

        let mut bad = bytes.clone();
        bad[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::UnsupportedVersion)
        ));

        let mut bad = bytes.clone();
        bad[16..24].copy_from_slice(&1u64.to_le_bytes());
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::UnknownRequiredFlags)
        ));

        let mut bad = bytes.clone();
        bad[44..48].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::NonZeroReserved)
        ));

        // A total_len disagreeing with the table counts.
        let mut bad = bytes.clone();
        bad[24..28].copy_from_slice(&(bytes.len() as u32 + 8).to_le_bytes());
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::BadBounds)
        ));

        // A route claiming more participants than the table holds.
        let mut builder = stream_graph();
        builder.routes[0].participant_count = 3;
        let bad = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::BadBounds)
        ));

        // A route with no participants is a declared edge nobody can use.
        let mut builder = Builder::new();
        let schema = builder.schema([0x11; 32], 0xAAAA, CONTRACT_KIND_STREAM);
        builder.route("telemetry", schema);
        let bad = builder.encode();
        assert!(matches!(
            FabricGraph::decode(&bad),
            Err(DecodeError::BadBounds)
        ));
    }
}
