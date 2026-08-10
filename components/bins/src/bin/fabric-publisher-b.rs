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
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_TAKEN, FLAG_LAST, MAX_INLINE_BYTES, STREAM_SAMPLE_MAGIC, WireStreamEvent,
    WireStreamSample,
};
use slime_proto::fabric_time::{
    FORMAT_VERSION as TIME_VERSION, TIME_ADVANCE_MAGIC, WireTimeAdvance,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::{valid_capability_transfer, valid_stream_event};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

/// Control endpoint to the fabric — this component's only route authority path.
const CONTROL_SLOT: u32 = 0;
/// `SharedBufferFactory` granted by the generation, bounded by this component's
/// own `shared-buffer-budget` entry.
const FACTORY_SLOT: u32 = 1;
/// `RIGHT_SUPERVISE` handle naming the fabric service. The upstream loan names
/// its receiver through this capability, never through an ambient task id.
const FABRIC_SLOT: u32 = 2;
/// Generation-granted simulated monotonic-time input. This is a separate
/// capability from both publish routes; possessing a route grants no clock.
const TIME_SLOT: u32 = 3;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const TELEMETRY_ROUTE: &str = "telemetry";
const DIAGNOSTICS_ROUTE: &str = "diagnostics";

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

/// One route's pair of provisioned capabilities: the send-only data endpoint
/// and the receive-only credit endpoint. Held apart so neither route can borrow
/// the other's authority, and so a missing half is a named failure rather than
/// a silent hang.
#[derive(Default)]
struct RoutePair {
    data: Option<u32>,
    credit: Option<u32>,
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-publisher-b] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_main();
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
        // Declared on both stream routes, so one request returns four
        // capabilities: data plus credit for telemetry, then the same for
        // diagnostics, in the order the graph declares them.
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
                (telemetry, DIRECTION_PUBLISH, 2),
                (diagnostics, DIRECTION_PUBLISH, 2),
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
    // Four capabilities arrive: a send-only data endpoint and a receive-only
    // credit endpoint for each of the two declared routes. Each is matched by
    // the route identity its own descriptor carries and its direction mask, so
    // arrival order is not authority.
    let mut telemetry_pair = RoutePair::default();
    let mut diagnostics_pair = RoutePair::default();
    for _ in 0..4 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared publisher was denied");
        }
        let pair = if valid_capability_transfer(
            &descriptor,
            &telemetry,
            DIRECTION_PUBLISH,
            OBJECT_KIND_ENDPOINT,
        ) {
            &mut telemetry_pair
        } else if valid_capability_transfer(
            &descriptor,
            &diagnostics,
            DIRECTION_PUBLISH,
            OBJECT_KIND_ENDPOINT,
        ) {
            &mut diagnostics_pair
        } else {
            fail(b"role names no declared route");
        };
        match descriptor.rights_mask {
            RIGHT_SEND => pair.data = Some(slot),
            RIGHT_RECV => pair.credit = Some(slot),
            _ => fail(b"publisher role carries more than one direction"),
        }
    }
    let (Some(telemetry_slot), Some(telemetry_credit)) =
        (telemetry_pair.data, telemetry_pair.credit)
    else {
        fail(b"a declared telemetry capability never arrived");
    };
    let Some(diagnostics_slot) = diagnostics_pair.data else {
        fail(b"a declared diagnostics capability never arrived");
    };
    // Two routes, distinct capabilities. One slot serving both would mean the
    // fabric had merged the edges.
    if telemetry_slot == diagnostics_slot {
        fail(b"two declared routes arrived as one capability");
    }
    slime_rt::debug_write(b"[fabric-publisher-b] both publish roles received\n");

    // Diagnostics first, so the transcript shows an unrelated route carrying
    // data before, during, and after the telemetry fan-out.
    publish(
        diagnostics_slot,
        &inline_sample(diagnostics_stream::TYPE_TAG, 1, FLAG_LAST).encode(),
    );
    slime_rt::debug_write(b"[fabric-publisher-b] diagnostics sample published\n");

    publish_large(telemetry_slot, telemetry_credit);
    slime_rt::debug_write(b"[fabric-publisher-b] large sample published\n");
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
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
    slime_rt::debug_write(b"[fabric-publisher-b] done\n");
}
fn visibility_main() {
    let diagnostics = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if request_roles() != ERR_SUCCESS {
        fail(b"visibility request");
    }
    let (descriptor, slot) = receive_role();
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
    publish(
        slot,
        &inline_sample(diagnostics_stream::TYPE_TAG, 1, FLAG_LAST).encode(),
    );
    slime_rt::debug_write(b"[fabric-publisher-b] unrelated diagnostics published\n");
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
    let loan = match slime_rt::shared_buffer_loan(buffer.slot, FABRIC_SLOT, 0, LOAN_LEN) {
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
    loop {
        match slime_rt::send(route_slot, &descriptor.encode(), &[loan.slot]) {
            ERR_SUCCESS => break,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(route_slot)]),
            _ => fail(b"publish descriptor"),
        }
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
        let length = match slime_rt::recv(credit_slot, &mut credit, &mut no_caps) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(credit_slot)]);
                continue;
            }
            n if n < 0 => fail(b"await fabric credit"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"credit is not one control message");
        }
        let Some(event) = WireStreamEvent::decode(&credit) else {
            fail(b"decode credit")
        };
        if !valid_stream_event(&event, telemetry_stream::TYPE_TAG) {
            fail(b"credit failed validation");
        }
        if event.event != EVENT_SAMPLE_TAKEN || event.sequence != descriptor.sequence {
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
    loop {
        match slime_rt::send(TIME_SLOT, &message, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(TIME_SLOT)]),
            _ => fail(b"time advance"),
        }
    }
}

fn await_time_credit(now_ns: u64) {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        let length = match slime_rt::recv(TIME_SLOT, &mut message, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(TIME_SLOT)]);
                continue;
            }
            n if n < 0 => fail(b"time credit"),
            n => n as usize,
        };
        let Some(value) = WireTimeAdvance::decode(&message[..length]) else {
            fail(b"time credit decode")
        };
        if !slime_proto::valid_time_advance(&value) || value.now_ns != now_ns {
            fail(b"time credit mismatch")
        }
        return;
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
    loop {
        match slime_rt::send(CONTROL_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            result => return result,
        }
    }
}

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
const _: () = assert!(slime_proto::sample_descriptor::DESCRIPTOR_LEN == MAX_MSG);
