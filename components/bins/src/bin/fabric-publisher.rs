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
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
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

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

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

fn main() {
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );

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
