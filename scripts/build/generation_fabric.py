from __future__ import annotations

import copy
import json
import os
import struct
import subprocess
from pathlib import Path

from boot_contracts import (
    FABRIC_COMPONENT_DOMAIN, FABRIC_CONTRACT_KIND_CALL, FABRIC_CONTRACT_KIND_OPERATION,
    FABRIC_CONTRACT_KIND_STREAM, FABRIC_DIRECTION_CLIENT, FABRIC_DIRECTION_PUBLISH,
    FABRIC_DIRECTION_SERVER, FABRIC_DIRECTION_SUBSCRIBE, FABRIC_DURABILITY_RETAINED,
    FABRIC_DURABILITY_VOLATILE, FABRIC_GRANT_DOMAIN, FABRIC_GRAPH_CHANNEL_QUEUE_DEPTH,
    FABRIC_GRAPH_CONTROL_MESSAGE_BYTES, FABRIC_GRAPH_FRAME_CAPACITY, FABRIC_GRAPH_HEADER,
    FABRIC_GRAPH_HEADER_BYTES, FABRIC_GRAPH_INTERPOSITION_ENTRY, FABRIC_GRAPH_INTERPOSITION_NONE,
    FABRIC_GRAPH_KERNEL_LOANS, FABRIC_GRAPH_KERNEL_MAPPINGS, FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
    FABRIC_GRAPH_LIMIT_BUFFERS, FABRIC_GRAPH_LIMIT_CAPABILITY_SLOTS, FABRIC_GRAPH_LIMIT_EVENT_DEPTH,
    FABRIC_GRAPH_LIMIT_HISTORY_DEPTH, FABRIC_GRAPH_LIMIT_IN_FLIGHT, FABRIC_GRAPH_LIMIT_QUEUE_DEPTH,
    FABRIC_GRAPH_LIMIT_RETAINED_SAMPLES, FABRIC_GRAPH_LIMIT_RETRIES, FABRIC_GRAPH_LIMIT_SAMPLE_BYTES,
    FABRIC_GRAPH_LIMIT_TRACE_DEPTH, FABRIC_GRAPH_MAGIC, FABRIC_GRAPH_PARTICIPANT_ENTRY,
    FABRIC_GRAPH_ROUTE_ENTRY, FABRIC_GRAPH_SCHEMA_ENTRY, FABRIC_GRAPH_TRACE_OVERFLOW_SATURATE,
    FABRIC_GRAPH_TRACE_TERMINAL_RESERVE, FABRIC_GRAPH_VERSION, FABRIC_LIVELINESS_AUTOMATIC,
    FABRIC_LIVELINESS_MANUAL, FABRIC_RELIABILITY_BEST_EFFORT, FABRIC_RELIABILITY_RELIABLE,
    FABRIC_ROUTE_DOMAIN, FABRIC_VISIBILITY_GRAPH, FABRIC_VISIBILITY_PRIVATE,
    MAX_FABRIC_GRAPH_INGRESS_SOURCES, MAX_FABRIC_GRAPH_INTERPOSITION_HOPS,
    MAX_FABRIC_GRAPH_PARTICIPANTS, MAX_FABRIC_GRAPH_ROLE_PARTICIPANTS, MAX_FABRIC_GRAPH_ROUTES,
    MAX_FABRIC_GRAPH_SCHEMAS, MAX_NORMALIZED_SCHEMAS, MAX_NORMALIZED_SCHEMAS_ARTIFACT_BYTES,
    NORMALIZED_SCHEMAS_ENTRY, NORMALIZED_SCHEMAS_HEADER, NORMALIZED_SCHEMAS_HEADER_BYTES,
    NORMALIZED_SCHEMAS_MAGIC, NORMALIZED_SCHEMAS_VERSION, sha256,
)
from fabric_trace_contract import FABRIC_TRACE_MAX_DEPTH, FABRIC_TRACE_OVERFLOW_SATURATE, FABRIC_TRACE_TERMINAL_RESERVE
from generation_resources import validated_shared_buffer_quotas
from harness import ROOT
from zutai_cli import STDLIB, binary

PAGE_SIZE = 4096
SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
SEL4_TARGET_PROFILES = ("aarch64-sel4-qemu-virt", "aarch64-rpi5")

def fail(message: str) -> None:
    raise SystemExit(message)

DEFAULT_FABRIC_PROFILE = "default"
# B11: the boot profile carrying the scaffolding the pre-C8.10 gate families
# exercise. `default` is the product boot and declares none of it.
TEST_BOOT_PROFILE = "test"
VISIBILITY_FABRIC_PROFILE = "visibility"
UNIFIED_FABRIC_PROFILE = "unified"
# C8.12's matching, visibility, and denial matrix. Its own profile name for the
# same reason `unified` is: the stream plane's supervision slots are numbered
# `FIRST_CONTROL_SLOT + len(controls) + index`, so a plane that adds a control
# renumbers every handle after it. Keeping the matrix's roster separate leaves
# every earlier plane's layout byte-for-byte unchanged.
MATRIX_FABRIC_PROFILE = "matrix"
FABRIC_FIRST_CONTROL_SLOT = 2
FABRIC_COPY_PAGES = 2
# The broker sizes its frame array from the same declaration (B70): the
# contract owns the number, this builder only enforces it against a graph's
# summed subscriber history.
FABRIC_FRAME_CAPACITY = FABRIC_GRAPH_FRAME_CAPACITY
FABRIC_STREAM_CONTROL_GRANTS = (
    "fabric-publisher-control",
    "fabric-subscriber-control",
    "fabric-intruder-control",
    "fabric-publisher-b-control",
    "fabric-subscriber-b-control",
)
# The single-broker default above is the fallback for a profile that declares no
# `streamControls` of its own. B60 moved the per-profile lists into the fixtures:
# `sel4-boot`/`sel4-traffic`/`sel4-fault`/`sel4-saturation` (C8.10's full-graph
# boot) and `sel4-matrix`/`sel4-matrix-unsatisfiable` (C8.12) each declare their
# seven-entry plane, where this builder previously selected one of two identical
# tuples by comparing the profile's *name*.
#
# Two properties those declarations carry, which is why they are declared in full
# per profile rather than derived from the default:
#
# * The full-graph boot names the unauthorized probe, the declared interposition
#   proxy, and the filtered-introspection client as three distinct component
#   identities. `fabric-intruder` — which carried all three roles at once behind
#   an env switch — drops out, leaving `fabric_visibility_check`'s markers and
#   source assertions, which still name it, undisturbed.
# * The stream plane's supervision slots are numbered
#   `FIRST_CONTROL_SLOT + len(controls) + index`. Lengthening one shared list
#   would renumber the subscriber supervision handles that the C8.3-C8.8 gates'
#   `launch_fabric_graph` grants positionally, and each of those gates would then
#   read a control endpoint where it expects a supervision handle. Every earlier
#   profile keeps its layout byte-for-byte by declaring nothing.
#
# C8.12's matrix plane declares `fabric-probe` deliberately: the denial under
# test is "no declared edge", not "no channel", so it must hold a real control
# endpoint before it can be refused on the graph rather than by the kernel.
FABRIC_CALL_CONTROL_GRANTS = (
    "fabric-call-client-control",
    "fabric-call-client-b-control",
    "fabric-call-server-control",
    "fabric-call-time-control",
)
FABRIC_OPERATION_CONTROL_GRANTS = (
    "fabric-op-client-control",
    "fabric-op-client-b-control",
    "fabric-op-server-control",
    "fabric-op-time-control",
)
FABRIC_OPERATION_REPLACEMENT_GRANTS = ("fabric-op-client-b-restart-control",)
# Routes a sibling seL4 manifest declares that the canonical x86 source does
# not. Named here rather than discovered by decoding every fixture, so a
# misspelling in a manifest is a build failure rather than a route silently
# absent from every worker's partition.
FABRIC_EXTRA_ROUTE_CATALOGUE = ("telemetry-alt",)
# C8.10 bounded route workers: whole routes, partitioned so no worker's live
# wake sources exceed the declared ingress ceiling. Declared here rather than
# inferred so
# the partition is a generation fact the resolver validates, not a runtime
# heuristic that could silently drift past the kernel bound.
FABRIC_ROUTE_WORKERS = (
    # `telemetry-alt` is C8.12's alternate-name route: the same interface under
    # a second name, which the identity fold makes a distinct route rather than
    # an alias. It belongs to the stream worker because it *is* a stream route;
    # the matrix graph budgets its ingress alongside the other two.
    ("stream", ("telemetry", "telemetry-alt", "diagnostics")),
    ("call", ("parameters",)),
    ("operation", ("navigation", "nav-backup")),
)
# How each worker shape's peak wake-source count is established.
#
# Counting one source per participant edge is right for the stream shape and
# wrong for the request/response ones, because the two brokers park differently.
# `park_on_streams` walks its live participant tables, so its set scales with the
# graph. `call_broker` and `operation_broker` park across *fixed slot arrays*
# (`CLIENTS = 2`, `supervision: [u32; CLIENTS + 1]`), adding a `SendCapacity` and
# a `Supervision` source per slot — so their peak is a property of the broker, not
# of how many components the graph names. A client replaced at runtime reuses its
# slot, and the operation shape's backup-route source is registered only while the
# server source is absent; deriving either from edge counts double-counts and
# rejects partitions the broker parks on comfortably.
#
# So a fixed-shape worker declares its peak and the resolver checks it against
# the kernel bound, rather than re-deriving a number the broker computes its own
# way. `graphDerived` shapes are summed from the routes the worker carries.
FABRIC_WORKER_WAIT_SHAPES = {
    # One ingress per publisher, one ack per subscriber — both counted as edges —
    # plus the capability-routed clock the QoS profile parks on.
    "stream": {"graphDerived": True, "fixed": 1},
    # Two client slots x (control endpoint + send capacity), plus the server
    # endpoint, the clock, and the server's supervision handle. Mirrors the
    # `[WaitSource; 7]` array in `call_broker::run`.
    #
    # Both shapes below sit at their bound with zero headroom — 7 of 7 for the
    # call array, 9 of 9 for the kernel set — and every combination of client
    # presence, send readiness, server presence, backup fallback, clock state,
    # and replacement-control state reaches it. So a broker that grows its park
    # set by even one source overflows immediately, and this number must move in
    # the same change rather than after the next boot fails.
    "call": {"graphDerived": False, "peak": 2 * 2 + 3},
    # As the call shape, plus one supervision source per client slot.
    "operation": {"graphDerived": False, "peak": 2 * 3 + 3},
}


class ResolvedFabricProfile:
    def __init__(
        self,
        graph: dict,
        artifact: dict,
        schemas: list,
        graph_bytes: bytes,
        manifest: dict,
    ):
        self.graph = graph
        self.artifact = artifact
        self.schemas = schemas
        self.graph_bytes = graph_bytes
        # The manifest narrowed to this profile's component set (B11). The
        # encoder reads this rather than the source manifest, so the generation
        # declares exactly what the profile resolved.
        self.manifest = manifest


FABRIC_CONTRACT_KIND = {
    "stream": FABRIC_CONTRACT_KIND_STREAM,
    "call": FABRIC_CONTRACT_KIND_CALL,
    "operation": FABRIC_CONTRACT_KIND_OPERATION,
}
FABRIC_DIRECTION = {
    "publish": FABRIC_DIRECTION_PUBLISH,
    "subscribe": FABRIC_DIRECTION_SUBSCRIBE,
    "client": FABRIC_DIRECTION_CLIENT,
    "server": FABRIC_DIRECTION_SERVER,
}
FABRIC_VISIBILITY = {
    "private": FABRIC_VISIBILITY_PRIVATE,
    "graph": FABRIC_VISIBILITY_GRAPH,
}
FABRIC_RELIABILITY = {
    "bestEffort": FABRIC_RELIABILITY_BEST_EFFORT,
    "reliable": FABRIC_RELIABILITY_RELIABLE,
}
FABRIC_DURABILITY = {
    "volatile": FABRIC_DURABILITY_VOLATILE,
    "retained": FABRIC_DURABILITY_RETAINED,
}
FABRIC_LIVELINESS = {
    "automatic": FABRIC_LIVELINESS_AUTOMATIC,
    "manual": FABRIC_LIVELINESS_MANUAL,
}
# Which directions each contract kind admits. Mixing them is a malformed
# graph, not a policy choice, and the decoder rejects it too.
FABRIC_KIND_DIRECTIONS = {
    FABRIC_CONTRACT_KIND_STREAM: {FABRIC_DIRECTION_PUBLISH, FABRIC_DIRECTION_SUBSCRIBE},
    FABRIC_CONTRACT_KIND_CALL: {FABRIC_DIRECTION_CLIENT, FABRIC_DIRECTION_SERVER},
    FABRIC_CONTRACT_KIND_OPERATION: {FABRIC_DIRECTION_CLIENT, FABRIC_DIRECTION_SERVER},
}
# Order matches the header layout after `fabric_component_identity`.
FABRIC_LIMIT_KEYS = (
    "routes",
    "ingressSources",
    "publishers",
    "subscribers",
    "clients",
    "servers",
    "sampleBytes",
    "queueDepth",
    "historyDepth",
    "eventDepth",
    "retainedSamples",
    "retries",
    "inFlightCalls",
    "inFlightOperations",
    "bufferPages",
    "buffers",
    "mappings",
    "loans",
    "capabilitySlots",
)
# Structural ceiling for each declared limit, mirroring what the decoder
# enforces in `validate_declared_limits` and `validate_against`. The page,
# mapping, and loan ceilings are the kernel's own table sizes, pinned in the
# contract and asserted against the kernel at compile time, so the builder can
# reject an over-declared budget here instead of emitting a graph the kernel
# refuses at boot.
#
# `publishers` and `subscribers` are bounded by `maxRoleParticipants`, not by
# the participant *table* bound: the stream broker holds one record with a full
# history per edge of each direction, so those two arrays are sized from the
# smaller ceiling and a graph declaring more than it can hold is refused here
# (B70). `clients` and `servers` keep the table bound -- a request/response
# broker's storage scales with in-flight calls, not with the declared client
# count.
FABRIC_LIMIT_CEILINGS = {
    "routes": MAX_FABRIC_GRAPH_ROUTES,
    "ingressSources": MAX_FABRIC_GRAPH_INGRESS_SOURCES,
    "publishers": MAX_FABRIC_GRAPH_ROLE_PARTICIPANTS,
    "subscribers": MAX_FABRIC_GRAPH_ROLE_PARTICIPANTS,
    "clients": MAX_FABRIC_GRAPH_PARTICIPANTS,
    "servers": MAX_FABRIC_GRAPH_PARTICIPANTS,
    "sampleBytes": FABRIC_GRAPH_LIMIT_SAMPLE_BYTES,
    "queueDepth": FABRIC_GRAPH_LIMIT_QUEUE_DEPTH,
    "historyDepth": FABRIC_GRAPH_LIMIT_HISTORY_DEPTH,
    "eventDepth": FABRIC_GRAPH_LIMIT_EVENT_DEPTH,
    "retainedSamples": FABRIC_GRAPH_LIMIT_RETAINED_SAMPLES,
    "retries": FABRIC_GRAPH_LIMIT_RETRIES,
    "inFlightCalls": FABRIC_GRAPH_LIMIT_IN_FLIGHT,
    "inFlightOperations": FABRIC_GRAPH_LIMIT_IN_FLIGHT,
    "capabilitySlots": FABRIC_GRAPH_LIMIT_CAPABILITY_SLOTS,
    "buffers": FABRIC_GRAPH_LIMIT_BUFFERS,
    "bufferPages": FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
    "mappings": FABRIC_GRAPH_KERNEL_MAPPINGS,
    "loans": FABRIC_GRAPH_KERNEL_LOANS,
}

# C8.11 trace-sink overflow vocabulary, mapped to the schema-owned codes the
# components compile against.
FABRIC_TRACE_OVERFLOW = {
    "saturate": FABRIC_TRACE_OVERFLOW_SATURATE,
}

# The graph header now carries the declared depth and discipline, so
# `contracts/fabric-graph/v1` restates the trace contract's bounds to refuse a
# graph no sink could honour. Two contracts stating one number is exactly the
# drift this build asserts away rather than trusts: a divergence fails here
# instead of at a boot whose worker cannot hold its own declared sink.
if (
    FABRIC_GRAPH_LIMIT_TRACE_DEPTH != FABRIC_TRACE_MAX_DEPTH
    or FABRIC_GRAPH_TRACE_TERMINAL_RESERVE != FABRIC_TRACE_TERMINAL_RESERVE
    or FABRIC_GRAPH_TRACE_OVERFLOW_SATURATE != FABRIC_TRACE_OVERFLOW_SATURATE
):
    fail("fabric graph and fabric trace contracts disagree about the sink bounds")


def validate_fabric_trace_sink(graph: dict) -> None:
    """Check the declared semantic-trace sink against its contract ceiling.

    The sink has to hold ordinary evidence *and* the mandatory terminal records
    that distinguish a completed trace from a truncated one, so a depth at or
    below the reservation is rejected: such a sink could never emit a record at
    all. The overflow discipline is a closed vocabulary rather than a free
    string, because a worker selects a code path from it.
    """
    depth = graph.get("traceDepth")
    if not isinstance(depth, int) or isinstance(depth, bool):
        fail("fabric graph: traceDepth must be an integer")
    if depth > FABRIC_TRACE_MAX_DEPTH:
        fail(f"fabric graph: traceDepth exceeds the contract ceiling {FABRIC_TRACE_MAX_DEPTH}")
    if depth <= FABRIC_TRACE_TERMINAL_RESERVE:
        fail(
            "fabric graph: traceDepth must exceed the terminal reservation "
            f"{FABRIC_TRACE_TERMINAL_RESERVE}"
        )
    overflow = graph.get("traceOverflow")
    if overflow not in FABRIC_TRACE_OVERFLOW:
        fail(f"fabric graph: unsupported traceOverflow {overflow!r}")


def validate_fabric_qos(member: dict, limits: dict, label: str) -> None:
    """Apply the same QoS truth table `fabric_graph::validate_qos` enforces.

    Duplicated deliberately: the decoder owns the rule for anything that
    reaches a boot, but a producing side that does not check it emits an
    artifact the kernel refuses, turning a manifest error into a boot panic.
    """
    scalars = ("historyDepth", "retainedDepth", "deadlineNs", "lifespanNs", "leaseNs")
    for key in scalars:
        value = member[key]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            fail(f"fabric graph: {label} has an invalid {key}")
    history = member["historyDepth"]
    retained = member["retainedDepth"]
    deadline = member["deadlineNs"]
    lifespan = member["lifespanNs"]
    lease = member["leaseNs"]
    if history > 0xFFFFFFFF or retained > 0xFFFFFFFF:
        fail(f"fabric graph: {label} depth exceeds the wire width")
    for key, value in (("deadlineNs", deadline), ("lifespanNs", lifespan), ("leaseNs", lease)):
        if value > 0xFFFFFFFFFFFFFFFF:
            fail(f"fabric graph: {label} has an out-of-range {key}")
    # KEEP_LAST is the only history policy this version defines, so the depth
    # is always finite and at least one; "keep all" would be unbounded.
    if history == 0 or history > limits["historyDepth"]:
        fail(f"fabric graph: {label} declares an out-of-range history depth")
    # Retained depth and durability are one fact stated twice.
    if (member["durability"] == "retained") == (retained == 0):
        fail(f"fabric graph: {label} durability and retained depth disagree")
    if retained > limits["retainedSamples"]:
        fail(f"fabric graph: {label} exceeds the declared retained-sample bound")
    # A lifespan shorter than the deadline expires every sample before its
    # deadline can be met.
    if deadline != 0 and lifespan != 0 and lifespan < deadline:
        fail(f"fabric graph: {label} lifespan is shorter than its deadline")
    # MANUAL liveliness needs a lease to assert against; AUTOMATIC must not
    # carry one.
    if (member["liveliness"] == "manual") == (lease == 0):
        fail(f"fabric graph: {label} liveliness and lease disagree")









def declared_fabric_profiles(manifest: dict) -> list[str]:
    """Every fabric profile the manifest names, selected or not."""
    graph = manifest.get("fabricGraph") or {}
    return [profile["name"] for profile in graph.get("profiles", [])]


def _control_sources(
    manifest: dict, grant_names: tuple[str, ...]
) -> tuple[list[str], str | None]:
    """The components holding each named control grant, in declared order, and
    the component those controls terminate at.

    B11: a grant whose source the selected boot profile does not declare is
    absent rather than invalid, so the list shortens for a profile that drops
    that participant. Order is the tuple's, and the tuple is per plane, so a
    profile declaring the same participants numbers its control slots exactly
    as it did before — which is what keeps the C8.3-C8.8 gates reading a
    control endpoint where they expect one. A grant that *is* declared must
    still be exactly right.

    B60: the holder is *read from the manifest* rather than chosen here. It used
    to be a Python string comparison on the profile name — `fabric-call-worker`
    under the full-graph profile, `fabric-service` otherwise — which made this
    builder the authority on where a plane's authority terminates while the
    fixture merely had to agree. The real invariant is that every control in one
    plane terminates at the *same* component, whichever it is; that is checked
    here, and the answer comes from the grants.

    Why the answer differs per profile at all: a bounded route worker (C8.10)
    authenticates a client by the control endpoint the request arrived on, so
    those controls must terminate at the worker itself, and a worker cannot be
    handed one afterwards — `grant_crosses_spawn` excludes endpoint grants from a
    spawn request and `nth_declared_capability` skips endpoint-kind minted
    bindings, so the generation is the only party that can place it (B55). Every
    single-plane profile declares no worker instance and terminates at
    `fabric-service`. One manifest carries one grant list, which is why a
    manifest declaring both kinds of profile cannot satisfy both rules (B56).
    """
    controls_by_name = [
        grant
        for grant in manifest["grants"]
        if grant["name"] in grant_names
    ]
    grants = {grant["name"]: grant for grant in controls_by_name}
    if len(grants) != len(controls_by_name):
        fail("fabric graph: duplicate control grant name")
    controls = []
    holder: str | None = None
    for name in grant_names:
        grant = grants.get(name)
        if grant is None:
            continue
        if grant["rights"] != ["send", "recv"]:
            fail(f"fabric graph: control grant {name} must carry send and recv")
        if holder is None:
            holder = grant["target"]
        elif grant["target"] != holder:
            fail(
                f"fabric graph: control grant {name} terminates at "
                f"{grant['target']} but its plane terminates at {holder}; a "
                "worker authenticates a client by the endpoint it arrived on, so "
                "one plane's controls cannot be split across holders"
            )
        controls.append(grant["source"])
    if len(set(controls)) != len(controls):
        fail("fabric graph: duplicate control source")
    return controls, holder


def _assert_declared_control_slots(
    manifest: dict, planes: list[dict], plane_grants: dict[str, tuple[str, ...]]
) -> None:
    """Refuse a manifest whose pinned control slots disagree with the order the
    brokers compile against.

    A control slot has two independent sources. The fixture pins an integer per
    binding (`{ grant = "fabric-call-client-control"; slot = 2; }`), while
    `fabric-service.rs` and the route workers recompute it at runtime as
    `FABRIC_FIRST_CONTROL_SLOT + position(component)` over the ordered array this
    builder emits. The two must land on identical numbers, and before B60 nothing
    but a comment said so — which is the mechanism B55 hit from the other side,
    where a stale supervision-row count shifted every call/op slot above it and
    only a full boot exposed it.

    Only the *holder's* binding is checked. An endpoint grant installs both ends,
    so it has two bindings: the client's, numbered in the client's own small
    namespace (slot 0 for its single control), and the holder's, which is the
    indexed table the broker walks. Comparing the client's would compare two
    unrelated numberings.

    And only a holder that owns *one* plane is checked. Each plane is numbered
    from `FABRIC_FIRST_CONTROL_SLOT` independently, because C8.10's bounded route
    workers are separate tasks with separate capability tables — slot 2 in the
    call worker and slot 2 in the operation worker name different objects. A
    holder owning several planes therefore cannot satisfy every plane's numbering
    at once, and `valid.zti`'s reference `fabric-service` is exactly that: one
    broker holding stream, call, and operation controls in one table, where the
    planes must be laid out consecutively rather than each from slot 2. Asserting
    the per-plane rule against it would demand a contradiction — the same shape of
    mistake B56 found in a gate that swept every profile through a rule only some
    could satisfy.

    Checked here rather than in a gate script because a divergence makes the
    generation wrong, not merely unproven: the build should not emit it.
    """
    # A plane's controls are emitted in the order of its grant-name tuple, so the
    # nth control's slot is the nth declared grant's. Rebuild that association by
    # position: `planes` carries components, and `_control_sources` dropped any
    # grant this profile does not declare, so re-filtering the same tuples the
    # same way recovers which grant produced which slot.
    derived: dict[tuple[str, str], int] = {}
    planes_per_holder: dict[str, set[str]] = {}
    grants = {grant["name"]: grant for grant in manifest["grants"]}
    for plane in planes:
        tuples = plane_grants.get(plane["name"], ())
        declared = [name for name in tuples if name in grants]
        if len(declared) != len(plane["controls"]):
            # The plane's tuple and its resolved controls disagree in length,
            # which means the two are no longer the same filter. Refusing beats
            # silently checking a shifted pairing.
            fail(
                f"fabric graph: plane {plane['name']} resolved "
                f"{len(plane['controls'])} controls from {len(declared)} declared "
                "grants; the control-slot cross-check cannot pair them"
            )
        for name, control in zip(declared, plane["controls"], strict=True):
            holder = grants[name]["target"]
            derived[(holder, name)] = control["slot"]
            # `operationReplacement` is numbered as a continuation of
            # `operation`, not as a plane of its own, so the two do not count as
            # separate planes against one holder.
            family = "operation" if plane["name"] == "operationReplacement" else plane["name"]
            planes_per_holder.setdefault(holder, set()).add(family)
    for instance in manifest["instances"]:
        for binding in instance.get("bindings", []):
            expected = derived.get((instance["name"], binding["grant"]))
            if expected is None or expected == binding["slot"]:
                continue
            if len(planes_per_holder.get(instance["name"], ())) != 1:
                continue
            fail(
                f"fabric graph: {instance['name']}'s binding for "
                f"{binding['grant']} pins slot {binding['slot']} but the plane "
                f"derives {expected}; the fixture and the broker's "
                "FABRIC_FIRST_CONTROL_SLOT + index must agree"
            )


def resolve_fabric_graph(graph: dict, profile_name: str) -> dict:
    profiles = graph.get("profiles", [])
    names = [profile.get("name") for profile in profiles]
    if len(names) != len(set(names)):
        fail("fabric graph: duplicate profile name")
    matches = [profile for profile in profiles if profile.get("name") == profile_name]
    if len(matches) != 1:
        fail(f"fabric graph: expected exactly one {profile_name} profile")
    resolved = copy.deepcopy(graph)
    resolved.pop("profiles", None)
    seen: set[tuple[str, str]] = set()
    for override in matches[0]["interpositions"]:
        target = (override["route"], override["participant"])
        if target in seen:
            fail("fabric graph: duplicate profile override")
        seen.add(target)
        matches = [
            member
            for route in resolved["routes"]
            if route["name"] == target[0]
            for member in route["participants"]
            if member["component"] == target[1]
        ]
        if len(matches) != 1:
            fail(
                "fabric graph: profile interposition must name exactly one "
                f"participant ({target[1]} on {target[0]})"
            )
        chain = override["chain"]
        if not isinstance(chain, list) or not chain:
            fail("fabric graph: profile interposition chain must be non-empty")
        matches[0]["interposition"] = chain
    return resolved


def build_normalized_schema_artifact(schemas: list) -> bytes:
    if len(schemas) > MAX_NORMALIZED_SCHEMAS:
        fail("normalized schema artifact exceeds the schema bound")
    records = bytearray()
    payload = bytearray()
    for interface in schemas:
        records += NORMALIZED_SCHEMAS_ENTRY.pack(
            interface.identity, len(interface.normalized), 0
        )
        payload += interface.normalized
    total_len = NORMALIZED_SCHEMAS_HEADER_BYTES + len(records) + len(payload)
    if total_len > MAX_NORMALIZED_SCHEMAS_ARTIFACT_BYTES:
        fail("normalized schema artifact exceeds its byte bound")
    return NORMALIZED_SCHEMAS_HEADER.pack(
        NORMALIZED_SCHEMAS_MAGIC,
        NORMALIZED_SCHEMAS_VERSION,
        NORMALIZED_SCHEMAS_HEADER_BYTES,
        0,
        len(schemas),
        total_len,
    ) + records + payload


def validate_route_worker_names(declared_routes: set[str]) -> None:
    """Every name in `FABRIC_ROUTE_WORKERS` is a route some manifest declares.

    Checked against the **full catalogue** — every route name any manifest in
    this repository declares — rather than against the graph being built. Those
    are different questions, and conflating them is what makes the check either
    useless or wrong:

    * against the graph being built, a manifest declaring a subset of the
      routes (P5.5.2's seL4 graph declares the two stream routes alone) fails
      on a tuple that has no typo in it;
    * without the check at all, a genuine misspelling in the tuple silently
      drops a route from its worker, and the partition assertion below then
      reports the route as uncovered rather than the worker as misspelled.

    So the typo check reads the source of truth for what routes exist, and the
    partition check reads the graph. A worker whose routes this graph does not
    declare simply has no work here.

    The catalogue is the canonical x86 manifest's routes plus
    `FABRIC_EXTRA_ROUTE_CATALOGUE`, for routes a sibling seL4 manifest declares
    and the x86 source does not. Reading every manifest instead would make one
    build decode twenty-odd fixtures to answer a question about a constant, and
    would silently accept a typo the moment some fixture happened to contain it.
    """
    catalogue = {
        route["name"]
        for route in _canonical_manifest()["fabricGraph"]["routes"]
    } | set(FABRIC_EXTRA_ROUTE_CATALOGUE)
    unknown_declared = sorted(declared_routes - catalogue)
    if unknown_declared:
        fail(
            f"fabric graph: this manifest declares route(s) {unknown_declared}, "
            "which the route catalogue does not name; add them to "
            "FABRIC_EXTRA_ROUTE_CATALOGUE and to a route worker"
        )
    for worker_name, worker_routes in FABRIC_ROUTE_WORKERS:
        unknown = [route for route in worker_routes if route not in catalogue]
        if unknown:
            fail(
                f"fabric graph: worker {worker_name} names {unknown}, which no "
                "declared route matches"
            )


def _canonical_manifest() -> dict:
    """The x86 source manifest, which is the union of every declared route.

    Decoded through the same Zutai binary every other manifest goes through,
    rather than parsed here, so the catalogue this validates against is the one
    the builder would actually encode.
    """
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    try:
        output = subprocess.run(
            [str(binary()), "json", str(SOURCE)],
            cwd=ROOT,
            env=environment,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        fail(f"cannot read the canonical manifest {SOURCE}: {error}")
    return json.loads(output)


def resolve_fabric_profile(manifest: dict, interfaces: list, profile_name: str, resolve_boot_profile) -> ResolvedFabricProfile:
    """Resolve one named boot profile into everything downstream reads.

    B11 folded the component set into this one selector, so this is where the
    manifest is narrowed: `profile_name` names a `bootProfiles` entry, that
    entry names the scaffolding to declare and the `fabricGraph.profiles` entry
    to apply, and every later stage reads the narrowed manifest this returns.
    Narrowing here rather than in `main` keeps the host-side gates that call
    this function directly on the same path a real build takes.
    """
    # Captured before narrowing: the route workers are a partition of the routes
    # the *manifest* declares, so a typo in `FABRIC_ROUTE_WORKERS` must stay
    # detectable even under a profile that drops the route it misspells.
    declared_routes = {route["name"] for route in manifest["fabricGraph"]["routes"]}
    if manifest.get("bootProfiles"):
        manifest = resolve_boot_profile(manifest, profile_name)
        fabric_profile_name = manifest["fabricGraph"]["profiles"][0]["name"]
    else:
        fabric_profile_name = profile_name
    # The record itself, captured before `resolve_fabric_graph` drops the
    # `profiles` list: B60 reads this profile's declared stream control plane
    # from it rather than inferring one from its name.
    fabric_profile = next(
        (
            profile
            for profile in manifest["fabricGraph"].get("profiles", [])
            if profile.get("name") == fabric_profile_name
        ),
        {},
    )
    graph = resolve_fabric_graph(manifest["fabricGraph"], fabric_profile_name)
    executable_names = {executable["name"] for executable in manifest["executables"]}
    graph_bytes = build_fabric_graph(graph, executable_names, interfaces)
    by_interface = {interface.name: interface for interface in interfaces}
    used_schemas = {route["interface"]: by_interface[route["interface"]] for route in graph["routes"]}
    schemas = sorted(used_schemas.values(), key=lambda interface: interface.identity)
    route_rows = sorted(
        (
            fabric_route_identity(
                route["name"],
                by_interface[route["interface"]].identity,
                FABRIC_CONTRACT_KIND[by_interface[route["interface"]].kind],
            ),
            route,
        )
        for route in graph["routes"]
    )
    # B60: which grants form the stream control plane is the *profile's* to
    # declare. This used to be a Python comparison on the profile name, which
    # made the builder the authority on a plane's membership while the manifest
    # merely had to agree — and order is authority-bearing here, since a broker
    # resolves a client's slot as `FIRST_CONTROL_SLOT + position`.
    #
    # A profile declaring no list keeps the single-broker default: every earlier
    # gate's plane stays byte-identical, and a source the profile does not declare
    # drops out rather than failing, so the product profile resolves the same
    # plane with fewer participants.
    declared_stream_controls = tuple(fabric_profile.get("streamControls", ()))
    stream_control_grants = declared_stream_controls or FABRIC_STREAM_CONTROL_GRANTS
    stream_controls, _stream_holder = _control_sources(manifest, stream_control_grants)
    # B60: the holder each plane terminates at is the manifest's to declare, and
    # `_control_sources` reads it from the grants rather than this builder
    # choosing it by profile name. The operation plane's two grant families must
    # still land on one holder — they share one worker's control table — so that
    # is checked here rather than assumed.
    call_controls, _call_holder = _control_sources(manifest, FABRIC_CALL_CONTROL_GRANTS)
    operation_controls, operation_holder = _control_sources(
        manifest, FABRIC_OPERATION_CONTROL_GRANTS
    )
    replacement_controls, replacement_holder = _control_sources(
        manifest, FABRIC_OPERATION_REPLACEMENT_GRANTS
    )
    if (
        operation_holder is not None
        and replacement_holder is not None
        and operation_holder != replacement_holder
    ):
        fail(
            f"fabric graph: operation controls terminate at {operation_holder} but "
            f"their replacement controls terminate at {replacement_holder}; both "
            "are one worker's control table"
        )
    participants = []
    for _route_identity, route in route_rows:
        interface = by_interface[route["interface"]]
        for member in route["participants"]:
            participants.append(
                {
                    "component": member["component"],
                    "route": route["name"],
                    "interface": interface.name,
                    "direction": FABRIC_DIRECTION[member["direction"]],
                    "visibility": FABRIC_VISIBILITY[member["visibility"]],
                    "reliability": FABRIC_RELIABILITY[member["reliability"]],
                    "durability": FABRIC_DURABILITY[member["durability"]],
                    "liveliness": FABRIC_LIVELINESS[member["liveliness"]],
                    "historyDepth": member["historyDepth"],
                    "retainedDepth": member["retainedDepth"],
                    "deadlineNs": member["deadlineNs"],
                    "lifespanNs": member["lifespanNs"],
                    "leaseNs": member["leaseNs"],
                    "interposition": member["interposition"],
                }
            )
    # Every ring participant, not only subscribers (B46). A v2 stream edge is a
    # writable shared ring the fabric loans to its peer, and a loan names its
    # receiver through a supervision capability -- so a publisher needs one for
    # exactly the reason a subscriber does. Under v1 only subscribers received
    # anything (samples were messages, and only the downstream sample hop was a
    # loan), which is why this list used to be subscribers alone.
    ring_components = {
        participant["component"]
        for participant in participants
        if participant["direction"]
        in (FABRIC_DIRECTION_PUBLISH, FABRIC_DIRECTION_SUBSCRIBE)
    }
    # A declared interposition proxy needs one for the same reason a ring holder
    # does. It holds no ring, but the fabric has to observe its *death*: a
    # native Endpoint gives no peer-death signal, so a broker blocked on a hop
    # through a dead proxy would wait forever. A supervision capability is the
    # only thing in this model that answers "is that task gone", and the chain
    # naming the proxy is a generation fact, so the handle is one too.
    proxy_components = {
        proxy
        for participant in participants
        for proxy in participant["interposition"]
    }
    # C8.12's ungranted probe needs one for a third reason, and only on the
    # matrix plane. It holds no ring and interposes on nothing, so neither rule
    # above names it — but the matrix broker's dispatch loop has to know when it
    # has stopped asking, and a native Endpoint reports no peer death. Without a
    # handle the loop would poll a silent endpoint forever, unable to tell a
    # refused caller that has exited from one that is merely slow.
    #
    # Scoped to this profile because a handle is a slot: granting one everywhere
    # would renumber every earlier plane's supervision table. It grants the
    # probe nothing either way — the *fabric* holds the handle, not the probe.
    denied_components = (
        {
            component
            for component in stream_controls
            if component not in ring_components and component not in proxy_components
        }
        if fabric_profile_name == MATRIX_FABRIC_PROFILE
        else set()
    )
    holders = [
        component
        for component in stream_controls
        if component in ring_components
        or component in proxy_components
        or component in denied_components
    ]
    supervision = [
        {"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + len(stream_controls) + index}
        for index, component in enumerate(holders)
    ]
    # C8.10: every plane coexists in one boot, so its control slots are summed
    # into one disjoint layout rather than overlaid. `max()` here would size the
    # table for whichever single plane happened to be largest, which is exactly
    # the mutually-exclusive assumption the milestone removes: two planes would
    # then be numbered from the same base and collide on the same slot.
    plane_control_counts = (
        len(stream_controls) * 2 + len(holders),
        len(call_controls),
        len(operation_controls) + len(replacement_controls),
    )
    retained_route_endpoints = sum(plane_control_counts)
    required_capability_slots = FABRIC_FIRST_CONTROL_SLOT + retained_route_endpoints + graph["limits"]["buffers"]
    # C8.10 bounded route workers. Each worker owns whole routes and must be able
    # to park on every live source those routes produce at once. A graph that
    # cannot be split into workers under the kernel wait bound would have to poll,
    # so it fails the build rather than the boot.
    #
    # The count is every source the broker registers, which is not the same as
    # every participant. How it is established depends on the worker's shape —
    # see `FABRIC_WORKER_WAIT_SHAPES` — because the stream broker's set scales
    # with the graph while the request/response brokers park across fixed slot
    # arrays of their own.
    validate_route_worker_names(declared_routes)
    workers = []
    for worker_name, worker_routes in FABRIC_ROUTE_WORKERS:
        # A route this manifest does not declare is not this manifest's to
        # partition. Every name in the tuple was already checked against the
        # full route catalogue by `validate_route_worker_names` above, so what
        # is filtered here is genuinely "not in this graph" rather than
        # "misspelled" — which is the distinction the two checks exist to keep
        # apart. P5.5.2's seL4 graph declares the two stream routes alone.
        worker_routes = tuple(route for route in worker_routes if route in declared_routes)
        if not worker_routes:
            continue
        # A route whose every participant was scaffolding is absent from this
        # profile. Its worker still exists and still owns the routes that
        # remain; only a worker left with no route at all drops out, so the
        # partition below stays a statement about this profile's graph.
        declared = [route for route in graph["routes"] if route["name"] in worker_routes]
        if not declared:
            continue
        shape = FABRIC_WORKER_WAIT_SHAPES.get(worker_name)
        if shape is None:
            fail(f"fabric graph: worker {worker_name} declares no wait-source shape")
        if shape["graphDerived"]:
            members = [member for route in declared for member in route["participants"]]
            sources = (
                sum(
                    1
                    for member in members
                    if member["direction"] in ("publish", "client", "subscribe", "server")
                )
                + shape["fixed"]
            )
        else:
            sources = shape["peak"]
        if sources > MAX_FABRIC_GRAPH_INGRESS_SOURCES:
            fail(
                f"fabric graph: worker {worker_name} needs {sources} wake sources, "
                f"above the declared ceiling of {MAX_FABRIC_GRAPH_INGRESS_SOURCES}"
            )
        workers.append(
            {
                "name": worker_name,
                "routes": sorted(route["name"] for route in declared),
                "waitSources": sources,
            }
        )
    covered = [route for worker in workers for route in worker["routes"]]
    if sorted(covered) != sorted(route["name"] for route in graph["routes"]):
        fail("fabric graph: route workers do not partition the declared routes")
    if len(covered) != len(set(covered)):
        fail("fabric graph: a route is claimed by more than one worker")
    artifact = {
        "formatVersion": 1,
        "name": profile_name,
        "fabricComponent": graph["fabricComponent"],
        "firstControlSlot": FABRIC_FIRST_CONTROL_SLOT,
        "copyPages": FABRIC_COPY_PAGES,
        "frameCapacity": FABRIC_FRAME_CAPACITY,
        "requiredCapabilitySlots": required_capability_slots,
        # C8.11: the sink's capacity and overflow code, resolved once here so
        # every worker in the graph compiles against the same two numbers.
        "traceDepth": graph["traceDepth"],
        "traceOverflow": FABRIC_TRACE_OVERFLOW[graph["traceOverflow"]],
        "limits": [{"name": key, "value": graph["limits"][key]} for key in FABRIC_LIMIT_KEYS],
        "schemas": [
            {
                "name": interface.name,
                "identity": interface.identity.hex(),
                "typeTag": f"{interface.type_tag:016x}",
                "contractKind": FABRIC_CONTRACT_KIND[interface.kind],
                "maxEncodedBytes": interface.max_encoded_bytes,
            }
            for interface in schemas
        ],
        "routes": [
            {
                "name": route["name"],
                "interface": route["interface"],
                "identity": identity.hex(),
                "contractKind": FABRIC_CONTRACT_KIND[by_interface[route["interface"]].kind],
            }
            for identity, route in route_rows
        ],
        "participants": participants,
        "workers": workers,
        # C8.10: one plane is one bounded route worker. Each worker is its own
        # task with its own capability table, so numbering every plane from
        # `FABRIC_FIRST_CONTROL_SLOT` is disjoint by construction rather than
        # colliding: slot 2 in the stream worker and slot 2 in the call worker
        # name different objects in different tables. What must not collide is
        # the aggregate init hands out, which `requiredCapabilitySlots` sums.
        "planes": [
            {"name": "stream", "controls": [{"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + index} for index, component in enumerate(stream_controls)]},
            {"name": "call", "controls": [{"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + index} for index, component in enumerate(call_controls)]},
            {"name": "operation", "controls": [{"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + index} for index, component in enumerate(operation_controls)]},
            {"name": "operationReplacement", "controls": [{"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + len(operation_controls) + index} for index, component in enumerate(replacement_controls)]},
        ],
        "supervision": supervision,
    }
    _assert_declared_control_slots(
        manifest,
        artifact["planes"],
        {
            "stream": stream_control_grants,
            "call": FABRIC_CALL_CONTROL_GRANTS,
            "operation": FABRIC_OPERATION_CONTROL_GRANTS,
            "operationReplacement": FABRIC_OPERATION_REPLACEMENT_GRANTS,
        },
    )
    quotas = validated_shared_buffer_quotas(manifest["sharedBufferBudget"])
    quota = quotas.get(graph["fabricComponent"])
    if quota is None:
        fail("fabric graph: fabric holder has no shared-buffer quota")
    limits = graph["limits"]
    for limit, quota_key in (
        ("bufferPages", "bytePages"),
        ("buffers", "bufferCount"),
        ("mappings", "mappingCount"),
        ("loans", "loanCount"),
    ):
        if limits[limit] > quota[quota_key]:
            fail(f"fabric graph: {limit} exceeds the fabric holder quota")
    sample_pages = (limits["sampleBytes"] + PAGE_SIZE - 1) // PAGE_SIZE
    if sample_pages > FABRIC_COPY_PAGES:
        fail("fabric graph: sampleBytes exceeds the generated copy layout")
    # Everything that pins a fabric-owned frame, not just subscriber queues
    # (C10.4). A `retained` publisher holds its last `retainedDepth` samples for
    # late joiners, and those references are live *concurrently* with every
    # subscriber's queue rather than instead of it — `retain_for_late_joiners`
    # takes one per entry and only `release_retained` drops them.
    #
    # This must match `fabric-service`'s own `declared_capacity` exactly. The
    # component sizes its frame table from the same two terms and refuses a graph
    # whose sum passes the same ceiling, so a builder summing fewer terms would
    # certify manifests the component then refuses at boot — a generation the
    # toolchain approved and the graph's own holder rejects. `max(1)` mirrors
    # `provision_edge`'s `StreamHistory::new(qos.retained_depth.max(1))`.
    #
    # Both filters compare against the *encoded* constants and index directly,
    # because `participants` stores every enum already mapped through
    # `FABRIC_DURABILITY`/`FABRIC_DIRECTION`. A first version of this compared
    # `durability` to the string `"retained"`, which no record ever holds: the
    # term silently evaluated to zero and the guard did nothing while every gate
    # stayed green. `.get(..., default)` on keys these records always set masked
    # it, so both are direct lookups now — a renamed key raises instead of
    # defaulting to a value that disables the check.
    ring_capacity = sum(
        participant["historyDepth"]
        for participant in participants
        if participant["direction"] == FABRIC_DIRECTION_SUBSCRIBE
    ) + sum(
        max(participant["retainedDepth"], 1)
        for participant in participants
        if participant["direction"] == FABRIC_DIRECTION_PUBLISH
        and participant["durability"] == FABRIC_DURABILITY_RETAINED
    )
    if ring_capacity > FABRIC_FRAME_CAPACITY:
        fail("fabric graph: declared frame demand exceeds the frame table")
    if limits["eventDepth"] % 2 != 0 or limits["eventDepth"] < 2:
        fail("fabric graph: operation event depth is not evenly allocatable")
    if required_capability_slots > limits["capabilitySlots"]:
        fail("fabric graph: generated capability layout exceeds its declaration")
    if any(len(route["name"].encode("utf-8")) > 16 for route in graph["routes"]):
        fail("fabric graph: route name exceeds the 16-byte record bound")
    return ResolvedFabricProfile(graph, artifact, schemas, graph_bytes, manifest)


def _zti_value(value: object, indent: int = 0) -> str:
    padding = "  " * indent
    child = "  " * (indent + 1)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, list):
        if not value:
            return "[]"
        return "[\n" + "".join(f"{child}{_zti_value(item, indent + 1)};\n" for item in value) + f"{padding}]"
    if isinstance(value, dict):
        return "{\n" + "".join(
            f"{child}{key} = {_zti_value(item, indent + 1)};\n" for key, item in value.items()
        ) + f"{padding}}}"
    fail(f"unsupported canonical profile value {type(value).__name__}")


def write_resolved_profile(output: Path, resolved: ResolvedFabricProfile) -> tuple[Path, Path]:
    """Write the canonical resolved profile and its normalized schema corpus.

    The third output this used to write -- a Rust constant table every fabric
    component `include!`d -- is gone with B70's last consumer. Everything it
    carried is now either a field of the authenticated `fabric-graph` resource
    the component queries at runtime, a name it resolves through the root, or a
    ceiling published by `contracts/fabric-graph/v1`. The `.zti` artifact
    remains because it is the canonical record this builder's own gate compares
    and the Zutai contract validates; nothing compiles against it.
    """
    profile_path = output / "data-fabric-profile.zti"
    schemas_path = output / "normalized-interface-schemas.bin"
    profile_path.write_text(_zti_value(resolved.artifact) + "\n", encoding="utf-8")
    schemas_path.write_bytes(build_normalized_schema_artifact(resolved.schemas))
    return profile_path, schemas_path




def fabric_component_identity(name: str) -> bytes:
    """Stable component identity, matching `boot_contracts::fabric_graph`.

    Deliberately a different domain from `holder_identity`: shared-buffer
    quota authority and fabric route authority are separate domains, so one
    identity may never be replayed into the other.
    """
    encoded = name.encode("utf-8")
    return sha256(
        FABRIC_COMPONENT_DOMAIN + struct.pack("<H", len(encoded)) + encoded
    )


def fabric_route_identity(name: str, interface_identity: bytes, contract_kind: int) -> bytes:
    encoded = name.encode("utf-8")
    return sha256(
        FABRIC_ROUTE_DOMAIN
        + struct.pack("<H", len(encoded))
        + encoded
        + interface_identity
        + struct.pack("<I", contract_kind)
    )


def fabric_grant_identity(route_identity: bytes, component: bytes, direction: int) -> bytes:
    return sha256(
        FABRIC_GRANT_DOMAIN + route_identity + component + struct.pack("<I", direction)
    )




def build_fabric_graph(graph: dict, component_names: set[str], interfaces: list) -> bytes:
    """Encode the C8.2 fabric-graph resource object.

    Route, schema, and participant tables are sorted by identity and must be
    unique: the decoder rejects an unsorted or duplicated table, so the sort
    here is part of the format. A component absent from the participant table
    holds no route authority at all — omission is meaningful, not a default.
    """
    by_name = {interface.name: interface for interface in interfaces}
    fabric = graph["fabricComponent"]
    if fabric not in component_names:
        fail(f"fabric graph: unknown fabric component {fabric}")

    # Every declared limit is checked against the contract's own structural
    # ceiling here, mirroring `FabricGraph::validate_declared_limits`. Without
    # this the builder would emit a graph the kernel decoder rejects, so an
    # over-declared limit would surface as a boot panic instead of a build
    # failure — green build, unbootable image.
    limits = graph["limits"]
    limit_values = []
    for key in FABRIC_LIMIT_KEYS:
        value = limits[key]
        if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= 0xFFFFFFFF:
            fail(f"fabric graph: invalid limit {key}")
        ceiling = FABRIC_LIMIT_CEILINGS.get(key)
        if ceiling is not None and value > ceiling:
            fail(f"fabric graph: limit {key} exceeds the contract ceiling {ceiling}")
        limit_values.append(value)

    # C8.11: the trace sink's capacity and overflow discipline are graph facts,
    # checked here against the schema-owned ceiling for the same reason every
    # limit above is: a component compiles its sink array from this number, so
    # an over-declared depth would be a build that emits an image whose worker
    # cannot hold its own declared sink.
    validate_fabric_trace_sink(graph)

    routes = graph["routes"]
    if len(routes) > MAX_FABRIC_GRAPH_ROUTES:
        fail("fabric graph exceeds route bound")

    # Only interfaces a route actually names are admitted into the resource:
    # the schema table is the graph's own closure, not the whole catalog.
    used: dict[str, object] = {}
    route_rows: list[tuple[bytes, str, int]] = []
    for route in routes:
        interface_name = route["interface"]
        interface = by_name.get(interface_name)
        if interface is None:
            fail(f"fabric graph: route {route['name']} names unknown interface {interface_name}")
        contract_kind = FABRIC_CONTRACT_KIND.get(interface.kind)
        if contract_kind is None:
            fail(f"fabric graph: unsupported contract kind {interface.kind}")
        used[interface_name] = interface
        route_rows.append(
            (
                fabric_route_identity(route["name"], interface.identity, contract_kind),
                interface_name,
                contract_kind,
            )
        )

    schemas = sorted(used.values(), key=lambda item: item.identity)
    if len(schemas) > MAX_FABRIC_GRAPH_SCHEMAS:
        fail("fabric graph exceeds schema bound")
    schema_index = {interface.name: index for index, interface in enumerate(schemas)}

    ordered = sorted(zip(route_rows, routes, strict=True), key=lambda pair: pair[0][0])
    if len({row[0] for row, _ in ordered}) != len(ordered):
        fail("fabric graph: duplicate route identity")
    # A graph admitting more routes than it budgets is over-committed at rest,
    # before a single participant launches.
    if len(ordered) > limits["routes"]:
        fail("fabric graph admits more routes than it budgets")

    # Hops are emitted per participant so each chain owns its own slots; the
    # decoder walks `next_hop` and rejects a revisit or a self-hop.
    hops: list[tuple[bytes, int]] = []
    participants: list[tuple] = []
    per_direction = {name: 0 for name in FABRIC_DIRECTION}
    route_records = bytearray()
    for route_index, (row, route) in enumerate(ordered):
        route_identity, interface_name, contract_kind = row
        members = route["participants"]
        if not members:
            fail(f"fabric graph: route {route['name']} has no participants")
        for member in members:
            component = member["component"]
            if component not in component_names:
                fail(f"fabric graph: unknown participant component {component}")
            direction = FABRIC_DIRECTION.get(member["direction"])
            visibility = FABRIC_VISIBILITY.get(member["visibility"])
            reliability = FABRIC_RELIABILITY.get(member["reliability"])
            durability = FABRIC_DURABILITY.get(member["durability"])
            liveliness = FABRIC_LIVELINESS.get(member["liveliness"])
            if None in (direction, visibility, reliability, durability, liveliness):
                fail(f"fabric graph: unsupported policy for {component} on {route['name']}")
            if direction not in FABRIC_KIND_DIRECTIONS[contract_kind]:
                fail(
                    f"fabric graph: {member['direction']} is not a "
                    f"{interface_name} direction on {route['name']}"
                )
            validate_fabric_qos(member, limits, f"{component} on {route['name']}")
            per_direction[member["direction"]] += 1
            head = FABRIC_GRAPH_INTERPOSITION_NONE
            chain = member["interposition"]
            if chain:
                if len(hops) + len(chain) > MAX_FABRIC_GRAPH_INTERPOSITION_HOPS:
                    fail("fabric graph exceeds interposition hop bound")
                if len(set(chain)) != len(chain):
                    fail(f"fabric graph: repeated interposition hop on {route['name']}")
                if component in chain:
                    fail(f"fabric graph: {component} interposes on itself")
                head = len(hops)
                for offset, hop in enumerate(chain):
                    if hop not in component_names:
                        fail(f"fabric graph: unknown interposition component {hop}")
                    last = offset == len(chain) - 1
                    hops.append(
                        (
                            fabric_component_identity(hop),
                            FABRIC_GRAPH_INTERPOSITION_NONE if last else head + offset + 1,
                        )
                    )
            identity = fabric_component_identity(component)
            participants.append(
                (
                    fabric_grant_identity(route_identity, identity, direction),
                    identity,
                    route_index,
                    direction,
                    visibility,
                    head,
                    member["deadlineNs"],
                    member["lifespanNs"],
                    member["leaseNs"],
                    member["historyDepth"],
                    member["retainedDepth"],
                    reliability,
                    durability,
                    liveliness,
                    0,
                )
            )
        route_records += FABRIC_GRAPH_ROUTE_ENTRY.pack(
            route_identity, schema_index[interface_name], contract_kind, len(members), 0
        )

    if len(participants) > MAX_FABRIC_GRAPH_PARTICIPANTS:
        fail("fabric graph exceeds participant bound")
    participants.sort(key=lambda entry: entry[0])
    if len({entry[0] for entry in participants}) != len(participants):
        fail("fabric graph: duplicate participant grant")

    # Aggregate demand: every declared participant live at once must fit the
    # limits the graph itself declares, so a validating graph is one the fabric
    # can honour in full rather than first-come-first-served. Mirrors
    # `FabricGraph::validate_against`.
    for name, budget in (
        ("publish", "publishers"),
        ("subscribe", "subscribers"),
        ("client", "clients"),
        ("server", "servers"),
    ):
        if per_direction[name] > limits[budget]:
            fail(f"fabric graph declares more {name} edges than its {budget} budget")
    # Every edge delivering into the fabric is a live wake source it must
    # register; a graph it cannot block on would have to poll.
    ingress = per_direction["publish"] + per_direction["client"]
    if ingress > limits["ingressSources"]:
        fail("fabric graph declares more live ingress sources than it budgets")
    # The fabric owes one receiver-bound downstream loan, and one mapping for
    # it, per matched subscriber.
    if limits["subscribers"] > limits["loans"] or limits["subscribers"] > limits["mappings"]:
        fail("fabric graph budgets fewer loans or mappings than subscribers")
    # A shared sample needs one fabric-owned buffer whose page footprint can
    # carry the graph's maximum admitted sample.
    sample_pages = (limits["sampleBytes"] + PAGE_SIZE - 1) // PAGE_SIZE
    if limits["sampleBytes"] > FABRIC_GRAPH_CONTROL_MESSAGE_BYTES and (
        limits["buffers"] == 0 or sample_pages > limits["bufferPages"]
    ):
        fail("fabric graph admits a shared sample its buffer budget cannot hold")
    if limits["queueDepth"] > FABRIC_GRAPH_CHANNEL_QUEUE_DEPTH:
        fail("fabric graph queue depth exceeds the kernel channel bound")
    for interface in schemas:
        if interface.max_encoded_bytes > limits["sampleBytes"]:
            fail(
                f"fabric graph: {interface.name} encodes larger than the declared sample bound"
            )

    schema_records = b"".join(
        FABRIC_GRAPH_SCHEMA_ENTRY.pack(
            interface.identity,
            interface.type_tag,
            FABRIC_CONTRACT_KIND[interface.kind],
            interface.max_encoded_bytes,
        )
        for interface in schemas
    )
    participant_records = b"".join(
        FABRIC_GRAPH_PARTICIPANT_ENTRY.pack(*entry) for entry in participants
    )
    hop_records = b"".join(
        FABRIC_GRAPH_INTERPOSITION_ENTRY.pack(identity, next_hop, 0)
        for identity, next_hop in hops
    )
    total_len = (
        FABRIC_GRAPH_HEADER_BYTES
        + len(schema_records)
        + len(route_records)
        + len(participant_records)
        + len(hop_records)
    )
    header = FABRIC_GRAPH_HEADER.pack(
        FABRIC_GRAPH_MAGIC,
        FABRIC_GRAPH_VERSION,
        FABRIC_GRAPH_HEADER_BYTES,
        0,
        total_len,
        len(schemas),
        len(ordered),
        len(participants),
        len(hops),
        0,
        fabric_component_identity(fabric),
        *limit_values,
        # C8.11's sink shape travels in the header rather than in a generated
        # per-plane constant table, so a worker built out of tree reads the
        # depth it must honour from the same authenticated resource that
        # declares its routes (B70). `validate_fabric_trace_sink` has already
        # bounded both against the trace contract.
        graph["traceDepth"],
        FABRIC_TRACE_OVERFLOW[graph["traceOverflow"]],
    )
    return header + schema_records + route_records + participant_records + hop_records
