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
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FLAG_RETAIN_TRANSFER, FORMAT_VERSION,
    OBJECT_KIND_ENDPOINT, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{
    FLAG_LAST, MAX_INLINE_BYTES, STREAM_SAMPLE_MAGIC, WireStreamSample,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

/// Control endpoint to the fabric. The only authority this component starts
/// with: it holds no factory, no route, and no peer endpoint.
const CONTROL_SLOT: u32 = 0;

/// B17's subject, when the graph declares one: an endpoint end this component
/// was *spawn-granted* at `send`+`transfer`. The two following slots are a
/// private carrier pair used to send the narrowed authority back to this same
/// component, so the fabric never consumes the proof message.
const PROBE_SLOT: u32 = 1;
const PROBE_CARRIER_SEND_SLOT: u32 = 2;
const PROBE_CARRIER_RECV_SLOT: u32 = 3;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;
const RIGHT_TRANSFER: u64 = 4;

const ROUTE_NAME: &str = "telemetry";

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

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-publisher] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_main();
        return;
    }
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        // C8.10 launches every plane at once and asserts the graph reaches
        // healthy blocked idle *with no traffic*, so this publisher takes its
        // declared role — data endpoint plus credit channel — and parks rather
        // than publishing. The sample path stays C8.4's to prove.
        slime_components::fabric_boot::provision_and_park(
            b"fabric-publisher",
            ROUTE_NAME,
            &route,
            telemetry_stream::TYPE_TAG,
            DIRECTION_PUBLISH,
            2,
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
    // Before the role request, because it is the only point at which
    // `PROBE_SLOT` is unambiguous: provisioning installs capabilities at the
    // first free slots, so after it slot 1 holds a route role in any graph that
    // granted no probe. Running here, the slot holds either the spawn-granted
    // probe or nothing at all.
    subset_test_arm(&route);

    if send_request(&request) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-publisher] role requested\n");
    // Two capabilities arrive for one route: the send-only data endpoint and a
    // receive-only credit endpoint the fabric uses to report that a loaned
    // sample has been taken. This publisher sends only inline samples, so it
    // never waits on the credit channel — it accepts it because the graph
    // declares one publisher role, and both halves of that role are provisioned
    // together.
    let mut data = None;
    let mut credit = None;
    for _ in 0..2 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared publisher was denied");
        }
        if !valid_capability_transfer(&descriptor, &route, DIRECTION_PUBLISH, OBJECT_KIND_ENDPOINT)
        {
            fail(b"descriptor does not name this role");
        }
        // The descriptor is the kernel's own record of what it installed, so an
        // endpoint arriving with more than one direction would show here.
        match descriptor.rights_mask {
            RIGHT_SEND => data = Some((descriptor, slot)),
            RIGHT_RECV => credit = Some(slot),
            _ => fail(b"publisher role carries more than one direction"),
        }
    }
    let (Some((descriptor, route_slot)), Some(_credit_slot)) = (data, credit) else {
        fail(b"a declared publisher capability never arrived");
    };
    slime_rt::debug_write(b"[fabric-publisher] publish role received\n");

    // A publisher has no receive authority on its own route: the fabric holds
    // the other half of the channel, and this half was narrowed to send.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(route_slot, &mut discard, &mut no_caps) != ERR_BAD_CAP {
        fail(b"publisher could receive on its route");
    }
    slime_rt::debug_write(b"[fabric-publisher] route receive denied\n");

    // A provisioned role is terminal. Re-transferring it — even back over the
    // control endpoint, even narrowing further — must fail: the move omitted
    // `RIGHT_TRANSFER`, so the kernel refuses before anything crosses.
    let redelegation = WireCapabilityTransfer {
        rights_mask: RIGHT_SEND,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &redelegation.encode()) != ERR_BAD_CAP {
        fail(b"publisher re-delegated its route");
    }
    slime_rt::debug_write(b"[fabric-publisher] re-delegation denied\n");

    // Nor can it widen its own role by asking the kernel for more than it
    // holds: `derive` inside the transfer path is narrow-only.
    let widened = WireCapabilityTransfer {
        rights_mask: RIGHT_SEND | RIGHT_RECV,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &widened.encode()) != ERR_BAD_CAP {
        fail(b"publisher widened its route rights");
    }
    slime_rt::debug_write(b"[fabric-publisher] widening denied\n");

    // With every denial observed, the role does what it is for.
    for sequence in 1..=INLINE_SAMPLES {
        publish(route_slot, &inline_sample(sequence, 0).encode());
    }
    slime_rt::debug_write(b"[fabric-publisher] inline samples published\n");

    // Keep publishing past the point where the stalled subscriber stops
    // acking. Its ring is what must absorb these — evicting the oldest at its
    // declared depth — so a stall has an observable cost. Without them the
    // stall would be free and the loss arm would prove nothing.
    //
    // These are sent unconditionally rather than after a handshake: the fabric
    // holds them in each subscriber's ring regardless of when that subscriber
    // reads, so the keeping-up reader still receives every one while the
    // stalled reader loses the oldest. Coordinating instead would make the
    // check depend on a scheduling order rather than on the ring's own bound.
    for sequence in INLINE_SAMPLES + 1..=INLINE_SAMPLES + STALL_SAMPLES {
        publish(route_slot, &inline_sample(sequence, 0).encode());
    }
    slime_rt::debug_write(b"[fabric-publisher] stall-window samples published\n");

    // The last sample retires this publisher's ingress, so the fabric can end
    // the route without waiting for the process to die.
    publish(
        route_slot,
        &inline_sample(INLINE_SAMPLES + STALL_SAMPLES + 1, FLAG_LAST).encode(),
    );
    slime_rt::debug_write(b"[fabric-publisher] done\n");
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
    let (descriptor, route_slot) = receive_role();
    if descriptor.rights_mask != RIGHT_SEND
        || !valid_capability_transfer(&descriptor, &route, DIRECTION_PUBLISH, OBJECT_KIND_ENDPOINT)
    {
        fail(b"visibility publish role");
    }
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(route_slot, &mut discard, &mut no_caps) != ERR_BAD_CAP {
        fail(b"visibility publisher widened");
    }
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &descriptor.encode()) != ERR_BAD_CAP {
        fail(b"visibility publisher retransfer");
    }
    publish(route_slot, &inline_sample(1, FLAG_LAST).encode());
    slime_rt::debug_write(b"[fabric-publisher] interposed sample published\n");
}

/// One inline sample: the payload is a deterministic function of the sequence,
/// so a subscriber can verify it received the exact sample the publisher sent
/// rather than merely a well-formed one.
fn inline_sample(sequence: u64, flags: u32) -> WireStreamSample {
    let mut payload = [0u8; MAX_INLINE_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (sequence as u8).wrapping_add(index as u8);
    }
    WireStreamSample {
        magic: STREAM_SAMPLE_MAGIC,
        version: FORMAT_VERSION,
        flags,
        payload_len: MAX_INLINE_BYTES as u32,
        sequence,
        type_identity: telemetry_stream::TYPE_TAG,
        payload,
    }
}

/// B17: the transfer contract's **subset test**, against the one capability
/// shape a declared graph can produce for it.
///
/// `cap_transfer` enforces four rules as one disjunction, and for every subject
/// this file already exercises, an earlier rule refuses a widening first. The
/// two arms above are exactly that: the route role carries no `RIGHT_TRANSFER`,
/// so the transfer-authority rule refuses both the re-delegation and the
/// widening before either mask is compared against anything.
///
/// Reaching the subset test needs a capability that holds transfer authority
/// **and** is strictly narrower than its kind admits. A route role is not one.
/// Nor is a factory (one operation right, no transfer bit) or an endpoint from
/// `endpoint_create` (exactly `send|recv|transfer`, which is what its kind
/// admits, so the per-kind rule catches any widening mask first).
///
/// A **spawn grant** is what produces it. The requested mask is installed
/// verbatim, so a parent granting `send|transfer` on an endpoint hands its
/// child a capability holding transfer authority at strictly less than
/// `Endpoint`'s `send|recv|transfer`. Asking to move that with `recv` restored
/// passes the transfer-authority rule, passes the descriptor/kind rule, and
/// computes zero against the per-kind mask — so only `rights & !source.rights`
/// can refuse it.
///
/// Guarded on **holding** the subject rather than on a check flag, because an
/// empty slot answers the same `ERR_BAD_CAP` the subset test does: a bare
/// widening arm would pass identically in a graph that never granted the
/// endpoint, which is coverage that looks real and is not. Possession is
/// established by *use* — a send on the granted end — so a graph without one
/// skips silently and claims nothing. The x86 graph grants none and skips; the
/// seL4 stream graph grants one and runs the arm.
fn subset_test_arm(route: &[u8; 32]) {
    // `ERR_WOULDBLOCK` counts as possession: the endpoint resolved and its
    // queue was full, which proves a live `send`-capable capability as much as
    // a delivery does. Only a slot holding no such capability answers
    // `ERR_BAD_CAP`.
    match slime_rt::send(PROBE_SLOT, &[0u8; MAX_MSG], &[]) {
        ERR_SUCCESS | ERR_WOULDBLOCK => {}
        // No probe in this graph. Nothing to test, and nothing claimed.
        _ => return,
    }
    // First prove a genuine subset operation succeeds: moving the capability
    // with exactly the rights it already holds. Retaining transfer authority
    // keeps the moved capability usable for the widening attempt below.
    let narrowing = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags: FLAG_RETAIN_TRANSFER,
        object_kind: OBJECT_KIND_ENDPOINT,
        direction: DIRECTION_PUBLISH,
        rights_mask: RIGHT_SEND | RIGHT_TRANSFER,
        route_identity: *route,
    };
    if slime_rt::cap_transfer(PROBE_CARRIER_SEND_SLOT, PROBE_SLOT, &narrowing.encode())
        != ERR_SUCCESS
    {
        fail(b"a valid narrowed transfer was refused");
    }
    let narrowed_slot = recv_cap(PROBE_CARRIER_RECV_SLOT);

    // Restoring `recv` is a strict widening relative to that proven source.
    let widening = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags: FLAG_RETAIN_TRANSFER,
        object_kind: OBJECT_KIND_ENDPOINT,
        direction: DIRECTION_PUBLISH,
        rights_mask: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        route_identity: *route,
    };
    if slime_rt::cap_transfer(PROBE_CARRIER_SEND_SLOT, narrowed_slot, &widening.encode())
        != ERR_BAD_CAP
    {
        fail(b"a spawn-granted role widened past its own rights");
    }
    slime_rt::debug_write(b"[fabric-publisher] narrowing succeeded and widening was refused\n");
}

fn recv_cap(slot: u32) -> u32 {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            value if value >= 0 && caps[0] != 0 => return caps[0] as u32,
            _ => fail(b"collect narrowed transfer"),
        }
    }
}

fn publish(route_slot: u32, message: &[u8; MAX_MSG]) {
    loop {
        match slime_rt::send(route_slot, message, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(route_slot)]),
            _ => fail(b"publish"),
        }
    }
}

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

fn send_request(request: &WireFabricRequest) -> i64 {
    let encoded = request.encode();
    loop {
        match slime_rt::send(CONTROL_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            result => return result,
        }
    }
}

/// Block until the fabric answers, returning the descriptor and the slot the
/// moved capability landed in (zero when the answer carried none).
fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            n if n < 0 => fail(b"role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message) else {
                    fail(b"decode role reply")
                };
                return (descriptor, received[0] as u32);
            }
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::SAMPLE_LEN == MAX_MSG);
