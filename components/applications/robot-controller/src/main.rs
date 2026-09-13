#![no_std]
#![no_main]

//! C9.6's controller: the one component holding two contract kinds at once.
//!
//! The middle of the sensor -> controller -> actuator chain, and the reason
//! C9.6 is a composition milestone rather than a fifth plane. It is
//! simultaneously:
//!
//! - a **stream subscriber** on the declared `telemetry` route, consuming the
//!   sensor's samples through a broker-provisioned shared ring, and
//! - a **call client** on the declared `parameters` route, issuing one bounded
//!   command per consumed sample and observing its typed outcome.
//!
//! No prior fixture declares one identity on two contract kinds. Both roles are
//! generation-declared authority reached through two separate control endpoints,
//! so neither can be inferred from the other: the stream broker authenticates
//! this component by the endpoint its role request arrived on, and the call
//! broker by the endpoint its request arrived on, and the two are distinct
//! objects installed by the root before this task ran.
//!
//! # The restart, and why it is scripted here
//!
//! On its first incarnation this component *faults* after consuming its second
//! sample. That is C9.6's injected controller restart, and it is scripted rather
//! than induced because a restart is only observable as a restart if the
//! transcript can distinguish it from a slow component: the supervisor observes
//! the death, the root charges the declared attempt, and the replacement comes
//! up holding reissued fabric authority and resumes the chain.
//!
//! Which incarnation this is comes from the *root's* record, not from a counter
//! this component keeps — a per-task counter would reset on the very death it
//! must survive. `lifecycle_state_read().predecessor_cause` is `live` on a first
//! launch and names the previous incarnation's terminal cause afterwards, so the
//! replacement selects its own behaviour from an authenticated observation.
//!
//! The configuration it applies is the parameter its supervisor wrote once,
//! before the first death. Reading it back after the restart is what makes
//! "fresh authority, original configuration" a data claim rather than a marker.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_SUBSCRIBE, route_identity};
use boot_contracts::lifecycle_policy::UNDECLARED_CAUSE_ID;
use boot_contracts::scheduling_class::CLASS_NORMAL;
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FORMAT_VERSION,
    OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_call::{
    CALL_MAGIC, KIND_CANCEL, KIND_REPLY, KIND_REPLY_ACK, KIND_REQUEST, KIND_TERMINAL,
    KIND_TERMINAL_ACK, STATUS_CANCELLED, STATUS_REJECTED, STATUS_SUCCESS, STATUS_TIMEOUT,
    WireCallEnvelope,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{STREAM_EVENT_MAGIC, WireStreamEvent};
use slime_proto::interface_schema::{parameter_call, telemetry_stream};
use slime_proto::ring::{Ring, RingError};
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// Control endpoint to the stream broker, declared by the generation.
const STREAM_CONTROL_SLOT: u32 = 0;

/// Route endpoint to the call broker, declared by the generation. The call plane
/// performs no role handshake: both halves are installed before either task
/// runs, which is what binds this endpoint to this identity.
const CALL_ROUTE_SLOT: u32 = 1;

/// This component's declared wait binding on `robot-controller-telemetry-ready`.
const READY_SLOT: u32 = 2;

/// This component's declared half of `robot-controller-clock-phase`, the
/// barrier that orders the clock's advances after the request they must expire.
const CLOCK_PHASE_SLOT: u32 = 5;

const RING_BASE: u64 = 0x0000_0012_0000_0000;
const RING_BYTES: usize = 4096;

const ROUTE_NAME: &str = "telemetry";

/// The parameter key the supervisor wrote and every incarnation reads back.
const CONFIG_KEY: u64 = 3;

/// The session this component stamps its own requests with. The broker rewrites
/// it before forwarding, so it is a client-side correlation domain rather than
/// authority.
const CLIENT_SESSION: u64 = 0x00c1_0000_0000_0001;

/// Samples consumed before the first incarnation faults.
///
/// Two rather than one: the restart must interrupt a chain that had demonstrably
/// started carrying data, so the transcript shows commands applied both before
/// and after it.
const SAMPLES_BEFORE_FAULT: u32 = 2;

/// The command value the actuator's scenario deliberately never answers, so the
/// route's declared deadline is what settles the request.
///
/// A dedicated sentinel rather than a value outside the actuator's range: an
/// out-of-range command is *refused*, which is a settlement, and a refusal and
/// a timeout are two of the outcomes this plane must keep distinct. The only way
/// to reach a timeout is a request nothing answers at all.
const TIMEOUT_COMMAND: u64 = 4_242;

/// Base for the two terminal requests issued once, after the last sample.
/// Offset well clear of any tick-derived request id: there are five ticks in
/// this scenario, so `TERMINAL_REQUEST_BASE` and `TERMINAL_REQUEST_BASE + 1`
/// can never collide with one.
const TERMINAL_REQUEST_BASE: u64 = 1_000_000;

/// The declared signal slot releasing the actuator once this controller has
/// observed its unanswered request settle `STATUS_TIMEOUT`.
const TIMEOUT_OBSERVED_SLOT: u32 = 5;

/// A command outside the actuator's declared actuation range, issued once so a
/// refusal is an observed outcome rather than an unexercised branch. It must
/// exceed the actuator's `MAX_COMMAND` and differ from `TIMEOUT_COMMAND`: a
/// refusal settles the request, which is exactly what distinguishes it from the
/// deadline miss.
const REJECTED_COMMAND: u64 = 5_000;

fn main(_startup_arg: u32) {
    let class = match slime_rt::scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"class read", error),
    };
    if class.class_id != CLASS_NORMAL {
        fail(b"the controller is not in the declared normal band")
    }

    // Which incarnation this is, from the root's own per-instance record. `live`
    // means nothing preceded this task.
    let state = match slime_rt::lifecycle_state_read() {
        Ok(state) => state,
        Err(error) => fail_with(b"lifecycle state read", error),
    };
    let first_incarnation = state.predecessor_cause == UNDECLARED_CAUSE_ID;
    write_value(
        b"[robot-controller] incarnation cause=",
        state.predecessor_cause as u64,
    );

    // The configuration the supervisor wrote, read through this component's own
    // declared reflexive edge. Read on every incarnation: after the restart this
    // is the observation that parameter state belongs to the declared instance
    // rather than to a task.
    let scale = match slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, CONFIG_KEY)
    {
        Ok(value) => value,
        Err(error) => fail_with(b"parameter read", error),
    };
    if scale == 0 {
        fail(b"the controller read no declared command scale")
    }
    // Tagged by incarnation for the same reason the tick markers are: the claim
    // "the configuration outlived the task" needs the *replacement's* read to be
    // a distinct, individually pinnable fact rather than a second copy of the
    // first incarnation's line.
    if first_incarnation {
        write_value(b"[robot-controller] scale=", scale);
    } else {
        write_value(b"[robot-controller] scale retained=", scale);
    }

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    let mut ring = provision_stream(&route);
    // The re-provisioning marker: on a replacement this is the observation that
    // the root reissued this component's stream control endpoint and the broker
    // granted a *fresh* ring role to a task that did not exist when the graph
    // was composed.
    if first_incarnation {
        slime_rt::debug_write(b"[robot-controller] subscribe role received\n");
    } else {
        slime_rt::debug_write(b"[robot-controller] subscribe role reissued\n");
    }

    let wake = slime_rt::resolve_binding(b"notification:fabric-call-worker-parameters-ready")
        .unwrap_or_else(|_| fail(b"resolve parameters-ready notification"));

    let mut consumed = 0u32;
    let mut commanded = 0u32;
    loop {
        let mut payload = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, last)) => {
                    if length == 0 {
                        fail(b"empty sample")
                    }
                    // Credit first: the sensor blocks on a full ring, and it is
                    // at a higher band, so releasing the slot before the call
                    // below keeps the cadence the plane measures.
                    let _ = slime_rt::notification_signal(credit_slot());
                    consumed += 1;
                    let tick = sample_tick(&payload, length);
                    // Tagged by incarnation, so a replacement's progress is a
                    // distinct marker rather than a repetition of the first
                    // incarnation's. The gate pins both, and a pinned marker
                    // that two different facts can emit is one the gate's own
                    // deletion control cannot prove load-bearing.
                    if first_incarnation {
                        write_value(b"[robot-controller] consumed tick=", tick);
                    } else {
                        write_value(b"[robot-controller] resumed tick=", tick);
                    }

                    // C9.6's injected restart. Taken after the credit so the
                    // sensor is not left blocked on a dead peer, and after the
                    // marker so the transcript records exactly how far the first
                    // incarnation got.
                    if first_incarnation && consumed == SAMPLES_BEFORE_FAULT {
                        slime_rt::debug_write(b"[robot-controller] injected fault\n");
                        inject_fault();
                    }

                    let command = tick.saturating_mul(scale);
                    commanded += 1;
                    // The request id is the tick's own ordinal, not a
                    // per-incarnation counter: `commanded` resets to zero on
                    // every restart, and the call broker's duplicate
                    // suppression is a per-client high-water mark, so a
                    // replacement reusing request id 1 for its first command
                    // is rejected as a duplicate of the first incarnation's
                    // request 1. A tick is consumed exactly once across every
                    // incarnation combined, so it is never reused.
                    command_actuator(tick, command, wake, first_incarnation);

                    if last {
                        write_value(b"[robot-controller] consumed total=", consumed as u64);
                        // The withdrawn command: a request this component
                        // cancels rather than completes, which is how
                        // cancellation stays a distinct outcome at the userspace
                        // boundary rather than a timeout or a fault.
                        cancel_command(TERMINAL_REQUEST_BASE, command, wake);
                        // The refused command: out of the actuator's declared
                        // range, so it comes back settled `STATUS_REJECTED`.
                        // Issued before the timeout arm because a refusal *is*
                        // a settlement — proving refusal and deadline miss are
                        // told apart needs both to actually occur, and the
                        // unanswered sentinel below is the last request this
                        // component ever issues.
                        expect_rejection(TERMINAL_REQUEST_BASE + 1, wake);
                        // The timed-out command, which is a *different* outcome
                        // and must be produced differently: this one is issued
                        // and simply never settled by the actuator, and the
                        // clock then advances past the route's declared
                        // `deadlineNs`. Releasing the clock's two barrier
                        // phases here rather than at startup is what orders the
                        // advance after the request exists — an advance against
                        // no in-flight call expires nothing and would prove
                        // nothing about deadline handling.
                        expect_timeout(TERMINAL_REQUEST_BASE + 2, wake);
                        write_value(b"[robot-controller] commanded total=", commanded as u64);
                        slime_rt::debug_write(b"[robot-controller] control loop complete\n");
                        slime_rt::exit(0)
                    }
                }
                // The ring is drained. Wait on the control endpoint *blocked*:
                // the broker announces QoS and terminal events with a blocking
                // send that rendezvous only with a waiting receiver, so polling
                // here and sleeping on the ring notification instead would make
                // this component invisible to them.
                Err(RingError::Empty) => break,
                Err(_) => fail(b"consume ring"),
            }
        }
        await_stream_event();
    }
}

/// Ask the stream broker for this component's declared subscribe role, import
/// the ring it grants, and attach it at the declared depth.
fn provision_stream(route: &[u8; 32]) -> Ring<'static> {
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_SUBSCRIBE,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name: route_name_bytes(),
        reserved: [0; 4],
    };
    if slime_rt::send(STREAM_CONTROL_SLOT, &request.encode(), &[]) != ERR_SUCCESS {
        fail(b"stream role request");
    }
    let (descriptor, ring_slot) = receive_role();
    if descriptor.status != 0
        || !valid_capability_transfer(
            &descriptor,
            route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        )
    {
        fail(b"ring descriptor does not name this role");
    }
    if slime_rt::shared_buffer_loan_map(ring_slot, RING_BASE, 0, RING_BYTES as u64) != ERR_SUCCESS {
        fail(b"controller ring map");
    }
    // The depth comes from this component's own graph row: the broker formats
    // the ring at the declared depth and `Ring::attach` compares the header's
    // count against the caller's, so a local guess is a disagreement waiting to
    // happen.
    let slots = slime_components::fabric_self_view::ring_slots(route)
        .unwrap_or_else(|| fail(b"route declares no history depth"));
    let bytes = unsafe { core::slice::from_raw_parts_mut(RING_BASE as *mut u8, RING_BYTES) };
    Ring::attach(bytes, telemetry_stream::TYPE_TAG, slots)
        .unwrap_or_else(|_| fail(b"controller ring attach"))
}

/// Issue one command and settle its typed outcome.
///
/// `first_incarnation` only selects which marker names the actuation. The two
/// are distinct so the gate can pin "data crossed before the restart" and "the
/// graph resumed" as separate facts — a single marker emitted by both would make
/// either deletion undetectable, which is precisely what the gate-control's
/// deletion mutation exists to catch.
fn command_actuator(request_id: u64, command: u64, wake: u32, first_incarnation: bool) {
    send_call(
        envelope(request_id, KIND_REQUEST, STATUS_SUCCESS, command),
        wake,
    );
    let reply = expect_reply(request_id);
    if reply.status != STATUS_SUCCESS {
        write_value(
            b"[robot-controller] command rejected status=",
            reply.status.unsigned_abs() as u64,
        );
        return;
    }
    let applied = payload_value(&reply);
    if applied != command {
        fail(b"the actuator applied a command this controller did not issue")
    }
    if first_incarnation {
        write_value(b"[robot-controller] command applied value=", applied);
    } else {
        write_value(b"[robot-controller] command resumed value=", applied);
    }
}

/// Issue a command and withdraw it, observing the cancellation as its own
/// outcome.
///
/// The cancel reuses the request's exact `(session, request_id)` tuple: a
/// cancellation is a statement about one in-flight request, so a new correlation
/// would be a second request rather than a withdrawal.
fn cancel_command(request_id: u64, command: u64, wake: u32) {
    send_call(
        envelope(request_id, KIND_REQUEST, STATUS_SUCCESS, command),
        wake,
    );
    send_call(envelope(request_id, KIND_CANCEL, STATUS_CANCELLED, 0), wake);
    let settled = expect_reply(request_id);
    if settled.status != STATUS_CANCELLED {
        fail(b"a withdrawn command did not settle as cancelled")
    }
    slime_rt::debug_write(b"[robot-controller] command cancellation observed\n");
}

/// Issue a command outside the actuator's declared range and observe the
/// refusal as its own outcome.
///
/// A refusal is a *settlement*, which is what separates it from the deadline
/// miss below: the actuator answers, the request closes, and no deadline can
/// ever expire it. Asserted rather than merely logged, because an unexercised
/// rejection branch cannot show that refusal, cancellation, and timeout are
/// told apart.
fn expect_rejection(request_id: u64, wake: u32) {
    send_call(
        envelope(request_id, KIND_REQUEST, STATUS_SUCCESS, REJECTED_COMMAND),
        wake,
    );
    let settled = expect_reply(request_id);
    if settled.status != STATUS_REJECTED {
        fail(b"an out-of-range command did not settle as refused")
    }
    slime_rt::debug_write(b"[robot-controller] command refusal observed\n");
}

/// Issue a command the actuator never answers, release the clock, and observe
/// the deadline expire it.
///
/// The command value is `TIMEOUT_COMMAND`, which the actuator's own scenario
/// leaves unanswered. That is the only way this outcome can be produced from
/// userspace: a timeout is the *absence* of a settlement within the declared
/// window, so a component cannot ask for one — it can only issue a request
/// nothing answers and then let declared time pass.
///
/// Both clock phases are released here, in order. Phase 1 moves simulated time
/// to an instant strictly inside the declared `deadlineNs`, which is the control
/// arm: an advance alone must not expire anything. Phase 2 moves past it, and
/// the broker's own deadline sweep is what then settles this request
/// `STATUS_TIMEOUT` and records `EVENT_DEADLINE_MISSED` on the edge.
fn expect_timeout(request_id: u64, wake: u32) {
    send_call(
        envelope(request_id, KIND_REQUEST, STATUS_SUCCESS, TIMEOUT_COMMAND),
        wake,
    );
    release_clock_phase(1);
    release_clock_phase(2);
    let settled = expect_reply(request_id);
    if settled.status != STATUS_TIMEOUT {
        fail(b"an unanswered command did not settle as timed out")
    }
    slime_rt::debug_write(b"[robot-controller] command deadline observed\n");
    // Release the actuator only now. It is still blocked in its receive loop
    // holding this request unanswered, and the broker adjudicates server death
    // ahead of the time advance in a single sweep — so an actuator that exited
    // before this point could have its death settle this very request
    // `STATUS_PEER_DEAD` instead, turning a declared deadline miss into a
    // scheduling coin flip. Signalling after the settlement is observed is what
    // makes the ordering a property of the composition rather than of priority.
    if slime_rt::notification_signal(TIMEOUT_OBSERVED_SLOT) < 0 {
        fail(b"release the actuator after the deadline settled")
    }
}

/// Release one declared barrier phase to the clock.
///
/// A blocking send, because the clock blocks in the matching receive: the
/// barrier's whole purpose is to order the advance after this request exists, so
/// a non-blocking release that the clock had not yet reached would drop the
/// ordering it was added to establish.
fn release_clock_phase(phase: u8) {
    loop {
        match slime_rt::send(CLOCK_PHASE_SLOT, &[phase], &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            error => fail_with(b"clock phase release", error),
        }
    }
}

fn envelope(request_id: u64, kind: u32, status: i32, value: u64) -> WireCallEnvelope {
    let mut payload = [0u8; 16];
    let width = core::mem::size_of::<u64>();
    payload[..width].copy_from_slice(&value.to_le_bytes());
    WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind,
        flags: 0,
        session: CLIENT_SESSION,
        request_id,
        type_identity: parameter_call::TYPE_TAG,
        status,
        payload_len: if value == 0 { 0 } else { width as u32 },
        payload,
    }
}

/// Send one call record, waking the broker first.
///
/// The broker parks whenever no participant is runnable, so the wake this send
/// needs is the one raised immediately before it — not one raised once at
/// startup.
fn send_call(record: WireCallEnvelope, wake: u32) {
    let encoded = record.encode();
    loop {
        let _ = slime_rt::notification_signal(wake);
        match slime_rt::send(CALL_ROUTE_SLOT, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            error => fail_with(b"call send", error),
        }
    }
}

/// Read this request's settlement, acknowledging a broker terminal so the
/// broker can retire its record.
///
/// A terminal that is never acked is offered forever and the broker outlives a
/// finished graph, so the ack is part of the protocol rather than politeness.
fn expect_reply(request_id: u64) -> WireCallEnvelope {
    let record = recv_call();
    if !slime_proto::valid_call_envelope(&record, parameter_call::TYPE_TAG)
        || record.request_id != request_id
    {
        fail(b"a settlement named a request this controller did not issue")
    }
    match record.kind {
        KIND_REPLY => {
            // Ack the reply, for the same reason the terminal branch below
            // acks a terminal: the broker offers it with a non-blocking
            // send, which reports nothing, so the ack is the only thing
            // that retires the call record. Left unacked, this call stays
            // outstanding until the route's declared deadline force-closes
            // it, which is exactly the failure this fixes — every reply
            // this component ever read was landing as a *second*,
            // deadline-driven terminal for a request already settled.
            let mut ack = record;
            ack.kind = KIND_REPLY_ACK;
            ack.payload = [0u8; 16];
            ack.payload_len = 0;
            let mut discard = [0u8; MAX_MSG];
            let ack_result = slime_rt::call(CALL_ROUTE_SLOT, &ack.encode(), &mut discard);
            if ack_result < 0 {
                fail_with(b"reply ack", ack_result)
            }
            record
        }
        KIND_TERMINAL => {
            let mut ack = record;
            ack.kind = KIND_TERMINAL_ACK;
            ack.payload = [0u8; 16];
            ack.payload_len = 0;
            // `call` rather than `send`: the broker retires its record on
            // the ack's rendezvous, and every other client on this plane
            // acknowledges the same way.
            let mut discard = [0u8; MAX_MSG];
            let ack_result = slime_rt::call(CALL_ROUTE_SLOT, &ack.encode(), &mut discard);
            if ack_result < 0 {
                fail_with(b"terminal ack", ack_result)
            }
            record
        }
        _ => fail(b"unexpected call record kind"),
    }
}

fn recv_call() -> WireCallEnvelope {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(CALL_ROUTE_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            value if value < 0 => fail_with(b"call receive", value),
            value if value as usize != MAX_MSG => fail(b"call record length"),
            _ => {
                release_caps(&caps);
                return WireCallEnvelope::decode(&bytes)
                    .filter(|record| record.magic == CALL_MAGIC)
                    .unwrap_or_else(|| fail(b"call decode"));
            }
        }
    }
}

/// Block on the stream control endpoint for the broker's next event.
///
/// Every declared stream outcome this component can be told about arrives here,
/// and each is reported under its own name: the plane requires deadline miss,
/// liveliness loss, and peer loss to stay distinguishable from the ordinary end.
fn await_stream_event() {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = slime_rt::recv_blocking(STREAM_CONTROL_SLOT, &mut message, &mut received);
    if length < 0 || length as usize != MAX_MSG {
        // Nothing usable arrived; wait for the ring instead.
        let _ = slime_rt::notification_wait(READY_SLOT);
        return;
    }
    release_caps(&received);
    let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
    if magic == QOS_EVENT_MAGIC {
        let event = WireQosEvent::decode(&message).unwrap_or_else(|| fail(b"QoS event decode"));
        write_value(b"[robot-controller] qos event=", event.event as u64);
    } else if magic == STREAM_EVENT_MAGIC {
        let event =
            WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"stream event decode"));
        write_value(b"[robot-controller] stream event=", event.event as u64);
    }
}

/// Terminate this incarnation with a fault the root records as `fault`.
///
/// A null read rather than `exit`: the supervisor's restart policy names both
/// `exit` and `fault`, and the plane asserts the *fault* cause specifically —
/// which only a real fault produces, since a clean exit is recorded as `exit`.
fn inject_fault() -> ! {
    // SAFETY: deliberately unsound. This is the scripted degradation C9.6
    // requires, and a fault is the only thing that produces the `fault` terminal
    // cause the generation's restart policy names.
    unsafe {
        core::ptr::null::<u64>().read_volatile();
    }
    fail(b"the injected fault did not fault")
}

/// The tick ordinal a sample carries.
fn sample_tick(payload: &[u8], length: usize) -> u64 {
    let width = core::mem::size_of::<u64>();
    if length < width {
        fail(b"sample carries no tick ordinal")
    }
    u64::from_le_bytes(payload[..width].try_into().expect("tick prefix"))
}

fn payload_value(record: &WireCallEnvelope) -> u64 {
    let width = core::mem::size_of::<u64>();
    if record.payload_len as usize == width {
        u64::from_le_bytes(record.payload[..width].try_into().expect("payload prefix"))
    } else {
        0
    }
}

fn credit_slot() -> u32 {
    slime_rt::resolve_binding(b"notification:robot-controller-telemetry-credit")
        .unwrap_or_else(|_| fail(b"resolve telemetry-credit notification"))
}

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

/// The typed role descriptor and the slot its shared ring landed in.
///
/// Discriminated on the record's own magic: the broker sends QoS events on this
/// same control endpoint, and a v2 role reply carries no capability in the
/// message, so the received-capability array cannot tell the two apart.
fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(STREAM_CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message)
                    .filter(|record| record.magic == CAPABILITY_TRANSFER_MAGIC)
                else {
                    continue;
                };
                if descriptor.status != 0 {
                    return (descriptor, 0);
                }
                let slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"import role"));
                return (descriptor, slot);
            }
        }
    }
}

/// Drop anything that arrived alongside a control record.
fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().copied().filter(|slot| *slot != 0) {
        let _ = slime_rt::cap_drop(slot as u32);
    }
}

fn write_value(prefix: &[u8], value: u64) {
    let mut digits = [0u8; 20];
    slime_rt::debug_write(prefix);
    slime_rt::debug_write(decimal(value, &mut digits));
    slime_rt::debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut index = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        index -= 1;
        digits[index] = b'0';
    }
    while remaining != 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[robot-controller] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[robot-controller] FAIL ");
    slime_rt::debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    slime_rt::exit(1)
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_call::CALL_LEN == MAX_MSG);
