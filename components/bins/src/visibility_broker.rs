//! C8.8 filtered introspection and one declared interposition chain.
//!
//! This is a dedicated generation profile over the same authenticated graph as
//! the stream broker. It keeps the C8.4 data path small while making the C8.8
//! authority topology explicit: publisher -> fabric -> proxy -> subscriber is
//! the only telemetry path, while diagnostics remains a direct unrelated route.

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_STREAM, DIRECTION_CLIENT, DIRECTION_PUBLISH, DIRECTION_SERVER,
    DIRECTION_SUBSCRIBE, TransportQos, route_identity,
};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION as TRANSFER_VERSION, OBJECT_KIND_ENDPOINT,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{WireStreamAck, WireStreamSample};
use slime_proto::fabric_visibility::{
    EVENT_PROXY_LOST, FORMAT_VERSION, INTERPOSITION_TRACE_MAGIC, RECORD_LEN, STATUS_END,
    STATUS_RECORD, TRACE_PROXY_LOST, TRACE_RELAYED, VISIBILITY_QOS_MAGIC, VISIBILITY_REQUEST_MAGIC,
    VISIBILITY_ROUTE_MAGIC, WireInterpositionTrace, WireVisibilityQosRecord, WireVisibilityRequest,
    WireVisibilityRouteRecord,
};
use slime_proto::interface_schema::{
    diagnostics_stream, navigation_operation, parameter_call, telemetry_stream,
};
use slime_proto::{
    valid_fabric_request, valid_interposition_trace, valid_stream_ack, valid_stream_sample,
    valid_visibility_request,
};
use slime_rt::{ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

use super::{
    FABRIC_INTERPOSITIONS, FIRST_CONTROL_SLOT, ROUTE_NAMES, control_clients, fail,
    release_received, supervision_slot_for,
};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{RIGHT_RECV, RIGHT_SEND};

const TELEMETRY: usize = 0;
const DIAGNOSTICS: usize = 1;
const PROXY: &[u8] = b"fabric-intruder";
const DOWNSTREAM: &[u8] = b"fabric-subscriber";
const DIAGNOSTICS_SUBSCRIBER: &[u8] = b"fabric-subscriber-b";
const TELEMETRY_PUBLISHER: &[u8] = b"fabric-publisher";
const DIAGNOSTICS_PUBLISHER: &[u8] = b"fabric-publisher-b";
const SAMPLE_SEQUENCE: u64 = 1;
/// The broker's own declared route endpoints, each resolved from the root by the
/// name the generation gives that edge.
///
/// These were computed as `FIRST_CONTROL_SLOT + FABRIC_CLIENTS.len() +
/// FABRIC_SUPERVISION.len() + n`, which is a component reconstructing the
/// builder's own numbering rule from generated tables — it has to know that
/// route endpoints sit after the controls, that the supervision handles sit
/// between them, and in what order the profile emitted each. Every one of these
/// slots is an ordinary declared grant with a name, so asking for the name
/// replaces all of that with the one fact the broker actually needs. Verified
/// equal to the derived numbers on this plane before the change, rather than
/// assumed: `telemetry-ingress` is 12 under `sel4-visibility.zti`, which is what
/// `FIRST_ROUTE_SLOT` computed.
///
/// The names are the *route's*, not the manifest's. They were
/// `visibility-telemetry-ingress` and so on, which named the same role
/// `sel4-matrix.zti` spelled `matrix-telemetry-ingress` — one fact under two
/// vocabularies, so a broker could only be read against the fixture it was
/// written for. What a name now denotes is a role in the graph: the telemetry
/// route's ingress, its interposed upstream hop, the diagnostics egress. Both
/// fixtures spell each role identically.
///
/// The role, not the endpoint pair. `telemetry-proxy-upstream` is
/// `fabric-service -> fabric-intruder` here and `fabric-service -> fabric-proxy`
/// under `sel4-matrix.zti`, because the two planes deliberately interpose
/// different components; `diagnostics-egress` likewise reaches a different
/// subscriber. That is the point rather than a gap — the broker needs the hop it
/// relays through, not which component the composition put there, which is
/// exactly what a component built outside this repo could not have known.
fn telemetry_ingress_slot() -> u32 {
    route_slot(b"telemetry-ingress")
}
fn proxy_upstream_slot() -> u32 {
    route_slot(b"telemetry-proxy-upstream")
}
fn proxy_upstream_ack_slot() -> u32 {
    route_slot(b"telemetry-proxy-upstream-ack")
}
fn proxy_event_slot() -> u32 {
    route_slot(b"telemetry-proxy-event")
}
fn diagnostics_ingress_slot() -> u32 {
    route_slot(b"diagnostics-ingress")
}
fn diagnostics_egress_slot() -> u32 {
    route_slot(b"diagnostics-egress")
}
fn diagnostics_ack_slot() -> u32 {
    route_slot(b"diagnostics-ack")
}

/// One declared route edge, by grant name. Absence is a composition defect on
/// this plane rather than something to tolerate: every name above is declared by
/// the only manifest that reaches this broker.
fn route_slot(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"visibility route slot"))
}

#[derive(Default)]
struct Roles {
    proxy_control: Option<u32>,
    subscriber_control: Option<u32>,
}

pub(super) fn run() {
    assert_declared_chain();
    let routes = route_identities();
    let graph = GraphView::read(&routes);
    let mut clients = control_clients();
    let mut roles = Roles::default();

    // Process authenticated controls in manifest order. Each request remains
    // cursor-paged and blocking, so this fixes the transcript order without a
    // poll loop or a graph-sized response queue.
    for client in &mut clients {
        while !client.answered {
            let mut message = [0u8; MAX_MSG];
            let mut received = [0u64; MAX_CAPS_PER_MSG];
            let length = loop {
                match slime_rt::recv(client.control_slot, &mut message, &mut received) {
                    ERR_WOULDBLOCK => slime_rt::yield_now(),
                    ERR_PEER_DEAD => fail(b"visibility client died before provisioning"),
                    n if n < 0 => fail(b"visibility control recv"),
                    n => break n as usize,
                }
            };
            let carried_capability = received.iter().any(|slot| *slot != 0);
            release_received(&received);

            if length == RECORD_LEN
                && u32::from_le_bytes(message[..4].try_into().expect("visibility magic"))
                    == VISIBILITY_REQUEST_MAGIC
            {
                if carried_capability {
                    fail(b"visibility request carried capability");
                }
                let request = WireVisibilityRequest::decode(&message)
                    .filter(valid_visibility_request)
                    .unwrap_or_else(|| fail(b"visibility request"));
                send_view(
                    &graph,
                    client.control_slot,
                    client.component,
                    request.cursor,
                    0,
                );
                continue;
            }

            if carried_capability {
                fail(b"provisioning request carried capability");
            }
            let request = WireFabricRequest::decode(&message[..length.min(MAX_MSG)])
                .filter(valid_fabric_request)
                .unwrap_or_else(|| fail(b"visibility provisioning request"));
            let _ = (request.route_name, request.direction, request.type_identity);
            provision(client.component, client.control_slot, &routes, &mut roles);
            client.answered = true;
        }
    }
    slime_rt::debug_write(b"[fabric] filtered graph views complete\n");

    relay_declared_chain(&graph, &routes, &mut roles);
    relay_unrelated_route(&mut roles);
    slime_rt::debug_write(b"[fabric] visibility plane complete\n");
}

fn assert_declared_chain() {
    let declared = FABRIC_INTERPOSITIONS
        .iter()
        .any(|(component, route, chain)| {
            *component == DOWNSTREAM && *route == ROUTE_NAMES[TELEMETRY] && *chain == [PROXY]
        });
    if !declared {
        fail(b"declared interposition chain missing");
    }
}

/// This plane's declared participant table, read from the root once.
///
/// B70/CP2. `FABRIC_PARTICIPANTS`, `FABRIC_VISIBILITY`, and `FABRIC_QOS` were
/// three parallel tables `components/bins/build.rs` generated by parsing the
/// manifest, and the broker joined them *positionally* -- `FABRIC_QOS[i]`
/// belonged to `FABRIC_PARTICIPANTS[i]`. The graph carries all three facts on
/// one row, so the join is a field access and the correspondence cannot drift.
///
/// Read once at startup rather than per request: `send_view` answers from a
/// dispatch loop, and each `graph_read` stages its reply through this
/// component's single transfer window -- which descriptor-granting also uses.
///
/// # Holder scope
///
/// `route_matched` asks whether a route has *both* an offer and a request, which
/// spans two components and so exceeds self-scope. `sel4-visibility.zti` names
/// this component as the graph's `fabricComponent`, so this read returns the
/// whole participant table. A non-holder would see only its own rows and could
/// not answer that question -- the scope is a precondition, not an incidental.
struct GraphView {
    rows: [slime_components::fabric_self_view::Row;
        slime_components::fabric_self_view::MAX_GRAPH_ROWS],
    row_count: usize,
    route_indices: [u32; 2],
}

impl GraphView {
    fn read(routes: &[[u8; 32]; 2]) -> Self {
        let mut rows = slime_components::fabric_self_view::EMPTY_ROWS;
        // Not an empty table: a read that did not complete would otherwise
        // satisfy every "no such row" test below out of a failed syscall.
        let Ok(row_count) = slime_components::fabric_self_view::rows(&mut rows) else {
            fail(b"visibility graph read did not complete");
        };
        let mut route_indices = [0u32; 2];
        for (route, identity) in routes.iter().enumerate() {
            let Ok(index) = slime_rt::graph_route_index(identity) else {
                fail(b"a declared route name is absent from the graph");
            };
            route_indices[route] = index as u32;
        }
        Self {
            rows,
            row_count,
            route_indices,
        }
    }

    fn declared(&self) -> &[slime_components::fabric_self_view::Row] {
        &self.rows[..self.row_count]
    }

    /// The rows this component declares on `route`, if any.
    fn rows_on(
        &self,
        route: usize,
    ) -> impl Iterator<Item = &slime_components::fabric_self_view::Row> + '_ {
        let wanted = self.route_indices[route];
        self.declared()
            .iter()
            .filter(move |row| row.route_index == wanted)
    }

    /// Whether `component` declares a graph-visible row anywhere.
    fn sees_graph(&self, component: &[u8]) -> bool {
        let identity = component_identity_of(component);
        self.declared().iter().any(|row| {
            row.component_identity == identity
                && row.visibility == boot_contracts::fabric_graph::VISIBILITY_GRAPH
        })
    }
}

fn component_identity_of(component: &[u8]) -> [u8; 32] {
    let Ok(name) = core::str::from_utf8(component) else {
        fail(b"component name is not utf-8");
    };
    boot_contracts::fabric_graph::component_identity(name)
}

fn route_identities() -> [[u8; 32]; 2] {
    [
        route_identity(
            ROUTE_NAMES[TELEMETRY],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[DIAGNOSTICS],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
    ]
}

fn provision(component: &'static [u8], control: u32, routes: &[[u8; 32]; 2], roles: &mut Roles) {
    match component {
        TELEMETRY_PUBLISHER => {
            descriptor(control, RIGHT_SEND, DIRECTION_PUBLISH, &routes[TELEMETRY])
        }
        DOWNSTREAM => {
            roles.subscriber_control = Some(control);
            descriptor(control, RIGHT_RECV, DIRECTION_SUBSCRIBE, &routes[TELEMETRY]);
            descriptor(control, RIGHT_SEND, DIRECTION_SUBSCRIBE, &routes[TELEMETRY]);
            descriptor(control, RIGHT_RECV, DIRECTION_SUBSCRIBE, &routes[TELEMETRY]);
        }
        PROXY => {
            roles.proxy_control = Some(control);
            // Stated before the roles are handed out, not after. The claim is
            // about the *bindings* — the broker holds the upstream half of the
            // proxy's downstream edge and has no direct edge to the subscriber
            // — so it is true the moment provisioning begins, and asserting it
            // first keeps it ordered ahead of the proxy's own validation and
            // of any relay that could otherwise mask a bypass.
            slime_rt::debug_write(b"[fabric] direct interposition bypass absent by binding\n");
            descriptor(control, RIGHT_RECV, DIRECTION_SUBSCRIBE, &routes[TELEMETRY]);
            descriptor(control, RIGHT_SEND, DIRECTION_SUBSCRIBE, &routes[TELEMETRY]);
            descriptor(control, RIGHT_SEND, DIRECTION_PUBLISH, &routes[TELEMETRY]);
            descriptor(control, RIGHT_RECV, DIRECTION_PUBLISH, &routes[TELEMETRY]);
        }
        DIAGNOSTICS_PUBLISHER => {
            descriptor(control, RIGHT_SEND, DIRECTION_PUBLISH, &routes[DIAGNOSTICS]);
        }
        DIAGNOSTICS_SUBSCRIBER => {
            descriptor(
                control,
                RIGHT_RECV,
                DIRECTION_SUBSCRIBE,
                &routes[DIAGNOSTICS],
            );
            descriptor(
                control,
                RIGHT_SEND,
                DIRECTION_SUBSCRIBE,
                &routes[DIAGNOSTICS],
            );
        }
        _ => fail(b"undeclared visibility control client"),
    }
}

fn relay_declared_chain(graph: &GraphView, routes: &[[u8; 32]; 2], roles: &mut Roles) {
    let ingress = telemetry_ingress_slot();
    let upstream = proxy_upstream_slot();
    let upstream_ack = proxy_upstream_ack_slot();
    let proxy_control = take(&mut roles.proxy_control);

    let sample_bytes = recv_message(ingress);
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| valid_stream_sample(sample, telemetry_stream::TYPE_TAG, 32))
        .filter(|sample| sample.sequence == SAMPLE_SEQUENCE)
        .unwrap_or_else(|| fail(b"interposed sample"));
    if !send_proxy_message(upstream, &sample_bytes) {
        finish_proxy_loss(
            graph,
            routes,
            roles,
            sample.sequence,
            upstream,
            upstream_ack,
            proxy_control,
        );
        return;
    }

    let Some(ack_bytes) = recv_proxy_message(upstream_ack) else {
        finish_proxy_loss(
            graph,
            routes,
            roles,
            sample.sequence,
            upstream,
            upstream_ack,
            proxy_control,
        );
        return;
    };
    let valid_ack = WireStreamAck::decode(&ack_bytes)
        .filter(|ack| valid_stream_ack(ack, telemetry_stream::TYPE_TAG))
        .is_some_and(|ack| ack.sequence == sample.sequence);
    if !valid_ack {
        finish_proxy_loss(
            graph,
            routes,
            roles,
            sample.sequence,
            upstream,
            upstream_ack,
            proxy_control,
        );
        return;
    }

    let Some(trace_bytes) = recv_proxy_message(proxy_control) else {
        finish_proxy_loss(
            graph,
            routes,
            roles,
            sample.sequence,
            upstream,
            upstream_ack,
            proxy_control,
        );
        return;
    };
    let Some(trace) = WireInterpositionTrace::decode(&trace_bytes)
        .filter(valid_interposition_trace)
        .filter(|trace| {
            trace.event == TRACE_RELAYED
                && trace.route_identity == routes[TELEMETRY]
                && trace.sequence == sample.sequence
        })
    else {
        finish_proxy_loss(
            graph,
            routes,
            roles,
            sample.sequence,
            upstream,
            upstream_ack,
            proxy_control,
        );
        return;
    };
    write_record(b"[fabric-trace] ", &trace.encode());
    slime_rt::debug_write(b"[fabric] declared proxy relayed telemetry\n");

    // Wait for the proxy to actually be gone, through the supervision handle
    // the generation granted for exactly this.
    //
    // Not by receiving on its control endpoint: a native seL4 Endpoint reports
    // no peer death, so a dead proxy is indistinguishable from a silent one and
    // this loop would never end.
    await_exit(PROXY);
    finish_proxy_loss(
        graph,
        routes,
        roles,
        sample.sequence,
        upstream,
        upstream_ack,
        proxy_control,
    );
}

fn finish_proxy_loss(
    graph: &GraphView,
    routes: &[[u8; 32]; 2],
    roles: &mut Roles,
    sequence: u64,
    upstream: u32,
    upstream_ack: u32,
    proxy_control: u32,
) {
    let _ = (upstream, upstream_ack, proxy_control);
    let lost = WireInterpositionTrace {
        magic: INTERPOSITION_TRACE_MAGIC,
        version: FORMAT_VERSION,
        event: TRACE_PROXY_LOST,
        flags: 0,
        route_identity: routes[TELEMETRY],
        sequence,
        reserved: [0; 16],
    };
    if !valid_interposition_trace(&lost) || EVENT_PROXY_LOST != 1 {
        fail(b"proxy event constants");
    }
    // Recorded before the event is delivered. The proxy's death has already
    // been observed through its supervision handle, and the subscriber logs its
    // own two lines on receiving this event — so emitting afterwards races the
    // two tasks and inverts the causal order the gate reads.
    write_record(b"[fabric-trace] ", &lost.encode());
    slime_rt::debug_write(b"[fabric] proxy death isolated to telemetry\n");
    if !send_proxy_message(proxy_event_slot(), &lost.encode()) {
        fail(b"proxy loss event send");
    }
    serve_event_view(graph, take(&mut roles.subscriber_control), DOWNSTREAM);
}

/// Answer the downstream subscriber's post-loss view requests until it exits.
///
/// Bounded by its supervision handle for the same reason the proxy wait is: a
/// native Endpoint never reports `ERR_PEER_DEAD`, so a subscriber that has
/// already exited would leave this loop spinning forever.
fn serve_event_view(graph: &GraphView, control: u32, component: &'static [u8]) {
    let supervision = supervision_slot_for(component);
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(control, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                if !matches!(slime_rt::supervision_status(supervision), Ok(None)) {
                    return;
                }
                slime_rt::yield_now();
                continue;
            }
            ERR_PEER_DEAD => return,
            n if n < 0 => fail(b"event view recv"),
            n => n as usize,
        };
        let carried_capability = received.iter().any(|slot| *slot != 0);
        release_received(&received);
        let request = (length == RECORD_LEN && !carried_capability)
            .then(|| WireVisibilityRequest::decode(&message))
            .flatten()
            .filter(valid_visibility_request)
            .unwrap_or_else(|| fail(b"event view request"));
        send_view(graph, control, component, request.cursor, EVENT_PROXY_LOST);
    }
}

fn relay_unrelated_route(_roles: &mut Roles) {
    let ingress = diagnostics_ingress_slot();
    let egress = diagnostics_egress_slot();
    let ack_slot = diagnostics_ack_slot();
    let sample_bytes = recv_message(ingress);
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| valid_stream_sample(sample, diagnostics_stream::TYPE_TAG, 32))
        .filter(|sample| sample.sequence == SAMPLE_SEQUENCE)
        .unwrap_or_else(|| fail(b"unrelated diagnostics sample"));
    // Relay only once the publisher has finished. It prints its own line after
    // sending, so relaying immediately would let this hop's downstream markers
    // overtake the marker for the send that caused them.
    await_exit(DIAGNOSTICS_PUBLISHER);
    send_message(egress, &sample_bytes);
    let ack_bytes = recv_message(ack_slot);
    let ack = WireStreamAck::decode(&ack_bytes)
        .filter(|ack| valid_stream_ack(ack, diagnostics_stream::TYPE_TAG))
        .filter(|ack| ack.sequence == sample.sequence)
        .unwrap_or_else(|| fail(b"unrelated diagnostics ack"));
    let _ = ack;
    // The broker's summary comes last, after the subscriber that observed the
    // sample has run to completion. Both ends emit a line about this same round
    // trip, and the ack alone does not order them: the subscriber prints after
    // sending it, so without this wait the two race. Its supervision handle is
    // the one deterministic answer to "has that task finished".
    await_exit(DIAGNOSTICS_SUBSCRIBER);
    slime_rt::debug_write(b"[fabric] unrelated diagnostics route live after proxy death\n");
}

/// Block until `component` has terminated, via the supervision handle the
/// generation granted the fabric for it.
///
/// The only way this model answers "is that task gone". A native seL4 Endpoint
/// reports no peer death, so a dead peer is indistinguishable from a silent one
/// on the endpoint alone. An error reading the handle means the handle itself
/// is gone, which is that same answer.
fn await_exit(component: &'static [u8]) {
    let supervision = supervision_slot_for(component);
    while let Ok(None) = slime_rt::supervision_status(supervision) {
        slime_rt::yield_now();
    }
}

fn send_view(graph: &GraphView, control: u32, component: &[u8], cursor: u8, event_mask: u32) {
    let position = usize::from(cursor / 2);
    let Some(route) = nth_visible_route(graph, component, position) else {
        let end = WireVisibilityRouteRecord {
            magic: VISIBILITY_ROUTE_MAGIC,
            version: FORMAT_VERSION,
            status: STATUS_END,
            cursor,
            contract_kind: 0,
            route_name_len: 0,
            reserved0: [0; 3],
            route_name: [0; 16],
            schema_identity: [0; 32],
            flags: 0,
        };
        let bytes = end.encode();
        send_message(control, &bytes);
        write_record(b"[fabric-view] ", &bytes);
        return;
    };
    let next = cursor.saturating_add(1);
    if cursor.is_multiple_of(2) {
        let (interface, contract_kind) = route_interface(route);
        let record = WireVisibilityRouteRecord {
            magic: VISIBILITY_ROUTE_MAGIC,
            version: FORMAT_VERSION,
            status: STATUS_RECORD,
            cursor: next,
            contract_kind,
            route_name_len: route.len() as u8,
            reserved0: [0; 3],
            route_name: fixed_name(route),
            schema_identity: schema_identity(interface),
            flags: 0,
        };
        let bytes = record.encode();
        send_message(control, &bytes);
        write_record(b"[fabric-view] ", &bytes);
    } else {
        let qos = qos_for(graph, component, route);
        let record = WireVisibilityQosRecord {
            magic: VISIBILITY_QOS_MAGIC,
            version: FORMAT_VERSION,
            status: STATUS_RECORD,
            cursor: next,
            flags: 0,
            route_name: fixed_name(route),
            reliability: qos.reliability,
            durability: qos.durability,
            liveliness: qos.liveliness,
            matched: u8::from(route_matched(graph, route)),
            history_depth: qos.history_depth,
            retained_depth: qos.retained_depth,
            deadline_ns: qos.deadline_ns,
            lifespan_ns: qos.lifespan_ns,
            lease_ns: qos.lease_ns,
            event_mask,
        };
        let bytes = record.encode();
        send_message(control, &bytes);
        write_record(b"[fabric-view] ", &bytes);
    }
}

/// The `wanted`th route `component` may see, in this plane's declared order.
///
/// A component with any graph-visible row sees every route that declares one; a
/// component without sees only the routes it declares privately itself. Both
/// halves now read `visibility` off the participant row.
///
/// Route *names* are not recoverable from the graph -- a route identity is a
/// one-way fold of name, interface, and contract kind -- so the local
/// `ROUTE_NAMES` supplies the name and the graph supplies its visibility.
fn nth_visible_route(graph: &GraphView, component: &[u8], wanted: usize) -> Option<&'static str> {
    let sees_graph = graph.sees_graph(component);
    let identity = component_identity_of(component);
    let mut visible = 0;
    for route in [TELEMETRY, DIAGNOSTICS] {
        let admitted = if sees_graph {
            graph
                .rows_on(route)
                .any(|row| row.visibility == boot_contracts::fabric_graph::VISIBILITY_GRAPH)
        } else {
            graph.rows_on(route).any(|row| {
                row.component_identity == identity
                    && row.visibility == boot_contracts::fabric_graph::VISIBILITY_PRIVATE
            })
        };
        if admitted {
            if visible == wanted {
                return Some(ROUTE_NAMES[route]);
            }
            visible += 1;
        }
    }
    None
}

/// This plane's local index for `route`, whose name came from `ROUTE_NAMES`.
fn route_number(route: &str) -> usize {
    match route {
        name if name == ROUTE_NAMES[TELEMETRY] => TELEMETRY,
        name if name == ROUTE_NAMES[DIAGNOSTICS] => DIAGNOSTICS,
        _ => fail(b"visibility route name"),
    }
}

/// The interface and contract kind of one of this plane's two routes.
///
/// Both are streams, and `route_identities` already folds them as such -- the
/// identity a route resolves by *is* the fold of this pair, so a mismatch here
/// would make the route unresolvable rather than mislabel it.
fn route_interface(route: &str) -> (&'static str, u8) {
    match route_number(route) {
        TELEMETRY => ("TelemetryStream", CONTRACT_KIND_STREAM as u8),
        _ => ("DiagnosticsStream", CONTRACT_KIND_STREAM as u8),
    }
}

fn schema_identity(interface: &str) -> [u8; 32] {
    match interface {
        "TelemetryStream" => telemetry_stream::INTERFACE_IDENTITY,
        "DiagnosticsStream" => diagnostics_stream::INTERFACE_IDENTITY,
        "ParameterCall" => parameter_call::INTERFACE_IDENTITY,
        "NavigationOperation" => navigation_operation::INTERFACE_IDENTITY,
        _ => fail(b"visibility schema identity"),
    }
}

/// The QoS `component` declares on `route`, or the route's own if it declares
/// none -- the same fallback the generated table's lookup had.
fn qos_for(graph: &GraphView, component: &[u8], route: &str) -> TransportQos {
    let number = route_number(route);
    let identity = component_identity_of(component);
    graph
        .rows_on(number)
        .find(|row| row.component_identity == identity)
        .or_else(|| graph.rows_on(number).next())
        .map(|row| row.qos)
        .unwrap_or_else(|| fail(b"visibility qos"))
}

/// Whether `route` has at least one offer whose QoS satisfies a request of the
/// matching direction.
///
/// The offer/request join was positional -- `FABRIC_QOS[offer_index]` paired
/// with `FABRIC_PARTICIPANTS[offer_index]` -- which held only because
/// `build.rs` emitted both tables in one pass. Here direction and QoS are two
/// fields of one row, so there is no correspondence left to break.
///
/// Admission already refuses a generation in which *any* matched pair is
/// incompatible (`all_pairs_qos_compatible`), so on a plane that booted the
/// QoS half cannot fail. The existential half still can: a route that declares
/// only offers, or only requests, is admissible and matches nothing.
fn route_matched(graph: &GraphView, route: &str) -> bool {
    let number = route_number(route);
    graph
        .rows_on(number)
        .filter(|row| matches!(row.direction, DIRECTION_PUBLISH | DIRECTION_SERVER))
        .any(|offer| {
            graph
                .rows_on(number)
                .filter(|row| matches!(row.direction, DIRECTION_SUBSCRIBE | DIRECTION_CLIENT))
                .any(|request| {
                    matches!(
                        (offer.direction, request.direction),
                        (DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE)
                            | (DIRECTION_SERVER, DIRECTION_CLIENT)
                    ) && TransportQos::offer_satisfies(&offer.qos, &request.qos)
                })
        })
}

fn fixed_name(name: &str) -> [u8; 16] {
    if name.len() > 16 {
        fail(b"visibility route name bound");
    }
    let mut bytes = [0; 16];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    bytes
}

fn descriptor(control: u32, rights: u64, direction: u32, route: &[u8; 32]) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: TRANSFER_VERSION,
        status: 0,
        flags: 0,
        object_kind: OBJECT_KIND_ENDPOINT,
        direction,
        rights_mask: rights,
        route_identity: *route,
    };
    send_message(control, &descriptor.encode());
}

fn send_proxy_message(slot: u32, message: &[u8; MAX_MSG]) -> bool {
    match slime_rt::send(slot, message, &[]) {
        ERR_SUCCESS => true,
        ERR_PEER_DEAD => false,
        _ => false,
    }
}

fn recv_proxy_message(slot: u32) -> Option<[u8; MAX_MSG]> {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => return None,
            n if n < 0 || n as usize != MAX_MSG => return None,
            _ => {
                if received.iter().any(|slot| *slot != 0) {
                    release_received(&received);
                    return None;
                }
                return Some(message);
            }
        }
    }
}

fn send_message(slot: u32, message: &[u8; MAX_MSG]) {
    if slime_rt::send(slot, message, &[]) != ERR_SUCCESS {
        fail(b"visibility send");
    }
}

fn recv_message(slot: u32) -> [u8; MAX_MSG] {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility recv"),
            n if n as usize != MAX_MSG => fail(b"visibility message length"),
            _ => {
                if received.iter().any(|slot| *slot != 0) {
                    release_received(&received);
                    fail(b"visibility message carried capability");
                }
                return message;
            }
        }
    }
}

fn take(slot: &mut Option<u32>) -> u32 {
    slot.take()
        .unwrap_or_else(|| fail(b"visibility role missing"))
}

fn write_record(prefix: &[u8], bytes: &[u8; RECORD_LEN]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = [0u8; RECORD_LEN * 2];
    for (index, byte) in bytes.iter().enumerate() {
        encoded[index * 2] = HEX[(byte >> 4) as usize];
        encoded[index * 2 + 1] = HEX[(byte & 0x0f) as usize];
    }
    slime_rt::debug_write(prefix);
    slime_rt::debug_write(&encoded);
    slime_rt::debug_write(b"\n");
}

const _: () = assert!(RECORD_LEN == MAX_MSG);
const _: () = assert!(FIRST_CONTROL_SLOT == 2);
