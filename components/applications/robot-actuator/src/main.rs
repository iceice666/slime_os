#![no_std]
#![no_main]

//! C9.6's actuator: the call server at the tail of the robot chain.
//!
//! The controller derives a command from each telemetry sample and issues it as
//! a bounded `ParameterCall` request; this component is what applies it. It is
//! the call-plane server on the declared `parameters` route, reached only
//! through the broker — it holds one preinstalled route endpoint and no
//! authority naming the controller at all, so it cannot reply to anyone the
//! broker did not forward.
//!
//! Two of C9.6's required-distinct outcomes are produced here, and they are
//! produced *deliberately* rather than emerging from a race:
//!
//! - **Cancellation.** A command the controller withdraws arrives as
//!   `KIND_CANCEL` on the same `(session, request_id)` tuple. This component
//!   settles it with `STATUS_CANCELLED` and says so before replying, because
//!   `send` blocks until the broker takes the message and the broker reports the
//!   cancellation the moment it does — replying first would always put the
//!   broker's marker ahead of this one and make the causal order unobservable.
//! - **Rejection.** A command outside the declared actuation range is refused
//!   with `STATUS_REJECTED`. A refusal is not a fault and not a timeout, and
//!   the plane asserts all three are told apart.
//!
//! Deadline miss and peer loss are *not* produced here: the broker's own
//! deadline arm owns the first, and the controller's scripted restart owns the
//! second. Each of the six outcomes has exactly one producer, which is what
//! keeps them distinguishable.

use slime_proto::fabric_call::{
    CALL_MAGIC, FORMAT_VERSION, KIND_CANCEL, KIND_REPLY, KIND_REQUEST, STATUS_CANCELLED,
    STATUS_REJECTED, STATUS_SUCCESS, WireCallEnvelope,
};
use slime_proto::interface_schema::parameter_call;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The preinstalled route endpoint every call participant receives. The call
/// plane performs no role handshake: the generation installs both halves before
/// either task runs, which is what binds this endpoint to this identity.
const ROUTE_SLOT: u32 = 0;

/// The declared wait slot on which this actuator learns the controller has
/// observed its unanswered request settle `STATUS_TIMEOUT`. Ordering this
/// component's exit after that settlement is what keeps the deadline miss
/// distinct from peer death; see the wait site in `main`.
const TIMEOUT_OBSERVED_SLOT: u32 = 2;

/// The session the broker rewrites every forwarded request to. A request
/// arriving under any other session did not come through the broker.
const BROKER_SESSION: u64 = 0x000e_0000_0000_0001;

/// The largest command this actuator declares itself able to apply. A command
/// above it is refused rather than clamped: silently applying a different
/// command than the one commanded is the failure mode a range check exists to
/// prevent.
const MAX_COMMAND: u64 = 1_000;

/// The one command this actuator deliberately leaves unanswered.
///
/// C9.6 requires a deadline miss to stay distinct from a refusal, and a refusal
/// *is* a settlement: replying `STATUS_REJECTED` to an out-of-range command
/// would close the request and no deadline could ever expire it. So the timeout
/// arm needs a request nothing answers at all, and it is a named sentinel rather
/// than an out-of-range value for exactly that reason. Checked before the range
/// test below, which would otherwise refuse it.
const UNANSWERED_COMMAND: u64 = 4_242;

/// The fewest commands the composition guarantees reach actuation.
///
/// Two: the first incarnation consumes two samples and commands on both before
/// its scripted fault, and the replacement commands on at least the sensor's
/// terminal sample. An exact count would be a claim about scheduling — how many
/// of the sensor's later samples land depends on when the replacement's fresh
/// ring is provisioned — so the floor is what the graph declares and the
/// transcript's own totals carry the rest.
const MIN_APPLIED: u32 = 2;

fn main(_startup_arg: u32) {
    // Wake the broker before this component blocks. The broker parks on this
    // notification when no participant is runnable, and it forwards with a
    // blocking send — so a server that blocked without signalling could be
    // waiting on a broker that is itself waiting to be woken.
    let wake = slime_rt::resolve_binding(b"notification:fabric-call-worker-parameters-ready")
        .unwrap_or_else(|_| fail(b"resolve parameters-ready notification"));
    slime_rt::debug_write(b"[robot-actuator] server ready\n");

    let mut applied = 0u32;
    let mut cancelled = 0u32;
    let mut rejected = 0u32;
    let mut unanswered = 0u32;
    // Ends on the *sentinel*, not a count of applications. How many samples
    // survive the controller's restart depends on when the replacement's fresh
    // ring is provisioned relative to the sensor's cadence, so an applied-count
    // bound would make this component's exit racy — and a server that exits
    // early leaves the broker forwarding to a dead peer. The unanswered command
    // is the last thing the controller ever issues, so observing it is the
    // deterministic end of this component's work.
    while unanswered == 0 {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        // Block in the kernel rather than poll: the broker forwards with a
        // blocking `send`, which rendezvous only with a receiver already
        // waiting, so a polling server and a blocking broker would never meet.
        let length = match slime_rt::recv_blocking(ROUTE_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            // No `ERR_PEER_DEAD` arm: a native seL4 Endpoint never answers one,
            // so this server ends on its own declared command count rather than
            // on an endpoint reporting a closed peer (B76).
            value if value < 0 => fail_with(b"server receive", value),
            value => value as usize,
        };
        if length != MAX_MSG {
            fail(b"server record length")
        }
        release_caps(&caps);
        let request = WireCallEnvelope::decode(&bytes)
            .filter(|record| record.magic == CALL_MAGIC)
            .unwrap_or_else(|| fail(b"server decode"));
        // Authority is the endpoint; the session is the *broker's* stamp, so a
        // request that did not pass through it is refused rather than served.
        // `valid_call_envelope` covers the record's own structural shape --
        // flags, non-zero request id, payload bounds, and per-kind status --
        // the same gate every other call-plane participant runs before
        // trusting a field.
        if !slime_proto::valid_call_envelope(&request, parameter_call::TYPE_TAG)
            || request.session != BROKER_SESSION
            || !matches!(request.kind, KIND_REQUEST | KIND_CANCEL)
        {
            fail(b"server received an invalid call record")
        }

        if request.kind == KIND_CANCEL {
            // Announced before the reply, for the ordering reason in the module
            // comment.
            cancelled += 1;
            write_value(
                b"[robot-actuator] command cancelled id=",
                request.request_id,
            );
            reply(&request, STATUS_CANCELLED, 0, wake);
            continue;
        }

        let command = command_of(&request);
        // Checked before the range test: the sentinel is above `MAX_COMMAND`, so
        // reaching that test would refuse it and close the request the plane
        // needs left open. No reply, no marker of its own — the absence is the
        // behaviour, and the broker's deadline sweep is what reports it.
        if command == UNANSWERED_COMMAND {
            unanswered += 1;
            write_value(
                b"[robot-actuator] command left unanswered id=",
                request.request_id,
            );
            continue;
        }
        if command > MAX_COMMAND {
            rejected += 1;
            write_value(b"[robot-actuator] command refused value=", command);
            reply(&request, STATUS_REJECTED, 0, wake);
            continue;
        }
        applied += 1;
        // The applied value is echoed so the gate can bind each actuation back
        // to the sample the controller derived it from. An acknowledgement
        // carrying no value would prove a call completed but not that the right
        // command crossed.
        write_value(b"[robot-actuator] applied value=", command);
        reply(&request, STATUS_SUCCESS, command, wake);
    }
    // Observing the unanswered request is *not* enough to exit on. The broker
    // adjudicates server death before it consumes a time advance
    // (`observe_server_death` runs ahead of `pump_time` in one sweep), and
    // retiring the server settles every outstanding call `STATUS_PEER_DEAD`.
    // So an actuator that exited here would race its own death against the
    // deadline the plane exists to observe: whichever the broker saw first
    // decided the status, and peer death wins the tie. The controller signals
    // this notification only after `expect_reply` returned `STATUS_TIMEOUT`,
    // which orders this exit strictly after the settlement — the timeout is a
    // declared outcome rather than a scheduling accident.
    if let Err(error) = slime_rt::notification_wait(TIMEOUT_OBSERVED_SLOT) {
        fail_with(b"await timeout settlement", error);
    }
    slime_rt::debug_write(b"[robot-actuator] timeout settlement observed\n");
    // The chain must have carried real data both before and after the
    // controller's restart, so a run where the replacement never reached
    // actuation is a failure rather than a shorter transcript. `MIN_APPLIED` is
    // the floor the composition guarantees, not the exact count: how many of the
    // sensor's later samples land depends on when the replacement's fresh ring
    // is provisioned, which is a scheduling fact rather than a declared one.
    if applied < MIN_APPLIED {
        fail(b"the chain applied fewer commands than the composition guarantees")
    }
    if rejected == 0 {
        fail(b"the plane produced no refusal, so a refusal is not told apart here")
    }
    write_value(b"[robot-actuator] applied total=", applied as u64);
    write_value(b"[robot-actuator] cancelled total=", cancelled as u64);
    write_value(b"[robot-actuator] refused total=", rejected as u64);
    write_value(b"[robot-actuator] unanswered total=", unanswered as u64);
    slime_rt::debug_write(b"[robot-actuator] actuation complete\n");
    slime_rt::exit(0)
}

/// The commanded value carried by a request, or zero when it carries none.
fn command_of(request: &WireCallEnvelope) -> u64 {
    if request.payload_len as usize == core::mem::size_of::<u64>() {
        u64::from_le_bytes(
            request.payload[..core::mem::size_of::<u64>()]
                .try_into()
                .expect("payload prefix"),
        )
    } else {
        0
    }
}

/// Answer one forwarded request on the route endpoint.
///
/// The session and request id are echoed unchanged: they are the broker's
/// correlation, and a reply that renamed either would be an uncorrelatable
/// record rather than an answer.
fn reply(request: &WireCallEnvelope, status: i32, value: u64, wake: u32) {
    let mut payload = [0u8; 16];
    let width = core::mem::size_of::<u64>();
    payload[..width].copy_from_slice(&value.to_le_bytes());
    let record = WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_REPLY,
        flags: 0,
        session: request.session,
        request_id: request.request_id,
        type_identity: parameter_call::TYPE_TAG,
        status,
        payload_len: if value == 0 { 0 } else { width as u32 },
        payload,
    };
    let encoded = record.encode();
    loop {
        // Signal before every attempt, not once at startup: the broker parks
        // whenever no participant is runnable, so the wake this send needs is
        // the one raised immediately before it.
        let _ = slime_rt::notification_signal(wake);
        match slime_rt::send(ROUTE_SLOT, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            error => fail_with(b"reply send", error),
        }
    }
}

/// Drop anything that arrived alongside a call record.
///
/// A call envelope carries no capability, so a received one is authority this
/// component never asked for and must not retain.
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
    slime_rt::debug_write(b"[robot-actuator] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[robot-actuator] FAIL ");
    slime_rt::debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    slime_rt::exit(1)
}

const _: () = assert!(slime_proto::fabric_call::CALL_LEN == MAX_MSG);
