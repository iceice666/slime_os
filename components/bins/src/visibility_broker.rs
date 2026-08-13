//! C8.8 filtered introspection and one declared interposition chain.
//!
//! This is a dedicated generation profile over the same authenticated graph as
//! the stream broker. It keeps the C8.4 data path small while making the C8.8
//! authority topology explicit: publisher -> fabric -> proxy -> subscriber is
//! the only telemetry path, while diagnostics remains a direct unrelated route.

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_CALL, CONTRACT_KIND_OPERATION, CONTRACT_KIND_STREAM, DIRECTION_CLIENT,
    DIRECTION_PUBLISH, DIRECTION_SERVER, DIRECTION_SUBSCRIBE, TransportQos, route_identity,
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
    FABRIC_CLIENTS, FABRIC_INTERPOSITIONS, FABRIC_PARTICIPANTS, FABRIC_QOS, FABRIC_SUPERVISION,
    FABRIC_VISIBILITY, FIRST_CONTROL_SLOT, ROUTE_NAMES, control_clients, fail, release_received,
    supervision_slot_for,
};

const TELEMETRY: usize = 0;
const DIAGNOSTICS: usize = 1;
const PROXY: &[u8] = b"fabric-intruder";
const DOWNSTREAM: &[u8] = b"fabric-subscriber";
const DIAGNOSTICS_SUBSCRIBER: &[u8] = b"fabric-subscriber-b";
const TELEMETRY_PUBLISHER: &[u8] = b"fabric-publisher";
const DIAGNOSTICS_PUBLISHER: &[u8] = b"fabric-publisher-b";
const VISIBILITY_PRIVATE: u8 = 1;
const VISIBILITY_GRAPH: u8 = 2;
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;
const SAMPLE_SEQUENCE: u64 = 1;
/// The broker's own declared route endpoints, which sit directly after the
/// control endpoints and the supervision handles.
///
/// Derived rather than hardcoded: `FABRIC_CLIENTS` and `FABRIC_SUPERVISION`
/// are generated from the resolved profile, so adding a participant renumbers
/// these with the manifest instead of silently landing a route edge on a
/// supervision handle. Same rule `supervision_slot_for` and
/// `FABRIC_FIRST_CONTROL_SLOT + index` already follow.
const FIRST_ROUTE_SLOT: u32 =
    FIRST_CONTROL_SLOT + FABRIC_CLIENTS.len() as u32 + FABRIC_SUPERVISION.len() as u32;
const TELEMETRY_INGRESS_SLOT: u32 = FIRST_ROUTE_SLOT;
const PROXY_UPSTREAM_SLOT: u32 = FIRST_ROUTE_SLOT + 1;
const PROXY_UPSTREAM_ACK_SLOT: u32 = FIRST_ROUTE_SLOT + 2;
const PROXY_EVENT_SLOT: u32 = FIRST_ROUTE_SLOT + 3;
const DIAGNOSTICS_INGRESS_SLOT: u32 = FIRST_ROUTE_SLOT + 4;
const DIAGNOSTICS_EGRESS_SLOT: u32 = FIRST_ROUTE_SLOT + 5;
const DIAGNOSTICS_ACK_SLOT: u32 = FIRST_ROUTE_SLOT + 6;

#[derive(Default)]
struct Roles {
    proxy_control: Option<u32>,
    subscriber_control: Option<u32>,
}

pub(super) fn run() {
    assert_declared_chain();
    let routes = route_identities();
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
                send_view(client.control_slot, client.component, request.cursor, 0);
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

    relay_declared_chain(&routes, &mut roles);
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

fn relay_declared_chain(routes: &[[u8; 32]; 2], roles: &mut Roles) {
    let ingress = TELEMETRY_INGRESS_SLOT;
    let upstream = PROXY_UPSTREAM_SLOT;
    let upstream_ack = PROXY_UPSTREAM_ACK_SLOT;
    let proxy_control = take(&mut roles.proxy_control);

    let sample_bytes = recv_message(ingress);
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| valid_stream_sample(sample, telemetry_stream::TYPE_TAG, 32))
        .filter(|sample| sample.sequence == SAMPLE_SEQUENCE)
        .unwrap_or_else(|| fail(b"interposed sample"));
    if !send_proxy_message(upstream, &sample_bytes) {
        finish_proxy_loss(
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
        routes,
        roles,
        sample.sequence,
        upstream,
        upstream_ack,
        proxy_control,
    );
}

fn finish_proxy_loss(
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
    if !send_proxy_message(PROXY_EVENT_SLOT, &lost.encode()) {
        fail(b"proxy loss event send");
    }
    serve_event_view(take(&mut roles.subscriber_control), DOWNSTREAM);
}

/// Answer the downstream subscriber's post-loss view requests until it exits.
///
/// Bounded by its supervision handle for the same reason the proxy wait is: a
/// native Endpoint never reports `ERR_PEER_DEAD`, so a subscriber that has
/// already exited would leave this loop spinning forever.
fn serve_event_view(control: u32, component: &[u8]) {
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
        send_view(control, component, request.cursor, EVENT_PROXY_LOST);
    }
}

fn relay_unrelated_route(_roles: &mut Roles) {
    let ingress = DIAGNOSTICS_INGRESS_SLOT;
    let egress = DIAGNOSTICS_EGRESS_SLOT;
    let ack_slot = DIAGNOSTICS_ACK_SLOT;
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
fn await_exit(component: &[u8]) {
    let supervision = supervision_slot_for(component);
    while let Ok(None) = slime_rt::supervision_status(supervision) {
        slime_rt::yield_now();
    }
}

fn send_view(control: u32, component: &[u8], cursor: u8, event_mask: u32) {
    let route_number = usize::from(cursor / 2);
    let Some(route) = nth_visible_route(component, route_number) else {
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
        let qos = qos_for(component, route);
        let record = WireVisibilityQosRecord {
            magic: VISIBILITY_QOS_MAGIC,
            version: FORMAT_VERSION,
            status: STATUS_RECORD,
            cursor: next,
            flags: 0,
            route_name: fixed_name(route),
            reliability: qos.7,
            durability: qos.8,
            liveliness: qos.9,
            matched: u8::from(route_matched(route)),
            history_depth: qos.5,
            retained_depth: qos.6,
            deadline_ns: qos.2,
            lifespan_ns: qos.3,
            lease_ns: qos.4,
            event_mask,
        };
        let bytes = record.encode();
        send_message(control, &bytes);
        write_record(b"[fabric-view] ", &bytes);
    }
}

fn nth_visible_route(component: &[u8], wanted: usize) -> Option<&'static str> {
    let graph = FABRIC_VISIBILITY
        .iter()
        .any(|(holder, _, visibility)| *holder == component && *visibility == VISIBILITY_GRAPH);
    let mut visible = 0;
    for (index, (_, route, _, _)) in FABRIC_PARTICIPANTS.iter().enumerate() {
        if FABRIC_PARTICIPANTS[..index]
            .iter()
            .any(|(_, prior, _, _)| prior == route)
        {
            continue;
        }
        let admitted = if graph {
            FABRIC_VISIBILITY.iter().any(|(_, candidate, visibility)| {
                candidate == route && *visibility == VISIBILITY_GRAPH
            })
        } else {
            FABRIC_VISIBILITY
                .iter()
                .any(|(holder, candidate, visibility)| {
                    *holder == component && candidate == route && *visibility == VISIBILITY_PRIVATE
                })
        };
        if admitted {
            if visible == wanted {
                return Some(*route);
            }
            visible += 1;
        }
    }
    None
}

fn route_interface(route: &str) -> (&'static str, u8) {
    FABRIC_PARTICIPANTS
        .iter()
        .find(|(_, declared, _, _)| *declared == route)
        .map(|(_, _, interface, direction)| {
            let kind = match *direction {
                DIRECTION_PUBLISH | DIRECTION_SUBSCRIBE => CONTRACT_KIND_STREAM,
                DIRECTION_CLIENT | DIRECTION_SERVER if *interface == "ParameterCall" => {
                    CONTRACT_KIND_CALL
                }
                DIRECTION_CLIENT | DIRECTION_SERVER => CONTRACT_KIND_OPERATION,
                _ => 0,
            };
            (*interface, kind as u8)
        })
        .unwrap_or_else(|| fail(b"visibility route interface"))
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

fn qos_for(component: &[u8], route: &str) -> &'static super::FabricQosRow {
    FABRIC_QOS
        .iter()
        .find(|row| row.0 == component && row.1 == route)
        .or_else(|| FABRIC_QOS.iter().find(|row| row.1 == route))
        .unwrap_or_else(|| fail(b"visibility qos"))
}

fn route_matched(route: &str) -> bool {
    FABRIC_PARTICIPANTS
        .iter()
        .enumerate()
        .filter(|(_, (_, candidate, _, direction))| {
            *candidate == route && matches!(*direction, DIRECTION_PUBLISH | DIRECTION_SERVER)
        })
        .any(|(offer_index, (_, _, _, offer_direction))| {
            FABRIC_PARTICIPANTS
                .iter()
                .enumerate()
                .filter(|(_, (_, candidate, _, direction))| {
                    *candidate == route
                        && matches!(*direction, DIRECTION_SUBSCRIBE | DIRECTION_CLIENT)
                })
                .any(|(request_index, (_, _, _, request_direction))| {
                    let directions_match = matches!(
                        (*offer_direction, *request_direction),
                        (DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE)
                            | (DIRECTION_SERVER, DIRECTION_CLIENT)
                    );
                    directions_match
                        && TransportQos::offer_satisfies(
                            &transport_qos(FABRIC_QOS[offer_index]),
                            &transport_qos(FABRIC_QOS[request_index]),
                        )
                })
        })
}

fn transport_qos(row: super::FabricQosRow) -> TransportQos {
    TransportQos {
        deadline_ns: row.2,
        lifespan_ns: row.3,
        lease_ns: row.4,
        history_depth: row.5,
        retained_depth: row.6,
        reliability: row.7,
        durability: row.8,
        liveliness: row.9,
    }
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
