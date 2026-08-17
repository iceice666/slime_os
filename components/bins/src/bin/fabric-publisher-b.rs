#![no_std]
#![no_main]

//! C8.4 second publisher: the large-sample originator, on two routes at once.
//!
//! This component exists to make three properties observable that one publisher
//! cannot show:
//!
//! 1. **Many-to-many fan-in.** It publishes on `telemetry` alongside
//!    `fabric-publisher`, so two publishers feed one route and every matched
//!    subscriber sees both.
//! 2. **A payload larger than the control bound.** It allocates a
//!    quota-charged shared buffer, fills it, seals it irreversibly, and loans
//!    the exact sealed region to the fabric — sending only the 64-byte C7.6
//!    descriptor. The fabric makes one copy and re-loans it per subscriber; the
//!    payload never enters a kernel message queue.
//! 3. **Route separation.** It also publishes on `diagnostics`, a different
//!    route carrying a different interface. Its two roles are separate
//!    capabilities: neither can carry the other's samples, and a subscriber on
//!    only one route never observes the other.
//!
//! The two roles arrive over one control endpoint, in the order the generation
//! declares them. The component learns nothing from the ordering it did not
//! already know from the graph — each descriptor names its own route identity,
//! and this component checks that before using either.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, route_identity};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, OBJECT_KIND_SHARED_BUFFER_LOAN,
    REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_TAKEN, FLAG_LAST, MAX_INLINE_BYTES, STREAM_SAMPLE_MAGIC, WireStreamEvent,
    WireStreamSample,
};
use slime_proto::fabric_time::{
    FORMAT_VERSION as TIME_VERSION, TIME_ADVANCE_MAGIC, WireTimeAdvance,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::ring::{Ring, RingError};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::{valid_capability_transfer, valid_stream_event};
use slime_rt::{CapabilityDisposition, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::RIGHT_SEND;

// C8.13.2: this participant's own shared-buffer occupancy evidence. Included
// here rather than through `slime_components` because a file may be a module
// only once per crate.
#[path = "../fabric_trace_log.rs"]
mod trace_log;

#[path = "../fabric_occupancy_trace.rs"]
mod occupancy_trace;

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// Control endpoint to the fabric — this component's only route authority path.
const CONTROL_SLOT: u32 = 0;
/// `SharedBufferFactory` granted by the generation, bounded by this component's
/// own `shared-buffer-budget` entry.
const FACTORY_SLOT: u32 = 1;
/// The fabric, named for the upstream loan. This is the control endpoint at
/// slot 0: a declared native endpoint names its peer, which the generation
/// fixed before either task ran, so the loan's receiver is still a capability
/// fact rather than an ambient task id. It is not a supervision handle because
/// the fabric loans this component its ring in the other direction, and each
/// spawning before the other is impossible.
const FABRIC_SLOT: u32 = 0;
/// Generation-granted simulated monotonic-time input. This is a separate
/// capability from both publish routes; possessing a route grants no clock.
const TIME_SLOT: u32 = 3;

/// The visibility plane's declared diagnostics ingress edge, send-only.
///
/// `sel4-visibility.zti` binds `visibility-diagnostics-ingress` here, after
/// the control endpoint at 0 and the minted buffer factory at 1.
const DIAGNOSTICS_INGRESS_SLOT: u32 = 2;

const TELEMETRY_ROUTE: &str = "telemetry";
const DIAGNOSTICS_ROUTE: &str = "diagnostics";
/// C8.12's alternate name over `TelemetryStream`. A distinct route from
/// `TELEMETRY_ROUTE`, because route authority folds the name into the identity.
const MATRIX_ALT_ROUTE: &str = "telemetry-alt";

const PAGE: u64 = 4096;
/// Pages the publisher allocates and loans whole.
const PAGES: usize = 3;
const LOAN_LEN: u64 = PAGES as u64 * PAGE;
/// Where the sample sits inside the loaned region, and how long it is.
///
/// Deliberately not zero: `loan_map` offsets are relative to the loan, and the
/// C7.6 descriptor admits any page-aligned in-bounds offset, so a broker that
/// hard-coded zero would copy the leading page instead of the payload. That
/// page is allocated and never written, so the mistake is silent unless a
/// sample really starts past it — which is what this offset makes true.
const PAYLOAD_OFFSET: u64 = PAGE;
const PAYLOAD_LEN: u64 = 2 * PAGE;
const BASE: u64 = 0x0000_000D_0000_0000;

#[derive(Default)]
struct RoutePair {
    data: Option<u32>,
}

/// One page per ring, so the two bases must differ by more than a page: each
/// ring *is* `RING_BYTES`, and adjacent bases would have the second mapping
/// land on the first.
const TELEMETRY_RING_BASE: u64 = 0x0000_0012_0000_0000;
const DIAGNOSTICS_RING_BASE: u64 = 0x0000_0015_0000_0000;
const RING_BYTES: usize = 4096;

/// This component's name, as the generation's participant table spells it.
const COMPONENT: &[u8] = b"fabric-publisher-b";

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen. Floored at
/// `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    FABRIC_HISTORY_DEPTHS
        .iter()
        .find(|(name, entry, _)| *name == COMPONENT && *entry == route)
        .map(|(_, _, depth)| *depth as usize)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
        .max(slime_proto::fabric_ring::MIN_RING_SLOTS)
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-publisher-b] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if GENERATION_BOOT_ACTION == "visibility" {
        visibility_main();
        return;
    }
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    // A payload larger than the control-message bound is the whole point of
    // this component's telemetry arm.
    if PAYLOAD_LEN <= MAX_MSG as u64 {
        fail(b"payload must exceed MAX_MSG");
    }

    let telemetry = route_identity(
        TELEMETRY_ROUTE,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        let diagnostics = route_identity(
            DIAGNOSTICS_ROUTE,
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        );
        slime_components::fabric_boot::provision_multi_and_park(
            b"fabric-publisher-b",
            TELEMETRY_ROUTE,
            telemetry_stream::TYPE_TAG,
            DIRECTION_PUBLISH,
            &[
                (telemetry, DIRECTION_PUBLISH, 1),
                (diagnostics, DIRECTION_PUBLISH, 1),
            ],
        );
    }
    let diagnostics = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );

    if request_roles() != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-publisher-b] roles requested\n");
    let mut telemetry_pair = RoutePair::default();
    let mut diagnostics_pair = RoutePair::default();
    for _ in 0..2 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared publisher was denied");
        }
        if descriptor.route_identity == telemetry {
            telemetry_pair.data = Some(slot);
        } else if descriptor.route_identity == diagnostics {
            diagnostics_pair.data = Some(slot);
        } else {
            fail(b"role names no declared route");
        }
    }
    let Some(telemetry_slot) = telemetry_pair.data else {
        fail(b"telemetry ring missing");
    };
    let Some(diagnostics_slot) = diagnostics_pair.data else {
        fail(b"diagnostics ring missing");
    };
    if slime_rt::shared_buffer_loan_map(telemetry_slot, TELEMETRY_RING_BASE, 0, RING_BYTES as u64)
        != ERR_SUCCESS
        || slime_rt::shared_buffer_loan_map(
            diagnostics_slot,
            DIAGNOSTICS_RING_BASE,
            0,
            RING_BYTES as u64,
        ) != ERR_SUCCESS
    {
        fail(b"publisher ring map");
    }
    slime_rt::debug_write(b"[fabric-publisher-b] both publish roles received\n");

    let diagnostics_bytes =
        unsafe { core::slice::from_raw_parts_mut(DIAGNOSTICS_RING_BASE as *mut u8, RING_BYTES) };
    let mut diagnostics_ring = Ring::attach(
        diagnostics_bytes,
        diagnostics_stream::TYPE_TAG,
        ring_slots(DIAGNOSTICS_ROUTE),
    )
    .unwrap_or_else(|_| fail(b"diagnostics ring attach"));
    ring_publish(
        &mut diagnostics_ring,
        &inline_sample(diagnostics_stream::TYPE_TAG, 1, FLAG_LAST).payload,
        true,
        FABRIC_PUBLISHER_B_DIAGNOSTICS_READY_SLOT,
        FABRIC_PUBLISHER_B_DIAGNOSTICS_CREDIT_SLOT,
    );
    slime_rt::debug_write(b"[fabric-publisher-b] diagnostics sample published\n");

    publish_large(CONTROL_SLOT, CONTROL_SLOT);
    slime_rt::debug_write(b"[fabric-publisher-b] large sample published\n");
    if GENERATION_BOOT_ACTION == "qos" || GENERATION_BOOT_ACTION == "traffic" {
        for now_ns in [50u64, 100, 200, 300, 400, 500, 600] {
            advance_time(now_ns);
            await_time_credit(now_ns);
        }
        slime_rt::debug_write(b"[fabric-publisher-b] simulated time advanced\n");
    }
    // No second diagnostics sample. The first carries `FLAG_LAST`, which retires
    // this publisher's ingress at the fabric: `broker` skips a finished
    // publisher and `park_on_streams` drops it from the wait set, so a further
    // send is never read by anyone. It only sat in a queue.
    //
    // That made it worse than useless (B18). Once `diagnostics` is retired,
    // only `telemetry` keeps the service alive; when that drains the fabric
    // exits, and this send then answers `ERR_PEER_DEAD` — which `publish`
    // treats as fatal. So the component's exit status depended on whether the
    // fabric happened to still be running, which on seL4 it usually was not.
    //
    // Deleting it rather than moving `FLAG_LAST` to it: the route genuinely
    // ends at the first sample, `fabric-subscriber-b` consumes exactly one
    // diagnostics sample on both paths, and no gate requires the marker this
    // dropped. Moving the flag instead was tried and wedges
    // `just fabric_qos_check`, whose subscriber waits for the terminal event
    // the early flag produces.
    //
    // C8.13.2: gated to the traffic plane, so the standalone fixtures' fixed
    // `traceDepth` never has to carry these records. Two rings here, one per
    // declared route. The third mapping this role transiently holds -- for the
    // copy it creates, seals, and lends -- is unmapped and released inside
    // `publish_large` before this point, so what it reports is its two
    // provisioned rings.
    if GENERATION_BOOT_ACTION == "traffic" {
        occupancy_trace::report(b"publisher-b", FABRIC_TRACE_DEPTH);
    }
    slime_rt::debug_write(b"[fabric-publisher-b] done\n");
}
fn visibility_main() {
    // The unrelated diagnostics route is a generation-declared endpoint, not a
    // ring loan: the broker's reply names the edge this component already
    // holds, so there is no capability in it and nothing to import.
    let diagnostics = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if request_roles() != ERR_SUCCESS {
        fail(b"visibility request");
    }
    let descriptor = receive_declared_role();
    if descriptor.rights_mask != RIGHT_SEND
        || !valid_capability_transfer(
            &descriptor,
            &diagnostics,
            DIRECTION_PUBLISH,
            OBJECT_KIND_ENDPOINT,
        )
    {
        fail(b"visibility diagnostics role");
    }
    if slime_rt::send(
        DIAGNOSTICS_INGRESS_SLOT,
        &inline_sample(diagnostics_stream::TYPE_TAG, 1, FLAG_LAST).encode(),
        &[],
    ) != ERR_SUCCESS
    {
        fail(b"visibility diagnostics publish");
    }
    slime_rt::debug_write(b"[fabric-publisher-b] unrelated diagnostics published\n");
}

/// C8.12: the alternate-name route, and the two mismatches it makes visible.
///
/// This component publishes on `telemetry-alt` — the same `TelemetryStream`
/// interface `fabric-publisher` uses under a different name. Because route
/// authority folds the name into the identity, the two are distinct routes, and
/// this component holds an edge on exactly one of them.
///
/// Three requests, in the order a reader needs them:
///
/// 1. `telemetry` under the right type — the *other* route's name, which this
///    component holds no edge on. Refused, and the refusal names nothing, so it
///    cannot be used to discover whether that route exists.
/// 2. `telemetry-alt` under the *diagnostics* type tag — the right name under a
///    conflicting type. A different identity, therefore a different route, not
///    a badly typed request against a known one. Refused.
/// 3. `telemetry-alt` under its own type — the exact compatible tuple. Matched.
///
/// Asking for the denials first is deliberate: a broker that granted the role
/// before checking would pass the third request whatever the first two did.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    match request_role(
        TELEMETRY_ROUTE,
        telemetry_stream::TYPE_TAG,
        DIRECTION_PUBLISH,
    ) {
        Ok(Outcome::Denied(_)) => {
            slime_rt::debug_write(b"[fabric-publisher-b] alternate name denied\n");
        }
        Ok(Outcome::Role(_)) => fail(b"a route this component holds no edge on was granted"),
        Err(_) => fail(b"matrix alternate-name request"),
    }

    match request_role(
        MATRIX_ALT_ROUTE,
        diagnostics_stream::TYPE_TAG,
        DIRECTION_PUBLISH,
    ) {
        Ok(Outcome::Denied(_)) => {
            slime_rt::debug_write(b"[fabric-publisher-b] conflicting type denied\n");
        }
        Ok(Outcome::Role(_)) => fail(b"a conflicting interface aliased to a declared route"),
        Err(_) => fail(b"matrix conflicting-type request"),
    }

    let alt = route_identity(
        MATRIX_ALT_ROUTE,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    match request_role(
        MATRIX_ALT_ROUTE,
        telemetry_stream::TYPE_TAG,
        DIRECTION_PUBLISH,
    ) {
        Ok(Outcome::Role(descriptor)) => {
            if descriptor.rights_mask != RIGHT_SEND
                || !valid_capability_transfer(
                    &descriptor,
                    &alt,
                    DIRECTION_PUBLISH,
                    OBJECT_KIND_ENDPOINT,
                )
            {
                fail(b"matrix alternate-route role");
            }
        }
        Ok(Outcome::Denied(_)) => fail(b"the exact compatible tuple was denied"),
        Err(_) => fail(b"matrix alternate-route request"),
    }
    slime_rt::debug_write(b"[fabric-publisher-b] matrix alternate route matched\n");
}

/// A role reply that carries no capability.
///
/// The visibility broker answers with the descriptor alone, so unlike
/// [`receive_role`] there is nothing to import.
fn receive_declared_role() -> WireCapabilityTransfer {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message).filter(|record| {
                    record.magic == slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC
                }) else {
                    continue;
                };
                if descriptor.status != 0 {
                    fail(b"visibility diagnostics role");
                }
                return descriptor;
            }
        }
    }
}

/// Allocate, fill, seal, and loan one payload larger than the control bound,
/// then send only its descriptor and wait for the fabric to take it.
///
/// The loan is made to the fabric, named by the supervision capability the
/// generation granted. Waiting for the credit before returning is not
/// politeness: this task's own termination settles every loan it lent, so
/// exiting before the fabric has copied the bytes would reclaim the region out
/// from under the copy in flight. That is the C7.5 retention rule, asserted
/// rather than raced.
fn publish_large(route_slot: u32, credit_slot: u32) {
    let buffer = match slime_rt::shared_buffer_create(FACTORY_SLOT, PAGES, true) {
        Ok(buffer) => buffer,
        Err(_) => fail(b"create"),
    };
    // Map the whole buffer writable, then write the payload at its offset. The
    // leading page stays zero, so a reader that ignored the descriptor's offset
    // would verify zeros rather than the payload.
    if slime_rt::shared_buffer_map(buffer.slot, BASE, 0, LOAN_LEN, true) != ERR_SUCCESS {
        fail(b"writable map");
    }
    // SAFETY: the kernel installed a writable user mapping of exactly
    // `LOAN_LEN` bytes at `BASE`, and it stays mapped until the unmap below.
    unsafe {
        let bytes = (BASE + PAYLOAD_OFFSET) as *mut u8;
        for index in 0..PAYLOAD_LEN as usize {
            bytes.add(index).write_volatile((index % 251) as u8);
        }
    }
    // A loan requires an irreversibly sealed source: the publisher gives up its
    // own write authority before anyone else can read the bytes.
    if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {
        fail(b"seal");
    }
    let loan = match slime_rt::shared_buffer_loan(buffer.slot, FABRIC_SLOT, 0, LOAN_LEN, false) {
        Ok(loan) => loan,
        Err(_) => fail(b"loan"),
    };

    // Only the descriptor crosses the channel; it names the loan by its
    // unforgeable kernel-assigned identity, and the sample by its offset within
    // that loan.
    let descriptor = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: FORMAT_VERSION,
        flags: FLAG_LAST,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: loan.id,
        offset: PAYLOAD_OFFSET,
        length: PAYLOAD_LEN,
        type_identity: telemetry_stream::TYPE_TAG,
        sequence: 1,
        reserved: [0; 8],
    };
    if slime_rt::capability_delegate(
        route_slot,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        1 << 9,
        &descriptor.encode(),
    ) != ERR_SUCCESS
    {
        fail(b"publish descriptor");
    }

    // Block until the fabric reports this exact sample taken. Only then may
    // this component reclaim: the creator cannot pull pages out from under an
    // outstanding loan, and exiting would do exactly that.
    //
    // The credit is validated, not merely awaited: a `SAMPLE_TAKEN` naming a
    // different sequence would settle a sample this one is still waiting on,
    // so accepting any message here would turn the barrier into a coin flip.
    let mut credit = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        // A blocking receive, because this loop has nothing else to do and the
        // fabric is blocked sending the credit. Polling here made both sides
        // wait on each other: the broker's `send` never found a receiver, so it
        // could not return to the pass that would have satisfied this one.
        let length = match slime_rt::recv_blocking(credit_slot, &mut credit, &mut no_caps) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"await fabric credit"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"credit is not one control message");
        }
        let event = WireStreamEvent::decode(&credit).unwrap_or_else(|| fail(b"decode credit"));
        if !valid_stream_event(&event, telemetry_stream::TYPE_TAG)
            || event.event != EVENT_SAMPLE_TAKEN
            || event.sequence != descriptor.sequence
        {
            fail(b"credit names a different sample");
        }
        break;
    }
    slime_rt::debug_write(b"[fabric-publisher-b] loan settled by fabric\n");

    // Drop the local mapping and release the buffer. The kernel retains the
    // pages while the fabric's loan is outstanding, so this cannot pull the
    // bytes out from under the copy in flight (C7.5).
    if slime_rt::shared_buffer_unmap(buffer.slot, BASE) != ERR_SUCCESS {
        fail(b"unmap");
    }
    if slime_rt::shared_buffer_release(buffer.slot) != ERR_SUCCESS {
        fail(b"release");
    }
}

/// One inline sample of `type_tag`, payload derived from the sequence so a
/// subscriber can verify exactly which sample it received.
fn inline_sample(type_tag: u64, sequence: u64, flags: u32) -> WireStreamSample {
    let mut payload = [0u8; MAX_INLINE_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (sequence as u8).wrapping_mul(3).wrapping_add(index as u8);
    }

    WireStreamSample {
        magic: STREAM_SAMPLE_MAGIC,
        version: FORMAT_VERSION,
        flags,
        payload_len: MAX_INLINE_BYTES as u32,
        sequence,
        type_identity: type_tag,
        payload,
    }
}

fn advance_time(now_ns: u64) {
    let message = WireTimeAdvance {
        magic: TIME_ADVANCE_MAGIC,
        version: TIME_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns,
        reserved: [0; 40],
    }
    .encode();
    if slime_rt::send(TIME_SLOT, &message, &[]) < 0 {
        fail(b"time advance");
    }
}

fn await_time_credit(now_ns: u64) {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        let length = match slime_rt::recv(TIME_SLOT, &mut message, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"time credit"),
            n => n as usize,
        };
        let value = WireTimeAdvance::decode(&message[..length])
            .unwrap_or_else(|| fail(b"time credit decode"));
        if !slime_proto::valid_time_advance(&value) || value.now_ns != now_ns {
            fail(b"time credit mismatch")
        }
        return;
    }
}

fn ring_publish(
    ring: &mut Ring<'_>,
    payload: &[u8],
    last: bool,
    ready_slot: u32,
    credit_slot: u32,
) {
    loop {
        match ring.publish(payload, last) {
            Ok(_) => {
                let _ = slime_rt::notification_signal(ready_slot);
                return;
            }
            Err(RingError::Full) => {
                let _ = slime_rt::notification_wait(credit_slot);
            }
            Err(_) => fail(b"publish ring"),
        }
    }
}

/// One request provisions every edge the graph declares for this component. The
/// fields it carries are read and discarded by the fabric, exactly as for a
/// single-route participant.
fn request_roles() -> i64 {
    let mut route_name = [0u8; 32];
    route_name[..TELEMETRY_ROUTE.len()].copy_from_slice(TELEMETRY_ROUTE.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_PUBLISH,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: TELEMETRY_ROUTE.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    let encoded = request.encode();
    slime_rt::send(CONTROL_SLOT, &encoded, &[])
}

/// The typed role descriptor and the slot its shared ring landed in.
///
/// Only a record whose magic is `CAPABILITY_TRANSFER_MAGIC` is a role reply.
/// The fabric also sends QoS events on this same control endpoint, and a v2
/// role reply carries no capability in the message -- the ring crosses as a
/// root-side export this component claims -- so `received[0]` no longer tells
/// the two apart. Discriminating on the record's own magic does, and it is the
/// same field every other reader of these bytes already trusts.
fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message).filter(|record| {
                    record.magic == slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC
                }) else {
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

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(slime_proto::sample_descriptor::DESCRIPTOR_LEN == MAX_MSG);
