#![no_std]
#![no_main]

//! C8.10 filtered-introspection client: a component granted a *read-only,
//! visibility-filtered* view of the graph and nothing else.
//!
//! It pages the fabric's introspection cursor to exhaustion and asserts the
//! view it receives is exactly the one the generation's `visibility` policy
//! declares for it — no route it was not shown, no QoS record for an edge it
//! does not participate in, no metadata past the declared bound.
//!
//! The load-bearing property is what it *cannot* do. Holding a filtered view is
//! not a path to route authority: the observer never requests a role, and the
//! graph declares no participant edge for it, so it ends its run holding only
//! the control endpoint it started with. Read-only introspection and route
//! participation are separate authorities, and this component is the proof that
//! the first never yields the second.
//!
//! Distinct from [`fabric-probe`] (which asks for a role and is denied) and
//! [`fabric-proxy`] (which relays but sees an empty view): the milestone
//! requires three separate task identities with non-overlapping grants.

use slime_components::fabric_visibility::{ViewPage, request_page};
use slime_proto::interface_schema::telemetry_stream;

slime_rt::entry!(main);

/// Control endpoint to the fabric — this component's only authority, and the
/// only one it ever holds.
const CONTROL_SLOT: u32 = 0;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-observer] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if slime_components::fabric_boot::active() {
        // In the full-graph boot the observer is a declared telemetry
        // subscriber, so it takes its narrowed ring capability and parks. One
        // role: a v2 ring loan carries data and credit in one shared region, so
        // provisioning an edge is one `capability_delegate`, matching every
        // other single-route stream participant (`fabric-publisher`,
        // `fabric-subscriber`). Safe because `boot_graph` never calls `broker`:
        // nothing ever tries to deliver a real sample to this permanently
        // parked subscription. Its filtered *view* is C8.8's property to
        // prove; what this gate needs from it is that it exists as its own
        // task with its own grants.
        slime_components::fabric_boot::provision_and_park(
            b"fabric-observer",
            "telemetry",
            &boot_contracts::fabric_graph::route_identity(
                "telemetry",
                &telemetry_stream::INTERFACE_IDENTITY,
                boot_contracts::fabric_graph::CONTRACT_KIND_STREAM,
            ),
            telemetry_stream::TYPE_TAG,
            boot_contracts::fabric_graph::DIRECTION_SUBSCRIBE,
            1,
        );
    }
    if slime_components::fabric_boot::full_graph_active() {
        // C8.13's concurrent traffic plane calls `broker` for a real relay
        // loop, unlike `boot_graph`. A subscription that never drains would
        // wedge it forever: `deliver` retries a *blocking* send to whichever
        // control endpoint a matched subscriber's role names, with no notion
        // of "this peer will never consume again", so requesting the role at
        // all here would eventually block the broker delivering this
        // component's queued sample and starve every other subscriber behind
        // it. Parking without asking is exactly the declared interposition
        // proxy's treatment, and `traffic_graph` pre-marks this component
        // answered the same way it does the proxy.
        slime_components::fabric_boot::park_only(b"fabric-observer");
    }
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    // Page the whole cursor. The view is bounded by the generation's declared
    // visibility policy, so exhausting it is a finite walk rather than a poll.
    let mut cursor = 0;
    let mut routes = 0;
    let mut qos = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"filtered view")) {
            ViewPage::Route(record) => {
                if routes != 0
                    || &record.route_name[..record.route_name_len as usize] != b"telemetry"
                    || record.schema_identity != telemetry_stream::INTERFACE_IDENTITY
                {
                    fail(b"filtered route metadata");
                }
                routes += 1;
                cursor = record.cursor;
            }
            ViewPage::Qos(record) => {
                if record.route_name[..9] != *b"telemetry" || record.matched != 1 {
                    fail(b"filtered qos metadata");
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
    // Exactly the declared view: one route, one QoS record. More would mean the
    // filter leaked an edge this component does not participate in; fewer would
    // mean the assertion held vacuously.
    if routes != 1 || qos != 1 {
        fail(b"filtered view bound");
    }
    slime_rt::debug_write(b"[fabric-observer] filtered view routes=1\n");
    slime_rt::debug_write(b"[fabric-observer] view granted no route authority\n");
    slime_rt::debug_write(b"[fabric-observer] done\n");
}

/// C8.12: read-only visibility, and the proof it is not a path to authority.
///
/// The matrix graph declares this component a `diagnostics` subscriber under a
/// *private* visibility grant and nothing on either telemetry route. So its
/// filtered view must contain exactly one route — `diagnostics` — and asking
/// for a role on a route it can neither see nor participate in must be refused.
///
/// Both halves are needed. A view showing every route would make the filter a
/// pass-through; a view showing one route while a role request on another
/// succeeded would make the filter cosmetic.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    let mut cursor = 0;
    let mut routes = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"matrix filtered view")) {
            ViewPage::Route(record) => {
                if &record.route_name[..record.route_name_len as usize] != b"diagnostics" {
                    fail(b"filtered view exposed an ungranted route");
                }
                routes += 1;
                cursor = record.cursor;
            }
            ViewPage::Qos(record) => cursor = record.cursor,
            ViewPage::End(_) => break,
        }
    }
    if routes != 1 {
        fail(b"matrix filtered view bound");
    }
    slime_rt::debug_write(b"[fabric-observer] matrix filtered view routes=1\n");

    // Holding a view of one route is not authority over another. Asked under
    // the exact strings a real participant uses, so the refusal is a capability
    // property rather than a parse failure.
    match request_role(
        "telemetry",
        telemetry_stream::TYPE_TAG,
        boot_contracts::fabric_graph::DIRECTION_SUBSCRIBE,
    ) {
        Ok(Outcome::Denied(_)) => {
            slime_rt::debug_write(b"[fabric-observer] matrix view granted no route authority\n");
        }
        Ok(Outcome::Role(_)) => fail(b"a filtered view yielded route authority"),
        Err(_) => fail(b"matrix observer role request"),
    }
    slime_rt::debug_write(b"[fabric-observer] done\n");
}
