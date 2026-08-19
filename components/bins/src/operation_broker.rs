//! C8.7 bounded native operations: the fabric side.
//!
//! An `Operation<Goal, Feedback, Result>` is not a new transport. It is a
//! composition of the two this fabric already brokers — a bounded call and a
//! bounded stream — held together by one correlation key. This module owns that
//! composition:
//!
//! - **Start goal** is a call. The client sends `KIND_GOAL`; the transport
//!   answers exactly once with `KIND_ACCEPTED` carrying `STATUS_SUCCESS` or
//!   `STATUS_REJECTED`. Acceptance is the server's answer, not the fabric's
//!   policy: the fabric forwards and correlates, never decides.
//! - **Feedback** is a stream keyed by `operation_id`, numbered from one, and
//!   bounded by the declared depth. Feedback after a terminal state is dropped
//!   rather than delivered, because a client that has been told the outcome must
//!   never see progress afterwards.
//! - **Result** is a call, retrievable once terminally and then retained for a
//!   declared window so a client that missed the push can still ask.
//! - **Cancellation** is a request, never a command: the client asks, the
//!   transport reports `STATUS_CANCEL_REQUESTED`, and the server still owns the
//!   outcome.
//!
//! **Authority is per role, per operation.** Every leg is routed only to the
//! holder of the exact operation-role capability. A client is authenticated by
//! the control endpoint init bound to it at spawn, never by the identity a
//! record claims — so knowing another client's `operation_id` buys nothing:
//! observation, result retrieval, and cancellation all check that the operation
//! belongs to the asking endpoint. That is what makes two concurrent operations
//! non-cross-correlatable rather than merely unlikely to collide.
//!
//! **Everything is bounded before admission.** Active operations, feedback
//! depth, retained results, and retries all come from the authenticated
//! generation graph via the build-time profile, so no table here grows with
//! traffic. Application goal policy and the ROS action state machine stay
//! outside: `status` names transport outcomes only.

use boot_contracts::fabric_graph::{DIRECTION_CLIENT, DIRECTION_SERVER};
use slime_proto::fabric_operation::{
    FORMAT_VERSION, KIND_ACCEPTED, KIND_CANCEL, KIND_FEEDBACK, KIND_GOAL, KIND_RESULT,
    KIND_RESULT_REQUEST, KIND_SERVER_IDLE, KIND_TERMINAL, OPERATION_MAGIC, STATUS_ACTIVE,
    STATUS_CANCEL_REQUESTED, STATUS_CANCELLED, STATUS_DUPLICATE, STATUS_EXPIRED, STATUS_MALFORMED,
    STATUS_PEER_DEAD, STATUS_REJECTED, STATUS_RETRY_EXHAUSTED, STATUS_STALE, STATUS_SUCCESS,
    STATUS_TIMEOUT, WireOperationEnvelope,
};
use slime_proto::fabric_time::WireTimeAdvance;
use slime_proto::interface_schema::navigation_operation;

#[allow(dead_code)]
mod fabric_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}
// Included once per binary by the binary itself, because `fabric-service`
// includes both brokers and a file may be a module only once in a crate.
use super::trace_log;
use fabric_profile::*;
use slime_rt::{ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

const ROUTE_NAME: &str = "navigation";
const BACKUP_ROUTE_NAME: &str = "nav-backup";
const INTERFACE_NAME: &str = "NavigationOperation";
/// The session the fabric presents to the server. Distinct from every client
/// session, so a client cannot forge a record that looks like it came from the
/// transport itself.
const SERVER_SESSION: u64 = 0x000f_0000_0000_0001;
const CLIENTS: usize = 2;
/// Active operations, bounded by the graph's declared `inFlightOperations`.
const MAX_OPERATIONS: usize = FABRIC_MAX_IN_FLIGHT_OPERATIONS;
/// Retained terminal results, bounded by the graph's `retainedSamples`.
const MAX_RETAINED: usize = FABRIC_MAX_RETAINED_SAMPLES;
/// Pending mandatory records are bounded from authenticated graph ceilings.
/// Each client can contribute at most the request-event share while reads are
/// enabled, plus accepted/result/terminal records for every admitted active
/// operation. Feedback remains lossy at its declared history bound.
const MAX_PENDING_REQUEST_DELIVERIES_PER_CLIENT: usize = FABRIC_MAX_EVENT_DEPTH / CLIENTS;
const MAX_PENDING_DELIVERIES: usize =
    CLIENTS * (MAX_PENDING_REQUEST_DELIVERIES_PER_CLIENT + MAX_OPERATIONS * 3);
const RETRY_LIMIT: u8 = FABRIC_MAX_RETRIES;
const DEADLINE_NS: u64 = FABRIC_OPERATION_DEADLINE_NS;
/// How long a terminal result stays retrievable after it lands. A retained
/// result is storage the client has not claimed, so it expires rather than
/// living for the boot.
const RETENTION_NS: u64 = DEADLINE_NS;

/// Where one operation stands, from the transport's point of view only.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Free,
    /// Goal forwarded; the server has not answered accepted/rejected yet.
    Starting,
    /// Accepted and running: feedback may flow, cancellation may be requested.
    Active,
    /// The client asked to cancel and the server has been told. The server still
    /// owns the outcome, so this is not terminal.
    CancelRequested,
}

/// One live operation. `client_index` is the authority key: every leg is checked
/// against it, so an operation is only ever observable by the endpoint that
/// started it.
#[derive(Clone, Copy)]
struct Operation {
    phase: Phase,
    /// Identity as the client named it, and as the fabric renamed it for the
    /// server. Two clients may pick the same id; the fabric keeps them distinct.
    client_operation_id: u64,
    server_operation_id: u64,
    client_session: u64,
    client_slot: u32,
    client_index: u8,
    /// Highest feedback sequence consumed and number of samples admitted. The
    /// former rejects replay/reordering; the latter enforces the graph's
    /// history-depth ceiling even when sequence numbers jump.
    feedback_sequence: u32,
    feedback_samples: u32,
    deadline_ns: u64,
}

impl Operation {
    const EMPTY: Self = Self {
        phase: Phase::Free,
        client_operation_id: 0,
        server_operation_id: 0,
        client_session: 0,
        client_slot: 0,
        client_index: 0,
        feedback_sequence: 0,
        feedback_samples: 0,
        deadline_ns: 0,
    };
}

/// One terminal result held for later retrieval, with the authority that may
/// retrieve it and the instant it stops being retrievable.
#[derive(Clone, Copy)]
struct Retained {
    live: bool,
    expired: bool,
    operation_id: u64,
    /// The broker's own correlation for this operation.
    ///
    /// The retained table is keyed by the *client's* id, because that is what a
    /// client names when it asks for a result. The trace correlates by the
    /// broker's server-side id everywhere else, so carrying both here keeps one
    /// operation's goal, deadline, retrieval, and expiry records in a single
    /// namespace -- otherwise a reader could not follow an operation across its
    /// own lifetime.
    server_operation_id: u64,
    client_index: u8,
    status: i32,
    payload: [u8; 16],
    payload_len: u32,
    expires_ns: u64,
    /// Monotonic insertion order, used to break equal-expiry eviction ties.
    age: u64,
}

impl Retained {
    const EMPTY: Self = Self {
        live: false,
        expired: false,
        operation_id: 0,
        server_operation_id: 0,
        client_index: 0,
        status: 0,
        payload: [0; 16],
        payload_len: 0,
        expires_ns: 0,
        age: 0,
    };
}

#[derive(Clone, Copy)]
struct PendingDelivery {
    client_index: u8,
    slot: u32,
    order: u64,
    record: WireOperationEnvelope,
}

pub struct Broker {
    replacement_control: u32,
    replacement_start: Option<u32>,
    replacement_supervision: u32,
    /// The server request whose final reply record has not yet been fenced.
    ///
    /// A native Endpoint permits two senders to block at once. Forwarding a
    /// goal or cancel while the single-threaded server still owes a blocking
    /// reply leaves broker and server blocked sending to each other. The server
    /// therefore emits `KIND_SERVER_IDLE` after every handled request, and only
    /// that matching record clears this guard. A logical operation may remain
    /// active after the request is fenced.
    server_request: Option<u64>,
    time_control: u32,
    supervision: [u32; CLIENTS + 1],
    clients: [Option<u32>; CLIENTS],
    server_slot: Option<u32>,
    replacement_control_closed: bool,
    backup_route_slot: Option<u32>,
    operations: [Operation; MAX_OPERATIONS],
    retained: [Retained; MAX_RETAINED],
    pending_deliveries: [Option<PendingDelivery>; MAX_PENDING_DELIVERIES],
    /// One server-bound request per client taken while the server owes replies.
    deferred_requests: [Option<WireOperationEnvelope>; CLIENTS],
    high_water: [u64; CLIENTS],
    /// The most operations this broker has ever held live at once.
    ///
    /// C8.11 records resource *high-water* evidence: a fact about the run, not
    /// about the table. `MAX_OPERATIONS` would be identical in every boot and
    /// every graph, so an occupancy regression would leave the artifact
    /// unchanged. Distinct from `high_water` above, which is a per-client
    /// operation-id watermark for duplicate suppression.
    peak_operations: u32,
    /// The most terminal results this broker has ever retained live at once,
    /// awaiting retrieval or expiry.
    peak_retained: u32,
    client_sessions: [u64; CLIENTS],
    next_server_operation_id: u64,
    next_retained_age: u64,
    next_pending_order: u64,
    now_ns: u64,
    time_closed: bool,
    server_settled: bool,
    /// C8.11's bounded semantic trace for this worker. One sink per worker: the
    /// three route workers are separate tasks, and a shared sink would need an
    /// IPC round trip per record.
    trace: trace_log::Trace,
    /// The `navigation` route identity, folded in `run` because the fold hashes
    /// and the constructor is `const`.
    route: u64,
}

impl Broker {
    pub const fn new(
        clients: [u32; CLIENTS],
        server_slot: u32,
        time_control: u32,
        replacement_control: u32,
        replacement_start: Option<u32>,
        backup_route_slot: u32,
        supervision: [u32; CLIENTS + 1],
        replacement_supervision: u32,
    ) -> Self {
        Self {
            replacement_control,
            replacement_start,
            replacement_supervision,
            server_request: None,
            time_control,
            supervision,
            clients: [Some(clients[0]), Some(clients[1])],
            replacement_control_closed: false,
            server_slot: Some(server_slot),
            operations: [Operation::EMPTY; MAX_OPERATIONS],
            backup_route_slot: Some(backup_route_slot),
            retained: [Retained::EMPTY; MAX_RETAINED],
            pending_deliveries: [None; MAX_PENDING_DELIVERIES],
            deferred_requests: [None; CLIENTS],
            high_water: [0; CLIENTS],
            peak_operations: 0,
            peak_retained: 0,
            client_sessions: [client_session(0), client_session(1)],
            next_server_operation_id: 1,
            next_retained_age: 1,
            next_pending_order: 1,
            now_ns: 0,
            time_closed: false,
            server_settled: false,
            trace: trace_log::Trace::new(FABRIC_TRACE_DEPTH),
            route: 0,
        }
    }

    /// Retire the server: record its death once, on whichever path observes it.
    ///
    /// Four paths learn the server is gone -- the supervision handle, and an
    /// `ERR_PEER_DEAD` from a goal forward, a cancel forward, or a reply read --
    /// and they race. Wiring the trace record to the supervision arm alone made
    /// the evidence depend on which observation won: `settle_all` also sets
    /// `server_settled`, so `finished()` could become true on an endpoint-error
    /// path and the worker would flush a complete, terminal-bearing trace that
    /// simply lacked its peer-death record, with nothing reporting the omission.
    ///
    /// Idempotent: the second observation of one death records nothing.
    fn retire_server(&mut self) {
        if self.server_slot.is_none() {
            return;
        }
        self.server_slot = None;
        let _ = self.trace.peer_death(self.route);
    }

    /// Record one refusal.
    ///
    /// Carries the refusal status and neither a route identity nor a
    /// correlation: a denial that named the edge would confirm the edge exists,
    /// and one that echoed the caller's operation id would republish an identity
    /// the broker just refused -- which on this plane may belong to another
    /// client entirely.
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

    /// Record the live-operation and retained-result occupancy if this sweep
    /// set a new peak.
    ///
    /// Sampled between sweeps rather than at each claim site: a sweep can start
    /// and settle several operations, and what is worth reporting is how many
    /// were live at once.
    fn sample_peak(&mut self) {
        let live = self
            .operations
            .iter()
            .filter(|operation| operation.phase != Phase::Free)
            .count() as u32;
        if live > self.peak_operations {
            self.peak_operations = live;
        }
        let retained = self.retained.iter().filter(|entry| entry.live).count() as u32;
        if retained > self.peak_retained {
            self.peak_retained = retained;
        }
    }

    pub fn run(&mut self) {
        self.verify_graph();
        self.route = trace_log::route_word(&boot_contracts::fabric_graph::route_identity(
            ROUTE_NAME,
            &navigation_operation::INTERFACE_IDENTITY,
            boot_contracts::fabric_graph::CONTRACT_KIND_OPERATION,
        ));
        let _ = self.trace.edge(
            slime_proto::fabric_trace::KIND_ROUTE,
            slime_proto::fabric_trace::ORDER_DATA,
            self.route,
            0,
            0,
            0,
        );
        slime_rt::debug_write(b"[fabric] operation endpoints ready\n");
        loop {
            let mut progressed = self.pump_pending_deliveries();
            for index in 0..CLIENTS {
                progressed |= self.observe_client_death(index);
            }
            let mut client_progress = false;
            for index in 0..CLIENTS {
                if self.server_request.is_none()
                    && let Some(record) = self.deferred_requests[index].take()
                    && let Some(slot) = self.clients[index]
                {
                    self.dispatch_client(index, slot, record);
                    client_progress = true;
                }
                if self.deferred_requests[index].is_none()
                    && self.clients[index].is_some()
                    && self.can_receive_client(index)
                {
                    client_progress |= self.pump_client(index);
                }
                if index == 1 && self.clients[index].is_none() {
                    client_progress |= self.pump_replacement(index);
                }
            }
            // Sample before the early restart below, not after `pump_time`: that
            // `continue` fires on exactly the sweeps where clients admitted
            // operations, so sampling later skipped the sweeps most likely to
            // hold the run's true peak -- and which sweeps were skipped depended
            // on client send timing, making the reported figure differ between
            // two boots of one generation.
            self.sample_peak();
            if client_progress {
                // Restart the sweep before consuming a server reply. A client
                // may send its next control record immediately after receiving
                // the record this sweep produced -- notably a duplicate cancel
                // after `STATUS_CANCEL_REQUESTED`. Consuming the server's result
                // first would block delivering it to a client that is itself
                // blocked sending that control record. The next sweep drains
                // the client endpoint first, then the server can answer.
                continue;
            }
            progressed |= self.observe_server_death();
            progressed |= self.pump_server();
            progressed |= self.pump_backup_route();
            progressed |= self.pump_time();
            if self.finished() {
                // The most operations this run ever held live at once, paired
                // with the count held right here -- which `finished` already
                // establishes is zero, since every operation is `Phase::Free`.
                // The table's *capacity* would be the same number in every
                // boot, so it would carry no evidence at all.
                let _ = self.trace.resource(
                    slime_proto::fabric_trace::RESOURCE_OPERATIONS,
                    self.peak_operations,
                );
                let _ = self
                    .trace
                    .resource(slime_proto::fabric_trace::RESOURCE_OPERATIONS, 0);
                // Retained results are the one table `finished` does not
                // require empty: an unclaimed result stays live until it
                // expires even after every client is gone, since expiry -- not
                // client presence -- is what ends its retrievability window. So
                // the baseline here is read fresh rather than assumed zero.
                let _ = self.trace.resource(
                    slime_proto::fabric_trace::RESOURCE_RETAINED,
                    self.peak_retained,
                );
                let live_retained = self.retained.iter().filter(|entry| entry.live).count() as u32;
                let _ = self
                    .trace
                    .resource(slime_proto::fabric_trace::RESOURCE_RETAINED, live_retained);
                let _ = self.trace.terminal();
                self.trace.flush(b"operation");
                slime_rt::debug_write(b"[fabric] operation state reclaimed\n");
                return;
            }
            if progressed {
                continue;
            }
            slime_rt::yield_now();
        }
    }

    fn can_receive_client(&self, client: usize) -> bool {
        self.pending_deliveries
            .iter()
            .flatten()
            .filter(|pending| pending.client_index as usize == client)
            .count()
            < MAX_PENDING_REQUEST_DELIVERIES_PER_CLIENT
    }

    fn has_pending_delivery(&self, slot: u32) -> bool {
        self.pending_deliveries
            .iter()
            .flatten()
            .any(|pending| pending.slot == slot)
    }

    /// The plane is done when no operation is active and nothing can still ask
    /// for anything.
    ///
    /// A retained result only holds the fabric open while a client could still
    /// claim it. Once every client endpoint is gone the entry is unclaimable by
    /// construction, so waiting for it to expire would park on sources that will
    /// never be ready — a hang, not patience. The server must also be settled, so
    /// no in-flight work is abandoned silently.
    fn finished(&self) -> bool {
        self.operations
            .iter()
            .all(|operation| operation.phase == Phase::Free)
            && self.pending_deliveries.iter().all(Option::is_none)
            && self.deferred_requests.iter().all(Option::is_none)
            && self.server_settled
            && self.backup_route_slot.is_none()
    }

    /// Echo one bounded liveness probe on the unrelated operation route. This
    /// endpoint is independent of the primary route and server supervision, so
    /// a successful post-fault exchange proves the fault did not tear it down.
    fn pump_backup_route(&mut self) -> bool {
        let Some(slot) = self.backup_route_slot else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.backup_route_slot = None;
                let _ = slime_rt::cap_drop(slot);
                return true;
            }
            value if value < 0 => fail(b"backup operation recv"),
            value => value as usize,
        };
        release_caps(&caps);
        // The participant's post-transfer rights probe is intentionally
        // one-way; consume it before the milestone's liveness exchange.
        if length == 5 && &bytes[..5] == b"probe" {
            return true;
        }
        if length != 1 || bytes[0] != 0xa7 {
            fail(b"backup operation probe");
        }
        loop {
            match slime_rt::send(slot, &bytes[..1], &[]) {
                ERR_SUCCESS => break,
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                ERR_PEER_DEAD => {
                    self.backup_route_slot = None;
                    let _ = slime_rt::cap_drop(slot);
                    return true;
                }
                _ => fail(b"backup operation reply"),
            }
        }
        slime_rt::debug_write(b"[fabric] unrelated operation route live\n");
        true
    }

    fn verify_graph(&self) {
        let declared_clients: [&[u8]; CLIENTS] = [b"fabric-op-client", b"fabric-op-client-b"];
        for component in declared_clients {
            if declared_edges(component, ROUTE_NAME, DIRECTION_CLIENT) != 1 {
                fail(b"operation client graph declaration");
            }
        }
        if declared_edges(b"fabric-op-client", BACKUP_ROUTE_NAME, DIRECTION_CLIENT) != 1 {
            fail(b"backup operation graph declaration");
        }
        if declared_edges(b"fabric-op-server", ROUTE_NAME, DIRECTION_SERVER) != 1 {
            fail(b"operation server graph declaration");
        }
    }

    fn pump_replacement(&mut self, client: usize) -> bool {
        if client != 1 || self.replacement_control_closed || self.clients[client].is_some() {
            return false;
        }
        if declared_edges(b"fabric-op-client-b-restart", ROUTE_NAME, DIRECTION_CLIENT) != 1 {
            fail(b"replacement operation graph declaration");
        }
        self.clients[client] = Some(self.replacement_control);
        self.supervision[client] = self.replacement_supervision;
        self.replacement_control_closed = true;
        // Every caller of `Broker::new` declares this barrier now: the
        // standalone C8.7 plane names it directly, and C8.13's worker names
        // its own copy (`fabric-op-worker::RESTART_START_SLOT`) since it drives
        // the same restart scenario for real. `None` stays representable only
        // because the boot-parked plane's replacement never exits and so never
        // reaches this arm; reaching it with no barrier declared would be a
        // composition failure, not permission to guess a slot in another
        // CSpace.
        let Some(start) = self.replacement_start else {
            fail(b"replacement start absent")
        };
        if slime_rt::send(start, &[1], &[]) != ERR_SUCCESS {
            fail(b"replacement start")
        }
        slime_rt::debug_write(b"[fabric] operation participant restarted\n");
        true
    }

    /// Consume at most one record from one client's role endpoint.
    fn pump_client(&mut self, client: usize) -> bool {
        let Some(slot) = self.clients[client] else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.clients[client] = None;
                self.reclaim_client(client, slot);
                return true;
            }
            value if value < 0 => fail(b"client recv"),
            value => value as usize,
        };
        release_caps(&caps);
        let Some(record) = WireOperationEnvelope::decode(&bytes[..length.min(MAX_MSG)]) else {
            return true;
        };
        if length != MAX_MSG
            || !slime_proto::valid_operation_envelope(&record, navigation_operation::TYPE_TAG)
        {
            self.queue_terminal(
                client,
                slot,
                self.client_sessions[client],
                record.operation_id.max(1),
                STATUS_MALFORMED,
            );
            slime_rt::debug_write(b"[fabric] malformed operation record rejected\n");
            return true;
        }
        if record.session != self.client_sessions[client] {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_STALE,
            );
            slime_rt::debug_write(b"[fabric] stale operation session rejected\n");
            return true;
        }
        self.dispatch_client(client, slot, record);
        true
    }

    fn dispatch_client(&mut self, client: usize, slot: u32, record: WireOperationEnvelope) {
        match record.kind {
            KIND_GOAL if self.server_request.is_some() => {
                self.deferred_requests[client] = Some(record);
            }
            // No trace here: emission belongs after the authorization and
            // admission decisions these calls make, so the artifact records
            // outcomes rather than client-supplied attempts. Tracing at this
            // point wrote a matched record naming an operation id the caller may
            // not own -- this plane's scenario has client B ask for client A's
            // result, which `retrieve` deliberately refuses without revealing
            // whether the id was unknown or merely someone else's.
            KIND_GOAL => self.start(client, slot, record),
            KIND_CANCEL => self.request_cancel(client, slot, record),
            KIND_RESULT_REQUEST => self.retrieve(client, slot, record),
            _ => {
                self.queue_terminal(
                    client,
                    slot,
                    record.session,
                    record.operation_id,
                    STATUS_STALE,
                );
                slime_rt::debug_write(b"[fabric] client role authority denied\n");
            }
        }
    }

    /// Admit one goal, or refuse it with the exact reason.
    fn start(&mut self, client: usize, slot: u32, record: WireOperationEnvelope) {
        // A goal that does not advance the client's high water mark is a
        // duplicate or a replay. Refusing it is what stops a restarted
        // participant from starting the same work twice.
        if record.operation_id <= self.high_water[client] {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_DUPLICATE,
            );
            self.trace_denial(STATUS_DUPLICATE);
            slime_rt::debug_write(b"[fabric] duplicate operation goal rejected\n");
            return;
        }
        let Some(server) = self.server_slot else {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_PEER_DEAD,
            );
            return;
        };
        let Some(index) = self
            .operations
            .iter()
            .position(|operation| operation.phase == Phase::Free)
        else {
            // The declared active-operation ceiling is a real limit: refusing is
            // bounded and reported, queueing without bound is not.
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_REJECTED,
            );
            slime_rt::debug_write(b"[fabric] operation capacity exhausted\n");
            return;
        };
        // No server request may still owe a reply record here. Logical
        // operations can remain active after the server's explicit idle fence.
        let server_operation_id = self.next_server_operation_id;
        self.next_server_operation_id = self
            .next_server_operation_id
            .checked_add(1)
            .unwrap_or_else(|| fail(b"operation correlation exhausted"));
        let mut outward = record;
        outward.session = SERVER_SESSION;
        outward.operation_id = server_operation_id;
        match slime_rt::send(server, &outward.encode(), &[]) {
            ERR_SUCCESS => {
                self.high_water[client] = record.operation_id;
                self.server_request = Some(server_operation_id);
                // Admitted and forwarded: the correlation is the broker's own
                // server-side identity, the same namespace the deadline and
                // expiry records use, so a reader can follow one operation from
                // its goal to its outcome without crossing namespaces.
                let _ = self.trace.edge(
                    slime_proto::fabric_trace::KIND_OPERATION,
                    slime_proto::fabric_trace::ORDER_DATA,
                    self.route,
                    server_operation_id,
                    0,
                    slime_proto::fabric_qos::EVENT_MATCHED,
                );
            }
            ERR_WOULDBLOCK => {
                // No operation identity has been consumed yet, so the caller
                // may retry the same goal after the bounded refusal.
                self.queue_terminal(
                    client,
                    slot,
                    record.session,
                    record.operation_id,
                    STATUS_REJECTED,
                );
                return;
            }
            ERR_PEER_DEAD => {
                self.queue_terminal(
                    client,
                    slot,
                    record.session,
                    record.operation_id,
                    STATUS_PEER_DEAD,
                );
                self.retire_server();
                self.settle_all(STATUS_PEER_DEAD);
                return;
            }
            _ => fail(b"operation goal forward"),
        }
        self.operations[index] = Operation {
            phase: Phase::Starting,
            client_operation_id: record.operation_id,
            server_operation_id,
            client_session: record.session,
            client_slot: slot,
            client_index: client as u8,
            feedback_sequence: 0,
            feedback_samples: 0,
            deadline_ns: self.now_ns.saturating_add(DEADLINE_NS),
        };
        slime_rt::debug_write(b"[fabric] operation goal forwarded\n");
    }

    /// Relay a cancellation *request*. The client asks; the server decides. The
    /// transport only reports that the ask was registered, which is why this
    /// never produces a terminal outcome by itself.
    fn request_cancel(&mut self, client: usize, slot: u32, record: WireOperationEnvelope) {
        // Authority is the (client index, operation id) pair, so knowing another
        // client's identity does not make it cancellable.
        let Some(index) = self.find_client_operation(client, record.operation_id) else {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_STALE,
            );
            slime_rt::debug_write(b"[fabric] unauthorized operation cancel denied\n");
            return;
        };
        if self.operations[index].phase == Phase::CancelRequested {
            self.answer(index, STATUS_CANCEL_REQUESTED);
            return;
        }
        if self.server_request.is_some() {
            self.deferred_requests[client] = Some(record);
            return;
        }
        let Some(server) = self.server_slot else {
            self.settle(index, STATUS_PEER_DEAD);
            return;
        };
        let mut outward = record;
        outward.session = SERVER_SESSION;
        outward.operation_id = self.operations[index].server_operation_id;
        match slime_rt::send(server, &outward.encode(), &[]) {
            ERR_SUCCESS => {
                self.server_request = Some(self.operations[index].server_operation_id);
                self.operations[index].phase = Phase::CancelRequested;
                self.answer(index, STATUS_CANCEL_REQUESTED);
                slime_rt::debug_write(b"[fabric] operation cancel requested\n");
            }
            ERR_WOULDBLOCK => {
                let status = if RETRY_LIMIT == 0 {
                    STATUS_REJECTED
                } else {
                    STATUS_RETRY_EXHAUSTED
                };
                self.settle(index, status);
                slime_rt::debug_write(b"[fabric] operation cancel retry exhausted\n");
            }
            ERR_PEER_DEAD => {
                self.retire_server();
                self.settle_all(STATUS_PEER_DEAD);
            }
            _ => fail(b"operation cancel forward"),
        }
    }

    /// Answer a retained-result retrieval.
    ///
    /// The retained entry carries the client index that owns it, so a caller who
    /// knows an operation identity it did not start is refused exactly like one
    /// naming an identity that never existed — the denial leaks nothing about
    /// which of the two it was.
    fn retrieve(&mut self, client: usize, slot: u32, record: WireOperationEnvelope) {
        let found = self.retained.iter().position(|entry| {
            (entry.live || entry.expired)
                && entry.operation_id == record.operation_id
                && entry.client_index as usize == client
        });
        let Some(index) = found else {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_STALE,
            );
            self.trace_denial(STATUS_STALE);
            slime_rt::debug_write(b"[fabric] unauthorized operation result denied\n");
            return;
        };
        // Authorized: only now does the correlation name an operation this
        // client owns, so only now may the artifact carry it.
        let _ = self.trace.edge(
            slime_proto::fabric_trace::KIND_OPERATION,
            slime_proto::fabric_trace::ORDER_ACK,
            self.route,
            self.retained[index].server_operation_id,
            0,
            slime_proto::fabric_qos::EVENT_MATCHED,
        );
        let entry = self.retained[index];
        if entry.expired {
            self.queue_terminal(
                client,
                slot,
                record.session,
                record.operation_id,
                STATUS_EXPIRED,
            );
            slime_rt::debug_write(b"[fabric] expired operation result reported\n");
            return;
        }
        let reply = WireOperationEnvelope {
            magic: OPERATION_MAGIC,
            version: FORMAT_VERSION,
            kind: KIND_RESULT,
            status: entry.status,
            session: self.client_sessions[client],
            operation_id: entry.operation_id,
            type_identity: navigation_operation::TYPE_TAG,
            sequence: 0,
            payload_len: entry.payload_len,
            payload: entry.payload,
        };
        if self.queue_delivery(client, slot, reply, b"operation result delivery") {
            // Claim on acceptance into the bounded delivery queue, not on the
            // eventual syscall send. A repeated request cannot create a second
            // reply while the first is waiting for endpoint capacity.
            self.retained[index] = Retained::EMPTY;
            slime_rt::debug_write(b"[fabric] operation result retrieved\n");
        }
    }

    /// Consume at most one record from the server's role endpoint.
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
                self.settle_all(STATUS_PEER_DEAD);
                slime_rt::debug_write(b"[fabric] operation peer death propagated\n");
                return true;
            }
            value if value < 0 => fail(b"server recv"),
            value => value as usize,
        };
        release_caps(&caps);
        let Some(record) = WireOperationEnvelope::decode(&bytes[..length.min(MAX_MSG)]) else {
            return true;
        };
        if length != MAX_MSG
            || !slime_proto::valid_operation_envelope(&record, navigation_operation::TYPE_TAG)
            || record.session != SERVER_SESSION
        {
            if let Some(index) = self.find_server_operation(record.operation_id) {
                self.settle(index, STATUS_MALFORMED);
                slime_rt::debug_write(b"[fabric] malformed operation reply rejected\n");
            }
            return true;
        }
        // The fence follows the final record, which may already have settled
        // and freed its operation. Match it against the outstanding request
        // before looking for active operation state.
        if record.kind == KIND_SERVER_IDLE {
            if self.server_request == Some(record.operation_id) {
                self.server_request = None;
            } else {
                slime_rt::debug_write(b"[fabric] stale operation server-idle rejected\n");
            }
            return true;
        }
        let Some(index) = self.find_server_operation(record.operation_id) else {
            slime_rt::debug_write(b"[fabric] stale operation reply rejected\n");
            return true;
        };
        match record.kind {
            KIND_ACCEPTED => self.accepted(index, record),
            KIND_FEEDBACK => self.feedback(index, record),
            KIND_RESULT => self.result(index, record),
            // Goals, cancels, retrievals and terminals are not the server's to
            // send on this route.
            _ => {
                self.settle(index, STATUS_MALFORMED);
                slime_rt::debug_write(b"[fabric] server role authority denied\n");
            }
        }
        true
    }

    /// The server's answer to a goal. Acceptance is the only path to `Active`,
    /// so feedback can never precede it.
    fn accepted(&mut self, index: usize, record: WireOperationEnvelope) {
        if self.operations[index].phase != Phase::Starting {
            // A second acceptance for one goal is a duplicate answer, not a
            // state change.
            slime_rt::debug_write(b"[fabric] duplicate operation acceptance rejected\n");
            return;
        }
        if record.status == STATUS_REJECTED {
            self.settle(index, STATUS_REJECTED);
            slime_rt::debug_write(b"[fabric] operation rejected\n");
            return;
        }
        self.operations[index].phase = Phase::Active;
        self.answer(index, STATUS_ACTIVE);
        slime_rt::debug_write(b"[fabric] operation accepted\n");
    }

    /// One feedback sample, keyed to the operation and ordered within it.
    fn feedback(&mut self, index: usize, record: WireOperationEnvelope) {
        let operation = self.operations[index];
        if !matches!(operation.phase, Phase::Active | Phase::CancelRequested) {
            slime_rt::debug_write(b"[fabric] feedback outside active state dropped\n");
            return;
        }
        // Sequence must advance. A replayed or reordered sample is dropped
        // rather than delivered, so a client's view of progress is monotonic.
        if record.sequence <= operation.feedback_sequence {
            slime_rt::debug_write(b"[fabric] stale operation feedback dropped\n");
            return;
        }
        let depth = declared_history_depth(operation.client_index as usize);
        if self.operations[index].feedback_samples >= depth
            || self.has_pending_delivery(operation.client_slot)
        {
            slime_rt::debug_write(b"[fabric] operation feedback dropped at bound\n");
            return;
        }
        let mut outward = record;
        outward.session = operation.client_session;
        outward.operation_id = operation.client_operation_id;
        match slime_rt::send(operation.client_slot, &outward.encode(), &[]) {
            ERR_SUCCESS => {
                self.operations[index].feedback_sequence = record.sequence;
                self.operations[index].feedback_samples += 1;
                slime_rt::debug_write(b"[fabric] operation feedback routed\n");
            }
            // Feedback is progress, not an outcome. A full endpoint drops the
            // sample without consuming its sequence, so the server may retry.
            ERR_WOULDBLOCK => {
                slime_rt::debug_write(b"[fabric] operation feedback dropped at bound\n");
            }
            ERR_PEER_DEAD => self.drop_dead_client(index),
            _ => fail(b"operation feedback delivery"),
        }
    }

    /// The server's terminal result. Delivered once, then retained for the
    /// declared window so a client that missed the push can still retrieve it.
    fn result(&mut self, index: usize, record: WireOperationEnvelope) {
        let operation = self.operations[index];
        if operation.phase == Phase::Starting {
            // A result before acceptance would leave the client never told
            // whether its goal was taken. Treat the result as the acceptance.
            self.operations[index].phase = Phase::Active;
            self.answer(index, STATUS_ACTIVE);
            // `answer` reclaims the operation when the client endpoint died.
            // Never continue with the stale local copy in that case.
            if self.operations[index].phase == Phase::Free {
                return;
            }
        }
        let status = match record.status {
            STATUS_REJECTED => STATUS_REJECTED,
            STATUS_CANCELLED => STATUS_CANCELLED,
            _ => STATUS_SUCCESS,
        };
        let mut outward = record;
        outward.session = operation.client_session;
        outward.operation_id = operation.client_operation_id;
        outward.status = status;
        let delivered = self.queue_delivery(
            operation.client_index as usize,
            operation.client_slot,
            outward,
            b"operation result delivery",
        );
        // Retained either way: a delivered result is still claimable until it
        // expires, which is what makes a client restart survivable.
        self.retain(index, status, record.payload, record.payload_len);
        self.settle(index, status);
        if delivered {
            slime_rt::debug_write(b"[fabric] operation result routed\n");
        }
    }
    /// Record one terminal result for later retrieval, bounded by the declared
    /// retained-sample ceiling. At capacity the oldest expiry is displaced: the
    /// bound is fixed, so something must give, and the entry closest to expiring
    /// is the one least likely to still be wanted.
    fn retain(&mut self, index: usize, status: i32, payload: [u8; 16], payload_len: u32) {
        let operation = self.operations[index];
        let age = self.next_retained_age;
        self.next_retained_age = self
            .next_retained_age
            .checked_add(1)
            .unwrap_or_else(|| fail(b"retained age exhausted"));
        let entry = Retained {
            live: true,
            expired: false,
            operation_id: operation.client_operation_id,
            server_operation_id: operation.server_operation_id,
            client_index: operation.client_index,
            status,
            payload,
            payload_len,
            expires_ns: self.now_ns.saturating_add(RETENTION_NS),
            age,
        };
        if let Some(free) = self
            .retained
            .iter()
            .position(|slot| !slot.live && !slot.expired)
        {
            self.retained[free] = entry;
            return;
        }
        let oldest = self
            .retained
            .iter()
            .enumerate()
            .min_by_key(|(_, slot)| (slot.expires_ns, slot.age))
            .map(|(slot, _)| slot)
            .unwrap_or(0);
        self.retained[oldest] = entry;
        slime_rt::debug_write(b"[fabric] retained operation result displaced\n");
    }

    /// Send the transport's `KIND_ACCEPTED` progress answer for one operation.
    fn answer(&mut self, index: usize, status: i32) {
        let operation = self.operations[index];
        let record = WireOperationEnvelope {
            magic: OPERATION_MAGIC,
            version: FORMAT_VERSION,
            kind: KIND_ACCEPTED,
            status,
            session: operation.client_session,
            operation_id: operation.client_operation_id,
            type_identity: navigation_operation::TYPE_TAG,
            sequence: 0,
            payload_len: 0,
            payload: [0; 16],
        };
        self.queue_delivery(
            operation.client_index as usize,
            operation.client_slot,
            record,
            b"operation acceptance delivery",
        );
    }

    /// Close one operation with a terminal outcome and free its entry.
    fn settle(&mut self, index: usize, status: i32) {
        let operation = self.operations[index];
        if operation.phase == Phase::Free {
            return;
        }
        self.operations[index] = Operation::EMPTY;
        self.queue_terminal(
            operation.client_index as usize,
            operation.client_slot,
            operation.client_session,
            operation.client_operation_id,
            status,
        );
    }

    /// Emit one terminal record or queue it until the client endpoint has send
    /// capacity. All mandatory records share one ordered bounded queue, so an
    /// acceptance can never be overtaken by feedback, result, or terminal state.
    fn queue_terminal(
        &mut self,
        client: usize,
        slot: u32,
        session: u64,
        operation_id: u64,
        status: i32,
    ) {
        let record = terminal_record(session, operation_id, status);
        self.queue_delivery(client, slot, record, b"operation terminal");
    }

    /// Send a mandatory record now or retain it in graph-bounded FIFO state.
    /// Returns false only when the endpoint died and its client state was
    /// reclaimed; `ERR_WOULDBLOCK` is successful admission into pending state.
    fn queue_delivery(
        &mut self,
        client: usize,
        slot: u32,
        record: WireOperationEnvelope,
        failure: &[u8],
    ) -> bool {
        if !self.has_pending_delivery(slot) {
            match slime_rt::send(slot, &record.encode(), &[]) {
                ERR_SUCCESS => return true,
                ERR_WOULDBLOCK => {}
                ERR_PEER_DEAD => {
                    self.clients[client] = None;
                    self.reclaim_client(client, slot);
                    let _ = slime_rt::cap_drop(slot);
                    return false;
                }
                _ => fail(failure),
            }
        }
        let order = self.next_pending_order;
        self.next_pending_order = self
            .next_pending_order
            .checked_add(1)
            .unwrap_or_else(|| fail(b"pending delivery order exhausted"));
        let Some(index) = self.pending_deliveries.iter().position(Option::is_none) else {
            fail(b"pending operation delivery bound");
        };
        self.pending_deliveries[index] = Some(PendingDelivery {
            client_index: client as u8,
            slot,
            order,
            record,
        });
        slime_rt::debug_write(b"[fabric] operation delivery queued\n");
        true
    }

    fn pump_pending_deliveries(&mut self) -> bool {
        let mut progressed = false;
        for client in 0..CLIENTS {
            let Some((index, value)) = self
                .pending_deliveries
                .iter()
                .enumerate()
                .filter_map(|(index, pending)| pending.map(|value| (index, value)))
                .filter(|(_, pending)| pending.client_index as usize == client)
                .min_by_key(|(_, pending)| pending.order)
            else {
                continue;
            };
            match slime_rt::send(value.slot, &value.record.encode(), &[]) {
                ERR_SUCCESS => {
                    self.pending_deliveries[index] = None;
                    progressed = true;
                }
                ERR_WOULDBLOCK => {}
                ERR_PEER_DEAD => {
                    self.pending_deliveries[index] = None;
                    self.clients[client] = None;
                    self.reclaim_client(client, value.slot);
                    let _ = slime_rt::cap_drop(value.slot);
                    progressed = true;
                }
                _ => fail(b"pending operation delivery"),
            }
        }
        progressed
    }

    /// Advance simulated time, then expire whatever that made stale.
    ///
    /// Time is the only thing that expires an operation or a retained result, and
    /// it arrives as an explicit capability-routed record — never a poll — so
    /// every deadline transition is deterministic and replayable.
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
            value if value < 0 => fail(b"operation time recv"),
            value => value as usize,
        };
        release_caps(&caps);
        let Some(value) = WireTimeAdvance::decode(&bytes[..length.min(MAX_MSG)]) else {
            return true;
        };
        // Monotonic by contract: time that goes backwards would make expiry
        // order depend on arrival order, so it fails closed instead.
        if length != MAX_MSG
            || !slime_proto::valid_time_advance(&value)
            || value.now_ns < self.now_ns
        {
            fail(b"invalid operation time");
        }
        // Recorded before the expiries it triggers: the advance closes the old
        // instant, and every deadline below belongs to the new one.
        let _ = self.trace.advance(value.now_ns);
        self.now_ns = value.now_ns;
        for index in 0..self.operations.len() {
            if self.operations[index].phase == Phase::Free {
                continue;
            }
            if self.now_ns >= self.operations[index].deadline_ns {
                let status = if self.operations[index].phase == Phase::CancelRequested {
                    STATUS_CANCELLED
                } else {
                    STATUS_TIMEOUT
                };
                let correlation = self.operations[index].server_operation_id;
                self.settle(index, status);
                let _ = self.trace.edge(
                    slime_proto::fabric_trace::KIND_QOS,
                    slime_proto::fabric_trace::ORDER_DATA,
                    self.route,
                    correlation,
                    status,
                    slime_proto::fabric_qos::EVENT_DEADLINE_MISSED,
                );
                if status == STATUS_CANCELLED {
                    slime_rt::debug_write(b"[fabric] operation cancelled\n");
                } else {
                    slime_rt::debug_write(b"[fabric] operation timed out\n");
                }
            }
        }
        for index in 0..self.retained.len() {
            if self.retained[index].live && self.now_ns >= self.retained[index].expires_ns {
                self.retained[index].live = false;
                self.retained[index].expired = true;
                self.retained[index].status = STATUS_EXPIRED;
                self.retained[index].payload = [0; 16];
                self.retained[index].payload_len = 0;
                // A retained result aging out is a lifespan expiry, not a
                // deadline: the operation already ended, and this is the window
                // for retrieving its outcome closing.
                let _ = self.trace.edge(
                    slime_proto::fabric_trace::KIND_QOS,
                    slime_proto::fabric_trace::ORDER_DATA,
                    self.route,
                    self.retained[index].server_operation_id,
                    STATUS_EXPIRED,
                    slime_proto::fabric_qos::EVENT_LIFESPAN_EXPIRED,
                );
                slime_rt::debug_write(b"[fabric] operation result expired\n");
            }
        }
        true
    }

    /// Observe client death before touching its route endpoint. The supervision
    /// handle cannot be masked by an endpoint whose peer died without closing;
    /// clearing the route slot opens the authenticated control path for a
    /// replacement while retained results remain keyed to the client index.
    fn observe_client_death(&mut self, client: usize) -> bool {
        let Some(slot) = self.clients[client] else {
            return false;
        };
        match slime_rt::supervision_status(self.supervision[client]) {
            Ok(None) => false,
            Ok(Some(_)) => {
                self.clients[client] = None;
                self.reclaim_client(client, slot);
                // Client A alone owns the peer of the unrelated backup route.
                // A native Endpoint never reports that peer's death, so its
                // supervision transition is the authoritative close signal.
                // The liveness exchange has completed before A exits.
                if client == 0 {
                    self.backup_route_slot = None;
                }
                // The endpoint itself is not dropped. `cap_drop` addresses the
                // root's logical export table, and a generation-installed
                // native Endpoint was never exported through it.
                true
            }
            Err(_) => fail(b"client supervision"),
        }
    }

    /// Observe server death through its supervision handle rather than inferring
    /// it from a channel, so a server that exits without closing is still seen.
    fn observe_server_death(&mut self) -> bool {
        if self.server_slot.is_none() {
            return false;
        }
        match slime_rt::supervision_status(self.supervision[CLIENTS]) {
            Ok(None) => false,
            Ok(Some(_)) => {
                self.retire_server();
                self.settle_all(STATUS_PEER_DEAD);
                slime_rt::debug_write(b"[fabric] operation peer death propagated\n");
                true
            }
            Err(_) => fail(b"server supervision"),
        }
    }

    /// Locate an operation by the identity its own client named, which is what
    /// binds observation and cancellation to the endpoint that started it.
    fn find_client_operation(&self, client: usize, operation_id: u64) -> Option<usize> {
        self.operations.iter().position(|operation| {
            operation.phase != Phase::Free
                && operation.client_index as usize == client
                && operation.client_operation_id == operation_id
        })
    }

    fn find_server_operation(&self, server_operation_id: u64) -> Option<usize> {
        self.operations.iter().position(|operation| {
            operation.phase != Phase::Free && operation.server_operation_id == server_operation_id
        })
    }

    fn drop_dead_client(&mut self, index: usize) {
        let operation = self.operations[index];
        let client_index = operation.client_index as usize;
        let slot = operation.client_slot;
        self.operations[index] = Operation::EMPTY;
        self.clients[client_index] = None;
        self.reclaim_client(client_index, slot);
    }

    /// Release everything a departed client held. Active operations and queued
    /// endpoint-local terminals are unreachable after restart, but retained
    /// results stay keyed to the authenticated client index so the replacement
    /// can retrieve them through its fresh role.
    fn reclaim_client(&mut self, client: usize, slot: u32) {
        for index in 0..self.operations.len() {
            if self.operations[index].phase != Phase::Free
                && self.operations[index].client_slot == slot
            {
                self.operations[index] = Operation::EMPTY;
            }
        }
        for pending in &mut self.pending_deliveries {
            if pending.is_some_and(|pending| pending.slot == slot) {
                *pending = None;
            }
        }
        self.deferred_requests[client] = None;
    }

    /// Settle every active operation with one status, then mark the server
    /// account closed. Unrelated planes are untouched: this service brokers only
    /// the operation route, so a server fault here cannot disturb a stream or
    /// call route the same fabric carries in another profile.
    fn settle_all(&mut self, status: i32) {
        for index in 0..self.operations.len() {
            self.settle(index, status);
        }
        self.server_settled = true;
        self.server_request = None;
    }
}

/// The graph declares feedback history per authenticated participant/route.
/// Operation clients are in the same order as the control table and broker
/// slots, so lookup stays keyed to the authority index rather than a message.
/// The KEEP_LAST feedback depth the generation declares for one operation
/// client, read from the graph rather than a generated table (B70/CP2).
///
/// This asks about a *client*, not about the component running it, and the two
/// binaries this module compiles into answer that differently:
/// `fabric-service` is the graph's declared holder and reads every row, while
/// `fabric-op-worker` is not. The worker reaches its clients through the second
/// scope instead -- it holds each one's declared control endpoint, so the row is
/// about a peer the generation already bound it to. Neither binary needs to know
/// which one it is, which is why this reads the same on both.
fn declared_history_depth(client: usize) -> u32 {
    let component = match client {
        0 => "fabric-op-client",
        1 => "fabric-op-client-b",
        _ => fail(b"operation history client"),
    };
    let route = boot_contracts::fabric_graph::route_identity(
        ROUTE_NAME,
        &navigation_operation::INTERFACE_IDENTITY,
        boot_contracts::fabric_graph::CONTRACT_KIND_OPERATION,
    );
    let index = slime_rt::graph_route_index(&route)
        .unwrap_or_else(|_| fail(b"operation route is not declared by this graph"));
    let identity = boot_contracts::fabric_graph::component_identity(component);
    slime_components::fabric_self_view::history_depth_of(&identity, index as u32)
        .unwrap_or_else(|| fail(b"operation history depth")) as u32
}

fn declared_edges(component: &[u8], route: &str, direction: u32) -> usize {
    FABRIC_PARTICIPANTS
        .iter()
        .filter(|(name, route_name, interface, declared)| {
            *name == component
                && *route_name == route
                && *interface == INTERFACE_NAME
                && *declared == direction
        })
        .count()
}

/// Per-client session identity. Distinct per client and distinct from
/// `SERVER_SESSION`, so no client can present another's session or the
/// transport's.
const fn client_session(client: usize) -> u64 {
    0x00c2_0000_0000_0001 + client as u64 * 0x0001_0000_0000_0000
}

/// Drop any capability that arrived on a record with no business carrying one,
/// so a malformed peer cannot strand kernel objects in the fabric.
fn terminal_record(session: u64, operation_id: u64, status: i32) -> WireOperationEnvelope {
    WireOperationEnvelope {
        magic: OPERATION_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_TERMINAL,
        status,
        session,
        operation_id,
        type_identity: navigation_operation::TYPE_TAG,
        sequence: 0,
        payload_len: 0,
        payload: [0; 16],
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

const _: () = assert!(slime_proto::fabric_operation::OPERATION_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_time::TIME_ADVANCE_LEN == MAX_MSG);

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
