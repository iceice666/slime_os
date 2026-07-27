#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import hashlib
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

from boot_contracts import (
    BOOTSTATE_ACCEPTED_RELEASE_SEQUENCE_OFFSET,
    BOOTSTATE_CHECKSUM_END,
    BOOTSTATE_CHECKSUM_OFFSET,
    BOOTSTATE_GENERATION_ROOT_END,
    BOOTSTATE_GENERATION_ROOT_OFFSET,
    BOOTSTATE_KNOWN_GOOD_END,
    BOOTSTATE_KNOWN_GOOD_OFFSET,
    BOOTSTATE_MAGIC,
    BOOTSTATE_PENDING_END,
    BOOTSTATE_PENDING_OFFSET,
    BOOTSTATE_REMAINING_ATTEMPTS_OFFSET,
    BOOTSTATE_SLOT_BYTES,
    BOOTSTATE_STATE_ROOT_END,
    BOOTSTATE_STATE_ROOT_OFFSET,
    BOOTSTATE_VERSION,
    BOOTSTORE_CAPACITY,
    BOOTSTORE_DIRECTORY_OFFSET,
    BOOTSTORE_ENTRY,
    BOOTSTORE_GENERATIONS_OFFSET,
    BOOTSTORE_RELEASES_OFFSET,
    BOOTSTORE_HEADER,
    BOOTSTORE_MAGIC,
    BOOTSTORE_VERSION,
    FABRIC_COMPONENT_DOMAIN,
    FABRIC_CONTRACT_KIND_CALL,
    FABRIC_CONTRACT_KIND_OPERATION,
    FABRIC_CONTRACT_KIND_STREAM,
    FABRIC_DIRECTION_CLIENT,
    FABRIC_DIRECTION_PUBLISH,
    FABRIC_DIRECTION_SERVER,
    FABRIC_DIRECTION_SUBSCRIBE,
    FABRIC_DURABILITY_RETAINED,
    FABRIC_DURABILITY_VOLATILE,
    FABRIC_GRANT_DOMAIN,
    FABRIC_GRAPH_CONTROL_MESSAGE_BYTES,
    FABRIC_GRAPH_HEADER,
    FABRIC_GRAPH_HEADER_BYTES,
    FABRIC_GRAPH_INTERPOSITION_ENTRY,
    FABRIC_GRAPH_INTERPOSITION_NONE,
    FABRIC_GRAPH_KERNEL_LOANS,
    FABRIC_GRAPH_KERNEL_MAPPINGS,
    FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
    FABRIC_GRAPH_LIMIT_CAPABILITY_SLOTS,
    FABRIC_GRAPH_LIMIT_EVENT_DEPTH,
    FABRIC_GRAPH_LIMIT_HISTORY_DEPTH,
    FABRIC_GRAPH_LIMIT_IN_FLIGHT,
    FABRIC_GRAPH_LIMIT_QUEUE_DEPTH,
    FABRIC_GRAPH_LIMIT_RETAINED_SAMPLES,
    FABRIC_GRAPH_LIMIT_RETRIES,
    FABRIC_GRAPH_LIMIT_SAMPLE_BYTES,
    FABRIC_GRAPH_MAGIC,
    FABRIC_GRAPH_PARTICIPANT_ENTRY,
    FABRIC_GRAPH_ROUTE_ENTRY,
    FABRIC_GRAPH_SCHEMA_ENTRY,
    FABRIC_GRAPH_VERSION,
    FABRIC_LIVELINESS_AUTOMATIC,
    FABRIC_LIVELINESS_MANUAL,
    FABRIC_RELIABILITY_BEST_EFFORT,
    FABRIC_RELIABILITY_RELIABLE,
    FABRIC_ROUTE_DOMAIN,
    FABRIC_VISIBILITY_GRAPH,
    FABRIC_VISIBILITY_PRIVATE,
    MAX_FABRIC_GRAPH_INGRESS_SOURCES,
    MAX_FABRIC_GRAPH_INTERPOSITION_HOPS,
    MAX_FABRIC_GRAPH_PARTICIPANTS,
    MAX_FABRIC_GRAPH_ROUTES,
    MAX_FABRIC_GRAPH_SCHEMAS,
    GENERATION_COMPONENT,
    GENERATION_DEPENDENCY,
    GENERATION_GRANT,
    GENERATION_HEADER,
    GENERATION_HEALTH,
    GENERATION_MAGIC,
    GENERATION_OBJECT,
    GENERATION_STATE,
    GENERATION_VERSION,
    KERNEL_ABI_VERSION,
    KERNEL_HEADER,
    KERNEL_MAGIC,
    KERNEL_PREFERRED_BASE,
    KERNEL_RELOCATION,
    KERNEL_SEGMENT,
    KERNEL_VERSION,
    MAX_COMPONENTS,
    MAX_DEPENDENCIES,
    MAX_GENERATION_BYTES,
    MAX_GRANTS,
    MAX_HEALTH_COMPONENTS,
    MAX_KERNEL_IMAGE_BYTES,
    MAX_KERNEL_RELOCATIONS,
    MAX_KERNEL_SEGMENTS,
    MAX_OBJECT_PAYLOAD_BYTES,
    MAX_RECOVERY_STATE_OBJECTS,
    RECOVERY_INDEX_HEADER,
    RECOVERY_INDEX_MAGIC,
    RECOVERY_INDEX_VERSION,
    RECOVERY_STATE_ENTRY,
    MAX_OBJECTS,
    MAX_STATES,
    MAX_STRING_BYTES,
    MAX_STRING_TABLE_BYTES,
    SHARED_BUFFER_BUDGET_ENTRY,
    SHARED_BUFFER_BUDGET_HEADER,
    SHARED_BUFFER_BUDGET_HEADER_BYTES,
    SHARED_BUFFER_BUDGET_ENTRY_BYTES,
    SHARED_BUFFER_BUDGET_MAGIC,
    SHARED_BUFFER_BUDGET_VERSION,
    MAX_SHARED_BUFFER_BUDGET_HOLDERS,
    SEGMENT_EXEC,
    SEGMENT_WRITE,
    bootstate_checksum,
    bootstore_checksum,
    generation_identity,
    sha256,
)
from interface_schema import InterfaceSchemaError, admit_interfaces, resolve_interface_paths
from release_trust import RELEASE_BYTES, build_release
from zutai_cli import STDLIB, binary

from harness import ROOT

SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
TARGET = "x86_64-qemu-virtio"
COMPONENTS_TARGET_DIR = Path(
    os.environ.get("CARGO_TARGET_DIR") or ROOT / "target" / "components"
)
PAGE_SIZE = 4096
KIND = {"kernel": 1, "bootstrap": 2, "component": 3, "resource": 4}
ROLE = {"init": 1, "service": 2, "driver": 3, "application": 4}
RIGHT = {
    "send": 1 << 0,
    "recv": 1 << 1,
    "exec": 1 << 3,
    "mapMmio": 1 << 4,
    "dmaPin": 1 << 5,
    "dmaRelease": 1 << 6,
    "irqAck": 1 << 7,
    "bufferWrite": 1 << 8,
    "bufferMap": 1 << 9,
    "blockRead": 1 << 10,
    "blockWrite": 1 << 11,
    "storeRead": 1 << 12,
    "storeWrite": 1 << 13,
    "healthConfirm": 1 << 14,
    "bootUpdate": 1 << 15,
    "spawn": 1 << 16,
    "endpointCreate": 1 << 17,
    "supervise": 1 << 18,
    "directoryRead": 1 << 19,
    "directoryWrite": 1 << 20,
    "directoryList": 1 << 21,
    "directoryDerive": 1 << 22,
    "inputRead": 1 << 23,
    "bufferCreate": 1 << 24,
    "bufferLoan": 1 << 25,
}
RIGHT_TRANSFER = 1 << 2
RIGHT_ALL = RIGHT_TRANSFER | sum(RIGHT.values())
MAX_SPAWN_BUDGET = 32
POLICY = {
    "immutable": 1,
    "ephemeral": 2,
    "preserve": 3,
    "snapshotBeforeUpgrade": 4,
    "discardOnRollback": 5,
}

IMAGE_MAGIC = b"SLIMECMP"
IMAGE_FORMAT_VERSION = 1
IMAGE_KERNEL_ABI = 1
IMAGE_HEADER = struct.Struct("<8sIIIIHHI")
IMAGE_SEGMENT = struct.Struct("<IIIIHH")
IMAGE_BASE = 0x400000
MAX_COMPONENT_IMAGE_BYTES = 16 * 1024 * 1024
MAX_STACK_BYTES = 1024 * 1024
DEFAULT_STACK_BYTES = 16384


def fail(message: str) -> None:
    raise SystemExit(message)


def align_up(value: int, alignment: int) -> int:
    return (value + alignment - 1) & ~(alignment - 1)


def load_manifest() -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    output = subprocess.run(
        [str(binary()), "json", str(SOURCE)],
        cwd=ROOT,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    return json.loads(output)

def recovery_manifest(manifest: dict) -> dict:
    recovery = copy.deepcopy(manifest)
    recovery["objects"] = [
        object_ for object_ in recovery["objects"] if object_["id"] in {manifest["kernelObject"], "sha256:init"}
    ] + [
        {"id": "sha256:recovery", "kind": "component", "size": 65536},
        {"id": "recovery-index", "kind": "resource", "size": 4096},
    ]
    recovery["components"] = [
        {"name": "init", "object": "sha256:init", "role": "init", "dependencies": [], "spawnBudget": 1, "commandProfile": []},
        {"name": "recovery", "object": "sha256:recovery", "role": "service", "dependencies": ["init"], "spawnBudget": 0, "commandProfile": []},
    ]
    recovery["grants"] = [
        {"name": "endpoint-factory", "source": "init", "target": "init", "rights": ["endpointCreate"], "transferable": False},
        {"name": "recovery-control", "source": "init", "target": "recovery", "rights": ["bootUpdate"], "transferable": False},
        {"name": "recovery-target", "source": "init", "target": "recovery", "rights": ["blockRead", "blockWrite"], "transferable": False},
    ]
    recovery["state"] = []
    # Recovery boots two components with no data fabric; leaving the graph in
    # would declare route authority for components that do not exist there.
    recovery.pop("fabricGraph", None)
    recovery["health"] = {"bootAttempts": 1, "requiredComponents": ["init", "recovery"]}
    return recovery


def binding_identity(name: str) -> bytes:
    encoded = name.encode("utf-8")
    return sha256(b"slime-state-binding-v1" + struct.pack("<H", len(encoded)) + encoded)


def holder_identity(name: str) -> bytes:
    """Stable per-holder identity, matching `boot_contracts::shared_buffer_budget`."""
    encoded = name.encode("utf-8")
    return sha256(
        b"slime-shared-buffer-holder-v1" + struct.pack("<H", len(encoded)) + encoded
    )


def build_shared_buffer_budget(holders: list[dict]) -> bytes:
    """Encode the C7.3 shared-buffer budget resource object.

    Entries are sorted by holder identity and must be unique: the decoder
    rejects an unsorted or duplicated table, so the sort here is part of the
    format rather than a convenience. A component absent from the table gets no
    quota at all (deny by default), so omission is meaningful, not a default.
    """
    if len(holders) > MAX_SHARED_BUFFER_BUDGET_HOLDERS:
        fail("shared-buffer budget exceeds holder bound")
    entries = []
    for holder in holders:
        identity = holder_identity(holder["holder"])
        limits = (
            holder["bytePages"],
            holder["bufferCount"],
            holder["mappingCount"],
            holder["loanCount"],
        )
        for limit in limits:
            if not isinstance(limit, int) or not 0 <= limit <= 0xFFFFFFFF:
                fail(f"shared-buffer budget: invalid limit for {holder['holder']}")
        entries.append((identity, *limits))
    entries.sort(key=lambda entry: entry[0])
    identities = {entry[0] for entry in entries}
    if len(identities) != len(entries):
        fail("shared-buffer budget: duplicate holder")
    total_len = SHARED_BUFFER_BUDGET_HEADER_BYTES + len(entries) * SHARED_BUFFER_BUDGET_ENTRY_BYTES
    header = SHARED_BUFFER_BUDGET_HEADER.pack(
        SHARED_BUFFER_BUDGET_MAGIC,
        SHARED_BUFFER_BUDGET_VERSION,
        SHARED_BUFFER_BUDGET_HEADER_BYTES,
        0,
        len(entries),
        total_len,
    )
    return header + b"".join(SHARED_BUFFER_BUDGET_ENTRY.pack(*entry) for entry in entries)


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
FABRIC_LIMIT_CEILINGS = {
    "routes": MAX_FABRIC_GRAPH_ROUTES,
    "ingressSources": MAX_FABRIC_GRAPH_INGRESS_SOURCES,
    "publishers": MAX_FABRIC_GRAPH_PARTICIPANTS,
    "subscribers": MAX_FABRIC_GRAPH_PARTICIPANTS,
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
    "bufferPages": FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
    "mappings": FABRIC_GRAPH_KERNEL_MAPPINGS,
    "loans": FABRIC_GRAPH_KERNEL_LOANS,
}


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

    ordered = sorted(zip(route_rows, routes), key=lambda pair: pair[0][0])
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
    # A sample larger than the control-message bound must travel as a C7
    # shared-buffer loan, so the page budget has to be able to hold one.
    if limits["sampleBytes"] > FABRIC_GRAPH_CONTROL_MESSAGE_BYTES and limits["bufferPages"] == 0:
        fail("fabric graph admits >MAX_MSG samples with no page budget")
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
    )
    return header + schema_records + route_records + participant_records + hop_records


def build_recovery_index(
    target_generation: bytes,
    generation_root: bytes,
    accepted_release_sequence: int,
    target_pci_bdf: int,
    state_entries: list[tuple[str, bytes, int]],
    state_first_lba: int,
    state_last_lba: int,
) -> bytes:
    if len(state_entries) > MAX_RECOVERY_STATE_OBJECTS:
        fail("recovery state closure exceeds bound")
    entries = sorted(
        ((binding_identity(name), identity, schema) for name, identity, schema in state_entries),
        key=lambda entry: entry[0],
    )
    if any(identity == bytes(32) or schema <= 0 for _, identity, schema in entries):
        fail("invalid recovery state entry")
    encoded = b"".join(
        RECOVERY_STATE_ENTRY.pack(binding, identity, schema, bytes(4))
        for binding, identity, schema in entries
    )
    state_root = sha256(
        b"".join(binding + identity + struct.pack("<I", schema) for binding, identity, schema in entries)
    )
    header = RECOVERY_INDEX_HEADER.pack(
        RECOVERY_INDEX_MAGIC,
        RECOVERY_INDEX_VERSION,
        RECOVERY_INDEX_HEADER.size,
        0,
        target_generation,
        generation_root,
        state_root,
        accepted_release_sequence,
        target_pci_bdf,
        len(entries),
        RECOVERY_INDEX_HEADER.size + len(encoded),
        state_first_lba,
        state_last_lba,
        bytes(4),
    )
    return header + encoded


def build_rust_components(
    generation_number: int,
    recovery: bool = False,
    candidate_identity: bytes | None = None,
) -> Path:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = str(generation_number)
    if candidate_identity is None and os.environ.get("SLIME_TRANSFER_RECEIVER") == "1":
        environment["SLIME_TRANSFER_RECEIVER"] = "1"
    else:
        environment.pop("SLIME_TRANSFER_RECEIVER", None)
    if candidate_identity is not None and os.environ.get("SLIME_TRANSFER_ACTIVATE") == "1":
        environment["SLIME_TRANSFER_ACTIVATE"] = "1"
    else:
        environment.pop("SLIME_TRANSFER_ACTIVATE", None)
    if recovery:
        environment["SLIME_RECOVERY_IMAGE"] = "1"
    if environment.get("SLIME_GENERATION_CMD_CHECK") == "1" and candidate_identity is not None:
        environment["SLIME_GENERATION_CANDIDATE"] = candidate_identity.hex()
    if recovery:
        target_name = "recovery"
    elif candidate_identity is None and os.environ.get("SLIME_TRANSFER_RECEIVER") == "1":
        target_name = f"generation-{generation_number}-transfer-receiver"
    elif candidate_identity is not None and os.environ.get("SLIME_TRANSFER_ACTIVATE") == "1":
        target_name = f"generation-{generation_number}-transfer-activate"
    else:
        target_name = f"generation-{generation_number}"
    target_dir = COMPONENTS_TARGET_DIR / target_name
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    subprocess.run(
        ["cargo", "build", "--release", "-p", "slime-components"],
        cwd=ROOT / "components",
        env=environment,
        check=True,
    )
    return target_dir / "x86_64-unknown-none" / "release"


def component_image(name: str, elf: Path, stack_bytes: int) -> bytes:
    data = elf.read_bytes()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail(f"{name}: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 2 or machine != 62:
        fail(f"{name}: not a static x86-64 executable")
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    _, phentsize, phnum = struct.unpack_from("<HHH", data, 52)
    segments: list[tuple[int, int, int, int, int]] = []
    for index in range(phnum):
        header = phoff + index * phentsize
        if header + phentsize > len(data):
            fail(f"{name}: truncated program header")
        p_type, p_flags = struct.unpack_from("<II", data, header)
        p_offset, p_vaddr, _, p_filesz, p_memsz = struct.unpack_from("<QQQQQ", data, header + 8)
        if p_type == 1 and p_memsz:
            segments.append((p_vaddr, p_offset, p_filesz, p_memsz, p_flags))
    segments.sort()
    if not 1 <= len(segments) <= 16 or segments[0][0] != IMAGE_BASE or entry < IMAGE_BASE:
        fail(f"{name}: invalid component load layout")
    records = bytearray()
    payload = bytearray()
    previous_end = 0
    entry_offset = entry - IMAGE_BASE
    entry_ok = False
    total_pages = 0
    for vaddr, offset, filesz, memsz, elf_flags in segments:
        if filesz > memsz or vaddr % PAGE_SIZE or vaddr < previous_end or offset + filesz > len(data):
            fail(f"{name}: invalid or overlapping segment")
        flags = (SEGMENT_EXEC if elf_flags & 1 else 0) | (SEGMENT_WRITE if elf_flags & 2 else 0)
        if flags == SEGMENT_EXEC | SEGMENT_WRITE:
            fail(f"{name}: writable executable segment")
        relative = vaddr - IMAGE_BASE
        entry_ok |= bool(flags & SEGMENT_EXEC and relative <= entry_offset < relative + memsz)
        records += IMAGE_SEGMENT.pack(relative, memsz, len(payload), filesz, flags, 0)
        payload += data[offset : offset + filesz]
        previous_end = vaddr + memsz
        total_pages += -(-memsz // PAGE_SIZE)
    if not entry_ok or total_pages * PAGE_SIZE > MAX_COMPONENT_IMAGE_BYTES:
        fail(f"{name}: invalid entry or image size")
    return IMAGE_HEADER.pack(IMAGE_MAGIC, IMAGE_FORMAT_VERSION, IMAGE_HEADER.size, IMAGE_KERNEL_ABI, entry_offset, len(segments), 0, stack_bytes) + records + payload


def parse_elf64(data: bytes) -> tuple[int, list[tuple[int, int, int, int, int]], list[tuple[int, int]]]:
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail("kernel: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 3 or machine != 62:
        fail("kernel: expected x86-64 PIE ELF")
    entry, phoff, shoff = struct.unpack_from("<QQQ", data, 24)
    _, phentsize, phnum, shentsize, shnum = struct.unpack_from("<HHHHH", data, 52)
    segments: list[tuple[int, int, int, int, int]] = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        if offset + phentsize > len(data):
            fail("kernel: truncated program header")
        p_type, p_flags = struct.unpack_from("<II", data, offset)
        p_offset, p_vaddr, _, p_filesz, p_memsz = struct.unpack_from("<QQQQQ", data, offset + 8)
        if p_type == 1 and p_memsz:
            segments.append((p_vaddr, p_offset, p_filesz, p_memsz, p_flags))
    segments.sort()
    relocations: list[tuple[int, int]] = []
    for index in range(shnum):
        offset = shoff + index * shentsize
        if offset + shentsize > len(data):
            fail("kernel: truncated section header")
        sh_type = struct.unpack_from("<I", data, offset + 4)[0]
        sh_offset, sh_size = struct.unpack_from("<QQ", data, offset + 24)
        sh_entsize = struct.unpack_from("<Q", data, offset + 56)[0]
        if sh_type != 4 or sh_size == 0:  # SHT_RELA
            continue
        if sh_entsize != 24 or sh_offset + sh_size > len(data):
            fail("kernel: malformed RELA section")
        for rela_offset in range(sh_offset, sh_offset + sh_size, sh_entsize):
            target, info, addend = struct.unpack_from("<QQq", data, rela_offset)
            if info & 0xFFFF_FFFF != 8 or info >> 32 != 0:
                fail("kernel: unsupported relocation")
            relocations.append((target, addend))
    relocations.sort()
    return entry, segments, relocations


def kernel_image(path: Path) -> bytes:
    data = path.read_bytes()
    entry, segments, relocations = parse_elf64(data)
    if not 1 <= len(segments) <= MAX_KERNEL_SEGMENTS or len(relocations) > MAX_KERNEL_RELOCATIONS:
        fail("kernel: segment or relocation count exceeds bound")
    if not segments or segments[0][0] != KERNEL_PREFERRED_BASE or entry < KERNEL_PREFERRED_BASE:
        fail("kernel: unexpected preferred base")
    records = bytearray()
    payload = bytearray()
    previous_end = KERNEL_PREFERRED_BASE
    entry_ok = False
    writable: list[tuple[int, int]] = []
    image_end = KERNEL_PREFERRED_BASE
    table_bytes = KERNEL_HEADER.size + len(segments) * KERNEL_SEGMENT.size + len(relocations) * KERNEL_RELOCATION.size
    payload_cursor = table_bytes
    for vaddr, file_offset, file_len, mem_len, elf_flags in segments:
        if vaddr % PAGE_SIZE or vaddr < previous_end or file_len > mem_len or file_offset + file_len > len(data):
            fail("kernel: invalid or overlapping segment")
        flags = (SEGMENT_EXEC if elf_flags & 1 else 0) | (SEGMENT_WRITE if elf_flags & 2 else 0)
        if flags == SEGMENT_EXEC | SEGMENT_WRITE:
            fail("kernel: writable executable segment")
        relative = vaddr - KERNEL_PREFERRED_BASE
        entry_ok |= bool(flags & SEGMENT_EXEC and vaddr <= entry < vaddr + mem_len)
        if flags & SEGMENT_WRITE:
            writable.append((relative, relative + mem_len))
        records += KERNEL_SEGMENT.pack(relative, mem_len, payload_cursor, file_len, flags, 0)
        payload += data[file_offset : file_offset + file_len]
        payload_cursor += file_len
        previous_end = vaddr + mem_len
        image_end = max(image_end, previous_end)
    if not entry_ok or image_end - KERNEL_PREFERRED_BASE > MAX_KERNEL_IMAGE_BYTES:
        fail("kernel: entry or image footprint invalid")
    relocation_records = bytearray()
    for target, addend in relocations:
        if target < KERNEL_PREFERRED_BASE or target % 8:
            fail("kernel: relocation target invalid")
        relative = target - KERNEL_PREFERRED_BASE
        if not any(start <= relative and relative + 8 <= end for start, end in writable):
            fail("kernel: relocation target outside writable segment")
        absolute_addend = addend if addend >= KERNEL_PREFERRED_BASE else (1 << 64) + addend
        if not KERNEL_PREFERRED_BASE <= absolute_addend <= align_up(image_end, PAGE_SIZE):
            fail("kernel: relocation addend outside image")
        signed_addend = absolute_addend - (1 << 64) if absolute_addend >= 1 << 63 else absolute_addend
        relocation_records += KERNEL_RELOCATION.pack(relative, signed_addend)
    image_len = table_bytes + len(payload)
    if image_len > MAX_KERNEL_IMAGE_BYTES:
        fail("kernel: image bytes exceed bound")
    header = KERNEL_HEADER.pack(
        KERNEL_MAGIC, KERNEL_VERSION, KERNEL_HEADER.size, KERNEL_ABI_VERSION, 0,
        KERNEL_PREFERRED_BASE, entry - KERNEL_PREFERRED_BASE, len(segments), len(relocations),
        table_bytes, image_len,
    )
    return header + records + relocation_records + payload


def validate_interface_schemas(entries: object) -> list:
    """Admit the manifest's declared interface set and return it compiled.

    The C8.2 fabric graph is built from these exact admitted interfaces, so a
    route can only ever name a schema that already passed C8.1 normalization,
    identity, tag-collision, and bounds admission.
    """
    try:
        paths = resolve_interface_paths(entries)
        return admit_interfaces(paths)
    except InterfaceSchemaError as error:
        fail(str(error))
        raise


def unique_sorted(items: list[dict], key: str, label: str) -> list[dict]:
    values = [item[key] for item in items]
    if len(set(values)) != len(values):
        fail(f"{label} must be unique")
    return sorted(items, key=lambda item: item[key])


def validate_acyclic(components: list[dict]) -> None:
    graph = {component["name"]: component["dependencies"] for component in components}
    for name, dependencies in graph.items():
        if name in dependencies or len(set(dependencies)) != len(dependencies):
            fail(f"component {name}: invalid dependencies")
        for dependency in dependencies:
            if dependency not in graph:
                fail(f"component {name}: missing dependency {dependency}")
    active: set[str] = set()
    complete: set[str] = set()
    def visit(name: str) -> None:
        if name in complete: return
        if name in active: fail("component dependency cycle")
        active.add(name)
        for dependency in graph[name]: visit(dependency)
        active.remove(name); complete.add(name)
    for name in graph: visit(name)


def build_generation(manifest: dict, payloads: dict[str, bytes], parent: bytes | None, number: int) -> bytes:
    objects = unique_sorted(manifest["objects"], "id", "object ids")
    components = unique_sorted(manifest["components"], "name", "component names")
    grants = sorted(manifest["grants"], key=lambda grant: (grant["name"], grant["source"], grant["target"]))
    states = unique_sorted(manifest["state"], "name", "state names")
    if len({(grant["name"], grant["source"], grant["target"]) for grant in grants}) != len(grants): fail("grant identities must be unique")
    if not 1 <= len(objects) <= MAX_OBJECTS or not 1 <= len(components) <= MAX_COMPONENTS or len(grants) > MAX_GRANTS or len(states) > MAX_STATES:
        fail("manifest count exceeds bound")
    validate_acyclic(components)
    object_index = {obj["id"]: index for index, obj in enumerate(objects)}
    component_index = {component["name"]: index for index, component in enumerate(components)}
    if manifest["target"] != TARGET: fail("unexpected target")
    if object_index.get(manifest["kernelObject"]) is None or objects[object_index[manifest["kernelObject"]]]["kind"] != "kernel": fail("kernelObject must name kernel")
    bootstrap = component_index.get(manifest["bootstrapComponent"])
    if bootstrap is None or components[bootstrap]["role"] != "init": fail("bootstrapComponent must name init")

    strings = bytearray()
    offsets: dict[str, int] = {}
    def string_offset(value: str) -> int:
        if value in offsets: return offsets[value]
        encoded = value.encode("utf-8")
        if len(encoded) > MAX_STRING_BYTES: fail("string exceeds bound")
        offset = len(strings); strings.extend(struct.pack("<H", len(encoded))); strings.extend(encoded); offsets[value] = offset
        if len(strings) > MAX_STRING_TABLE_BYTES: fail("string table exceeds bound")
        return offset

    target_offset = string_offset(manifest["target"])
    object_records = bytearray()
    component_records = bytearray()
    dependency_records = bytearray()
    grant_records = bytearray()
    state_records = bytearray()
    health_records = bytearray()
    blobs = bytearray()
    payload_start = (
        GENERATION_HEADER.size + len(objects) * GENERATION_OBJECT.size + len(components) * GENERATION_COMPONENT.size
        + sum(len(component["dependencies"]) for component in components) * GENERATION_DEPENDENCY.size
        + len(grants) * GENERATION_GRANT.size + len(states) * GENERATION_STATE.size
        + len(manifest["health"]["requiredComponents"]) * GENERATION_HEALTH.size
    )
    # Strings are visited canonically before payload offsets are frozen.
    for obj in objects: string_offset(obj["id"])
    for component in components: string_offset(component["name"])
    for grant in grants: string_offset(grant["name"])
    for state in states: string_offset(state["name"])
    payload_start += len(strings)
    for obj in objects:
        if obj["kind"] not in KIND: fail(f"unsupported object kind {obj['kind']}")
        payload = payloads.get(obj["id"])
        if payload is None: fail(f"missing payload for {obj['id']}")
        if len(payload) > MAX_OBJECT_PAYLOAD_BYTES: fail(f"payload too large for {obj['id']}")
        object_records += GENERATION_OBJECT.pack(string_offset(obj["id"]), KIND[obj["kind"]], payload_start + len(blobs), len(payload), sha256(payload))
        blobs += payload
    dependency_count = 0
    for component in components:
        obj = object_index.get(component["object"])
        if obj is None: fail(f"component {component['name']}: missing object")
        if component["role"] not in ROLE: fail("unsupported component role")
        spawn_budget = component["spawnBudget"]
        if not isinstance(spawn_budget, int) or not 0 <= spawn_budget <= MAX_SPAWN_BUDGET:
            fail(f"component {component['name']}: invalid spawn budget")
        dependencies = sorted(component["dependencies"])
        start = dependency_count
        for dependency in dependencies:
            dependency_records += GENERATION_DEPENDENCY.pack(component_index[dependency])
            dependency_count += 1
        component_records += GENERATION_COMPONENT.pack(
            string_offset(component["name"]), obj, ROLE[component["role"]], start,
            len(dependencies), spawn_budget,
        )
    if dependency_count > MAX_DEPENDENCIES: fail("dependency count exceeds bound")
    for grant in grants:
        source = component_index.get(grant["source"])
        target = component_index.get(grant["target"])
        if source is None or target is None: fail(f"grant endpoint missing: {grant['name']}")
        rights = 0
        for right in grant["rights"]:
            if right not in RIGHT: fail(f"unsupported right {right}")
            rights |= RIGHT[right]
        transferable = int(bool(grant["transferable"])); rights |= RIGHT_TRANSFER if transferable else 0
        if rights == 0 or rights & ~RIGHT_ALL: fail(f"invalid rights for {grant['name']}")
        grant_records += GENERATION_GRANT.pack(string_offset(grant["name"]), source, target, rights, transferable)
    for state in states:
        owner = component_index.get(state["owner"])
        if owner is None or state["schemaVersion"] <= 0 or state["policy"] not in POLICY: fail(f"invalid state {state['name']}")
        state_records += GENERATION_STATE.pack(string_offset(state["name"]), owner, state["schemaVersion"], POLICY[state["policy"]])
    health = manifest["health"]
    required = sorted(health["requiredComponents"])
    if health["bootAttempts"] <= 0 or len(required) > MAX_HEALTH_COMPONENTS or len(set(required)) != len(required): fail("invalid health policy")
    for component in required:
        if component not in component_index: fail(f"missing health component {component}")
        health_records += GENERATION_HEALTH.pack(component_index[component])

    object_offset = GENERATION_HEADER.size
    component_offset = object_offset + len(object_records)
    dependency_offset = component_offset + len(component_records)
    grant_offset = dependency_offset + len(dependency_records)
    state_offset = grant_offset + len(grant_records)
    health_offset = state_offset + len(state_records)
    string_table_offset = health_offset + len(health_records)
    actual_payload_offset = string_table_offset + len(strings)
    if actual_payload_offset != payload_start: fail("internal payload offset mismatch")
    total_len = actual_payload_offset + len(blobs)
    if total_len > MAX_GENERATION_BYTES: fail("generation exceeds bound")
    parent_bytes = parent or bytes(32)
    header = GENERATION_HEADER.pack(
        GENERATION_MAGIC, GENERATION_VERSION, GENERATION_HEADER.size, 0, bytes(32), number, parent_bytes,
        target_offset, object_index[manifest["kernelObject"]], bootstrap, health["bootAttempts"], len(objects), len(components),
        dependency_count, len(grants), len(states), len(required), object_offset, component_offset, dependency_offset,
        grant_offset, state_offset, health_offset, string_table_offset, len(strings), actual_payload_offset, total_len,
    )
    generation = bytearray(
        header
        + object_records
        + component_records
        + dependency_records
        + grant_records
        + state_records
        + health_records
        + strings
        + blobs
    )
    identity = generation_identity(generation)
    generation[24:56] = identity
    return bytes(generation)


def encode_bootstate(
    sequence: int,
    known_good: bytes,
    generation_root: bytes,
    pending: bytes | None = None,
    accepted_release_sequence: int = 0,
    remaining_attempts: int = 0,
    state_root: bytes | None = None,
) -> bytes:
    slot = bytearray(BOOTSTATE_SLOT_BYTES)
    slot[:8] = BOOTSTATE_MAGIC
    struct.pack_into("<IIQQ", slot, 8, BOOTSTATE_VERSION, BOOTSTATE_SLOT_BYTES, 0, sequence)
    slot[BOOTSTATE_KNOWN_GOOD_OFFSET:BOOTSTATE_KNOWN_GOOD_END] = known_good
    if pending is not None:
        slot[BOOTSTATE_PENDING_OFFSET:BOOTSTATE_PENDING_END] = pending
    struct.pack_into("<II", slot, BOOTSTATE_REMAINING_ATTEMPTS_OFFSET, remaining_attempts, 0)
    slot[BOOTSTATE_GENERATION_ROOT_OFFSET:BOOTSTATE_GENERATION_ROOT_END] = generation_root
    slot[BOOTSTATE_STATE_ROOT_OFFSET:BOOTSTATE_STATE_ROOT_END] = state_root or sha256(b"")
    struct.pack_into("<Q", slot, BOOTSTATE_ACCEPTED_RELEASE_SEQUENCE_OFFSET, accepted_release_sequence)
    slot[BOOTSTATE_CHECKSUM_OFFSET:BOOTSTATE_CHECKSUM_END] = bootstate_checksum(slot)
    return bytes(slot)


def build_bootstore(generations: list[bytes]) -> bytes:
    release_sequences = [index + 1 for index in range(len(generations))]
    pending_sequence = os.environ.get("SLIME_PENDING_RELEASE_SEQUENCE")
    if pending_sequence is not None:
        release_sequences[-1] = int(pending_sequence)
    entries = sorted(
        ((generation[24:56], generation, build_release(generation, release_sequences[index])) for index, generation in enumerate(generations)),
        key=lambda item: item[0],
    )
    generation_root = sha256(b"".join(identity for identity, _, _ in entries))
    known_good = generations[-1][24:56]
    pending = None
    remaining_attempts = 0
    if os.environ.get("SLIME_KNOWN_GOOD_FIRST") == "1":
        known_good = generations[0][24:56]
    if os.environ.get("SLIME_PENDING_GENERATION") == "1":
        known_good = generations[0][24:56]
        pending = generations[-1][24:56]
        remaining_attempts = int(os.environ.get("SLIME_PENDING_ATTEMPTS") or "2")
    image = bytearray(BOOTSTORE_CAPACITY)
    accepted_sequence = int(
        os.environ.get("SLIME_ACCEPTED_RELEASE_SEQUENCE")
        or (1 if known_good == generations[0][24:56] else len(generations))
    )
    image[:BOOTSTATE_SLOT_BYTES] = encode_bootstate(
        2,
        known_good,
        generation_root,
        pending=pending,
        accepted_release_sequence=accepted_sequence,
        remaining_attempts=remaining_attempts,
    )
    image[BOOTSTATE_SLOT_BYTES : BOOTSTATE_SLOT_BYTES * 2] = encode_bootstate(
        1,
        known_good,
        generation_root,
        pending=pending,
        accepted_release_sequence=accepted_sequence,
        remaining_attempts=remaining_attempts,
    )
    directory = bytearray()
    release_cursor = BOOTSTORE_RELEASES_OFFSET
    generation_cursor = BOOTSTORE_GENERATIONS_OFFSET
    for identity, generation, release in entries:
        release_cursor = align_up(release_cursor, RELEASE_BYTES)
        generation_cursor = align_up(generation_cursor, PAGE_SIZE)
        directory += BOOTSTORE_ENTRY.pack(
            identity,
            generation_cursor,
            len(generation),
            release_cursor,
            len(release),
        )
        image[release_cursor : release_cursor + len(release)] = release
        image[generation_cursor : generation_cursor + len(generation)] = generation
        release_cursor += len(release)
        generation_cursor += len(generation)
    if release_cursor > BOOTSTORE_GENERATIONS_OFFSET or generation_cursor > BOOTSTORE_CAPACITY:
        fail("boot store capacity exceeded")
    header = BOOTSTORE_HEADER.pack(
        BOOTSTORE_MAGIC,
        BOOTSTORE_VERSION,
        BOOTSTORE_HEADER.size,
        0,
        len(entries),
        0,
        len(directory),
        BOOTSTORE_CAPACITY,
        bytes(32),
    )
    image[BOOTSTORE_DIRECTORY_OFFSET : BOOTSTORE_DIRECTORY_OFFSET + len(header)] = header
    image[
        BOOTSTORE_DIRECTORY_OFFSET
        + len(header) : BOOTSTORE_DIRECTORY_OFFSET
        + len(header)
        + len(directory)
    ] = directory
    checksum = bootstore_checksum(image)
    image[BOOTSTORE_DIRECTORY_OFFSET + 48 : BOOTSTORE_DIRECTORY_OFFSET + 80] = checksum
    return bytes(image)


def main() -> None:
    if len(sys.argv) != 3: fail("usage: build-generation.py <kernel-elf> <output-dir>")
    kernel = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2]).resolve()
    manifest = load_manifest()
    if manifest["formatVersion"] != 1: fail("unsupported source formatVersion")
    interfaces = validate_interface_schemas(manifest["interfaceSchemas"])
    output.mkdir(parents=True, exist_ok=True)
    policy_number = int(os.environ.get("SLIME_GENERATION_NUMBER") or manifest["generation"])
    # Generation 1 is the known-good baseline: its components must carry their own
    # generation number (1) so the generation-manager runs the known-good path,
    # not the pending/failing path baked for `policy_number`. Booting the two
    # generations from one build (rollback/bootstate) otherwise makes the
    # known-good recovery boot report the pending generation's unhealthy status.
    # The transfer receiver is the exception: there generation 1 *is* the
    # policy-numbered receiver generation, built with the receiver flag.
    generation1_number = policy_number if os.environ.get("SLIME_TRANSFER_RECEIVER") == "1" else 1
    generation1_components = build_rust_components(generation1_number, candidate_identity=None)
    payloads: dict[str, bytes] = {manifest["kernelObject"]: kernel_image(kernel)}
    object_by_id = {obj["id"]: obj for obj in manifest["objects"]}
    if "shared-buffer-budget" in object_by_id:
        payloads["shared-buffer-budget"] = build_shared_buffer_budget(
            manifest.get("sharedBufferBudget", [])
        )
    if "fabric-graph" in object_by_id:
        graph = manifest.get("fabricGraph")
        if graph is None:
            fail("fabric-graph object declared without a fabricGraph manifest section")
        payloads["fabric-graph"] = build_fabric_graph(
            graph,
            {component["name"] for component in manifest["components"]},
            interfaces,
        )
    elif manifest.get("fabricGraph") is not None:
        fail("fabricGraph declared without a fabric-graph resource object")
    for component in manifest["components"]:
        stack = component.get("stackBytes", DEFAULT_STACK_BYTES)
        if not isinstance(stack, int) or stack <= 0 or stack % PAGE_SIZE or stack > MAX_STACK_BYTES:
            fail(f"component {component['name']}: invalid stack")
        if component["object"] not in object_by_id: fail(f"component {component['name']}: missing object")
        payloads[component["object"]] = component_image(component["name"], generation1_components / component["name"], stack)
    generation1 = build_generation(manifest, payloads, None, 1)
    generation2_components = build_rust_components(policy_number, candidate_identity=generation1[24:56])
    for component in manifest["components"]:
        stack = component.get("stackBytes", DEFAULT_STACK_BYTES)
        if not isinstance(stack, int) or stack <= 0 or stack % PAGE_SIZE or stack > MAX_STACK_BYTES:
            fail(f"component {component['name']}: invalid stack")
        payloads[component["object"]] = component_image(component["name"], generation2_components / component["name"], stack)
    parent_override = os.environ.get("SLIME_GENERATION_PARENT")
    generation2_parent = bytes.fromhex(parent_override) if parent_override else generation1[24:56]
    generation2 = build_generation(manifest, payloads, generation2_parent, policy_number)
    recovery_components = build_rust_components(5, recovery=True)
    recovery = recovery_manifest(manifest)
    state_first_lba = int(os.environ.get("SLIME_RECOVERY_STATE_FIRST_LBA") or BOOTSTORE_CAPACITY // 512)
    state_last_lba = int(os.environ.get("SLIME_RECOVERY_STATE_LAST_LBA") or state_first_lba + 127)
    target_bdf = int(os.environ.get("SLIME_RECOVERY_TARGET_BDF") or "0x000018", 0)
    state_entries: list[tuple[str, bytes, int]] = []
    generation_root = sha256(b"".join(sorted((generation1[24:56], generation2[24:56]))))
    recovery_payloads = {
        manifest["kernelObject"]: payloads[manifest["kernelObject"]],
        "sha256:init": component_image("init", recovery_components / "init", DEFAULT_STACK_BYTES),
        "sha256:recovery": component_image("recovery", recovery_components / "recovery", DEFAULT_STACK_BYTES),
        "recovery-index": build_recovery_index(
            generation2[24:56],
            generation_root,
            2,
            target_bdf,
            state_entries,
            state_first_lba,
            state_last_lba,
        ),
    }
    recovery_generation = build_generation(recovery, recovery_payloads, None, 5)
    recovery_bootstore = build_bootstore([recovery_generation])
    bootstore = build_bootstore([generation1, generation2])
    (output / "generation-1.bin").write_bytes(generation1)
    (output / "generation-2.bin").write_bytes(generation2)
    (output / "generation.bin").write_bytes(generation2)
    (output / "boot-store.bin").write_bytes(bootstore)
    (output / "recovery-generation.bin").write_bytes(recovery_generation)
    (output / "recovery-boot-store.bin").write_bytes(recovery_bootstore)
    print(f"Built generation 1 {generation1[24:56].hex()}")
    print(f"Built generation 2 {generation2[24:56].hex()} parent={generation1[24:56].hex()}")
    print(f"Built boot-store.bin ({len(bootstore)} bytes)")
    print(f"Built recovery generation {recovery_generation[24:56].hex()}")


if __name__ == "__main__":
    main()
