use boot_contracts::fabric_graph::{DIRECTION_CLIENT, DIRECTION_SERVER};
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::fabric_call::{
    CALL_MAGIC, FLAG_NON_IDEMPOTENT, FORMAT_VERSION, KIND_CANCEL, KIND_REPLY, KIND_REQUEST,
    KIND_TERMINAL, KIND_TERMINAL_ACK, STATUS_CANCELLED, STATUS_DUPLICATE, STATUS_MALFORMED_REPLY,
    STATUS_PEER_DEAD, STATUS_REJECTED, STATUS_RETRY_EXHAUSTED, STATUS_STALE, STATUS_TIMEOUT,
    WireCallEnvelope, WireCallTimeAdvance,
};
use slime_proto::interface_schema::parameter_call;
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};

#[allow(dead_code)]
mod fabric_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}

// The trace emitter is included once per *binary*, by the binary, because
// `fabric-service` includes both brokers and a file may be a module only once
// in a crate. `use super::trace_log` reaches whichever include its host
// provided; each worker binary is its own task, so each gets its own sink --
// which is what the format wants: one bounded trace per worker.
use super::trace_log;
use fabric_profile::*;
use slime_rt::{
    CapabilityDisposition, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK,
    MAX_CAPS_PER_MSG, MAX_MSG,
};

const SESSION: u64 = 0x000e_0000_0000_0001;
const MAX_CALLS: usize = FABRIC_MAX_IN_FLIGHT_CALLS;
const RETRY_LIMIT: u8 = FABRIC_MAX_RETRIES;
const DEADLINE_NS: u64 = FABRIC_CALL_DEADLINE_NS;
const RETRY_INTERVAL_NS: u64 = DEADLINE_NS / (RETRY_LIMIT as u64 + 1);
const PAGE: u64 = 4096;
const LOAN_BASE: u64 = 0x7200_0000;
const BUFFER_BASE: u64 = LOAN_BASE + PAGE;
const MAX_PENDING_TERMINALS_PER_CLIENT: usize = MAX_CALLS * 2;
const MAX_PENDING_TERMINALS: usize = MAX_PENDING_TERMINALS_PER_CLIENT * 2;
/// Client slots this broker serves. The generation declares two clients on the
/// call route, and a client replaced at runtime reuses its slot, so this bounds
/// the park set rather than the number of components that ever hold a role.
const CLIENTS: usize = 2;
/// The one route this broker carries, as the graph names it. The literal was
/// already spelled twice in `verify_graph`; naming it once keeps the identity
/// the trace folds and the declaration it checks from drifting apart.
const ROUTE_NAME: &str = "parameters";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Free,
    Forwarding,
    AwaitingReply,
    Cancelling,
    ForwardingReply,
    PendingTerminal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Payload {
    None,
    Inline(WireCallEnvelope),
    Shared {
        buffer_slot: u32,
        descriptor: WireSampleDescriptor,
    },
    SharedOutstanding {
        buffer_slot: u32,
        loan_id: u64,
    },
    CancellingShared {
        buffer_slot: u32,
        loan_id: u64,
        message: WireCallEnvelope,
    },
    InlineReply(WireCallEnvelope),
    SharedReply {
        buffer_slot: u32,
        descriptor: WireSampleDescriptor,
    },
}

#[derive(Clone, Copy)]
struct Call {
    phase: Phase,
    request_id: u64,

    server_request_id: u64,
    client_session: u64,
    client_slot: u32,
    client_index: u8,
    retries: u8,
    deadline_ns: u64,
    next_retry_ns: u64,
    terminal_status: i32,
    /// Whether this record's queued marker has been emitted.
    ///
    /// A terminal is re-offered every pass until its client takes it, and
    /// each marker is a root round trip -- so this keeps the announcement
    /// one per record rather than one per attempt.
    offered: bool,
    payload: Payload,
}

impl Call {
    const EMPTY: Self = Self {
        phase: Phase::Free,
        request_id: 0,
        server_request_id: 0,
        client_session: 0,
        client_slot: 0,
        client_index: 0,
        retries: 0,
        deadline_ns: 0,
        next_retry_ns: 0,
        terminal_status: 0,
        offered: false,
        payload: Payload::None,
    };
}

pub struct Broker {
    buffer_factory_slot: u32,
    clients: [Option<u32>; CLIENTS],
    server_slot: Option<u32>,
    time_control: u32,
    supervision: [u32; 3],
    calls: [Call; MAX_CALLS],
    high_water: [u64; 2],
    /// The most calls this broker has ever held live at once.
    ///
    /// C8.11 records resource *high-water* evidence, which is a fact about the
    /// run rather than about the table: reporting `MAX_CALLS` would emit the
    /// same number for every boot and every graph, so a real occupancy
    /// regression would leave the artifact unchanged. Distinct from the
    /// `high_water` above, which is a per-client request-id watermark used for
    /// duplicate suppression and is not an occupancy at all.
    peak_calls: u32,
    next_server_request_id: u64,
    now_ns: u64,
    pending_terminals: [Option<Call>; MAX_PENDING_TERMINALS],
    time_closed: bool,
    /// The call the server is currently executing, if any.
    ///
    /// A native `send` blocks until the peer receives, and this server handles
    /// one call to completion before returning to its endpoint. Forwarding a
    /// second request while the first is unanswered blocks this broker against
    /// a peer that is itself blocked sending its reply here, which is a
    /// deadlock rather than backpressure.
    ///
    /// It names the call rather than being a bare "busy" flag, because those
    /// are different facts: a bare flag cannot tell a timeout on the call the
    /// server holds -- which releases it -- from a timeout on some other call,
    /// which does not. This plane's timeout arm sends requests the server
    /// deliberately never answers, so without that distinction the flag stays
    /// set forever and every later forward is deferred behind a call that will
    /// never complete.
    server_call: Option<u64>,
    /// C8.11's bounded semantic trace for this worker.
    ///
    /// Accumulated rather than written as it happens: a `debug_write` is a root
    /// round trip, so emitting inline would put the trace on the traffic's
    /// critical path and interleave this worker's records with the other two
    /// workers' in scheduling order. Flushed once at `run`'s exit, the serial
    /// order is the declared order instead.
    trace: trace_log::Trace,
    /// The `parameters` route identity, folded once at construction. Records
    /// name the edge by this identity rather than by a slot or a component
    /// name, so the artifact is comparable across boots.
    route: u64,
}

impl Broker {
    pub const fn new(
        buffer_factory_slot: u32,
        clients: [u32; CLIENTS],
        server_slot: u32,
        time_control: u32,
        supervision: [u32; 3],
    ) -> Self {
        Self {
            buffer_factory_slot,
            clients: [Some(clients[0]), Some(clients[1])],
            server_slot: Some(server_slot),
            time_control,
            supervision,
            calls: [Call::EMPTY; MAX_CALLS],
            high_water: [0; 2],
            peak_calls: 0,
            next_server_request_id: 1,
            now_ns: 0,
            pending_terminals: [None; MAX_PENDING_TERMINALS],
            time_closed: false,
            server_call: None,
            trace: trace_log::Trace::new(FABRIC_TRACE_DEPTH),
            // Folded in `run` rather than here: `route_identity` hashes, which
            // is not a const operation, and zero is not a valid route identity
            // so an unfolded broker cannot emit an edge record by accident.
            route: 0,
        }
    }

    /// Record the live-call occupancy if this sweep set a new peak.
    ///
    /// Sampled once per sweep rather than at each claim site: a sweep can admit
    /// and settle several calls, and the number worth reporting is how many were
    /// live at once, which is only observable between sweeps.
    fn sample_peak(&mut self) {
        let live = self
            .calls
            .iter()
            .filter(|call| call.phase != Phase::Free)
            .count() as u32;
        if live > self.peak_calls {
            self.peak_calls = live;
        }
    }

    pub fn run(&mut self) {
        self.verify_graph();
        // The route identity is the graph's own name for this edge, folded from
        // the same three inputs the generation folds. Recorded once here so the
        // trace names the admitted edge and not a local label.
        self.route = trace_log::route_word(&boot_contracts::fabric_graph::route_identity(
            ROUTE_NAME,
            &parameter_call::INTERFACE_IDENTITY,
            boot_contracts::fabric_graph::CONTRACT_KIND_CALL,
        ));
        // C8.11 route evidence: the worker holds the edge the graph declared.
        // Emitted before any traffic so a reader can interpret every later
        // record against a route it has already seen provisioned.
        let _ = self.trace.edge(
            slime_proto::fabric_trace::KIND_ROUTE,
            slime_proto::fabric_trace::ORDER_DATA,
            self.route,
            0,
            0,
            0,
        );
        slime_rt::debug_write(b"[fabric] call endpoints ready\n");
        loop {
            let mut progressed = false;
            for index in 0..self.clients.len() {
                if self.can_receive_client(index) {
                    progressed |= self.pump_client(index);
                }
            }
            progressed |= self.observe_server_death();
            progressed |= self.pump_terminals();
            progressed |= self.pump_pending_terminals();
            progressed |= self.pump_server();
            progressed |= self.pump_replies();
            progressed |= self.pump_time();
            progressed |= self.reclaim_dead_clients();
            self.sample_peak();
            if self.calls.iter().all(|call| call.phase == Phase::Free)
                && self.pending_terminals.iter().all(Option::is_none)
                && self.server_slot.is_none()
                && self.time_closed
            {
                // The sink's mandatory terminal: a reader that sees this knows
                // the trace is complete rather than truncated by a wedge.
                // Resource evidence before the terminal: the most calls this run
                // ever held live at once. The table's *capacity* would be the
                // same number in every boot and every graph, so it would carry
                // no evidence: an occupancy regression would not change it.
                let _ = self
                    .trace
                    .resource(slime_proto::fabric_trace::RESOURCE_CALLS, self.peak_calls);
                let _ = self.trace.terminal();
                self.trace.flush(b"call");
                slime_rt::debug_write(b"[fabric] call state reclaimed\n");
                return;
            }
            if progressed {
                continue;
            }
            // Nothing moved, so every peer with something to say is blocked in
            // `send` -- and `seL4_NBRecv` takes a message only from a sender
            // already blocked, which yielding does nothing to change. The
            // broker must wait, and it cannot wait on any one endpoint: a
            // client that blocks after the sweep passed it would be invisible
            // until the next sweep, and a broker parked on the server never
            // runs one.
            //
            // So it waits on the Notification every peer is badged into. A peer
            // signals it *before* its blocking send, so the wake is already
            // pending by the time the broker gets here and the next sweep finds
            // that sender waiting. This is what a single Endpoint cannot
            // express: "wake me when any of these speak".
            //
            // Except while a terminal is owed. A client waiting for one is
            // blocked in `recv` and will never signal, so the wake that would
            // release this broker cannot arrive -- and the terminal it is
            // holding is the very thing that would let the client run again.
            // Re-offering is the only way out, so it yields instead.
            let owed = self
                .calls
                .iter()
                .any(|call| call.phase == Phase::PendingTerminal)
                || self.pending_terminals.iter().any(Option::is_some);
            // A call at the server is also a reason not to park. The server
            // signals before its reply, but it may exit instead of replying --
            // this plane injects exactly that -- and a peer that has exited
            // signals nothing. Parking then waits for a wake no one can send,
            // while the supervision handle that reports the death is only read
            // by a later sweep. Yielding keeps the sweeps coming.
            if owed
                || self.server_call.is_some()
                || FABRIC_SERVICE_PARAMETERS_READY_SLOT == u32::MAX
            {
                slime_rt::yield_now();
                continue;
            }
            let _ = slime_rt::notification_wait(FABRIC_SERVICE_PARAMETERS_READY_SLOT);
        }
    }

    fn can_receive_client(&self, client: usize) -> bool {
        self.pending_terminals
            .iter()
            .flatten()
            .filter(|call| call.client_index as usize == client)
            .count()
            < MAX_PENDING_TERMINALS_PER_CLIENT
    }

    fn verify_graph(&self) {
        let declared_clients: [&[u8]; CLIENTS] = [b"fabric-call-client", b"fabric-call-client-b"];
        for component in declared_clients {
            let expected = FABRIC_PARTICIPANTS
                .iter()
                .filter(|(name, route_name, interface, direction)| {
                    *name == component
                        && *route_name == ROUTE_NAME
                        && *interface == "ParameterCall"
                        && *direction == DIRECTION_CLIENT
                })
                .count();
            if expected != 1 {
                fail(b"call client graph declaration");
            }
        }
        let servers = FABRIC_PARTICIPANTS
            .iter()
            .filter(|(name, route_name, interface, direction)| {
                *name == b"fabric-call-server"
                    && *route_name == ROUTE_NAME
                    && *interface == "ParameterCall"
                    && *direction == DIRECTION_SERVER
            })
            .count();
        if servers != 1 {
            fail(b"call server graph declaration");
        }
    }

    fn pump_client(&mut self, client: usize) -> bool {
        if !self.can_receive_client(client) {
            return false;
        }
        let Some(slot) = self.clients[client] else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.clients[client] = None;
                self.reclaim_client(slot);
                if slime_rt::cap_drop(slot) != ERR_SUCCESS {
                    fail(b"client endpoint drop")
                }
                return true;
            }
            value if value < 0 => fail(b"client recv"),
            value => value as usize,
        };
        let magic = (length >= 4).then(|| u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        match magic {
            Some(CALL_MAGIC) => {
                release_caps(&caps);
                let Some(message) = WireCallEnvelope::decode(&bytes[..length.min(MAX_MSG)]) else {
                    return true;
                };
                if length != MAX_MSG
                    || !slime_proto::valid_call_envelope(&message, parameter_call::TYPE_TAG)
                {
                    self.reject_terminal(
                        client,
                        slot,
                        message.session.max(1),
                        message.request_id.max(1),
                        STATUS_STALE,
                    );
                    return true;
                }
                // An acknowledgement settles a record; it never opens one, so
                // it is handled before any rejection. It echoes the session of
                // the terminal it acks, so an ack for a *stale-session*
                // terminal would otherwise be refused as a stale call -- and
                // that refusal queues another terminal, which is acked, which
                // is refused, without end.
                if message.kind == KIND_TERMINAL_ACK {
                    // Reply *first*, before anything else on this path. The
                    // caller is blocked in `seL4_Call` and its reply capability
                    // is the one "stored when the thread was last called" --
                    // which any intervening IPC overwrites, including a
                    // `debug_write`, since that is a root round trip. Retiring
                    // the record does not need the caller waiting.
                    let _ = slime_rt::reply(&message.encode());
                    self.retire_terminal(client, message.request_id);
                    // An acknowledgement settles a record rather than opening
                    // one, which is why it sorts after data at the same instant.
                    let _ = self.trace.edge(
                        slime_proto::fabric_trace::KIND_CALL,
                        slime_proto::fabric_trace::ORDER_ACK,
                        self.route,
                        message.request_id,
                        0,
                        slime_proto::fabric_qos::EVENT_MATCHED,
                    );
                    return true;
                }
                if message.session == SESSION {
                    self.reject_terminal(
                        client,
                        slot,
                        message.session,
                        message.request_id,
                        STATUS_STALE,
                    );
                    slime_rt::debug_write(b"[fabric] stale call rejected\n");
                    return true;
                }
                match message.kind {
                    KIND_REQUEST => self.admit_inline(client, slot, message),
                    KIND_CANCEL => self.cancel(client, slot, message),
                    _ => {
                        self.reject_terminal(
                            client,
                            slot,
                            message.session,
                            message.request_id,
                            STATUS_STALE,
                        );
                        slime_rt::debug_write(b"[fabric] client reply authority denied\n");
                    }
                }
            }
            Some(SAMPLE_DESCRIPTOR_MAGIC) => {
                // A delegated loan is a root-recorded export, not an in-message
                // capability: only a native Endpoint travels inline, so
                // `caps[0]` is always zero here.
                let loan_slot = slime_rt::capability_import().unwrap_or(0);
                let Some(descriptor) = WireSampleDescriptor::decode(&bytes[..length.min(MAX_MSG)])
                else {
                    if loan_slot != 0 {
                        let _ = slime_rt::cap_drop(loan_slot);
                    }
                    return true;
                };
                if length != MAX_MSG
                    || loan_slot == 0
                    || !slime_proto::valid_sample_descriptor(
                        &descriptor,
                        descriptor.loan_id,
                        parameter_call::TYPE_TAG,
                        PAGE,
                    )
                    || descriptor.capability_kind != CAPABILITY_KIND_LOAN
                {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    self.reject_terminal(
                        client,
                        slot,
                        client_session(client),
                        descriptor.sequence.max(1),
                        STATUS_STALE,
                    );
                    return true;
                }
                self.admit_shared(client, slot, descriptor, loan_slot);
            }
            _ => release_caps(&caps),
        }
        true
    }

    fn admit_inline(&mut self, client: usize, slot: u32, message: WireCallEnvelope) {
        self.admit(
            client,
            slot,
            message.session,
            message.request_id,
            Payload::Inline(message),
        );
    }

    fn admit_shared(
        &mut self,
        client: usize,
        slot: u32,
        descriptor: WireSampleDescriptor,
        loan_slot: u32,
    ) {
        let buffer_slot = relay_shared_payload(self.buffer_factory_slot, loan_slot, &descriptor);
        self.admit(
            client,
            slot,
            client_session(client),
            descriptor.sequence,
            Payload::Shared {
                buffer_slot,
                descriptor,
            },
        );
    }

    fn admit(&mut self, client: usize, slot: u32, session: u64, request_id: u64, payload: Payload) {
        // The trace records the *outcome* of admission, never the attempt.
        //
        // Tracing at dispatch entry -- where an earlier revision did it -- wrote
        // a matched data record naming a client-supplied `request_id` before any
        // of the three refusals below had run. This plane's own scenario fires
        // twenty-four requests bearing a foreign session, every one of them
        // refused, so the artifact reported refused traffic as accepted and
        // republished correlations the broker had rejected. A denial is recorded
        // as a denial: negative status, and no route identity, because a refusal
        // must not confirm which edge the caller was probing.
        if session != client_session(client) {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_STALE);
            self.trace_denial(STATUS_STALE);
            slime_rt::debug_write(b"[fabric] stale call rejected\n");
            return;
        }
        if request_id <= self.high_water[client] {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_DUPLICATE);
            self.trace_denial(STATUS_DUPLICATE);
            slime_rt::debug_write(b"[fabric] duplicate call rejected\n");
            return;
        }
        let Some(index) = self.calls.iter().position(|call| call.phase == Phase::Free) else {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_RETRY_EXHAUSTED);
            self.trace_denial(STATUS_RETRY_EXHAUSTED);
            slime_rt::debug_write(b"[fabric] call retry exhausted\n");
            return;
        };
        let server_request_id = self.next_server_request_id;
        self.next_server_request_id = self
            .next_server_request_id
            .checked_add(1)
            .unwrap_or_else(|| fail(b"server correlation exhausted"));
        self.high_water[client] = request_id;
        self.calls[index] = Call {
            phase: Phase::Forwarding,
            request_id,
            server_request_id,
            client_session: session,
            client_slot: slot,
            client_index: client as u8,
            retries: 0,
            deadline_ns: self.now_ns.saturating_add(DEADLINE_NS),
            next_retry_ns: self.now_ns,
            terminal_status: 0,
            offered: false,
            payload,
        };
        // Admitted: now the correlation names a record this broker owns.
        let _ = self.trace.edge(
            slime_proto::fabric_trace::KIND_CALL,
            slime_proto::fabric_trace::ORDER_DATA,
            self.route,
            server_request_id,
            0,
            slime_proto::fabric_qos::EVENT_MATCHED,
        );
        self.forward(index);
    }

    /// Retire the server: record its death once, settle every call it owed, and
    /// let the loop reach its exit.
    ///
    /// Four paths learn the server is gone -- the supervision handle, and an
    /// `ERR_PEER_DEAD` from a forward, a reply read, or a terminal send -- and
    /// they race. Wiring the consequences to the supervision arm alone made them
    /// depend on which observation won: `observe_server_death` returns early
    /// once `server_slot` is `None`, so an endpoint error that cleared the slot
    /// first permanently suppressed the arm that set `time_closed`. Since the
    /// exit predicate is the sole flush site, the worker then never flushed and
    /// its whole trace was silently lost.
    ///
    /// What this does *not* do is close the clock. `time_closed` stops
    /// `pump_time` consuming further advances, and an `ERR_PEER_DEAD` from the
    /// server's route endpoint is not evidence that the clock component has gone:
    /// they are separate declared instances. Closing it here would cut the run
    /// short by however many advances were still to come, and which observation
    /// won that race would then change the artifact between two boots. Only the
    /// supervision arm and an `ERR_PEER_DEAD` from the clock endpoint itself
    /// close it.
    ///
    /// Idempotent: the second observation of one death does nothing.
    fn retire_server(&mut self) {
        if self.server_slot.is_none() {
            return;
        }
        self.server_slot = None;
        // Peer death sorts after data and acknowledgement at the same instant:
        // everything already in flight was observable before the peer went away.
        let _ = self.trace.peer_death(self.route);
        self.reclaim_all(STATUS_PEER_DEAD);
    }

    /// Record one refusal.
    ///
    /// Carries the refusal status and no route identity or correlation. A denial
    /// that named the edge would confirm to a caller that the edge exists, which
    /// is the metadata the refusal exists to withhold; a denial that echoed the
    /// caller's correlation would republish an identity the broker just refused.
    fn trace_denial(&mut self, status: i32) {
        let _ = self.trace.edge(
            slime_proto::fabric_trace::KIND_DENIAL,
            slime_proto::fabric_trace::ORDER_DATA,
            0,
            0,
            denial_status(status),
            0,
        );
    }

    fn reject_terminal(
        &mut self,
        client: usize,
        slot: u32,
        session: u64,
        request_id: u64,
        status: i32,
    ) {
        let pending = Call {
            phase: Phase::PendingTerminal,
            request_id,
            server_request_id: 0,
            client_session: session,
            client_slot: slot,
            client_index: client as u8,
            retries: 0,
            deadline_ns: self.now_ns.saturating_add(DEADLINE_NS),
            next_retry_ns: 0,
            terminal_status: status,
            offered: false,
            payload: Payload::None,
        };
        // Queue rather than hand over from inside the receive path: this
        // client is typically blocked in `send` on its next request, so a
        // delivery attempt here would wait on a peer waiting on us.
        if let Some(index) = self.calls.iter().position(|call| call.phase == Phase::Free) {
            self.calls[index] = pending;
            slime_rt::debug_write(b"[fabric] terminal delivery queued\n");
            return;
        }
        let Some(index) = self.pending_terminals.iter().position(Option::is_none) else {
            self.clients[client] = None;
            self.reclaim_client(slot);
            let _ = slime_rt::cap_drop(slot);
            slime_rt::debug_write(b"[fabric] saturated client isolated\n");
            return;
        };
        self.pending_terminals[index] = Some(pending);
        slime_rt::debug_write(b"[fabric] terminal delivery queued\n");
    }

    fn pump_terminal(&mut self, index: usize) -> bool {
        let call = self.calls[index];
        match try_send_terminal(
            call.client_slot,
            call.client_session,
            call.request_id,
            call.terminal_status,
        ) {
            ERR_SUCCESS | ERR_PEER_DEAD => {
                self.calls[index] = Call::EMPTY;
                true
            }
            ERR_WOULDBLOCK => {
                // Once per record, not once per offer. A terminal is re-offered
                // on every pass until its client takes it, and each
                // `debug_write` is a root round trip -- so announcing each
                // attempt spends the root's graph-iteration budget on a
                // condition that has not changed, and starves the very
                // exchange that would clear it.
                if !self.calls[index].offered {
                    self.calls[index].offered = true;
                    slime_rt::debug_write(b"[fabric] terminal delivery queued\n");
                }
                false
            }
            _ => fail(b"call terminal"),
        }
    }

    /// Retire the terminal `client` acknowledged.
    ///
    /// Matching on the request id keeps the retirement exact: an ack settles
    /// the record it names, never a batch, so a client acknowledging out of
    /// order or twice cannot drop a terminal it has not seen.
    ///
    /// Every match is retired rather than the first. A request id settles
    /// exactly once, so duplicates are the same terminal recorded twice --
    /// which happens when a call already holding a `PendingTerminal` is
    /// finished again -- and leaving one behind blocks the client's queue
    /// forever, since it will never ack an id it has already passed.
    fn retire_terminal(&mut self, client: usize, request_id: u64) {
        for index in 0..self.calls.len() {
            if self.calls[index].phase == Phase::PendingTerminal
                && self.calls[index].client_index as usize == client
                && self.calls[index].request_id == request_id
            {
                self.calls[index] = Call::EMPTY;
            }
        }
        for pending in &mut self.pending_terminals {
            if pending.is_some_and(|call| {
                call.client_index as usize == client && call.request_id == request_id
            }) {
                *pending = None;
            }
        }
    }

    fn pump_pending_terminals(&mut self) -> bool {
        let mut progressed = false;
        let mut dead_slot = None;
        // Same ordering rule as `pump_terminals`: only the client's lowest
        // outstanding id, so a later terminal cannot reach a client waiting on
        // an earlier one.
        let lowest = self.lowest_pending_terminal();
        for index in 0..self.pending_terminals.len() {
            let Some(call) = self.pending_terminals[index] else {
                continue;
            };
            if lowest[call.client_index as usize] != Some(call.request_id) {
                continue;
            }
            let pending = &mut self.pending_terminals[index];
            match try_send_terminal(
                call.client_slot,
                call.client_session,
                call.request_id,
                call.terminal_status,
            ) {
                ERR_SUCCESS => {
                    *pending = None;
                    progressed = true;
                }
                ERR_PEER_DEAD => {
                    *pending = None;
                    dead_slot = Some(call.client_slot);
                    progressed = true;
                    break;
                }
                ERR_WOULDBLOCK => {}
                _ => fail(b"call terminal"),
            }
        }
        if let Some(slot) = dead_slot {
            for pending in &mut self.pending_terminals {
                if pending.is_some_and(|call| call.client_slot == slot) {
                    *pending = None;
                }
            }
            self.reclaim_client(slot);
        }
        progressed
    }

    fn forward(&mut self, index: usize) {
        let Some(server) = self.server_slot else {
            self.finish(index, STATUS_PEER_DEAD);
            return;
        };
        // At most one request may be at the server at a time.
        //
        // The server handles one call to completion and answers with a blocking
        // `send`. Forwarding a second request while the first is unanswered
        // blocks this broker in `send` against a server already blocked sending
        // its reply here: neither can move, and it is a deadlock rather than
        // backpressure. A call left in `Phase::Forwarding` is retried by
        // `pump_terminals` on a later pass, which is exactly the queue this
        // needs -- the server's own reply is what frees the slot.
        if self.server_call.is_some() {
            self.calls[index].phase = Phase::Forwarding;
            return;
        }
        let call = self.calls[index];
        let mut next_payload = Payload::None;
        let result = match call.payload {
            Payload::Inline(mut message) => {
                message.session = SESSION;
                message.request_id = call.server_request_id;
                slime_rt::send(server, &message.encode(), &[])
            }
            Payload::Shared {
                buffer_slot,
                mut descriptor,
            } => {
                descriptor.sequence = call.server_request_id;
                let loan = match slime_rt::shared_buffer_loan(
                    buffer_slot,
                    self.supervision[2],
                    0,
                    descriptor.length,
                    false,
                ) {
                    Ok(loan) => loan,
                    Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => return,
                    Err(_) => fail(b"shared request loan"),
                };
                descriptor.loan_id = loan.id;
                let sent = slime_rt::capability_delegate(
                    server,
                    loan.slot,
                    CapabilityDisposition::Move,
                    OBJECT_KIND_SHARED_BUFFER_LOAN,
                    1 << 9,
                    &descriptor.encode(),
                );
                if sent == ERR_SUCCESS {
                    next_payload = Payload::SharedOutstanding {
                        buffer_slot,
                        loan_id: loan.id,
                    };
                } else {
                    let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                }
                sent
            }
            Payload::CancellingShared {
                buffer_slot,
                loan_id,
                message,
            } => {
                let sent = slime_rt::send(server, &message.encode(), &[]);
                if sent == ERR_SUCCESS {
                    next_payload = Payload::SharedOutstanding {
                        buffer_slot,
                        loan_id,
                    };
                }
                sent
            }
            Payload::None
            | Payload::SharedOutstanding { .. }
            | Payload::InlineReply(_)
            | Payload::SharedReply { .. } => fail(b"forward invalid payload"),
        };
        match result {
            ERR_SUCCESS => {
                let was_cancelling = self.calls[index].phase == Phase::Cancelling;
                self.calls[index].phase = if was_cancelling {
                    Phase::Cancelling
                } else {
                    Phase::AwaitingReply
                };
                self.calls[index].payload = next_payload;
                // The server is now executing this call and will not receive
                // again until it has answered. Naming the call lets a
                // timeout on *this* request release it, while a timeout on
                // any other leaves it held.
                self.server_call = Some(self.calls[index].server_request_id);
                if was_cancelling {
                    slime_rt::debug_write(b"[fabric] call cancellation forwarded\n");
                } else {
                    slime_rt::debug_write(b"[fabric] call forwarded\n");
                }
            }
            ERR_WOULDBLOCK => {
                self.calls[index].retries = self.calls[index].retries.saturating_add(1);
                self.calls[index].next_retry_ns = self.now_ns.saturating_add(RETRY_INTERVAL_NS);
                if self.calls[index].retries >= RETRY_LIMIT {
                    let status = if self.calls[index].phase == Phase::Cancelling {
                        STATUS_CANCELLED
                    } else {
                        STATUS_RETRY_EXHAUSTED
                    };
                    self.finish(index, status);
                    if status == STATUS_CANCELLED {
                        slime_rt::debug_write(b"[fabric] call cancelled\n");
                    } else {
                        slime_rt::debug_write(b"[fabric] call retry exhausted\n");
                    }
                }
            }
            ERR_PEER_DEAD => {
                self.retire_server();
                self.finish(index, STATUS_PEER_DEAD);
            }
            _ => fail(b"call forward"),
        }
    }

    fn cancel(&mut self, client: usize, client_slot: u32, message: WireCallEnvelope) {
        let Some(index) = self.calls.iter().position(|call| {
            call.phase != Phase::Free
                && call.request_id == message.request_id
                && call.client_session == message.session
                && call.client_slot == client_slot
        }) else {
            self.reject_terminal(
                client,
                client_slot,
                message.session,
                message.request_id,
                STATUS_STALE,
            );
            return;
        };
        // A call still queued in `Forwarding` is cancelled the same way as one
        // the server is executing. It has not reached the server yet, but the
        // request it names will: dropping it here would settle the cancel
        // locally and leave the server to execute a request the client has
        // already withdrawn. Staging the cancellation keeps withdrawal a fact
        // the server observes.
        if matches!(
            self.calls[index].phase,
            Phase::AwaitingReply | Phase::Forwarding
        ) {
            // The staged cancellation is delivered by `pump_terminals`, which
            // resolves the server slot itself when the peer is reachable again.
            if self.server_slot.is_none() {
                self.finish(index, STATUS_PEER_DEAD);
                return;
            }
            let cancel = WireCallEnvelope {
                magic: CALL_MAGIC,
                version: FORMAT_VERSION,
                kind: KIND_CANCEL,
                flags: 0,
                session: SESSION,
                request_id: self.calls[index].server_request_id,
                type_identity: parameter_call::TYPE_TAG,
                status: STATUS_CANCELLED,
                payload_len: 0,
                payload: [0; 16],
            };
            // The server is single-threaded and this call is the one it is
            // working on, so it is not in `recv`: a blocking send here would
            // wait on a peer that is waiting on us. Stage the cancellation as
            // the call's payload and let `pump_terminals` deliver it once the
            // server comes back around, which is the same queue a deferred
            // forward uses.
            {
                let outstanding = self.calls[index].payload;
                self.calls[index].phase = Phase::Cancelling;
                self.calls[index].payload = match outstanding {
                    Payload::SharedOutstanding {
                        buffer_slot,
                        loan_id,
                    } => Payload::CancellingShared {
                        buffer_slot,
                        loan_id,
                        message: cancel,
                    },
                    _ => Payload::Inline(cancel),
                };
                self.calls[index].retries = 0;
                self.calls[index].next_retry_ns = self.now_ns;
                self.calls[index].terminal_status = STATUS_CANCELLED;
                // The cancelled request's own deadline has usually passed by
                // now -- that is often why it is being cancelled. Settling on
                // it here would report the withdrawal before the server has
                // seen it, so the cancellation gets its own deadline: the
                // client is told once the server has actually settled it, and
                // the timeout still bounds a server that never does.
                self.calls[index].deadline_ns = self.now_ns.saturating_add(DEADLINE_NS);
            }
            return;
        }
        self.finish(index, STATUS_CANCELLED);
        slime_rt::debug_write(b"[fabric] call cancelled\n");
    }

    fn pump_server(&mut self) -> bool {
        let Some(slot) = self.server_slot else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.retire_server();
                return true;
            }
            value if value < 0 => fail(b"server recv"),
            value => value as usize,
        };
        self.handle_server_record(&bytes, &caps, length);
        true
    }

    fn handle_server_record(
        &mut self,
        bytes: &[u8; MAX_MSG],
        caps: &[u64; MAX_CAPS_PER_MSG],
        length: usize,
    ) {
        // Anything received from the server means it finished a call and went
        // back to its endpoint, so it is reachable by a blocking send again.
        self.server_call = None;
        let magic = (length >= 4).then(|| u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        match magic {
            Some(CALL_MAGIC) => {
                release_caps(caps);
                let decoded = WireCallEnvelope::decode(&bytes[..length.min(MAX_MSG)]);
                let Some(reply) = decoded.filter(|reply| {
                    length == MAX_MSG
                        && slime_proto::valid_call_envelope(reply, parameter_call::TYPE_TAG)
                        && reply.kind == KIND_REPLY
                        && reply.session == SESSION
                }) else {
                    if let Some(reply) = decoded
                        && let Some(index) = self.find_call(reply.request_id)
                    {
                        self.finish(index, STATUS_MALFORMED_REPLY);
                        slime_rt::debug_write(b"[fabric] malformed call reply rejected\n");
                    }
                    return;
                };
                let Some(index) = self.find_call(reply.request_id) else {
                    slime_rt::debug_write(b"[fabric] stale call reply rejected\n");
                    return;
                };
                let mut outward = reply;
                settle_outstanding_request(&mut self.calls[index]);
                outward.session = self.calls[index].client_session;
                outward.request_id = self.calls[index].request_id;
                let status = outward.status;
                if self.calls[index].phase == Phase::Cancelling {
                    // The server answers the original request and the cancel
                    // separately, and the request's own reply usually lands
                    // first. Settling on it would tell the client the call was
                    // withdrawn before the server had settled the withdrawal.
                    // The reply that settles a cancel is the one carrying
                    // `STATUS_CANCELLED`; anything else is the in-flight
                    // request's answer, which a cancelled call discards.
                    if status != STATUS_CANCELLED {
                        return;
                    }
                    self.finish(index, STATUS_CANCELLED);
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                } else {
                    self.deliver_inline_reply(index, outward);
                    if status == STATUS_REJECTED {
                        slime_rt::debug_write(b"[fabric] server rejection routed\n");
                    } else {
                        slime_rt::debug_write(b"[fabric] call reply correlated\n");
                    }
                }
            }
            Some(SAMPLE_DESCRIPTOR_MAGIC) => {
                // As above: the loan is claimed, never read out of the message.
                let loan_slot = slime_rt::capability_import().unwrap_or(0);
                let Some(descriptor) = WireSampleDescriptor::decode(&bytes[..length.min(MAX_MSG)])
                else {
                    if loan_slot != 0 {
                        let _ = slime_rt::cap_drop(loan_slot);
                    }
                    return;
                };
                let Some(index) = self.find_call(descriptor.sequence) else {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    slime_rt::debug_write(b"[fabric] stale call reply rejected\n");
                    return;
                };
                if length != MAX_MSG
                    || loan_slot == 0
                    || !slime_proto::valid_sample_descriptor(
                        &descriptor,
                        descriptor.loan_id,
                        parameter_call::TYPE_TAG,
                        PAGE,
                    )
                {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    self.finish(index, STATUS_MALFORMED_REPLY);
                    return;
                }
                settle_outstanding_request(&mut self.calls[index]);
                if self.calls[index].phase == Phase::Cancelling {
                    let _ = slime_rt::shared_buffer_return(loan_slot);
                    self.finish(index, STATUS_CANCELLED);
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                    return;
                }
                let mut outward = descriptor;
                outward.sequence = self.calls[index].request_id;
                let buffer_slot =
                    relay_shared_payload(self.buffer_factory_slot, loan_slot, &descriptor);
                self.deliver_shared_reply(index, outward, buffer_slot);
            }
            _ => release_caps(caps),
        }
    }

    fn deliver_inline_reply(&mut self, index: usize, outward: WireCallEnvelope) {
        let client = self.calls[index].client_slot;
        match slime_rt::send(client, &outward.encode(), &[]) {
            ERR_SUCCESS => self.calls[index] = Call::EMPTY,
            ERR_WOULDBLOCK => {
                self.calls[index].phase = Phase::ForwardingReply;
                self.calls[index].payload = Payload::InlineReply(outward);
            }
            ERR_PEER_DEAD => self.drop_dead_client(index, b"client endpoint drop"),
            _ => fail(b"reply delivery"),
        }
    }

    fn deliver_shared_reply(
        &mut self,
        index: usize,
        mut descriptor: WireSampleDescriptor,
        buffer_slot: u32,
    ) {
        let client = self.calls[index].client_slot;
        let supervision = self.supervision[self.calls[index].client_index as usize];
        let loan = match slime_rt::shared_buffer_loan(
            buffer_slot,
            supervision,
            0,
            descriptor.length,
            false,
        ) {
            Ok(loan) => loan,
            Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => {
                self.calls[index].phase = Phase::ForwardingReply;
                self.calls[index].payload = Payload::SharedReply {
                    buffer_slot,
                    descriptor,
                };
                return;
            }
            Err(_) => fail(b"shared client loan"),
        };
        descriptor.loan_id = loan.id;
        match slime_rt::capability_delegate(
            client,
            loan.slot,
            CapabilityDisposition::Move,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
            1 << 9,
            &descriptor.encode(),
        ) {
            ERR_SUCCESS => {
                let _ = slime_rt::shared_buffer_release(buffer_slot);
                self.calls[index] = Call::EMPTY;
            }
            ERR_WOULDBLOCK => {
                let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                self.calls[index].phase = Phase::ForwardingReply;
                self.calls[index].payload = Payload::SharedReply {
                    buffer_slot,
                    descriptor,
                };
            }
            ERR_PEER_DEAD => {
                let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                let _ = slime_rt::shared_buffer_release(buffer_slot);
                self.drop_dead_client(index, b"shared client endpoint drop");
            }
            _ => fail(b"shared reply delivery"),
        }
    }

    fn pump_replies(&mut self) -> bool {
        let mut progressed = false;
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::ForwardingReply {
                continue;
            }
            let call = self.calls[index];
            match call.payload {
                Payload::InlineReply(reply) => {
                    match slime_rt::send(call.client_slot, &reply.encode(), &[]) {
                        ERR_SUCCESS => {
                            self.calls[index] = Call::EMPTY;
                            progressed = true;
                        }
                        ERR_WOULDBLOCK => {}
                        ERR_PEER_DEAD => {
                            self.drop_dead_client(index, b"client endpoint drop");
                            progressed = true;
                        }
                        _ => fail(b"reply delivery"),
                    }
                }
                Payload::SharedReply {
                    buffer_slot,
                    mut descriptor,
                } => {
                    let supervision = self.supervision[call.client_index as usize];
                    let loan = match slime_rt::shared_buffer_loan(
                        buffer_slot,
                        supervision,
                        0,
                        descriptor.length,
                        false,
                    ) {
                        Ok(loan) => loan,
                        Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => continue,
                        Err(_) => fail(b"shared client loan"),
                    };
                    descriptor.loan_id = loan.id;
                    match slime_rt::capability_delegate(
                        call.client_slot,
                        loan.slot,
                        CapabilityDisposition::Move,
                        OBJECT_KIND_SHARED_BUFFER_LOAN,
                        1 << 9,
                        &descriptor.encode(),
                    ) {
                        ERR_SUCCESS => {
                            let _ = slime_rt::shared_buffer_release(buffer_slot);
                            self.calls[index] = Call::EMPTY;
                            progressed = true;
                        }
                        ERR_WOULDBLOCK => {
                            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                        }
                        ERR_PEER_DEAD => {
                            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                            let _ = slime_rt::shared_buffer_release(buffer_slot);
                            self.drop_dead_client(index, b"shared client endpoint drop");
                            progressed = true;
                        }
                        _ => fail(b"shared reply delivery"),
                    }
                }
                _ => fail(b"pending reply payload"),
            }
        }
        progressed
    }

    fn drop_dead_client(&mut self, index: usize, reason: &[u8]) {
        let client_index = self.calls[index].client_index as usize;
        let client = self.calls[index].client_slot;
        settle_payload(self.calls[index].payload);
        self.clients[client_index] = None;
        self.calls[index] = Call::EMPTY;
        if slime_rt::cap_drop(client) != ERR_SUCCESS {
            fail(reason)
        }
    }

    fn pump_time(&mut self) -> bool {
        if self.time_closed {
            return false;
        }
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(self.time_control, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.time_closed = true;
                return true;
            }
            value if value < 0 => fail(b"call time recv"),
            value => value as usize,
        };
        release_caps(&caps);
        let Some(value) = WireCallTimeAdvance::decode(&bytes[..length.min(MAX_MSG)]) else {
            return true;
        };
        if length != MAX_MSG
            || !slime_proto::valid_call_time_advance(&value)
            || value.now_ns < self.now_ns
        {
            fail(b"invalid call time");
        }
        // The advance closes the previous instant, so it is recorded before any
        // deadline it triggers: the trace's tie order puts time last at the
        // instant it ends, and the expiries below belong to the new one.
        let _ = self.trace.advance(value.now_ns);
        self.now_ns = value.now_ns;
        for index in 0..self.calls.len() {
            if self.calls[index].phase == Phase::Free {
                continue;
            }
            if self.now_ns >= self.calls[index].deadline_ns {
                let status = if self.calls[index].phase == Phase::Cancelling {
                    STATUS_CANCELLED
                } else {
                    STATUS_TIMEOUT
                };
                // A timeout on the call the server is executing releases it:
                // the deadline says it owes nothing on that request any more.
                // A timeout on any *other* call leaves it held, because the
                // request it is actually working on is still outstanding.
                if self.server_call == Some(self.calls[index].server_request_id) {
                    self.server_call = None;
                }
                // Read the correlation *before* `finish`: it clears the record
                // to `Call::EMPTY` on the delivered and peer-dead paths, so
                // reading afterwards yields zero on the common path and the
                // real id only when the terminal had to be queued -- making the
                // traced correlation depend on transient endpoint capacity.
                let correlation = self.calls[index].server_request_id;
                self.finish(index, status);
                // A deadline transition is a QoS policy event on this edge, at
                // the instant the clock just reached.
                let _ = self.trace.edge(
                    slime_proto::fabric_trace::KIND_QOS,
                    slime_proto::fabric_trace::ORDER_DATA,
                    self.route,
                    correlation,
                    status,
                    slime_proto::fabric_qos::EVENT_DEADLINE_MISSED,
                );
                if status == STATUS_CANCELLED {
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                } else {
                    slime_rt::debug_write(b"[fabric] call timed out\n");
                }
            } else if matches!(
                self.calls[index].phase,
                Phase::Forwarding | Phase::Cancelling
            ) && !matches!(self.calls[index].payload, Payload::None)
                && self.now_ns >= self.calls[index].next_retry_ns
            {
                self.forward(index);
            }
        }
        true
    }

    /// Observe the server's termination through its supervision handle.
    ///
    /// Deliberately *not* guarded on `server_slot.is_some()`. An `ERR_PEER_DEAD`
    /// from the route endpoint clears the slot first about as often as not, and
    /// the old guard then suppressed this arm permanently -- which mattered
    /// because this is the only place that closes the clock, and the exit
    /// predicate waits on it. `time_closed` short-circuits the body, so the
    /// supervision handle is read once per death rather than every sweep.
    fn observe_server_death(&mut self) -> bool {
        if self.time_closed {
            return false;
        }
        match slime_rt::supervision_status(self.supervision[2]) {
            Ok(None) => false,
            Ok(Some(_)) => {
                self.retire_server();
                // The server's task hosts this plane's clock, so its confirmed
                // termination is what says no further advance can arrive.
                self.time_closed = true;
                slime_rt::debug_write(b"[fabric] call peer death propagated\n");
                true
            }
            Err(_) => fail(b"server supervision"),
        }
    }

    fn find_call(&self, server_request_id: u64) -> Option<usize> {
        self.calls.iter().position(|call| {
            matches!(call.phase, Phase::AwaitingReply | Phase::Cancelling)
                && call.server_request_id == server_request_id
        })
    }
    fn finish(&mut self, index: usize, status: i32) {
        let call = self.calls[index];
        if call.phase == Phase::Free {
            return;
        }
        settle_payload(call.payload);
        self.calls[index].payload = Payload::None;
        match try_send_terminal(
            call.client_slot,
            call.client_session,
            call.request_id,
            status,
        ) {
            ERR_SUCCESS | ERR_PEER_DEAD => self.calls[index] = Call::EMPTY,
            ERR_WOULDBLOCK => {
                self.calls[index].phase = Phase::PendingTerminal;
                self.calls[index].terminal_status = status;
            }
            _ => fail(b"call terminal"),
        }
    }

    /// The lowest outstanding terminal request id per client, across both the
    /// in-`calls` records and the overflow queue.
    ///
    /// Both hold terminals for the same client, so taking a minimum within
    /// each separately would still let the two offer different ids.
    fn lowest_pending_terminal(&self) -> [Option<u64>; CLIENTS] {
        let mut lowest: [Option<u64>; CLIENTS] = [None; CLIENTS];
        let mut note = |client: usize, request_id: u64| {
            if lowest[client].is_none_or(|current| request_id < current) {
                lowest[client] = Some(request_id);
            }
        };
        for call in self.calls.iter() {
            if call.phase == Phase::PendingTerminal {
                note(call.client_index as usize, call.request_id);
            }
        }
        for call in self.pending_terminals.iter().flatten() {
            note(call.client_index as usize, call.request_id);
        }
        lowest
    }
    fn pump_terminals(&mut self) -> bool {
        let mut progressed = false;
        // Offer only the lowest outstanding request id per client, across both
        // queues. A client reads terminals in the order it issued the requests
        // and takes one per receive, so offering the whole set lets a later
        // terminal reach a client waiting for an earlier one -- which it
        // refuses as a mismatch, and which no re-offer can repair because the
        // client never advances past the id it is waiting for.
        let lowest = self.lowest_pending_terminal();
        for (client, target) in lowest.into_iter().enumerate() {
            let Some(target) = target else {
                continue;
            };
            let next = self.calls.iter().position(|call| {
                call.phase == Phase::PendingTerminal
                    && call.client_index as usize == client
                    && call.request_id == target
            });
            if let Some(index) = next {
                progressed |= self.pump_terminal(index);
            }
        }
        // Deliver a staged cancellation, but only once the server's reply to
        // the cancelled request has come back: until then it is executing that
        // call, not sitting in `recv`, and a blocking send would wait on a peer
        // that is waiting on us. `settle_outstanding_request` clears the
        // payload when the reply lands, so a `Cancelling` call still holding a
        // staged message is one whose server has not answered yet.
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::Cancelling || self.server_call.is_some() {
                continue;
            }
            let (Payload::Inline(message) | Payload::CancellingShared { message, .. }) =
                self.calls[index].payload
            else {
                continue;
            };
            let Some(server) = self.server_slot else {
                self.finish(index, STATUS_PEER_DEAD);
                progressed = true;
                continue;
            };
            // Announce the forward before the send. `send` blocks until the
            // server takes the message, and the server announces the settlement
            // the moment it does -- so printing afterwards would always put
            // this marker after the server's, inverting the causal order the
            // plane is asserting.
            slime_rt::debug_write(b"[fabric] call cancellation forwarded\n");
            match slime_rt::send(server, &message.encode(), &[]) {
                ERR_SUCCESS => {
                    self.calls[index].payload = match self.calls[index].payload {
                        Payload::CancellingShared {
                            buffer_slot,
                            loan_id,
                            ..
                        } => Payload::SharedOutstanding {
                            buffer_slot,
                            loan_id,
                        },
                        _ => Payload::None,
                    };
                    progressed = true;
                }
                ERR_PEER_DEAD => {
                    self.retire_server();
                    self.finish(index, STATUS_PEER_DEAD);
                    progressed = true;
                }
                _ => fail(b"call cancel forward"),
            }
        }
        // Retry one call deferred because the server was busy. Its reply is
        // what makes the server reachable again, so this resumes exactly then.
        if self.server_call.is_none()
            && let Some(index) = self
                .calls
                .iter()
                .position(|call| call.phase == Phase::Forwarding)
        {
            self.forward(index);
            progressed = true;
        }
        progressed
    }

    fn reclaim_client(&mut self, slot: u32) {
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::Free && self.calls[index].client_slot == slot {
                settle_payload(self.calls[index].payload);
                self.calls[index] = Call::EMPTY;
            }
        }
    }

    /// Drop terminals owed to a client that has exited.
    ///
    /// A terminal is only retired when its client acks it, which is what makes
    /// delivery a fact rather than a guess. A client that exits first can never
    /// ack, so those records would be offered for ever and the broker would
    /// outlive a finished graph. Supervision is the only thing that separates
    /// "not reading yet" from "gone".
    fn reclaim_dead_clients(&mut self) -> bool {
        let mut progressed = false;
        for client in 0..CLIENTS {
            if matches!(
                slime_rt::supervision_status(self.supervision[client]),
                Ok(None)
            ) {
                continue;
            }
            for index in 0..self.calls.len() {
                if self.calls[index].phase == Phase::PendingTerminal
                    && self.calls[index].client_index as usize == client
                {
                    self.calls[index] = Call::EMPTY;
                    progressed = true;
                }
            }
            for slot in self.pending_terminals.iter_mut() {
                if slot.is_some_and(|call| call.client_index as usize == client) {
                    *slot = None;
                    progressed = true;
                }
            }
        }
        if progressed {
            slime_rt::debug_write(b"[fabric] terminal delivery abandoned\n");
        }
        progressed
    }

    fn reclaim_all(&mut self, status: i32) {
        for index in 0..self.calls.len() {
            self.finish(index, status);
        }
    }
}

fn settle_outstanding_request(call: &mut Call) {
    let outstanding = match call.payload {
        Payload::SharedOutstanding {
            buffer_slot,
            loan_id,
        }
        | Payload::CancellingShared {
            buffer_slot,
            loan_id,
            ..
        } => Some((buffer_slot, loan_id)),
        _ => None,
    };
    if let Some((buffer_slot, loan_id)) = outstanding {
        let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan_id);
        let _ = slime_rt::shared_buffer_release(buffer_slot);
        call.payload = Payload::None;
    }
}

fn client_session(client: usize) -> u64 {
    0x00c1_0000_0000_0001 + client as u64 * 0x0001_0000_0000_0000
}

fn settle_payload(payload: Payload) {
    match payload {
        Payload::Shared { buffer_slot, .. } | Payload::SharedReply { buffer_slot, .. } => {
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        Payload::SharedOutstanding {
            buffer_slot,
            loan_id,
        }
        | Payload::CancellingShared {
            buffer_slot,
            loan_id,
            ..
        } => {
            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan_id);
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        Payload::None | Payload::Inline(_) | Payload::InlineReply(_) => {}
    }
}

fn relay_shared_payload(
    factory_slot: u32,
    loan_slot: u32,
    descriptor: &WireSampleDescriptor,
) -> u32 {
    if slime_rt::shared_buffer_loan_map(loan_slot, LOAN_BASE, 0, descriptor.length) != ERR_SUCCESS {
        let _ = slime_rt::shared_buffer_return(loan_slot);
        fail(b"shared relay source map");
    }
    let buffer = slime_rt::shared_buffer_create(factory_slot, 1, true)
        .unwrap_or_else(|_| fail(b"shared relay create"));
    if slime_rt::shared_buffer_map(buffer.slot, BUFFER_BASE, 0, descriptor.length, true)
        != ERR_SUCCESS
    {
        fail(b"shared relay target map");
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            LOAN_BASE as *const u8,
            BUFFER_BASE as *mut u8,
            descriptor.length as usize,
        );
    }
    if slime_rt::shared_buffer_unmap(loan_slot, LOAN_BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS
        || slime_rt::shared_buffer_unmap(buffer.slot, BUFFER_BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS
    {
        fail(b"shared relay settle");
    }
    buffer.slot
}

/// Offer one terminal record to a client, without waiting for it to be taken.
///
/// Blocking deadlocks here: the client this answers is typically blocked in
/// `send` on its next request -- exceeding `MAX_CALLS` is exactly that shape --
/// so a blocking send waits on a peer waiting on us. `seL4_NBSend` delivers
/// only to a receiver already blocked on the endpoint, which is precisely "the
/// client has stopped sending and come back to read", and discards otherwise.
///
/// It reports nothing either way, so this answers `ERR_WOULDBLOCK` rather than
/// claiming a delivery it cannot observe, and the record stays queued to be
/// re-offered. Repeating an offer is harmless: a terminal is idempotent, and
/// the client reads each one exactly once because only one can be transferred
/// per receive.
fn try_send_terminal(slot: u32, session: u64, request_id: u64, status: i32) -> i64 {
    let message = WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_TERMINAL,
        flags: 0,
        session,
        request_id,
        type_identity: parameter_call::TYPE_TAG,
        status,
        payload_len: 0,
        payload: [0; 16],
    };
    match slime_rt::try_send(slot, &message.encode(), &[]) {
        ERR_SUCCESS => ERR_WOULDBLOCK,
        other => other,
    }
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().filter(|slot| **slot != 0) {
        let _ = slime_rt::cap_drop(*slot as u32);
    }
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

const _: () = assert!(slime_proto::fabric_call::CALL_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_call::CALL_TIME_LEN == MAX_MSG);
const _: () = assert!(FLAG_NON_IDEMPOTENT == 1);

/// The one overflow discipline implemented. Asserted rather than branched on: a
/// generation declaring anything else names behaviour no worker has, and
/// discovering that at boot would be worse than not compiling.
const _: () = assert!(FABRIC_TRACE_OVERFLOW == slime_proto::fabric_trace::OVERFLOW_SATURATE);
/// And the declared depth must fit the sink the contract sizes.
///
/// `const _: ()` rather than relying on `TraceSink::with_const_capacity`'s own
/// assert: that constructor is a `const fn` reached from `fn main`, and a
/// `const fn` called at runtime evaluates at runtime -- so its assert would be a
/// boot panic inside a `no_std` component rather than the build failure it
/// claims to be. These items are evaluated at compile time unconditionally.
const _: () = assert!(FABRIC_TRACE_DEPTH <= slime_proto::fabric_trace::MAX_TRACE_DEPTH);
const _: () = assert!(FABRIC_TRACE_DEPTH > slime_proto::fabric_trace::TERMINAL_RESERVE);

/// A refusal status, as a failure code the trace's denial family admits.
///
/// The per-plane `STATUS_*` refusal codes are positive protocol enumerators, so
/// a denial record has to carry the negation. `-status.abs()` is wrong twice
/// over: it yields `0` for a zero input, which `valid_trace_record` refuses --
/// silently dropping the denial -- and `i32::MIN.abs()` overflows. Mapping
/// through `-1` for anything that is not a nameable positive refusal keeps every
/// denial a valid record, which is the property that makes a refused request
/// visible in the artifact at all.
fn denial_status(status: i32) -> i32 {
    match status {
        0 => -1,
        value if value > 0 => value.checked_neg().unwrap_or(-1),
        value => value,
    }
}
