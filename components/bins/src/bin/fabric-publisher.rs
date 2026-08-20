#![no_std]
#![no_main]

//! C8.3/C8.4 fabric publisher: one attenuated route role, then bounded typed
//! samples over it.
//!
//! Asks the fabric for its edge over the generation-provisioned control
//! endpoint, receives a `RIGHT_SEND`-only endpoint through the kernel's
//! narrow-on-transfer move, and attempts every operation the role must not
//! permit. Each denial is asserted before the operation it guards, so a
//! regression that widens a route role fails here even when the happy path
//! still publishes.
//!
//! It then publishes inline samples: records whose payload fits the control
//! message whole, so nothing is copied and no shared buffer is involved. The
//! `>MAX_MSG` arm belongs to `fabric-publisher-b`, which owns the descriptor
//! and loan path; keeping the two in separate components is what makes the
//! many-to-many check a real fan-in rather than one component talking twice.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, route_identity};
use slime_components::fabric_visibility::{ViewPage, request_page};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, OBJECT_KIND_SHARED_BUFFER_LOAN,
    REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{
    FLAG_LAST, MAX_INLINE_BYTES as STREAM_MAX_INLINE_BYTES, STREAM_SAMPLE_MAGIC, WireStreamSample,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::ring::{Ring, RingError};
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{RIGHT_RECV, RIGHT_SEND};

// C8.13.2: this participant's own shared-buffer occupancy evidence. Both files
// are included here rather than reached through `slime_components` because a
// file may be a module only once per crate, which is the same rule
// `fabric-call-worker` follows for the trace sink it hosts.
#[path = "../fabric_trace_log.rs"]
mod trace_log;

#[path = "../fabric_occupancy_trace.rs"]
mod occupancy_trace;

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// Control endpoint to the fabric. The only authority this component starts
/// with: it holds no factory, no route, and no peer endpoint.
const CONTROL_SLOT: u32 = 0;

/// B17's subject: a generation-declared endpoint this component holds at
/// `recv` plus `transfer`. Exporting it is allowed; exporting it *wider* than
/// the declaration is the subset test the root must refuse.
const PROBE_SLOT: u32 = 1;

/// The visibility plane's declared telemetry ingress edge, send-only.
///
/// A generation fact rather than a runtime grant: `sel4-visibility.zti` binds
/// `visibility-telemetry-ingress` here, so the endpoint is installed before
/// this component runs and the fabric's role reply only names it.
const TELEMETRY_INGRESS_SLOT: u32 = 1;

const RING_BASE: u64 = 0x0000_0011_0000_0000;
const RING_BYTES: usize = 4096;

const ROUTE_NAME: &str = "telemetry";
/// C8.14: when set, this publisher exits without publishing its terminal
/// sample, so the fabric observes a genuine stream-family peer death rather
/// than an orderly end. Compile-time, like the interposition hop's early exit,
/// so no plane gains an ambient switch and the product image cannot carry it.
const STREAM_EARLY_EXIT: bool = option_env!("SLIME_FABRIC_STREAM_EARLY_EXIT").is_some();

/// Inline samples published before the terminal one. Two is enough to prove the
/// fabric preserves sequence across the inline path without making the
/// transcript depend on a count.
const INLINE_SAMPLES: u64 = 2;

/// Samples published after the stalled subscriber stops acking.
///
/// Enough to overrun the shallow BEST_EFFORT ring — so a stall costs it
/// something the fabric must report rather than retry — while staying inside
/// the deeper RELIABLE ring, which must lose nothing. The two declared depths
/// are what make that gap exist.
const STALL_SAMPLES: u64 = 4;

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen, and it was
/// one: a ring formatted at the declared depth failed to attach against a
/// local guess. Floored at `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    let identity = route_identity(
        route,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    ring_slots_checked(&identity)
}

/// This component's declared KEEP_LAST depth, read from the graph.
///
/// Carried a cross-check against `FABRIC_HISTORY_DEPTHS` while both statements
/// of the depth existed; the table is gone, so agreement is no longer something
/// to assert -- there is one source (B70/CP2).
fn ring_slots_checked(identity: &[u8; 32]) -> usize {
    slime_components::fabric_self_view::ring_slots(identity)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
}

/// A non-holder reads *its own* rows and nothing else (B70/CP2 step 2).
///
/// Step 1 refused this component outright; step 2 answers it its own share. The
/// property that must still hold is the one C8.8 depends on: it cannot see the
/// graph.
///
/// Carried a cross-check against `FABRIC_PARTICIPANTS` while both statements of
/// the graph existed; the table is gone, so the count this component expects can
/// no longer be derived from anywhere but the reply itself, and comparing a reply
/// against a number read out of that same reply asserts nothing. What replaces it
/// is stronger than the count it drops: every returned row must carry *this*
/// component's identity. That claim is independent of the root because
/// `component_identity` is a hash of a name this component spells itself, and it
/// subsumes the old sibling-refusal check -- a leaked row fails it whether the
/// leak names `fabric-subscriber` or anything else (B70/CP2).
fn prove_graph_self_view() {
    let mut rows = slime_components::fabric_self_view::EMPTY_ROWS;
    let count = slime_components::fabric_self_view::rows(&mut rows)
        .unwrap_or_else(|_| fail(b"graph read refused a declared participant"));
    // A participant declares at least one row on every plane it runs on, so an
    // empty answer is a failed read wearing the shape of a scoped one.
    if count == 0 {
        fail(b"graph self view answered no rows to a declared participant");
    }
    let own = boot_contracts::fabric_graph::component_identity("fabric-publisher");
    for row in rows.iter().take(count) {
        if row.component_identity != own {
            fail(b"graph read disclosed a component this one shares no edge with");
        }
    }
    slime_rt::debug_write(b"[fabric-publisher] graph read is scoped to this component\n");
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-publisher] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    prove_graph_self_view();
    if GENERATION_BOOT_ACTION == "visibility" {
        visibility_main();
        return;
    }
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::provision_and_park(
            b"fabric-publisher",
            ROUTE_NAME,
            &route,
            telemetry_stream::TYPE_TAG,
            DIRECTION_PUBLISH,
            1,
        );
    }

    // The request names the route and type it wants. None of it is authority:
    // the fabric answers from the generation graph keyed by this control
    // endpoint, so these fields only prove the ask was well-formed.
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
    if send_request(&request) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-publisher] role requested\n");
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
        fail(b"publisher ring map");
    }
    slime_rt::debug_write(b"[fabric-publisher] publish role received\n");
    // B17's subset test, and the two rules that bound a declared role, on the
    // native export path. `PROBE_SLOT` holds a generation-declared endpoint
    // whose grant carries `recv` plus `transfer`: the root refuses an export
    // asking for anything the declaration does not carry, and refuses one from
    // a role whose grant is not transferable at all.
    //
    // Each denial is asserted *before* the samples below, so a regression that
    // widened a role would fail here even with the happy path intact.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(CONTROL_SLOT, &mut discard, &mut no_caps) == ERR_SUCCESS {
        fail(b"publisher received on a control endpoint with no traffic");
    }
    slime_rt::debug_write(b"[fabric-publisher] route receive denied\n");
    // The control endpoint is declared non-transferable, so exporting it is
    // refused whatever rights are asked for.
    if export_probe(CONTROL_SLOT, RIGHT_SEND) == ERR_SUCCESS {
        fail(b"publisher re-delegated its control endpoint");
    }
    slime_rt::debug_write(b"[fabric-publisher] re-delegation denied\n");
    // The probe endpoint *is* transferable, and declared `recv` only. Asking
    // for `send` as well is the subset test: `rights & !declared != 0`.
    if export_probe(PROBE_SLOT, RIGHT_SEND | RIGHT_RECV) == ERR_SUCCESS {
        fail(b"publisher widened a narrowed transfer role");
    }
    slime_rt::debug_write(b"[fabric-publisher] widening denied\n");
    let bytes = unsafe { core::slice::from_raw_parts_mut(RING_BASE as *mut u8, RING_BYTES) };
    let mut ring = Ring::attach(bytes, telemetry_stream::TYPE_TAG, ring_slots(ROUTE_NAME))
        .unwrap_or_else(|_| fail(b"publisher ring attach"));
    for sequence in 1..=INLINE_SAMPLES {
        publish(&mut ring, &inline_payload(sequence), false);
    }
    slime_rt::debug_write(b"[fabric-publisher] inline samples published\n");
    for sequence in INLINE_SAMPLES + 1..=INLINE_SAMPLES + STALL_SAMPLES {
        publish(&mut ring, &inline_payload(sequence), false);
    }
    slime_rt::debug_write(b"[fabric-publisher] stall-window samples published\n");
    // C8.14: on the fault variant this route's publisher leaves without ending
    // its stream, which is the one stream-family degradation no participant can
    // otherwise ask for -- an orderly `FLAG_LAST` and a peer death are mutually
    // exclusive, so the fault must be scripted the way the interposition hop's
    // is. Every other plane publishes the terminal sample as before.
    if !STREAM_EARLY_EXIT {
        publish(
            &mut ring,
            &inline_payload(INLINE_SAMPLES + STALL_SAMPLES + 1),
            true,
        );
    }
    // C8.13.2: gated to the traffic plane, so the standalone stream/QoS
    // fixtures — whose declared `traceDepth` is sized for the records C8.11
    // already emits — never receive these and never drop one.
    if GENERATION_BOOT_ACTION == "traffic" {
        occupancy_trace::report(b"publisher", FABRIC_TRACE_DEPTH);
    }
    slime_rt::debug_write(b"[fabric-publisher] done\n");
}

/// C8.12: the exactly-compatible tuple on the `telemetry` route.
///
/// This component is the plane's positive control. It asks under exactly the
/// (route name, type tag, direction) the graph declares for it and must receive
/// a role — so if the matrix refused everything, the plane would fail here
/// rather than pass on an absence of denials.
///
/// Its sample then travels the declared interposition chain. It sends on its
/// ingress edge and never to the subscriber, which it holds no edge to.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    match request_role(ROUTE_NAME, telemetry_stream::TYPE_TAG, DIRECTION_PUBLISH) {
        Ok(Outcome::Role(descriptor)) => {
            if descriptor.rights_mask != RIGHT_SEND
                || !valid_capability_transfer(
                    &descriptor,
                    &route,
                    DIRECTION_PUBLISH,
                    OBJECT_KIND_ENDPOINT,
                )
            {
                fail(b"matrix publish role");
            }
        }
        Ok(Outcome::Denied(_)) => fail(b"the exact compatible tuple was denied"),
        Err(_) => fail(b"matrix role request"),
    }
    slime_rt::debug_write(b"[fabric-publisher] matrix exact tuple matched\n");

    // The declared ingress edge is send-only. Asserted before the sample, so a
    // regression that widened the declaration fails here rather than passing
    // behind a successful publish.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(TELEMETRY_INGRESS_SLOT, &mut discard, &mut no_caps) == ERR_SUCCESS {
        fail(b"matrix publisher widened");
    }
    if slime_rt::send(
        TELEMETRY_INGRESS_SLOT,
        &inline_sample(1, FLAG_LAST).encode(),
        &[],
    ) != ERR_SUCCESS
    {
        fail(b"matrix publish");
    }
    slime_rt::debug_write(b"[fabric-publisher] matrix sample published\n");

    // B73: page this component's own graph-wide view and assert what it admits.
    //
    // The plane already proves the `private` branch through `fabric-observer`,
    // which sees exactly `diagnostics`. Nothing observed the `graph` branch, so
    // the route set a graph holder is shown went unasserted: the view was paged
    // and counted elsewhere, never read. Counting alone is invariant under any
    // permutation of the contents, so the names are asserted in order.
    //
    // The expected names are source literals rather than anything derived from
    // `sel4-matrix.zti`. An expectation generated from the same fixture a
    // mutation edits moves with it and stays green — the vacuity that retired
    // `FABRIC_INTERPOSITIONS`. These are the graph's public shape, which is
    // what `graph` visibility means, not another component's private data.
    let mut cursor = 0;
    let mut routes = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"matrix graph view")) {
            ViewPage::Route(record) => {
                let expected = if routes == 0 {
                    b"telemetry".as_slice()
                } else if routes == 1 {
                    b"telemetry-alt".as_slice()
                } else if routes == 2 {
                    b"diagnostics".as_slice()
                } else {
                    fail(b"matrix graph view exposed extra route")
                };
                if &record.route_name[..record.route_name_len as usize] != expected {
                    // Name the route that should have been at this position.
                    // A route dropping out of the view shifts every later one
                    // forward, so the position that mismatches is the position
                    // the missing route vacated.
                    slime_rt::debug_write(b"[fabric-publisher] matrix graph view expected ");
                    slime_rt::debug_write(expected);
                    slime_rt::debug_write(b" but was shown ");
                    slime_rt::debug_write(&record.route_name[..record.route_name_len as usize]);
                    slime_rt::debug_write(b"\n");
                    fail(b"matrix graph view route order");
                }
                if record.contract_kind != CONTRACT_KIND_STREAM as u8 {
                    fail(b"matrix graph route metadata");
                }
                routes += 1;
                cursor = record.cursor;
            }
            // The matrix broker answers routes only; kept for shape parity with
            // the visibility plane's loop, and deliberately not counted.
            ViewPage::Qos(record) => cursor = record.cursor,
            ViewPage::End(record) => {
                let _ = record.cursor;
                break;
            }
        }
    }
    if routes != 3 {
        fail(b"matrix graph view bound");
    }
    slime_rt::debug_write(b"[fabric-publisher] matrix graph view routes=3\n");
}
fn visibility_main() {
    let mut cursor = 0;
    let mut routes = 0;
    let mut qos = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"graph view")) {
            ViewPage::Route(record) => {
                let expected = if routes == 0 {
                    b"telemetry".as_slice()
                } else if routes == 1 {
                    b"diagnostics".as_slice()
                } else {
                    fail(b"graph view exposed extra route")
                };
                if &record.route_name[..record.route_name_len as usize] != expected
                    || record.contract_kind != CONTRACT_KIND_STREAM as u8
                {
                    fail(b"graph route metadata");
                }
                routes += 1;
                cursor = record.cursor;
            }
            ViewPage::Qos(record) => {
                if record.matched != 1 {
                    fail(b"graph match metadata");
                }
                qos += 1;
                cursor = record.cursor;
            }
            ViewPage::End(record) => {
                let _ = record.cursor;
                break;
            }
        }
    }
    if routes != 2 || qos != 2 {
        fail(b"graph view bound");
    }
    slime_rt::debug_write(b"[fabric-publisher] graph view routes=2\n");

    // The visibility plane's roles are *generation-declared endpoints*, not
    // ring loans. The fabric answers a role request with a descriptor alone —
    // there is no capability in the reply and nothing to import — because the
    // edge this component publishes on was fixed before either task ran and is
    // already installed in its CSpace. The descriptor still has to be checked:
    // it is how the broker states which route the declared slot serves, and a
    // reply naming another route or direction means the graph disagrees with
    // the manifest.
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
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
    if send_request(&request) != ERR_SUCCESS {
        fail(b"visibility role request");
    }
    let descriptor = receive_declared_role();
    if descriptor.rights_mask != RIGHT_SEND
        || !valid_capability_transfer(&descriptor, &route, DIRECTION_PUBLISH, OBJECT_KIND_ENDPOINT)
    {
        fail(b"visibility publish role");
    }
    // The declared ingress edge is send-only, so receiving on it must be
    // refused. Asserted before the sample so a regression that widened the
    // declaration fails here rather than passing silently.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(TELEMETRY_INGRESS_SLOT, &mut discard, &mut no_caps) == ERR_SUCCESS {
        fail(b"visibility publisher widened");
    }
    if slime_rt::send(
        TELEMETRY_INGRESS_SLOT,
        &inline_sample(1, FLAG_LAST).encode(),
        &[],
    ) != ERR_SUCCESS
    {
        fail(b"visibility publish");
    }
    slime_rt::debug_write(b"[fabric-publisher] interposed sample published\n");
}

/// One inline sample: the payload is a deterministic function of the sequence,
/// so a subscriber can verify it received the exact sample the publisher sent
/// rather than merely a well-formed one.
fn inline_sample(sequence: u64, flags: u32) -> WireStreamSample {
    let mut payload = [0u8; STREAM_MAX_INLINE_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (sequence as u8).wrapping_add(index as u8);
    }
    WireStreamSample {
        magic: STREAM_SAMPLE_MAGIC,
        version: FORMAT_VERSION,
        flags,
        payload_len: STREAM_MAX_INLINE_BYTES as u32,
        sequence,
        type_identity: telemetry_stream::TYPE_TAG,
        payload,
    }
}

/// A role reply that carries no capability.
///
/// The visibility broker answers with the descriptor alone, so unlike
/// [`receive_role`] there is nothing to import. QoS events share this control
/// endpoint, so the record's own magic is what tells the two apart.
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
                    fail(b"visibility publish role");
                }
                return descriptor;
            }
        }
    }
}

fn inline_payload(sequence: u64) -> [u8; slime_proto::fabric_ring::MAX_INLINE_BYTES] {
    let mut payload = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (sequence as u8).wrapping_add(index as u8);
    }
    payload
}

/// This component's two notification slots, resolved through the root by the
/// grant names the generation declares (CP2/B70).
///
/// `notification:` is its own namespace because `notificationBindings` is a
/// separate declaration from capability grants, and one grant binds a slot in
/// both peers -- the root answers per-holder, which is what makes a bare grant
/// name unambiguous here. Resolved once per `publish` rather than cached in a
/// static, because this component has no initialization phase that runs before
/// the first publish.
fn ready_slot() -> u32 {
    slime_rt::resolve_binding(b"notification:fabric-publisher-telemetry-ready")
        .unwrap_or_else(|_| fail(b"resolve telemetry-ready notification"))
}

fn credit_slot() -> u32 {
    slime_rt::resolve_binding(b"notification:fabric-publisher-telemetry-credit")
        .unwrap_or_else(|_| fail(b"resolve telemetry-credit notification"))
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
            Err(RingError::Full) => {
                let _ = slime_rt::notification_wait(credit_slot());
            }
            Err(_) => fail(b"publish ring"),
        }
    }
}

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

fn send_request(request: &WireFabricRequest) -> i64 {
    slime_rt::send(CONTROL_SLOT, &request.encode(), &[])
}

/// Ask the root to export `slot` at `rights` over the control endpoint.
///
/// The descriptor is well-formed on purpose: authority is decided by the
/// generation's declaration for the slot, never by what the message claims,
/// so a refusal here is a capability property rather than a parse failure.
fn export_probe(slot: u32, rights: u64) -> i64 {
    let descriptor = WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags: slime_proto::capability_transfer::FLAG_RETAIN_TRANSFER,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        direction: DIRECTION_PUBLISH,
        rights_mask: rights,
        route_identity: [0u8; 32],
    };
    slime_rt::capability_delegate(
        CONTROL_SLOT,
        slot,
        slime_rt::CapabilityDisposition::Retain,
        slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        rights,
        &descriptor.encode(),
    )
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
