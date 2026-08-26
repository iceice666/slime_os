#![no_std]
#![no_main]

//! C9.6's periodic sensor: the robot graph's source of typed samples.
//!
//! The head of the sensor -> controller -> actuator chain. It publishes bounded
//! `TelemetrySample` records on the declared `telemetry` stream route, one per
//! C9.1 timer expiry, at the `foreground` band the generation assigns it.
//!
//! Three things make it a *robot* sensor rather than the C8.4 publisher this is
//! shaped after:
//!
//! - **Cadence is a clock, not a loop.** Each sample is published after a real
//!   `timer_arm`/`notification_wait` pair, so the sensor spends most of its life
//!   blocked. That is what lets the declared `bestEffort` load run at all under
//!   strict priority, and therefore what makes "declared scheduling order
//!   preserved under contention" observable: each expiry preempts a burner loop
//!   the transcript shows to be in flight.
//! - **Each sample carries its own tick ordinal.** The controller derives its
//!   command from the sample payload, so the payload is the input of a
//!   deterministic function rather than filler. The gate compares the derived
//!   commands against the published ticks, which a sensor emitting a constant
//!   could not satisfy.
//! - **It ends its stream.** The chain's completion is an orderly `FLAG_LAST`,
//!   not a death. C9.6 requires peer loss to stay *distinct* from an ordinary
//!   end, so this component must produce the ordinary end; the controller's
//!   scripted restart produces the loss.
//!
//! It holds no factory, no route capability, and no peer endpoint: one control
//! endpoint to the stream broker, and the timer authority the generation's
//! `clockAuthority` grants it.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, route_identity};
use boot_contracts::scheduling_class::CLASS_FOREGROUND;
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FORMAT_VERSION,
    OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::ring::{Ring, RingError};
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// Control endpoint to the stream broker: this component's whole starting
/// authority over the fabric, and the identity the broker authenticates it by.
const CONTROL_SLOT: u32 = 0;

const RING_BASE: u64 = 0x0000_0011_0000_0000;
const RING_BYTES: usize = 4096;

const ROUTE_NAME: &str = "telemetry";

/// Samples published before the terminal one.
///
/// Each is one timer expiry, so this is also the number of times the foreground
/// band preempts the declared load. Four rather than one: a single wake could
/// land before the burner was ever scheduled, and the property under test is a
/// sequence that keeps completing while a lower band is runnable.
const SENSOR_TICKS: u64 = 4;

/// The relative delay armed between samples.
///
/// Long enough that the burner is certain to be scheduled and to emit at least
/// one chunk marker while this component is blocked; short enough that the whole
/// sequence finishes well inside the plane's watchdog. The same magnitude
/// C9.3's foreground band uses, for the same reason.
const TICK_DELAY: u64 = 2_000_000;

/// The declared timer badge bit from the generation's `clockAuthority`.
const TIMER_BADGE: u64 = 1 << 9;

fn main(_startup_arg: u32) {
    // The band is asserted, not assumed: a sensor silently demoted out of
    // `foreground` would still publish every sample while no longer being the
    // ordered-progress claim this plane makes about it.
    let class = match slime_rt::scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"class read", error),
    };
    if class.class_id != CLASS_FOREGROUND {
        fail(b"the sensor is not in the declared foreground band")
    }
    write_value(b"[robot-sensor] foreground priority=", class.priority);

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );

    // The request names the route and type it wants; none of it is authority.
    // The broker answers from the generation graph keyed by the control endpoint
    // the request arrived on, so these fields only prove the ask was well
    // formed.
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_PUBLISH,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name: route_name_bytes(),
        reserved: [0; 4],
    };
    if slime_rt::send(CONTROL_SLOT, &request.encode(), &[]) != ERR_SUCCESS {
        fail(b"role request");
    }
    slime_rt::debug_write(b"[robot-sensor] publish role requested\n");

    let (descriptor, ring_slot) = receive_role();
    if descriptor.status != 0
        || !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_PUBLISH,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        )
    {
        fail(b"ring descriptor does not name this role");
    }
    if slime_rt::shared_buffer_loan_map(ring_slot, RING_BASE, 0, RING_BYTES as u64) != ERR_SUCCESS {
        fail(b"sensor ring map");
    }
    slime_rt::debug_write(b"[robot-sensor] publish role received\n");

    // The ring depth comes from this component's own graph row, never from a
    // local constant: the broker formats the ring at the declared depth and
    // `Ring::attach` compares the header's count against the caller's, so a
    // guess here is a disagreement waiting to happen.
    let slots = slime_components::fabric_self_view::ring_slots(&route)
        .unwrap_or_else(|| fail(b"route declares no history depth"));
    let bytes = unsafe { core::slice::from_raw_parts_mut(RING_BASE as *mut u8, RING_BYTES) };
    let mut ring = Ring::attach(bytes, telemetry_stream::TYPE_TAG, slots)
        .unwrap_or_else(|_| fail(b"sensor ring attach"));

    for tick in 1..=SENSOR_TICKS {
        // Block on the declared clock before every sample, including the first:
        // publishing one immediately would put a sample on the route before the
        // controller had been spawned by its supervisor, and the cadence claim
        // would then hold for three of the four ticks.
        await_tick();
        publish(&mut ring, &sample_payload(tick), false);
        write_value(b"[robot-sensor] tick=", tick);
    }
    // The orderly end. Distinct from the peer loss the controller's restart
    // produces, which is exactly the distinction C9.6 requires: an
    // `EVENT_STREAM_END` and an `EVENT_PEER_DEAD` are mutually exclusive
    // outcomes for one route, and this component produces the former.
    await_tick();
    publish(&mut ring, &sample_payload(SENSOR_TICKS + 1), true);
    write_value(b"[robot-sensor] stream ended ticks=", SENSOR_TICKS + 1);
    slime_rt::exit(0)
}

/// Block until one declared timer expiry arrives.
///
/// Arms a real C9.1 timer and waits on the declared Notification rather than
/// spinning: a spin would satisfy the delay while proving nothing about the
/// clock, and — being at the `foreground` band — would starve the declared load
/// this plane's contention claim depends on.
fn await_tick() {
    if let Err(error) = slime_rt::timer_arm(TICK_DELAY) {
        // Fatal rather than returning: the caller's loop condition is unchanged
        // by a failed arm, so returning would degenerate into an unbounded spin
        // at the foreground band — the exact starvation blocking exists to
        // avoid. The generation grants this holder `timerUse` with a quota of
        // two and at most one timer is ever live, so the path is unreachable.
        fail_with(b"arm sensor tick", error)
    }
    loop {
        let badge = match slime_rt::notification_wait(TICK_SLOT) {
            Ok(badge) => badge,
            Err(error) => fail_with(b"wait sensor tick", error),
        };
        // The badge is checked rather than the wake being trusted: this
        // component's Notification carries exactly the declared timer bit, so a
        // wake without it means the root signalled something this component does
        // not know how to interpret.
        if badge & TIMER_BADGE != 0 {
            return;
        }
    }
}

/// This component's declared wait binding on `robot-sensor-tick`.
const TICK_SLOT: u32 = 0;

/// One typed sample: the tick ordinal in the leading `sequence` field of a
/// `TelemetrySample`, which is what the controller's command derives from.
fn sample_payload(tick: u64) -> [u8; 8] {
    tick.to_le_bytes()
}

fn publish(ring: &mut Ring<'_>, payload: &[u8], last: bool) {
    loop {
        match ring.publish(payload, last) {
            Ok(_) => {
                if slime_rt::notification_signal(ready_slot()) != ERR_SUCCESS {
                    fail(b"publish notify");
                }
                return;
            }
            // The declared depth is the backpressure: a full ring means the
            // controller has not consumed yet, so this waits for its credit
            // rather than dropping or growing.
            Err(RingError::Full) => {
                let _ = slime_rt::notification_wait(credit_slot());
            }
            Err(_) => fail(b"publish ring"),
        }
    }
}

fn ready_slot() -> u32 {
    slime_rt::resolve_binding(b"notification:robot-sensor-telemetry-ready")
        .unwrap_or_else(|_| fail(b"resolve telemetry-ready notification"))
}

fn credit_slot() -> u32 {
    slime_rt::resolve_binding(b"notification:robot-sensor-telemetry-credit")
        .unwrap_or_else(|_| fail(b"resolve telemetry-credit notification"))
}

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

/// The typed role descriptor and the slot its shared ring landed in.
///
/// Only a record whose own magic is `CAPABILITY_TRANSFER_MAGIC` is a role
/// reply: the broker also sends QoS events on this same control endpoint, and a
/// v2 role reply carries no capability in the message — the ring crosses as a
/// root-side export this component claims — so the received-capability array
/// cannot tell the two apart. The record's magic can.
///
/// Blocks in the kernel rather than polling. This component is declared
/// `foreground`, strictly above the broker it is waiting on here — the C8
/// planes this is modelled on never cross a priority boundary, so their
/// non-blocking poll-and-yield is harmless there. Here it is not: `yield_now`
/// only rotates within the caller's own band, so a poll loop at `foreground`
/// would keep re-readying itself forever and never let a strictly lower
/// broker run at all. A genuine kernel block removes this thread from the
/// ready queue entirely, which is what lets the scheduler descend.
fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv_blocking(CONTROL_SLOT, &mut message, &mut received) {
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
    slime_rt::debug_write(b"[robot-sensor] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[robot-sensor] FAIL ");
    slime_rt::debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    slime_rt::exit(1)
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
