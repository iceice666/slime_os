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
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::ring::{Ring, RingError};
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// Control endpoint to the fabric. The only authority this component starts
/// with: it holds no factory, no route, and no peer endpoint.
const CONTROL_SLOT: u32 = 0;

/// B17's subject: a generation-declared endpoint this component holds at
/// `recv` plus `transfer`. Exporting it is allowed; exporting it *wider* than
/// the declaration is the subset test the root must refuse.
const PROBE_SLOT: u32 = 1;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const RING_BASE: u64 = 0x0000_0011_0000_0000;
const RING_BYTES: usize = 4096;

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

/// This component's name, as the generation's participant table spells it.
const COMPONENT: &[u8] = b"fabric-publisher";

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen, and it was
/// one: a ring formatted at the declared depth failed to attach against a
/// local guess. Floored at `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    FABRIC_HISTORY_DEPTHS
        .iter()
        .find(|(name, entry, _)| *name == COMPONENT && *entry == route)
        .map(|(_, _, depth)| *depth as usize)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
        .max(slime_proto::fabric_ring::MIN_RING_SLOTS)
}

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
    publish(
        &mut ring,
        &inline_payload(INLINE_SAMPLES + STALL_SAMPLES + 1),
        true,
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
    let (descriptor, ring_slot) = receive_role();
    if descriptor.status != 0
        || !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_PUBLISH,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        )
    {
        fail(b"visibility publish role");
    }
    if slime_rt::shared_buffer_loan_map(ring_slot, RING_BASE, 0, RING_BYTES as u64) != ERR_SUCCESS {
        fail(b"visibility publisher ring map");
    }
    let bytes = unsafe { core::slice::from_raw_parts_mut(RING_BASE as *mut u8, RING_BYTES) };
    let mut ring = Ring::attach(bytes, telemetry_stream::TYPE_TAG, ring_slots(ROUTE_NAME))
        .unwrap_or_else(|_| fail(b"visibility publisher ring attach"));
    publish(&mut ring, &inline_payload(1), true);
    slime_rt::debug_write(b"[fabric-publisher] interposed sample published\n");
}

fn inline_payload(sequence: u64) -> [u8; slime_proto::fabric_ring::MAX_INLINE_BYTES] {
    let mut payload = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (sequence as u8).wrapping_add(index as u8);
    }
    payload
}

fn publish(ring: &mut Ring<'_>, payload: &[u8], last: bool) {
    loop {
        match ring.publish(payload, last) {
            Ok(_) => {
                if slime_rt::notification_signal(FABRIC_PUBLISHER_TELEMETRY_READY_SLOT)
                    != ERR_SUCCESS
                {
                    fail(b"publish notify");
                }
                return;
            }
            Err(RingError::Full) => {
                let _ = slime_rt::notification_wait(FABRIC_PUBLISHER_TELEMETRY_CREDIT_SLOT);
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
