//! C8.12 integrated matching, visibility, and denial matrix.
//!
//! Everything C8.3–C8.8 prove one property at a time, this plane proves at
//! once against one graph, with the cases that can only be told apart when
//! they run together:
//!
//! * **Matching is exact.** Three routes, two of them the *same interface
//!   under different names* (`telemetry`, `telemetry-alt`). Route authority is
//!   the fold of (name, full interface identity, contract kind), so the two
//!   never alias — asserted here at provisioning time rather than only in the
//!   builder, because a broker that matched on name or on type alone would
//!   still resolve a graph the builder accepted.
//! * **A mismatch is a non-match, not an error.** A caller supplying a route
//!   name it holds no edge for, or the right name under the wrong type tag,
//!   receives a denial with no rights, no capability, and no route identity —
//!   the same answer the ungranted probe gets, which is what makes the answer
//!   graph-independent.
//! * **Visibility is not authority.** The observer holds a private view of one
//!   route and no participant edge on the two telemetry routes. It pages its
//!   whole view and still ends holding only the control endpoint it started
//!   with.
//! * **Interposition is the only path.** The telemetry subscriber's chain
//!   names `fabric-proxy`, and the broker holds the upstream half of the
//!   proxy's downstream edge and no direct edge to the subscriber. Stated by
//!   binding, before any role is handed out.
//!
//! The four otherwise-unemitted C8.11 trace families land here, because these
//! are the events that produce them: `KIND_SCHEMA` for each admitted interface,
//! `KIND_VISIBILITY` for each filtered view answered, `KIND_INTERPOSITION` for
//! each declared hop traversed, and `KIND_DENIAL` for each refusal.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, route_identity};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION as TRANSFER_VERSION, OBJECT_KIND_ENDPOINT,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_trace::{
    GRAPH_HOP_TRAVERSED, GRAPH_VIEW_ANSWERED, KIND_DENIAL, KIND_INTERPOSITION, KIND_SCHEMA,
    KIND_VISIBILITY, ORDER_DATA,
};
use slime_proto::fabric_visibility::{
    FORMAT_VERSION, RECORD_LEN, VISIBILITY_REQUEST_MAGIC, WireVisibilityRequest,
    WireVisibilityRouteRecord,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::{valid_fabric_request, valid_visibility_request};
use slime_rt::{ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

use super::trace_log::{self, Trace};
use super::{FABRIC_INTERPOSITIONS, FABRIC_TRACE_DEPTH, control_clients, fail, release_received};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{RIGHT_RECV, RIGHT_SEND};

/// The declared proxy on the telemetry subscriber's interposition chain.
const PROXY: &[u8] = b"fabric-proxy";
/// The subscriber whose chain names the proxy.
const DOWNSTREAM: &[u8] = b"fabric-subscriber";
/// The component the graph declares no participant edge for at all.
const PROBE: &[u8] = b"fabric-probe";
/// The read-only introspection client.
const OBSERVER: &[u8] = b"fabric-observer";

/// Refusal codes. Distinct so the transcript shows *which* mismatch refused an
/// edge: a caller the graph declares nothing for, a caller asking under a route
/// name it holds no edge on, and a caller asking under the wrong type tag are
/// three different defects and must not read as one.
const STATUS_NOT_GRANTED: i32 = -1;
const STATUS_NAME_MISMATCH: i32 = -3;
const STATUS_TYPE_MISMATCH: i32 = -4;
const STATUS_BAD_REQUEST: i32 = -2;

pub(super) fn run() {
    resolve_route_slots();
    assert_declared_chain();
    let routes = route_identities();
    let graph = GraphView::read(&routes);
    assert_declared_composition(&graph);
    let mut trace = Trace::new(FABRIC_TRACE_DEPTH);

    // Schema admission first, before any edge exists to name. The records are
    // the generation's own closure, so they precede every route record in the
    // artifact for the same reason the schemas precede the routes in the graph.
    //
    // One per row of the generated table, not a hardcoded count: a generation
    // admitting a third interface must emit a third record, and a count stated
    // here would undercount it silently — neither the validator nor the gate
    // knows how many schemas the graph declared.
    for _schema in super::FABRIC_SCHEMAS {
        let _ = trace.edge(KIND_SCHEMA, ORDER_DATA, 0, 0, 0, 0);
    }
    slime_rt::debug_write(b"[fabric] matrix admitted ");
    write_count(super::FABRIC_SCHEMAS.len());
    slime_rt::debug_write(b" interface schemas\n");

    // The two telemetry routes carry the same interface under different names.
    // Distinct identities is what makes them distinct authority, so it is
    // asserted here rather than assumed from the builder having resolved both.
    if routes[TELEMETRY] == routes[TELEMETRY_ALT] {
        fail(b"alternate route names aliased to one identity");
    }
    // One `KIND_ROUTE` record per declared route: route provisioning names its
    // edge and reports no outcome, the same shape `fabric-service` emits once
    // per route it carries. Emitted here, once, for the same reason schema
    // admission precedes matching — this is the graph's own closure, not an
    // event on any one caller's request.
    for route in routes {
        let _ = trace.edge(
            slime_proto::fabric_trace::KIND_ROUTE,
            ORDER_DATA,
            trace_log::route_word(&route),
            0,
            0,
            0,
        );
    }
    slime_rt::debug_write(b"[fabric] alternate names hold distinct route identities\n");

    // Stated before any role is handed out, not after. The claim is about the
    // *bindings*: the broker holds the upstream half of the proxy's downstream
    // edge and has no direct edge to the subscriber, so it is true the moment
    // provisioning begins and cannot be masked by a relay that succeeds.
    slime_rt::debug_write(b"[fabric] direct interposition bypass absent by binding\n");

    // One dispatch loop over every source: each control endpoint, and the
    // telemetry ingress the declared chain relays from.
    //
    // Not two phases, because the phases are not separable. The subscriber and
    // the proxy do not exit until the relay has carried a sample to them, and a
    // caller has not finished asking until it exits — so a sweep that waited
    // for every caller to settle *before* relaying would wait on two callers
    // the relay itself has to release.
    //
    // Every source is polled through its non-blocking ABI and the loop parks
    // only when none progressed, so a caller that is merely slow costs a yield
    // rather than blocking the sources that are ready.
    let mut clients = control_clients();
    let mut relay = Relay::Waiting;
    // Whether the proxy's control request has been *replied to*, tracked
    // separately from `Client::answered`: that field means "settled — will
    // never ask again", which the proxy does not reach until after the relay
    // hands it a sample and it exits. Gating the relay on `answered` would
    // make the relay wait for the proxy to settle and the proxy wait for the
    // relay to run — the exact mutual wait this flag exists to avoid.
    let mut proxy_replied = false;
    // Roles actually granted this run — the resource high-water this plane
    // reports at close. Tracked live rather than counted off the graph rows:
    // the graph also declares the diagnostics edges this plane carries for
    // visibility filtering but never provisions, so counting declarations would
    // report roles nothing here ever handed out.
    let mut granted = 0u32;
    loop {
        let mut progressed = false;
        for client in clients.iter_mut().filter(|client| !client.answered) {
            match serve(
                client.control_slot,
                client.component,
                &routes,
                &graph,
                &mut trace,
                &mut granted,
            ) {
                Served::Answered => {
                    progressed = true;
                    if client.component == PROXY {
                        proxy_replied = true;
                    }
                }
                // A caller settles when it exits, which its supervision handle
                // reports. That is the only answer this model has to "will it
                // ask again": a native Endpoint that is merely quiet is
                // indistinguishable from one whose holder is gone.
                //
                // `settled` and the poll above are two separate observations,
                // not one atomic check: a request sent between them would be
                // discarded by a client this loop then never polls again. A
                // client that exits without a final handshake — the probe is
                // exactly this shape — can land in that window. Draining once
                // more after termination is observed closes it: retiring only
                // happens when that drain is also empty.
                Served::Idle => {
                    if settled(client.component) {
                        match serve(
                            client.control_slot,
                            client.component,
                            &routes,
                            &graph,
                            &mut trace,
                            &mut granted,
                        ) {
                            Served::Answered => {
                                progressed = true;
                                if client.component == PROXY {
                                    proxy_replied = true;
                                }
                            }
                            _ => client.answered = true,
                        }
                    }
                }
                Served::Gone => {
                    client.answered = true;
                    progressed = true;
                }
            }
        }
        progressed |= advance_relay(&mut relay, proxy_replied, &routes, &mut trace);
        if clients.iter().all(|client| client.answered) && relay == Relay::Done {
            break;
        }
        if !progressed {
            slime_rt::yield_now();
        }
    }
    slime_rt::debug_write(b"[fabric] matrix matching complete\n");
    // Resource evidence, then the terminal, then the flush — the close every
    // other trace-emitting worker performs, and required rather than tidy: the
    // contract reserves two sink slots for the terminal specifically so a reader
    // can tell a complete trace from a truncated one, and a trace without it is
    // by that definition truncated.
    //
    // The counter is exactly the exact-tuple matches this run granted, tracked
    // live as `provision` hands them out: the graph also declares the
    // diagnostics edges carried for visibility filtering that this plane never
    // provisions, so counting declarations instead would report roles nothing
    // here ever handed out.
    let _ = trace.resource(slime_proto::fabric_trace::RESOURCE_ROLES, granted);
    let _ = trace.terminal();
    trace.flush(b"matrix");
    slime_rt::debug_write(b"[fabric] matrix plane complete\n");
}

/// This plane's routes, in the order every table here indexes them.
///
/// Its own rather than `fabric-service`'s `ROUTE_NAMES`, which is the stream
/// plane's two. The matrix declares three, two of them the same interface under
/// different names — and that third name is the whole point of the plane, so it
/// cannot be borrowed from a table that does not have it. Checked against the
/// generated participant table at startup, so a fixture that renamed a route
/// fails here rather than silently matching nothing.
const ROUTE_NAMES: [&str; ROUTE_COUNT] = ["telemetry", "telemetry-alt", "diagnostics"];
const TELEMETRY: usize = 0;
const TELEMETRY_ALT: usize = 1;
const DIAGNOSTICS: usize = 2;
const ROUTE_COUNT: usize = 3;

fn route_identities() -> [[u8; 32]; ROUTE_COUNT] {
    [
        route_identity(
            ROUTE_NAMES[TELEMETRY],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[TELEMETRY_ALT],
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

/// The graph rows this broker reads, resolved once at startup (B70/CP2).
///
/// Read once and threaded through `serve` rather than re-read per request: this
/// broker answers from a dispatch loop, and a `graph_read` per sweep is a
/// syscall per sweep. It also stages its reply through this component's single
/// transfer window, which `descriptor` uses to hand out role capabilities — the
/// same contention `fabric-service` documents.
///
/// The three route names resolve to indices through `graph_route_index`, which
/// folds the interface into the identity: `telemetry` under a different
/// contract resolves to no index at all rather than matching by string. An
/// absent index is fatal here, not an empty result, because the negative
/// assertions below would otherwise be satisfied by an unresolved route rather
/// than by the graph.
struct GraphView {
    rows: [slime_components::fabric_self_view::Row;
        slime_components::fabric_self_view::MAX_GRAPH_ROWS],
    row_count: usize,
    route_indices: [u32; ROUTE_COUNT],
}

impl GraphView {
    fn read(routes: &[[u8; 32]; ROUTE_COUNT]) -> Self {
        let mut rows = slime_components::fabric_self_view::EMPTY_ROWS;
        let Ok(row_count) = slime_components::fabric_self_view::rows(&mut rows) else {
            fail(b"matrix graph read did not complete");
        };
        let mut route_indices = [0u32; ROUTE_COUNT];
        for route in 0..ROUTE_COUNT {
            let Ok(index) = slime_rt::graph_route_index(&routes[route]) else {
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

    /// Whether the graph declares any edge for `component` on a route this
    /// broker carries. Zero is a denial: authority is never ambient, so absence
    /// from the participant table is not a default role.
    fn declared_edges(&self, component: &[u8]) -> usize {
        let identity = component_identity_of(component);
        self.declared()
            .iter()
            .filter(|row| {
                row.component_identity == identity && self.route_indices.contains(&row.route_index)
            })
            .count()
    }

    fn declares_edge_on(&self, component: &[u8], route: usize) -> bool {
        let identity = component_identity_of(component);
        self.declared().iter().any(|row| {
            row.component_identity == identity && row.route_index == self.route_indices[route]
        })
    }

    /// Every declared row on one of this broker's routes.
    ///
    /// Keyed by the route's resolved graph index rather than by its position in
    /// `ROUTE_NAMES`: the graph orders its participant table by grant identity,
    /// and this plane's three routes need not occupy the same positions there.
    fn rows_on(
        &self,
        route: usize,
    ) -> impl Iterator<Item = &slime_components::fabric_self_view::Row> + '_ {
        let wanted = self.route_indices[route];
        self.declared()
            .iter()
            .filter(move |row| row.route_index == wanted)
    }

    /// Whether the generation grants `component` graph-wide visibility anywhere.
    ///
    /// Scanned over every declared row rather than only this broker's routes,
    /// because the grant is a property of the holder: one `graph` edge is what
    /// admits it to every route declared `graph`.
    fn sees_graph(&self, component: &[u8]) -> bool {
        let identity = component_identity_of(component);
        self.declared().iter().any(|row| {
            row.component_identity == identity
                && row.visibility == boot_contracts::fabric_graph::VISIBILITY_GRAPH
        })
    }

    /// Whether the generation grants `component` a `private` view of `route`.
    fn sees_private(&self, component: &[u8], route: usize) -> bool {
        let identity = component_identity_of(component);
        self.rows_on(route).any(|row| {
            row.component_identity == identity
                && row.visibility == boot_contracts::fabric_graph::VISIBILITY_PRIVATE
        })
    }

    /// Every direction the graph declares for `component` on `route`.
    fn directions(&self, component: &[u8], route: usize) -> impl Iterator<Item = u32> + '_ {
        let identity = component_identity_of(component);
        let wanted = self.route_indices[route];
        self.declared()
            .iter()
            .filter(move |row| row.component_identity == identity && row.route_index == wanted)
            .map(|row| row.direction)
    }
}

fn component_identity_of(component: &[u8]) -> [u8; 32] {
    let Ok(name) = core::str::from_utf8(component) else {
        fail(b"component name is not utf-8");
    };
    boot_contracts::fabric_graph::component_identity(name)
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

/// The identities the plane's negative arms depend on are present, and each is
/// what the milestone requires it to be.
///
/// Checked at startup rather than inferred from a marker never printing. A
/// probe absent from the graph would satisfy "the probe received nothing"
/// vacuously, and an observer whose view happened to span every route would
/// make "a filtered view shows only your grant" a claim about an unfiltered
/// one. Both are failure modes a transcript cannot distinguish from success.
fn assert_declared_composition(graph: &GraphView) {
    // A positive fact about the graph before any negative one. The probe and
    // observer arms below are both claims that something is *absent*, and an
    // absent row and an unread graph are the same observation from here. The
    // downstream subscriber holds a telemetry edge this plane cannot run
    // without, so requiring it first is what makes "the probe holds none" a
    // statement about the graph rather than about a read that returned nothing.
    if graph.declared_edges(DOWNSTREAM) == 0 {
        fail(b"the graph declares no edge for the downstream subscriber");
    }
    // The probe must hold no participant edge at all: its denial is the plane's
    // authority evidence, and an edge would make the refusal a bug rather than
    // the property.
    if graph.declared_edges(PROBE) != 0 {
        fail(b"the ungranted probe holds a declared edge");
    }
    // And it must hold exactly one capability: its control endpoint. Call,
    // serve, operate, cancel, and retrieve all reach their planes through an
    // endpoint, so an absent endpoint is an absent operation — which is what
    // makes those verbs refused by capability rather than by policy.
    //
    // Asserted here rather than by the probe invoking an empty slot: a raw
    // invocation on a slot holding no capability faults the task in this model
    // (the same constraint `fabric-proxy` documents for its own wrong-right
    // case), so a crash is the only thing probing could observe.
    if declared_capabilities(graph, PROBE) != 1 {
        fail(b"the ungranted probe holds more than its control endpoint");
    }
    slime_rt::debug_write(b"[fabric] matrix probe holds only its control endpoint\n");
    // The observer's view must be a strict subset of the graph. It participates
    // on one route under a `private` grant, so it must see that route and must
    // not see the two telemetry routes it holds nothing on — which is the
    // difference between a filter and a pass-through.
    let private = graph.declared().iter().any(|row| {
        row.component_identity == component_identity_of(OBSERVER)
            && row.visibility == boot_contracts::fabric_graph::VISIBILITY_PRIVATE
    });
    if !private {
        fail(b"the observer holds no private visibility grant");
    }
    if nth_visible_route(graph, OBSERVER, 1).is_some() {
        fail(b"the observer's filtered view spans more than its grant");
    }
    // Every route this broker indexes is one the generation declares —
    // established by `GraphView::read` resolving all three identities through
    // `graph_route_index` before this runs. That is strictly stronger than the
    // name comparison it replaces: the identity folds the interface in, so a
    // fixture that kept the name `telemetry` but moved it to another contract
    // now fails here rather than matching by string, which is the distinction
    // this whole plane exists to draw.
}

/// How many capabilities the generation declares for `component`, across every
/// class this profile can bind: the stream control endpoint, a call- or
/// operation-plane control endpoint, one per declared route edge, and one per
/// notification binding.
///
/// Every class, because this count is the load-bearing half of the claim that
/// an absent capability is an absent operation — asserted once here rather
/// than left to whichever caller cites it to re-derive correctly. A count that
/// only read stream-route edges would agree with a broader graph today by
/// coincidence and disagree the day a fixture bound one more class to the
/// probe.
fn declared_capabilities(graph: &GraphView, component: &[u8]) -> usize {
    let control = usize::from(super::FABRIC_CLIENTS.contains(&component));
    let call = usize::from(super::FABRIC_CALL_CLIENTS.contains(&component));
    let operation = usize::from(super::FABRIC_OPERATION_CLIENTS.contains(&component));
    let notifications = super::FABRIC_NOTIFICATION_BINDINGS
        .iter()
        .filter(|(holder, _, _, _, _)| *holder == component)
        .count();
    control + call + operation + notifications + graph.declared_edges(component)
}

/// One decimal count on the current line.
///
/// A `usize`-wide fixed buffer: the values this plane prints are all bounded
/// generation table lengths, but nothing here narrows the type that receives
/// them, so the buffer is sized for the type rather than for today's inputs.
fn write_count(mut value: usize) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    loop {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    slime_rt::debug_write(&digits[index..]);
}

/// What one poll of a control endpoint found.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Served {
    /// A request arrived and was answered — with a role, a denial, or one page
    /// of a filtered view.
    Answered,
    /// Nothing waiting. Says nothing about whether more is coming: only the
    /// caller's supervision handle answers that.
    Idle,
    /// The endpoint reported its peer gone, so nothing more can arrive.
    Gone,
}

/// Answer one control endpoint if it has a request waiting.
fn serve(
    control: u32,
    component: &'static [u8],
    routes: &[[u8; 32]; ROUTE_COUNT],
    graph: &GraphView,
    trace: &mut Trace,
    granted: &mut u32,
) -> Served {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = match slime_rt::recv(control, &mut message, &mut received) {
        ERR_WOULDBLOCK => return Served::Idle,
        ERR_PEER_DEAD => return Served::Gone,
        n if n < 0 => fail(b"matrix control recv"),
        n => n as usize,
    };
    let carried_capability = received.iter().any(|slot| *slot != 0);
    release_received(&received);
    if carried_capability {
        fail(b"matrix request carried capability");
    }

    if length == RECORD_LEN
        && u32::from_le_bytes(message[..4].try_into().expect("matrix magic"))
            == VISIBILITY_REQUEST_MAGIC
    {
        // A malformed request from a control endpoint is untrusted input, not
        // a broker invariant: `fail` here would let any holder of a channel —
        // including the deliberately ungranted probe — take down every other
        // participant's pending request by sending one bad message. The
        // provisioning arm below already treats malformed input this way;
        // this answers the same way, with the one reply shape a paging cursor
        // already understands as "nothing more here" rather than reusing the
        // capability-transfer format on an exchange that never carries one.
        let Some(request) =
            WireVisibilityRequest::decode(&message).filter(valid_visibility_request)
        else {
            send_message(control, &terminal_view(0).encode());
            return Served::Answered;
        };
        answer_view(control, component, request.cursor, routes, graph, trace);
        return Served::Answered;
    }

    let Some(request) =
        WireFabricRequest::decode(&message[..length.min(MAX_MSG)]).filter(valid_fabric_request)
    else {
        deny(control, STATUS_BAD_REQUEST, trace);
        return Served::Answered;
    };
    provision(control, component, &request, routes, graph, trace, granted);
    Served::Answered
}

/// Whether `component` has terminated, through the supervision handle the
/// generation granted the fabric for it.
///
/// The only answer this model has to "will it ask again". A native seL4
/// Endpoint reports no peer death, so a caller that has finished asking and one
/// that is merely slow look identical on the endpoint alone. An error reading
/// the handle means the handle itself is gone, which is that same answer.
fn settled(component: &'static [u8]) -> bool {
    // Resolved from the root by the supervised task's own name, rather than
    // looked up in a generated table. `supervision_slot_for` refuses a component
    // the generation granted no handle for, which is the same answer this
    // function's own `else` arm gave and for the same reason: no handle is no
    // termination signal at all on this transport, since a native Endpoint never
    // reports `ERR_PEER_DEAD`, so such a component would poll `Idle` forever and
    // the dispatch loop would never see it settle. Every control-plane
    // participant is granted a handle for exactly this reason
    // (`denied_components` in `build-generation.py`'s `resolve_fabric_profile`);
    // a graph that omitted one is a composition defect, refused rather than hung
    // on.
    let supervision = super::supervision_slot_for(component);
    !matches!(slime_rt::supervision_status(supervision), Ok(None))
}

/// Match one provisioning request against the graph, or refuse it.
///
/// Three independent facts must agree, and each disagreement has its own code:
/// the graph declares an edge for this component at all; that edge is on the
/// route the caller named; and the caller's type tag is the one that route's
/// interface folds into its identity. Checking them separately is what makes
/// "alternate name" and "conflicting type" distinguishable rather than one
/// undifferentiated refusal.
fn provision(
    control: u32,
    component: &'static [u8],
    request: &WireFabricRequest,
    routes: &[[u8; 32]; ROUTE_COUNT],
    graph: &GraphView,
    trace: &mut Trace,
    granted: &mut u32,
) {
    let edges = graph.declared_edges(component);
    if edges == 0 {
        slime_rt::debug_write(b"[fabric] matrix denied ungranted: ");
        slime_rt::debug_write(component);
        slime_rt::debug_write(b"\n");
        deny(control, STATUS_NOT_GRANTED, trace);
        return;
    }

    let name_len = (request.route_name_len as usize).min(request.route_name.len());
    let named = &request.route_name[..name_len];
    let Some(route) = ROUTE_NAMES
        .iter()
        .position(|candidate| candidate.as_bytes() == named)
        .filter(|route| graph.declares_edge_on(component, *route))
    else {
        // Either no such route, or one this component holds no edge on. The
        // two are deliberately one answer: distinguishing them would confirm
        // the route's existence to a caller with no authority over it.
        slime_rt::debug_write(b"[fabric] matrix denied name mismatch: ");
        slime_rt::debug_write(component);
        slime_rt::debug_write(b"\n");
        deny(control, STATUS_NAME_MISMATCH, trace);
        return;
    };

    if request.type_identity != route_type_tag(route) {
        // The right name under the wrong type. `telemetry` as a
        // `DiagnosticsStream` is not the `telemetry` route: the identity fold
        // includes the interface, so this is a different edge, not a badly
        // typed request against a known one.
        slime_rt::debug_write(b"[fabric] matrix denied type mismatch: ");
        slime_rt::debug_write(component);
        slime_rt::debug_write(b"\n");
        deny(control, STATUS_TYPE_MISMATCH, trace);
        return;
    }

    // The exact compatible tuple. One narrowed, non-delegable role per
    // direction the graph declares for this component on this route.
    for direction in graph.directions(component, route) {
        let rights = if direction == DIRECTION_PUBLISH {
            RIGHT_SEND
        } else {
            RIGHT_RECV
        };
        descriptor(control, rights, direction, &routes[route]);
        *granted += 1;
    }
    slime_rt::debug_write(b"[fabric] matrix matched exact tuple: ");
    slime_rt::debug_write(component);
    slime_rt::debug_write(b"\n");
}

fn route_type_tag(route: usize) -> u64 {
    match route {
        TELEMETRY | TELEMETRY_ALT => telemetry_stream::TYPE_TAG,
        DIAGNOSTICS => diagnostics_stream::TYPE_TAG,
        _ => fail(b"matrix route index"),
    }
}

/// Answer one page of `component`'s filtered graph view.
///
/// One record per visible route, then a terminal record. The filter is the
/// generation's declared visibility policy and nothing else: a caller holding
/// no grant on a route does not learn it exists, which is why the terminal
/// record's every field is zero rather than carrying a count.
fn answer_view(
    control: u32,
    component: &[u8],
    cursor: u8,
    routes: &[[u8; 32]; ROUTE_COUNT],
    graph: &GraphView,
    trace: &mut Trace,
) {
    let Some(route) = nth_visible_route(graph, component, usize::from(cursor)) else {
        send_message(control, &terminal_view(cursor).encode());
        return;
    };
    let name = ROUTE_NAMES[route];
    let record = WireVisibilityRouteRecord {
        magic: slime_proto::fabric_visibility::VISIBILITY_ROUTE_MAGIC,
        version: FORMAT_VERSION,
        status: slime_proto::fabric_visibility::STATUS_RECORD,
        cursor: cursor.saturating_add(1),
        contract_kind: CONTRACT_KIND_STREAM as u8,
        route_name_len: name.len() as u8,
        reserved0: [0; 3],
        route_name: fixed_name(name),
        schema_identity: match route {
            TELEMETRY | TELEMETRY_ALT => telemetry_stream::INTERFACE_IDENTITY,
            _ => diagnostics_stream::INTERFACE_IDENTITY,
        },
        flags: 0,
    };
    send_message(control, &record.encode());
    // Visibility is graph-shaped evidence: the edge observed, and an event
    // naming that a view was answered over it. It carries no outcome code,
    // because a filtered view has no outcome — it either shows an edge the
    // caller was granted or does not mention it.
    let _ = trace.edge(
        KIND_VISIBILITY,
        ORDER_DATA,
        trace_log::route_word(&routes[route]),
        0,
        0,
        GRAPH_VIEW_ANSWERED,
    );
    slime_rt::debug_write(b"[fabric] matrix view answered: ");
    slime_rt::debug_write(component);
    slime_rt::debug_write(b"\n");
}

fn terminal_view(cursor: u8) -> WireVisibilityRouteRecord {
    WireVisibilityRouteRecord {
        magic: slime_proto::fabric_visibility::VISIBILITY_ROUTE_MAGIC,
        version: FORMAT_VERSION,
        status: slime_proto::fabric_visibility::STATUS_END,
        cursor,
        contract_kind: 0,
        route_name_len: 0,
        reserved0: [0; 3],
        route_name: [0; 16],
        schema_identity: [0; 32],
        flags: 0,
    }
}

/// The `wanted`-th route `component` may see, under the generation's declared
/// visibility policy.
///
/// A holder granted `graph` visibility anywhere sees every route declared
/// `graph`; otherwise it sees only the routes it holds a `private` grant on. A
/// caller granted neither sees nothing — not an empty list of known routes, but
/// no route at all.
///
/// Read from the graph resource rather than the generated visibility table
/// (B70/CP2). The policy is unchanged; its source is now the same participant
/// rows every other decision here reads, which is what lets a component built
/// outside this repo answer the question at all.
fn nth_visible_route(graph: &GraphView, component: &[u8], wanted: usize) -> Option<usize> {
    let sees_graph = graph.sees_graph(component);
    let mut visible = 0;
    for route in [TELEMETRY, TELEMETRY_ALT, DIAGNOSTICS] {
        let admitted = if sees_graph {
            graph
                .rows_on(route)
                .any(|row| row.visibility == boot_contracts::fabric_graph::VISIBILITY_GRAPH)
        } else {
            graph.sees_private(component, route)
        };
        if admitted {
            if visible == wanted {
                return Some(route);
            }
            visible += 1;
        }
    }
    None
}

/// Where the declared interposition chain has got to.
///
/// A state machine rather than a straight-line function, because the relay
/// shares one dispatch loop with the control endpoints: the callers it releases
/// are the same callers the loop is still serving, so it cannot block on either
/// half without stalling the other.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Relay {
    /// No sample has arrived on the telemetry ingress yet.
    Waiting,
    /// The sample was forwarded upstream to the proxy; its acknowledgement has
    /// not come back.
    Forwarded,
    /// The hop completed and was recorded.
    Done,
}

/// Advance the declared chain by whatever is ready, and report whether it moved.
///
/// The broker never sends to the subscriber. It forwards upstream to the proxy
/// and reads the proxy's acknowledgement back; the subscriber's copy arrives
/// over the proxy's own downstream edge, which this component does not hold.
/// That is the interposition property, and it is a fact about the bindings
/// rather than about this code being careful.
fn advance_relay(
    relay: &mut Relay,
    proxy_replied: bool,
    routes: &[[u8; 32]; ROUTE_COUNT],
    trace: &mut Trace,
) -> bool {
    match relay {
        Relay::Waiting => {
            // The upstream send is unsolicited and blocking: the proxy has no
            // reason to be waiting on it until its own control request has
            // been replied to. A proxy still blocked asking for its role
            // cannot receive here, and a broker blocked here cannot answer
            // that request — neither side would ever move. Nothing declares
            // that the proxy is served before the publisher's sample arrives,
            // so this is checked rather than assumed from spawn order.
            if !proxy_replied {
                return false;
            }
            let Some(sample) = poll_message(telemetry_ingress_slot()) else {
                return false;
            };
            if slime_rt::send(proxy_upstream_slot(), &sample, &[]) != ERR_SUCCESS {
                fail(b"declared chain upstream send");
            }
            *relay = Relay::Forwarded;
            true
        }
        Relay::Forwarded => {
            if poll_message(proxy_upstream_ack_slot()).is_none() {
                return false;
            }
            // The hop is graph-shaped evidence: the edge it traversed, and an
            // event naming that it was traversed. No outcome code, because a
            // relayed hop has no status of its own — a failed hop is a fault
            // record, not this.
            let _ = trace.edge(
                KIND_INTERPOSITION,
                ORDER_DATA,
                trace_log::route_word(&routes[TELEMETRY]),
                0,
                0,
                GRAPH_HOP_TRAVERSED,
            );
            slime_rt::debug_write(b"[fabric] matrix relayed telemetry through declared proxy\n");
            *relay = Relay::Done;
            true
        }
        Relay::Done => false,
    }
}

/// One message from `slot`, or `None` if none is waiting.
///
/// A peer that is gone is `None` too: this loop's terminal condition is every
/// caller settled, which the supervision handles answer, so a dead peer here is
/// simply a source that will never be ready again.
fn poll_message(slot: u32) -> Option<[u8; MAX_MSG]> {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    match slime_rt::recv(slot, &mut message, &mut received) {
        ERR_WOULDBLOCK | ERR_PEER_DEAD => None,
        n if n < 0 => fail(b"matrix relay recv"),
        n if n as usize != MAX_MSG => fail(b"matrix relay message length"),
        _ => {
            if received.iter().any(|slot| *slot != 0) {
                release_received(&received);
                fail(b"matrix relay message carried capability");
            }
            Some(message)
        }
    }
}

/// Refuse one request, and record the refusal.
///
/// The reply is the transfer record with a nonzero status, an empty rights
/// mask, a zero object kind and direction, and — the load-bearing part — an
/// all-zero route identity and no capability attached. A denial that echoed the
/// route would confirm the edge exists, which is the protected metadata the
/// refusal is there to withhold.
///
/// The trace record withholds the same things, enforced by the format rather
/// than by this call site: `valid_trace_record` refuses a `KIND_DENIAL` record
/// carrying a route identity, a correlation, an event, or a non-negative
/// status.
fn deny(control: u32, status: i32, trace: &mut Trace) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: TRANSFER_VERSION,
        status,
        flags: 0,
        object_kind: 0,
        direction: 0,
        rights_mask: 0,
        route_identity: [0; 32],
    };
    if slime_rt::send(control, &descriptor.encode(), &[]) != ERR_SUCCESS {
        fail(b"matrix deny reply");
    }
    let _ = trace.edge(KIND_DENIAL, ORDER_DATA, 0, 0, status, 0);
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

fn fixed_name(name: &str) -> [u8; 16] {
    if name.len() > 16 {
        fail(b"matrix route name bound");
    }
    let mut bytes = [0; 16];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    bytes
}

fn send_message(slot: u32, message: &[u8; MAX_MSG]) {
    if slime_rt::send(slot, message, &[]) != ERR_SUCCESS {
        fail(b"matrix send");
    }
}

/// The broker's own declared route endpoints, each resolved from the root by the
/// name the generation gives that edge, for the reason `visibility_broker` gives.
///
/// These were computed from `FABRIC_CLIENTS.len()` and `FABRIC_SUPERVISION.len()`,
/// which made the broker reconstruct the builder's numbering rule — route edges
/// sit above the controls, with the supervision handles in between — out of two
/// generated tables. Each is a declared grant with a name, so the name is the
/// whole answer. Checked equal to the derived numbers before the change:
/// `telemetry-ingress` is 16 under `sel4-matrix.zti`, which is what
/// `FIRST_ROUTE_SLOT` computed.
///
/// The names are the *route's*, not the manifest's — they were
/// `matrix-telemetry-ingress` and so on, naming the same role
/// `sel4-visibility.zti` spelled `visibility-telemetry-ingress`. A name now
/// denotes a role in the graph — the telemetry route's ingress, its interposed
/// upstream hop — spelled identically by both fixtures. The role, not the
/// endpoint pair: this plane's `telemetry-proxy-upstream` reaches
/// `fabric-proxy` where the visibility plane's reaches `fabric-intruder`, since
/// the two interpose different components on purpose.
///
/// `matrix_broker` is only reached under `bootAction = "matrix"`, so these names
/// resolve against `sel4-matrix.zti` alone — and against its
/// `sel4-matrix-unsatisfiable` variant, which B62 reduced to a single
/// participant-QoS override of that same fixture, leaving every grant name
/// identical.
/// Resolved once at startup and cached, following `fabric_call_scenario`'s
/// `WAKE_SLOT`. These are read from `advance_relay`, which is a *poll* arm the
/// dispatch loop re-enters until the sample arrives, so resolving per read would
/// put a syscall in a spin loop that previously touched a constant.
static mut ROUTE_SLOTS: [u32; 3] = [u32::MAX; 3];

const TELEMETRY_INGRESS: usize = 0;
const PROXY_UPSTREAM: usize = 1;
const PROXY_UPSTREAM_ACK: usize = 2;

/// Resolve this broker's declared route edges through the root, once, before the
/// dispatch loop runs.
fn resolve_route_slots() {
    let slots = [
        route_slot(b"telemetry-ingress"),
        route_slot(b"telemetry-proxy-upstream"),
        route_slot(b"telemetry-proxy-upstream-ack"),
    ];
    // SAFETY: single-threaded, and called once before any read below.
    unsafe { *core::ptr::addr_of_mut!(ROUTE_SLOTS) = slots };
}

fn telemetry_ingress_slot() -> u32 {
    route_slot_at(TELEMETRY_INGRESS)
}
fn proxy_upstream_slot() -> u32 {
    route_slot_at(PROXY_UPSTREAM)
}
fn proxy_upstream_ack_slot() -> u32 {
    route_slot_at(PROXY_UPSTREAM_ACK)
}

fn route_slot_at(index: usize) -> u32 {
    // SAFETY: single-threaded, as above.
    let slot = unsafe { core::ptr::addr_of!(ROUTE_SLOTS).read()[index] };
    if slot == u32::MAX {
        fail(b"matrix route slot read before resolve");
    }
    slot
}

/// One declared route edge, by grant name. Absence is a composition defect on
/// this plane, not something to tolerate.
fn route_slot(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"matrix route slot"))
}

const _: () = assert!(RECORD_LEN == MAX_MSG);
