#!/usr/bin/env python3

"""C8.2 fabric-graph generation admission gate.

Proves the host half of the C8.2 exit condition: one authenticated generation
resource deterministically fixes every native interface, graph edge, direction,
QoS policy, visibility grant, interposition hop, and resource ceiling, and
malformed, unauthorized, or globally impossible graphs fail before launch.

The builder is the boundary under test. Each negative case mutates the real
manifest, rebuilds the resource through `build_fabric_graph`, and requires the
build to fail — so a bad graph never reaches an artifact, let alone a boot. The
determinism arm rebuilds the same graph twice and compares bytes, and the
decode arm re-reads the built object with the same offsets the Rust decoder
uses, so the two sides cannot silently disagree on layout.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import struct
import subprocess

from boot_contracts import (
    FABRIC_CONTRACT_KIND_CALL,
    FABRIC_CONTRACT_KIND_STREAM,
    FABRIC_DIRECTION_PUBLISH,
    FABRIC_DIRECTION_SUBSCRIBE,
    FABRIC_GRAPH_HEADER,
    FABRIC_GRAPH_HEADER_BYTES,
    FABRIC_GRAPH_INTERPOSITION_ENTRY,
    FABRIC_GRAPH_INTERPOSITION_ENTRY_BYTES,
    FABRIC_GRAPH_INTERPOSITION_NONE,
    FABRIC_GRAPH_KERNEL_LOANS,
    FABRIC_GRAPH_KERNEL_MAPPINGS,
    FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
    FABRIC_GRAPH_LIMIT_SAMPLE_BYTES,
    FABRIC_GRAPH_MAGIC,
    FABRIC_GRAPH_PARTICIPANT_ENTRY,
    FABRIC_GRAPH_PARTICIPANT_ENTRY_BYTES,
    FABRIC_GRAPH_ROUTE_ENTRY,
    FABRIC_GRAPH_ROUTE_ENTRY_BYTES,
    FABRIC_GRAPH_SCHEMA_ENTRY,
    FABRIC_GRAPH_SCHEMA_ENTRY_BYTES,
    FABRIC_GRAPH_VERSION,
    MAX_FABRIC_GRAPH_INGRESS_SOURCES,
)
from harness import ROOT, load_script

builder = load_script("build_generation", "build/build-generation.py")


def fail(message: str) -> None:
    raise SystemExit(f"fabric manifest check: {message}")


def rejected(label: str, mutate) -> None:
    """Require the builder to reject a mutated graph through its own check.

    `SystemExit` is `fail()`, the builder's deliberate rejection. Anything else
    — a `struct.error` from packing an out-of-range value, a `KeyError` from a
    missing manifest field — means the value reached the encoder before a rule
    caught it, so the builder is relying on an accident rather than a check.
    That is a gate failure, not a pass.
    """
    graph = copy.deepcopy(GRAPH)
    names = set(COMPONENT_NAMES)
    mutate(graph, names)
    try:
        builder.build_fabric_graph(graph, names, INTERFACES)
    except SystemExit:
        return
    except (struct.error, KeyError, TypeError, ValueError) as error:
        fail(f"{label} was not rejected by a builder check: {type(error).__name__}: {error}")
    fail(f"{label} was accepted")


manifest = builder.load_manifest()
INTERFACES = builder.validate_interface_schemas(manifest["interfaceSchemas"])
GRAPH = manifest["fabricGraph"]
# v5 split `components` into an executable catalogue and the instances built
# from it. A fabric participant is an *instance* -- a route names something
# that runs, not something that could be launched -- so this is the instance
# names, which is what `build_fabric_graph` validates against.
COMPONENT_NAMES = {instance["name"] for instance in manifest["instances"]}
INTERFACE_BY_NAME = {interface.name: interface for interface in INTERFACES}

# --- determinism -------------------------------------------------------------

first = builder.build_fabric_graph(GRAPH, set(COMPONENT_NAMES), INTERFACES)
second = builder.build_fabric_graph(GRAPH, set(COMPONENT_NAMES), INTERFACES)
if first != second:
    fail("identical graph input produced different resource bytes")

# --- structure ---------------------------------------------------------------

header = FABRIC_GRAPH_HEADER.unpack_from(first, 0)
(
    magic,
    version,
    header_size,
    required_flags,
    total_len,
    schema_count,
    route_count,
    participant_count,
    interposition_count,
    reserved,
    fabric_identity,
) = header[:11]
limits = header[11:-2]
trace_depth, trace_overflow = header[-2:]

if magic != FABRIC_GRAPH_MAGIC or version != FABRIC_GRAPH_VERSION:
    fail("built graph does not carry the contract magic/version")
if header_size != FABRIC_GRAPH_HEADER_BYTES or required_flags != 0 or reserved != 0:
    fail("built graph header is not the contract shape")
if total_len != len(first):
    fail("built graph total_len disagrees with its own length")
expected = (
    FABRIC_GRAPH_HEADER_BYTES
    + schema_count * FABRIC_GRAPH_SCHEMA_ENTRY_BYTES
    + route_count * FABRIC_GRAPH_ROUTE_ENTRY_BYTES
    + participant_count * FABRIC_GRAPH_PARTICIPANT_ENTRY_BYTES
    + interposition_count * FABRIC_GRAPH_INTERPOSITION_ENTRY_BYTES
)
if total_len != expected:
    fail("built graph sections do not sum to its declared length")
if fabric_identity != builder.fabric_component_identity(GRAPH["fabricComponent"]):
    fail("built graph names the wrong fabric component")
declared = [GRAPH["limits"][key] for key in builder.FABRIC_LIMIT_KEYS]
if list(limits) != declared:
    fail("built graph limits do not match the manifest")
if limits[1] > MAX_FABRIC_GRAPH_INGRESS_SOURCES:
    fail("built graph declares more ingress sources than the ceiling admits")
# The sink shape is a header field rather than a generated per-plane constant
# (B70), so the encoder's copy is checked against the manifest here for the
# same reason every limit above is.
if trace_depth != GRAPH["traceDepth"]:
    fail("built graph does not carry the manifest's declared trace depth")
if trace_overflow != builder.FABRIC_TRACE_OVERFLOW[GRAPH["traceOverflow"]]:
    fail("built graph does not carry the manifest's declared trace overflow discipline")

# --- tables ------------------------------------------------------------------

cursor = FABRIC_GRAPH_HEADER_BYTES
schemas = []
for _ in range(schema_count):
    schemas.append(FABRIC_GRAPH_SCHEMA_ENTRY.unpack_from(first, cursor))
    cursor += FABRIC_GRAPH_SCHEMA_ENTRY_BYTES
routes = []
for _ in range(route_count):
    routes.append(FABRIC_GRAPH_ROUTE_ENTRY.unpack_from(first, cursor))
    cursor += FABRIC_GRAPH_ROUTE_ENTRY_BYTES
participants = []
for _ in range(participant_count):
    participants.append(FABRIC_GRAPH_PARTICIPANT_ENTRY.unpack_from(first, cursor))
    cursor += FABRIC_GRAPH_PARTICIPANT_ENTRY_BYTES
hops = []
for _ in range(interposition_count):
    hops.append(FABRIC_GRAPH_INTERPOSITION_ENTRY.unpack_from(first, cursor))
    cursor += FABRIC_GRAPH_INTERPOSITION_ENTRY_BYTES
if cursor != len(first):
    fail("built graph has trailing bytes past its tables")

identities = [entry[0] for entry in schemas]
if identities != sorted(identities) or len(set(identities)) != len(identities):
    fail("schema table is unsorted or has a duplicate identity")
tags = [entry[1] for entry in schemas]
if len(set(tags)) != len(tags):
    fail("distinct schema identities share one generation-local tag")
route_ids = [entry[0] for entry in routes]
if route_ids != sorted(route_ids) or len(set(route_ids)) != len(route_ids):
    fail("route table is unsorted or has a duplicate identity")
grant_ids = [entry[0] for entry in participants]
if grant_ids != sorted(grant_ids) or len(set(grant_ids)) != len(grant_ids):
    fail("participant table is unsorted or has a duplicate grant")
if sum(entry[3] for entry in routes) != participant_count:
    fail("route participant counts do not sum to the participant table")

# Every emitted grant identity is exactly the fold of its authority tuple: the
# route identity (name + full interface identity + contract kind), the
# component identity, and the direction.
for grant, component, route_index, direction, _visibility, head, *_rest in participants:
    route = routes[route_index]
    if grant != builder.fabric_grant_identity(route[0], component, direction):
        fail("a participant grant is not the fold of its authority tuple")
    if head != FABRIC_GRAPH_INTERPOSITION_NONE and head >= interposition_count:
        fail("a participant names an interposition hop outside the table")

# --- distinct authority domains ---------------------------------------------

# Alternate names over one interface, and conflicting interfaces under one
# name, must remain distinct routes; so must the same name at a different
# contract kind.
telemetry = INTERFACE_BY_NAME["TelemetryStream"]
parameters = INTERFACE_BY_NAME["ParameterCall"]
base = builder.fabric_route_identity("telemetry", telemetry.identity, FABRIC_CONTRACT_KIND_STREAM)
variants = {
    base,
    builder.fabric_route_identity("diagnostics", telemetry.identity, FABRIC_CONTRACT_KIND_STREAM),
    builder.fabric_route_identity("telemetry", parameters.identity, FABRIC_CONTRACT_KIND_STREAM),
    builder.fabric_route_identity("telemetry", telemetry.identity, FABRIC_CONTRACT_KIND_CALL),
}
if len(variants) != 4:
    fail("alternate names, types, or kinds collapsed into one route authority")

# The fabric component domain must not be reachable from the C7.3 shared-buffer
# holder domain: one identity may never be replayed into the other's authority.
if builder.fabric_component_identity("dango") == builder.holder_identity("dango"):
    fail("fabric and shared-buffer identity domains are not separated")

# Direction is part of the fold, so one component's two roles on one route are
# two authorities.
component = builder.fabric_component_identity("sample-lender")
if builder.fabric_grant_identity(base, component, FABRIC_DIRECTION_PUBLISH) == (
    builder.fabric_grant_identity(base, component, FABRIC_DIRECTION_SUBSCRIBE)
):
    fail("direction does not participate in grant authority")

# --- negative corpus ---------------------------------------------------------


def unknown_fabric(graph, _names):
    graph["fabricComponent"] = "no-such-component"


def unknown_participant(graph, _names):
    graph["routes"][0]["participants"][0]["component"] = "no-such-component"


def unknown_interface(graph, _names):
    graph["routes"][0]["interface"] = "NoSuchInterface"


def unknown_hop(graph, _names):
    graph["routes"][1]["participants"][0]["interposition"] = ["no-such-component"]


def duplicate_grant(graph, _names):
    route = graph["routes"][0]
    route["participants"].append(copy.deepcopy(route["participants"][0]))


def duplicate_route(graph, _names):
    graph["routes"].append(copy.deepcopy(graph["routes"][0]))


def wrong_direction_for_kind(graph, _names):
    # A stream route cannot host a client.
    graph["routes"][0]["participants"][0]["direction"] = "client"


def unsupported_qos(graph, _names):
    graph["routes"][0]["participants"][0]["reliability"] = "atMostOnce"


def unsupported_visibility(graph, _names):
    graph["routes"][0]["participants"][0]["visibility"] = "public"


def self_interposition(graph, _names):
    route = graph["routes"][1]
    route["participants"][0]["interposition"] = [route["participants"][0]["component"]]


def repeated_interposition(graph, _names):
    graph["routes"][1]["participants"][0]["interposition"] = ["echo-agent", "echo-agent"]


def empty_route(graph, _names):
    graph["routes"][0]["participants"] = []


def over_bound_routes(graph, _names):
    template = graph["routes"][0]
    for index in range(64):
        clone = copy.deepcopy(template)
        clone["name"] = f"telemetry-{index}"
        graph["routes"].append(clone)


def over_bound_hops(graph, names):
    hops = []
    for index in range(32):
        name = f"proxy-{index}"
        names.add(name)
        hops.append(name)
    graph["routes"][1]["participants"][0]["interposition"] = hops


def negative_limit(graph, _names):
    graph["limits"]["loans"] = -1


def oversized_limit(graph, _names):
    graph["limits"]["sampleBytes"] = 1 << 40


def page_budget_above_kernel_ceiling(graph, _names):
    graph["limits"]["bufferPages"] = FABRIC_GRAPH_KERNEL_TOTAL_PAGES + 1


def mapping_budget_above_kernel_ceiling(graph, _names):
    graph["limits"]["mappings"] = FABRIC_GRAPH_KERNEL_MAPPINGS + 1


def loan_budget_above_kernel_ceiling(graph, _names):
    graph["limits"]["loans"] = FABRIC_GRAPH_KERNEL_LOANS + 1


def limit_above_contract_ceiling(graph, _names):
    graph["limits"]["sampleBytes"] = FABRIC_GRAPH_LIMIT_SAMPLE_BYTES + 1


def ingress_limit_above_wait_bound(graph, _names):
    graph["limits"]["ingressSources"] = MAX_FABRIC_GRAPH_INGRESS_SOURCES + 1


def route_budget_below_table(graph, _names):
    graph["limits"]["routes"] = 1


def direction_budget_below_demand(graph, _names):
    graph["limits"]["publishers"] = 0


def ingress_budget_below_demand(graph, _names):
    graph["limits"]["ingressSources"] = 1


def loans_below_subscribers(graph, _names):
    graph["limits"]["loans"] = 0


def large_samples_without_pages(graph, _names):
    graph["limits"]["bufferPages"] = 0


def schema_larger_than_sample_bound(graph, _names):
    graph["limits"]["sampleBytes"] = 1


def unbounded_history_depth(graph, _names):
    graph["routes"][0]["participants"][0]["historyDepth"] = 0


def history_above_declared_bound(graph, _names):
    graph["routes"][0]["participants"][0]["historyDepth"] = (
        graph["limits"]["historyDepth"] + 1
    )


def volatile_with_retained_depth(graph, _names):
    participant = graph["routes"][0]["participants"][1]
    participant["durability"] = "volatile"
    participant["retainedDepth"] = 2


def retained_without_depth(graph, _names):
    participant = graph["routes"][0]["participants"][0]
    participant["durability"] = "retained"
    participant["retainedDepth"] = 0


def manual_without_lease(graph, _names):
    participant = graph["routes"][1]["participants"][1]
    participant["liveliness"] = "manual"
    participant["leaseNs"] = 0


def automatic_with_lease(graph, _names):
    graph["routes"][0]["participants"][0]["leaseNs"] = 1000


def lifespan_shorter_than_deadline(graph, _names):
    participant = graph["routes"][0]["participants"][0]
    participant["deadlineNs"] = 5000
    participant["lifespanNs"] = 1000


def negative_qos_scalar(graph, _names):
    graph["routes"][0]["participants"][0]["historyDepth"] = -1


for label, mutation in (
    ("unknown fabric component", unknown_fabric),
    ("unknown participant component", unknown_participant),
    ("unknown interface reference", unknown_interface),
    ("unknown interposition component", unknown_hop),
    ("duplicate participant grant", duplicate_grant),
    ("duplicate route identity", duplicate_route),
    ("direction the contract kind does not admit", wrong_direction_for_kind),
    ("unsupported QoS reliability", unsupported_qos),
    ("unsupported visibility", unsupported_visibility),
    ("self-interposition bypass", self_interposition),
    ("repeated interposition hop", repeated_interposition),
    ("route with no participants", empty_route),
    ("over-bound route count", over_bound_routes),
    ("over-bound interposition chain", over_bound_hops),
    ("negative resource limit", negative_limit),
    ("out-of-range resource limit", oversized_limit),
    ("limit above the contract ceiling", limit_above_contract_ceiling),
    ("page budget above the kernel ceiling", page_budget_above_kernel_ceiling),
    ("mapping budget above the kernel ceiling", mapping_budget_above_kernel_ceiling),
    ("loan budget above the kernel ceiling", loan_budget_above_kernel_ceiling),
    ("ingress limit above the declared bound", ingress_limit_above_wait_bound),
    ("route budget below the route table", route_budget_below_table),
    ("direction budget below live demand", direction_budget_below_demand),
    ("ingress budget below live demand", ingress_budget_below_demand),
    ("loan budget below subscriber demand", loans_below_subscribers),
    ("large samples with no page budget", large_samples_without_pages),
    ("schema larger than the declared sample bound", schema_larger_than_sample_bound),
    ("unbounded KEEP_LAST depth", unbounded_history_depth),
    ("history depth above the declared bound", history_above_declared_bound),
    ("volatile durability with a retained depth", volatile_with_retained_depth),
    ("retained durability with no depth", retained_without_depth),
    ("manual liveliness with no lease", manual_without_lease),
    ("automatic liveliness with a lease", automatic_with_lease),
    ("lifespan shorter than its deadline", lifespan_shorter_than_deadline),
    ("negative QoS scalar", negative_qos_scalar),
):
    rejected(label, mutation)

# The Rust decoder is the second reader of these bytes, and the arm that owns
# the kernel-ceiling and aggregate-demand rules. Run its corpus here so a
# layout or rule drift between the builder and the decoder fails this gate.
subprocess.run(
    [
        "cargo",
        "test",
        "--quiet",
        "--lib",
        "-p",
        "boot-contracts",
        "fabric_graph",
    ],
    cwd=ROOT,
    check=True,
)

print(
    f"fabric graph: deterministic {total_len}-byte resource with {schema_count} schemas, "
    f"{route_count} routes, {participant_count} participants, {interposition_count} hops; "
    "authority tuples, distinct domains, bounds, and negative corpus ok"
)
