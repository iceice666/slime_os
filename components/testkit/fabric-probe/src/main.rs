#![no_std]
#![no_main]

//! C8.10 unauthorized probe: a component the generation graph declares no edge
//! for, running alongside every authorized plane in one generation.
//!
//! It holds a real, generation-provisioned control endpoint to the fabric — so
//! it is not blocked by lacking a channel — and it supplies the *exact* route
//! name, direction, and type identity the publisher supplies. Everything a
//! naive registry would accept is present.
//!
//! It must still receive nothing. This is the concrete form of "possession of
//! names or generic channel authority cannot mint a graph edge": the fabric
//! authenticates by the control endpoint, and the graph declares no participant
//! for this component, so the request is denied with no capability attached.
//!
//! Distinct from [`fabric-proxy`] and [`fabric-observer`] as its own task with
//! non-overlapping grants: the milestone requires the probe, the declared
//! interposition proxy, and the filtered-introspection client to be three
//! separate identities, so a denial here can never be confused for a role the
//! graph granted somewhere else.

use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

const CONTROL_SLOT: u32 = 0;

/// Byte-for-byte the publisher's ask, including the direction it wants.
const ROUTE_NAME: &str = "telemetry";
const DIRECTION_PUBLISH: u32 = 1;
const DIRECTION_SUBSCRIBE: u32 = 2;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    let mut route_name = [0u8; 32];
    route_name[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_PUBLISH,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    if slime_rt::send(CONTROL_SLOT, &request.encode(), &[]) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-probe] exact route strings supplied\n");

    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"reply"),
            _ => break,
        }
    }
    let Some(descriptor) = WireCapabilityTransfer::decode(&message) else {
        fail(b"decode reply")
    };
    // The denial must be total: a refusal status, an empty rights mask, and —
    // the load-bearing part — no capability in the message at all.
    if descriptor.status == 0 {
        fail(b"ungranted component was authorized");
    }
    if descriptor.rights_mask != 0 {
        fail(b"denial carried a rights mask");
    }
    if received.iter().any(|slot| *slot != 0) {
        fail(b"denial carried a capability");
    }
    slime_rt::debug_write(b"[fabric-probe] undeclared edge denied\n");
    slime_rt::debug_write(b"[fabric-probe] done\n");
    if slime_components::fabric_boot::active() {
        // The denial above is this component's whole assertion, and it holds in
        // the full-graph boot exactly as it does alone: a real control endpoint
        // and the exact route strings still buy nothing. Park rather than exit,
        // because the gate's exit condition is every role blocked — a probe that
        // terminated would be indistinguishable from one that was never
        // launched.
        slime_components::fabric_boot::park(b"fabric-probe");
    }
}

/// C8.12: the ungranted probe against every verb it can reach.
///
/// It holds one real control endpoint to the matrix broker and no participant
/// edge at all, so each attempt is refused on the *graph* rather than for want
/// of a channel. Two denial classes, both graph-independent:
///
/// * **Refused by the broker.** Every request over the control endpoint it does
///   hold — create/discover/publish/subscribe under the exact strings a real
///   participant supplies. `fabric_matrix` checks each refusal carries no
///   rights, no capability, and no route identity.
/// * **Refused by the kernel.** Call, serve, operate, cancel, and retrieve
///   target the call and operation planes, which this component holds no
///   endpoint to. The kernel refuses the unheld slot outright — a denial by
///   construction rather than by broker policy, and one that carries no
///   metadata because there is no broker to answer.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};
    use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};

    // Every route/direction/type tuple this graph declares, asked under the
    // exact strings its real participants use. `discover` and `create` are the
    // same request shape: this protocol has no verb a caller can name that the
    // broker does not answer from the graph.
    let attempts: [(&str, u64, u32); 4] = [
        ("telemetry", telemetry_stream::TYPE_TAG, DIRECTION_PUBLISH),
        ("telemetry", telemetry_stream::TYPE_TAG, DIRECTION_SUBSCRIBE),
        (
            "telemetry-alt",
            telemetry_stream::TYPE_TAG,
            DIRECTION_PUBLISH,
        ),
        (
            "diagnostics",
            diagnostics_stream::TYPE_TAG,
            DIRECTION_SUBSCRIBE,
        ),
    ];
    let mut denied = 0;
    for (route, type_tag, direction) in attempts {
        match request_role(route, type_tag, direction) {
            Ok(Outcome::Denied(_)) => denied += 1,
            Ok(Outcome::Role(_)) => fail(b"ungranted component was authorized"),
            Err(slime_components::fabric_matrix::Error::LeakyDenial) => {
                fail(b"denial carried authority or route metadata")
            }
            Err(_) => fail(b"matrix probe request"),
        }
    }
    if denied != attempts.len() {
        fail(b"not every ungranted attempt was refused");
    }
    slime_rt::debug_write(b"[fabric-probe] matrix refused every declared route\n");

    // The call and operation planes, which this component holds no endpoint to.
    //
    // Not attempted here, and that is the point rather than an omission. A raw
    // invocation on a slot holding no capability is not an error return in this
    // model — seL4 faults the task, exactly as `fabric-proxy` documents for its
    // own wrong-right case — so "the syscall was refused" would be a crash,
    // not evidence.
    //
    // What is observable is that this component's whole authority is the one
    // control endpoint it just spent on four refusals. Call, serve, operate,
    // cancel, and retrieve all reach their planes through an endpoint, and
    // there is no second endpoint here to reach them with. The broker asserts
    // that count against the generation's own tables at startup
    // (`[fabric] matrix probe holds only its control endpoint`), which is the
    // half of the claim a component cannot make about itself: this task can say
    // what it did not receive, only the graph can say what it was never given.
    slime_rt::debug_write(b"[fabric-probe] matrix reached every plane it holds an endpoint to\n");
    slime_rt::debug_write(b"[fabric-probe] done\n");
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
