#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))
# This script's own directory, for sibling modules when imported by host checks.
_sys.path.insert(0, str(_Path(__file__).resolve().parent))

import copy
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
    FABRIC_GRAPH_KERNEL_SHARED_BUFFERS,
    FABRIC_GRAPH_LIMIT_CAPABILITY_SLOTS,
    FABRIC_GRAPH_LIMIT_EVENT_DEPTH,
    FABRIC_GRAPH_LIMIT_HISTORY_DEPTH,
    FABRIC_GRAPH_LIMIT_IN_FLIGHT,
    FABRIC_GRAPH_LIMIT_BUFFERS,
    FABRIC_GRAPH_LIMIT_QUEUE_DEPTH,
    FABRIC_GRAPH_LIMIT_RETAINED_SAMPLES,
    FABRIC_GRAPH_LIMIT_RETRIES,
    FABRIC_GRAPH_LIMIT_SAMPLE_BYTES,
    FABRIC_GRAPH_MAGIC,
    FABRIC_GRAPH_CHANNEL_QUEUE_DEPTH,
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
    COMPONENT_DEFAULT_STACK_BYTES,
    COMPONENT_IMAGE_ELF_HEADER_LEN,
    COMPONENT_IMAGE_ELF_MAGIC,
    COMPONENT_IMAGE_ELF_VERSION,
    COMPONENT_IMAGE_HEADER,
    COMPONENT_IMAGE_KERNEL_ABI,
    COMPONENT_IMAGE_MAGIC,
    COMPONENT_IMAGE_MAX_SEGMENTS,
    COMPONENT_IMAGE_SEGMENT,
    COMPONENT_IMAGE_VERSION,
    COMPONENT_MAX_STACK_BYTES,
    COMPONENT_SEGMENT_FLAG_EXEC,
    COMPONENT_SEGMENT_FLAG_WRITE,
    MAX_COMPONENT_IMAGE_BYTES,
    GENERATION_BINDING,
    GENERATION_DEPENDENCY,
    GENERATION_EXECUTABLE,
    GENERATION_INSTANCE,
    GENERATION_GRANT,
    GENERATION_HEADER,
    GENERATION_HEALTH,
    GENERATION_MAGIC,
    GENERATION_OBJECT,
    GENERATION_STATE,
    GENERATION_PROCESS,
    GENERATION_THREAD,
    GENERATION_KERNEL_OBJECT,
    GENERATION_MAPPING,
    GENERATION_CAP_BINDING,
    GENERATION_SERVICE_BINDING,
    GENERATION_SCHEDULE,
    GENERATION_FAULT_POLICY,
    GENERATION_SPAWN_TEMPLATE,
    GENERATION_MINTED_BINDING,
    GENERATION_RESOURCE_QUOTA,
    GENERATION_VERSION,
    TARGET_PROFILES_BY_NAME,
    TargetProfile,
    MAX_BINDINGS,
    MAX_DEPENDENCIES,
    MAX_EXECUTABLES,
    MAX_INSTANCES,
    MAX_GENERATION_BYTES,
    MAX_GRANTS,
    MAX_HEALTH_INSTANCES,
    MAX_OBJECT_PAYLOAD_BYTES,
    MAX_OBJECTS,
    MAX_STATES,
    MAX_STRING_BYTES,
    MAX_STRING_TABLE_BYTES,
    MAX_PROCESSES,
    MAX_THREADS,
    MAX_KERNEL_OBJECTS,
    MAX_MAPPINGS,
    MAX_CAP_BINDINGS,
    MAX_SERVICE_BINDINGS,
    MAX_SCHEDULES,
    MAX_FAULT_POLICIES,
    MAX_SPAWN_TEMPLATES,
    MAX_MINTED_BINDINGS,
    MAX_RESOURCE_QUOTAS,
    SHARED_BUFFER_BUDGET_ENTRY,
    SHARED_BUFFER_BUDGET_HEADER,
    SHARED_BUFFER_BUDGET_HEADER_BYTES,
    SHARED_BUFFER_BUDGET_ENTRY_BYTES,
    SHARED_BUFFER_BUDGET_MAGIC,
    SHARED_BUFFER_BUDGET_VERSION,
    MAX_SHARED_BUFFER_BUDGET_HOLDERS,
    MAX_NORMALIZED_SCHEMAS,
    MAX_NORMALIZED_SCHEMAS_ARTIFACT_BYTES,
    NORMALIZED_SCHEMAS_ENTRY,
    NORMALIZED_SCHEMAS_HEADER,
    NORMALIZED_SCHEMAS_HEADER_BYTES,
    NORMALIZED_SCHEMAS_MAGIC,
    NORMALIZED_SCHEMAS_VERSION,
    bootstate_checksum,
    bootstore_checksum,
    generation_identity,
    sha256,
)
from boot_layout import build_boot_layout, render_rust as render_boot_layout_rust
from interface_schema import InterfaceSchemaError, admit_interfaces, resolve_interface_paths
from release_trust import RELEASE_BYTES, build_release
from zutai_cli import STDLIB, binary

from harness import ROOT

SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
# P5.2: the `aarch64-sel4-qemu-virt` graph is a sibling manifest rather than a
# boot profile of `valid.zti`, because `resolve_boot_profile` narrows by
# subtraction and naming a component in a new profile would drop it from
# `default`, changing the frozen product generation. See `sel4.md` beside it.
SEL4_SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4.zti"
SEL4_TARGET_PROFILE = "aarch64-sel4-qemu-virt"
# P5.3.1: a second seL4 graph, for the channel plane. It cannot be folded into
# `sel4.zti` because `init.rs` selects its scenario with `option_env!`, which is
# resolved at compile time -- one component build cannot serve two gates. Keyed
# by name rather than by target, because both graphs are built for the same
# target profile and it is the *graph* that differs. See `sel4-channel.md`.
SEL4_MANIFESTS = {
    "sel4": SEL4_SOURCE,
    "sel4-channel": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-channel.zti",
    "sel4-loan": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-loan.zti",
    "sel4-spawn": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-spawn.zti",
    "sel4-sample": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-sample.zti",
    "sel4-stream": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-stream.zti",
    "sel4-supervision": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-supervision.zti",
    "sel4-reclamation": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-reclamation.zti",
    "sel4-crossing": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-crossing.zti",
    "sel4-call": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-call.zti",
    "sel4-qos": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-qos.zti",
    "sel4-operation": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-operation.zti",
    "sel4-visibility": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-visibility.zti",
    "sel4-boot": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-boot.zti",
    "sel4-storage": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-storage.zti",
    "sel4-store": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-store.zti",
    "sel4-rollback": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-rollback.zti",
    "sel4-recovery": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-recovery.zti",
    "sel4-generation": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-generation.zti",
    "sel4-directory": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-directory.zti",
    "sel4-filesystem": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-filesystem.zti",
    "sel4-dango": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-dango.zti",
    "sel4-input": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-input.zti",
    "sel4-powerbox": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-powerbox.zti",
    "sel4-transfer": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-transfer.zti",
}
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

DEFAULT_FABRIC_PROFILE = "default"
# B11: the boot profile carrying the scaffolding the pre-C8.10 gate families
# exercise. `default` is the product boot and declares none of it.
TEST_BOOT_PROFILE = "test"
VISIBILITY_FABRIC_PROFILE = "visibility"
UNIFIED_FABRIC_PROFILE = "unified"
FABRIC_FIRST_CONTROL_SLOT = 2
FABRIC_COPY_PAGES = 2
FABRIC_FRAME_CAPACITY = 32
FABRIC_STREAM_CONTROL_GRANTS = (
    "fabric-publisher-control",
    "fabric-subscriber-control",
    "fabric-intruder-control",
    "fabric-publisher-b-control",
    "fabric-subscriber-b-control",
)
# C8.10's full-graph boot stream plane, declared in full rather than derived
# from the tuple above.
#
# Two things change together here. The unauthorized probe, the declared
# interposition proxy, and the filtered-introspection client join as three
# distinct component identities; `fabric-intruder` — which carried all three
# roles at once behind an env switch — drops out, so the new boot path is free
# of it without disturbing `fabric_visibility_check`, whose markers and source
# assertions still name it.
#
# Declared *per profile* because the stream plane's supervision slots are
# numbered `FIRST_CONTROL_SLOT + len(controls) + index`. Lengthening one shared
# list would renumber the subscriber supervision handles that the C8.3-C8.8
# gates' `launch_fabric_graph` grants positionally, and each of those gates
# would then read a control endpoint where it expects a supervision handle.
# Every earlier profile keeps its layout byte-for-byte.
FABRIC_BOOT_STREAM_CONTROL_GRANTS = (
    "fabric-publisher-control",
    "fabric-subscriber-control",
    "fabric-publisher-b-control",
    "fabric-subscriber-b-control",
    "fabric-observer-control",
    "fabric-probe-control",
    "fabric-proxy-control",
)
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
# C8.10 bounded route workers: whole routes, partitioned so no worker's live
# wake sources exceed one `SYS_WAIT` set. Declared here rather than inferred so
# the partition is a generation fact the resolver validates, not a runtime
# heuristic that could silently drift past the kernel bound.
FABRIC_ROUTE_WORKERS = (
    ("stream", ("telemetry", "diagnostics")),
    ("call", ("parameters",)),
    ("operation", ("navigation", "nav-backup")),
)
# How each worker shape's peak `SYS_WAIT` set is established.
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


def fail(message: str) -> None:
    raise SystemExit(message)


def align_up(value: int, alignment: int) -> int:
    return (value + alignment - 1) & ~(alignment - 1)


def manifest_source() -> Path:
    """Which generation manifest this build encodes.

    The target profile selects the family, so the target and the graph it
    declares cannot be chosen independently and then disagree. Within the seL4
    family `SLIME_SEL4_MANIFEST` names which graph, because P5.3.1 adds a second
    one built for the same target; absent, it is the P5.2 graph, so every
    existing caller keeps its behaviour without passing anything.
    """
    if os.environ.get("SLIME_TARGET_PROFILE") == SEL4_TARGET_PROFILE:
        name = os.environ.get("SLIME_SEL4_MANIFEST", "sel4")
        source = SEL4_MANIFESTS.get(name)
        if source is None:
            fail(f"unknown seL4 manifest {name!r}")
        return source
    return SOURCE


def load_manifest() -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    output = subprocess.run(
        [str(binary()), "json", str(manifest_source())],
        cwd=ROOT,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    return json.loads(output)


def resolve_target_profile(target: object) -> TargetProfile:
    if not isinstance(target, str):
        fail(f"unknown target {target!r}; admitted targets: {', '.join(sorted(TARGET_PROFILES_BY_NAME))}")
    profile = TARGET_PROFILES_BY_NAME.get(target)
    if profile is None:
        fail(f"unknown target {target!r}; admitted targets: {', '.join(sorted(TARGET_PROFILES_BY_NAME))}")
    return profile



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

def validated_shared_buffer_quotas(holders: list[dict]) -> dict[str, dict]:
    if len(holders) > MAX_SHARED_BUFFER_BUDGET_HOLDERS:
        fail("shared-buffer budget exceeds holder bound")
    by_name: dict[str, dict] = {}
    totals = {"bytePages": 0, "bufferCount": 0, "mappingCount": 0, "loanCount": 0}
    ceilings = {
        "bytePages": FABRIC_GRAPH_KERNEL_TOTAL_PAGES,
        "bufferCount": FABRIC_GRAPH_KERNEL_SHARED_BUFFERS,
        "mappingCount": FABRIC_GRAPH_KERNEL_MAPPINGS,
        "loanCount": FABRIC_GRAPH_KERNEL_LOANS,
    }
    for holder in holders:
        name = holder["holder"]
        if name in by_name:
            fail(f"shared-buffer budget: duplicate holder {name}")
        for key, ceiling in ceilings.items():
            value = holder[key]
            if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= ceiling:
                fail(f"shared-buffer budget: invalid {key} for {name}")
            totals[key] += value
        if holder["bufferCount"] > holder["bytePages"]:
            fail(f"shared-buffer budget: {name} buffers exceed its page quota")
        if holder["mappingCount"] > holder["bytePages"]:
            fail(f"shared-buffer budget: {name} mappings exceed its page quota")
        if holder["loanCount"] > holder["bufferCount"]:
            fail(f"shared-buffer budget: {name} loans exceed its buffer quota")
        by_name[name] = holder
    for key, ceiling in ceilings.items():
        if totals[key] > ceiling:
            fail(f"shared-buffer budget: aggregate {key} exceeds the kernel ceiling")
    return by_name


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
    "buffers": FABRIC_GRAPH_LIMIT_BUFFERS,
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

def selected_profile_name() -> str:
    """The boot profile this build resolves (B11).

    One selector names both the component set and the interposition chains: a
    profile entry declares which scaffolding the generation adds and which
    `fabricGraph.profiles` entry supplies its interpositions. The legacy flags
    keep resolving the profile they always did, so a gate that has not been
    updated to name a profile explicitly still builds the graph it expects.
    """
    explicit = os.environ.get("SLIME_FABRIC_PROFILE") or None
    visibility = os.environ.get("SLIME_FABRIC_VISIBILITY_CHECK") == "1"
    boot = os.environ.get("SLIME_FABRIC_BOOT_CHECK") == "1"
    legacy_modes = any(
        os.environ.get(name) == "1"
        for name in (
            "SLIME_FABRIC_QOS_CHECK",
            "SLIME_FABRIC_CALL_CHECK",
            "SLIME_FABRIC_OPERATION_CHECK",
        )
    )
    if visibility and boot:
        fail("fabric graph: ambiguous selected profile")
    legacy = (
        UNIFIED_FABRIC_PROFILE
        if boot
        else VISIBILITY_FABRIC_PROFILE
        if visibility
        else TEST_BOOT_PROFILE
        if legacy_modes
        else None
    )
    if explicit is not None and legacy is not None and explicit != legacy:
        fail("fabric graph: ambiguous selected profile")
    return explicit or legacy or DEFAULT_FABRIC_PROFILE


def declared_boot_profiles(manifest: dict) -> list[str]:
    """Every boot profile the manifest names, selected or not."""
    return [profile["name"] for profile in manifest.get("bootProfiles", [])]


def boot_profile(manifest: dict, name: str) -> dict:
    """The one boot profile `name` selects."""
    profiles = manifest.get("bootProfiles", [])
    names = [profile["name"] for profile in profiles]
    if len(names) != len(set(names)):
        fail("boot profile: duplicate profile name")
    matches = [profile for profile in profiles if profile["name"] == name]
    if len(matches) != 1:
        fail(f"boot profile: expected exactly one {name} profile")
    return matches[0]


def resolve_boot_profile(manifest: dict, name: str) -> dict:
    """Narrow the manifest to the instances one boot profile declares."""
    profile = boot_profile(manifest, name)
    scaffolding = profile["instances"]
    if len(set(scaffolding)) != len(scaffolding):
        fail(f"boot profile {name}: duplicate instance")
    declared = {instance["name"] for instance in manifest["instances"]}
    unknown = sorted(set(scaffolding) - declared)
    if unknown:
        fail(f"boot profile {name}: undeclared instance(s) {', '.join(unknown)}")
    scaffolding_everywhere = {
        instance
        for entry in manifest.get("bootProfiles", [])
        for instance in entry["instances"]
    }
    kept = (declared - scaffolding_everywhere) | set(scaffolding)
    resolved = copy.deepcopy(manifest)
    resolved.pop("bootProfiles", None)
    resolved["instances"] = [
        instance for instance in manifest["instances"] if instance["name"] in kept
    ]
    # Boot profiles select initial instances. The executable catalogue is
    # independent and remains complete so an initial instance may spawn any
    # executable its explicit exec grant authorizes.
    used_executables = {executable["name"] for executable in manifest["executables"]}
    resolved["executables"] = copy.deepcopy(manifest["executables"])
    kept_objects = {executable["object"] for executable in resolved["executables"]}
    resolved["objects"] = [
        object_
        for object_ in manifest["objects"]
        if object_["kind"] != "component" or object_["id"] in kept_objects
    ]
    resolved["grants"] = [
        grant
        for grant in manifest["grants"]
        if grant["source"] in kept
        and (grant["target"] in kept or grant["target"] in used_executables)
    ]
    retained_grants = {grant["name"] for grant in resolved["grants"]}
    for instance in resolved["instances"]:
        instance["bindings"] = [
            binding for binding in instance["bindings"] if binding["grant"] in retained_grants
        ]
    resolved["state"] = [binding for binding in manifest["state"] if binding["owner"] in kept]
    resolved["sharedBufferBudget"] = [
        entry for entry in manifest["sharedBufferBudget"] if entry["holder"] in kept
    ]
    required = profile["requiredInstances"] or manifest["health"]["requiredInstances"]
    missing = sorted(set(required) - kept)
    if missing:
        fail(f"boot profile {name}: required instance(s) {', '.join(missing)} not declared")
    resolved["health"] = dict(manifest["health"], requiredInstances=list(required))
    graph = resolved.get("fabricGraph")
    if graph is not None:
        graph["profiles"] = [
            entry
            for entry in graph.get("profiles", [])
            if entry["name"] == profile["fabricProfile"]
        ]
        if len(graph["profiles"]) != 1:
            fail(
                f"boot profile {name}: fabric profile "
                f"{profile['fabricProfile']} is not declared exactly once"
            )
        for chain in graph["profiles"][0]["interpositions"]:
            absent = sorted(set(chain["chain"]) - kept)
            if absent:
                fail(
                    f"boot profile {name}: interposition chain names "
                    f"{', '.join(absent)}, which this profile does not declare"
                )
        routes = []
        for route in graph["routes"]:
            route["participants"] = [
                member for member in route["participants"] if member["component"] in kept
            ]
            for member in route["participants"]:
                hidden = sorted(set(member["interposition"]) - kept)
                if hidden:
                    fail(
                        f"boot profile {name}: route {route['name']} interposes through "
                        f"{', '.join(hidden)}, which this profile does not declare"
                    )
            # A route every participant of which was scaffolding carries no
            # traffic in this profile. Keeping it would declare a route the
            # fabric must provision and nobody can use, and `build_fabric_graph`
            # rejects a participant-less route outright.
            if route["participants"]:
                routes.append(route)
        graph["routes"] = routes
    return resolved


def declared_fabric_profiles(manifest: dict) -> list[str]:
    """Every fabric profile the manifest names, selected or not."""
    graph = manifest.get("fabricGraph") or {}
    return [profile["name"] for profile in graph.get("profiles", [])]


def _control_sources(manifest: dict, grant_names: tuple[str, ...]) -> list[str]:
    """The components holding each named control grant, in declared order.

    B11: a grant whose source the selected boot profile does not declare is
    absent rather than invalid, so the list shortens for a profile that drops
    that participant. Order is the tuple's, and the tuple is per plane, so a
    profile declaring the same participants numbers its control slots exactly
    as it did before — which is what keeps the C8.3-C8.8 gates reading a
    control endpoint where they expect one. A grant that *is* declared must
    still be exactly right.
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
    for name in grant_names:
        grant = grants.get(name)
        if grant is None:
            continue
        if grant["target"] != "fabric-service" or grant["rights"] != ["send", "recv"]:
            fail(f"fabric graph: invalid control grant {name}")
        controls.append(grant["source"])
    if len(set(controls)) != len(controls):
        fail("fabric graph: duplicate control source")
    return controls


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


def validate_route_worker_names() -> None:
    """Every name in `FABRIC_ROUTE_WORKERS` is a route some manifest declares.

    Checked against the **full catalogue** — the union of every route the
    canonical x86 source declares — rather than against the graph being built.
    Those are different questions, and conflating them is what makes the check
    either useless or wrong:

    * against the graph being built, a manifest declaring a subset of the
      routes (P5.5.2's seL4 graph declares the two stream routes alone)
      fails on a tuple that has no typo in it;
    * without the check at all, a genuine misspelling in the tuple silently
      drops a route from its worker, and the partition assertion below then
      reports the route as uncovered rather than the worker as misspelled.

    So the typo check reads the source of truth for what routes exist, and the
    partition check reads the graph. A worker whose routes this graph does not
    declare simply has no work here.
    """
    catalogue = {
        route["name"]
        for route in _canonical_manifest()["fabricGraph"]["routes"]
    }
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


def resolve_fabric_profile(manifest: dict, interfaces: list, profile_name: str) -> ResolvedFabricProfile:
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
    # The full-graph boot profile declares its own stream plane; every other
    # profile keeps the exact control layout its gate already grants. A source
    # this profile does not declare drops out of the list rather than failing,
    # so the product profile resolves the same plane with fewer participants.
    stream_controls = _control_sources(
        manifest,
        FABRIC_BOOT_STREAM_CONTROL_GRANTS
        if fabric_profile_name == UNIFIED_FABRIC_PROFILE
        else FABRIC_STREAM_CONTROL_GRANTS,
    )
    call_controls = _control_sources(manifest, FABRIC_CALL_CONTROL_GRANTS)
    operation_controls = _control_sources(manifest, FABRIC_OPERATION_CONTROL_GRANTS)
    replacement_controls = _control_sources(manifest, FABRIC_OPERATION_REPLACEMENT_GRANTS)
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
    subscriber_components = {
        participant["component"]
        for participant in participants
        if participant["direction"] == FABRIC_DIRECTION_SUBSCRIBE
    }
    subscribers = [component for component in stream_controls if component in subscriber_components]
    supervision = [
        {"component": component, "slot": FABRIC_FIRST_CONTROL_SLOT + len(stream_controls) + index}
        for index, component in enumerate(subscribers)
    ]
    # C8.10: every plane coexists in one boot, so its control slots are summed
    # into one disjoint layout rather than overlaid. `max()` here would size the
    # table for whichever single plane happened to be largest, which is exactly
    # the mutually-exclusive assumption the milestone removes: two planes would
    # then be numbered from the same base and collide on the same slot.
    plane_control_counts = (
        len(stream_controls) * 2 + len(subscribers),
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
    validate_route_worker_names()
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
            fail(f"fabric graph: worker {worker_name} exceeds one SYS_WAIT set")
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
    ring_capacity = sum(
        participant["historyDepth"]
        for participant in participants
        if participant["direction"] == FABRIC_DIRECTION_SUBSCRIBE
    )
    if ring_capacity > FABRIC_FRAME_CAPACITY:
        fail("fabric graph: subscriber history exceeds the frame table")
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


def render_fabric_profile_rust(resolved: ResolvedFabricProfile) -> str:
    artifact = resolved.artifact
    limits = {entry["name"]: entry["value"] for entry in artifact["limits"]}
    participants = artifact["participants"]
    rust_string = lambda value: json.dumps(value, ensure_ascii=True)
    participant_rows = "".join(
        f"    (b{rust_string(row['component'])}, {rust_string(row['route'])}, {rust_string(row['interface'])}, {row['direction']}),\n"
        for row in participants
    )
    depth_rows = "".join(
        f"    (b{rust_string(row['component'])}, {rust_string(row['route'])}, {row['historyDepth']}),\n"
        for row in participants
    )
    qos_rows = "".join(
        f"    (b{rust_string(row['component'])}, {rust_string(row['route'])}, {row['deadlineNs']}, {row['lifespanNs']}, {row['leaseNs']}, {row['historyDepth']}, {row['retainedDepth']}, {row['reliability']}, {row['durability']}, {row['liveliness']}),\n"
        for row in participants
    )
    visibility_rows = "".join(
        f"    (b{rust_string(row['component'])}, {rust_string(row['route'])}, {row['visibility']}),\n"
        for row in participants
    )
    interposition_rows = "".join(
        f"    (b{rust_string(row['component'])}, {rust_string(row['route'])}, &[{', '.join(f'b{rust_string(hop)} as &[u8]' for hop in row['interposition'])}]),\n"
        for row in participants if row["interposition"]
    )
    planes = {plane["name"]: plane["controls"] for plane in artifact["planes"]}
    controls = lambda name: "".join(
        f"    b{rust_string(row['component'])},\n" for row in planes[name]
    )
    # C8.10 bounded route workers. One row per worker: the routes it carries and
    # the number of live wake sources it must hold at once. The fabric parks on
    # exactly this set, so the generation — not a runtime heuristic — decides how
    # the graph is partitioned across `SYS_WAIT` sets.
    worker_rows = "".join(
        f"    ({rust_string(row['name'])}, &[{', '.join(rust_string(route) for route in row['routes'])}], {row['waitSources']}),\n"
        for row in artifact["workers"]
    )
    supervision_rows = "".join(
        f"    (b{rust_string(row['component'])}, {row['slot']}),\n" for row in artifact["supervision"]
    )
    subscriber_rows = "".join(
        f"    b{rust_string(row['component'])},\n" for row in artifact["supervision"]
    )
    schema_rows = "".join(
        f"    ({rust_string(row['name'])}, {rust_string(row['identity'])}, 0x{row['typeTag']}, {row['contractKind']}, {row['maxEncodedBytes']}),\n"
        for row in artifact["schemas"]
    )
    route_rows = "".join(
        f"    ({rust_string(row['name'])}, {rust_string(row['interface'])}, {rust_string(row['identity'])}, {row['contractKind']}),\n"
        for row in artifact["routes"]
    )
    deadline_absent = (1 << 64) - 1

    def deadline(route: str) -> int:
        """The tightest deadline any request/response participant declares.

        `FABRIC_DEADLINE_ABSENT`, not zero, denotes a graph with no such route.
        Zero remains available as a real immediate deadline and cannot be
        conflated with absence by generated consumers.
        """
        deadlines = [
            row["deadlineNs"]
            for row in participants
            if row["route"] == route
            and row["direction"] in (FABRIC_DIRECTION_CLIENT, FABRIC_DIRECTION_SERVER)
        ]
        return min(deadlines, default=deadline_absent)
    return f'''// @generated from the canonical C8.9 resolved fabric profile; do not edit.
#[allow(dead_code)]
pub const FABRIC_PROFILE_NAME: &str = {rust_string(artifact['name'])};
#[allow(dead_code)]
pub const FABRIC_SCHEMAS: &[(&str, &str, u64, u32, u32)] = &[\n{schema_rows}];
#[allow(dead_code)]
pub const FABRIC_ROUTES: &[(&str, &str, &str, u32)] = &[\n{route_rows}];
pub const FABRIC_PARTICIPANTS: &[(&[u8], &str, &str, u32)] = &[\n{participant_rows}];
pub const FABRIC_HISTORY_DEPTHS: &[(&[u8], &str, u32)] = &[\n{depth_rows}];
pub type FabricQosRow = (&'static [u8], &'static str, u64, u64, u64, u32, u32, u8, u8, u8);
pub const FABRIC_QOS: &[FabricQosRow] = &[\n{qos_rows}];
pub const FABRIC_VISIBILITY: &[(&[u8], &str, u8)] = &[\n{visibility_rows}];
pub type FabricInterpositionRow = (&'static [u8], &'static str, &'static [&'static [u8]]);
pub const FABRIC_INTERPOSITIONS: &[FabricInterpositionRow] = &[\n{interposition_rows}];
pub type FabricWorkerRow = (&'static str, &'static [&'static str], usize);
pub const FABRIC_WORKERS: &[FabricWorkerRow] = &[\n{worker_rows}];
/// The wake sources the generation declares one worker parks on at once, or
/// `WORKER_ABSENT` when this graph declares no route that worker carries.
///
/// `const fn` so a broker can bind its own `SYS_WAIT` array to this number in a
/// `const _: () = assert!(..)`. The declared peak and the array that has to hold
/// it then cannot drift apart silently: a broker that grows its park set past
/// what the generation resolved stops compiling instead of overflowing at boot.
///
/// Absent is a real answer rather than a panic, because a broker is a *module*
/// of `fabric-service` and is therefore compiled into every graph, including
/// ones that declare no route for it. A stream-only graph has no call or
/// operation plane; panicking here would make such a graph fail to build over a
/// constant nothing in it ever reads. The asserts that consume this admit
/// `WORKER_ABSENT` and keep their exact check for every graph that does declare
/// the plane, so the drift they exist to catch is still caught.
#[allow(dead_code)]
pub const WORKER_ABSENT: usize = usize::MAX;
#[allow(dead_code)]
pub const fn fabric_worker_wait_sources(name: &str) -> usize {{
    let mut index = 0;
    while index < FABRIC_WORKERS.len() {{
        let (candidate, _, sources) = FABRIC_WORKERS[index];
        if konst_str_eq(candidate, name) {{
            return sources;
        }}
        index += 1;
    }}
    WORKER_ABSENT
}}

/// `str` equality usable in a `const fn`; `==` on `&str` is not yet const.
const fn konst_str_eq(left: &str, right: &str) -> bool {{
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {{
        return false;
    }}
    let mut index = 0;
    while index < left.len() {{
        if left[index] != right[index] {{
            return false;
        }}
        index += 1;
    }}
    true
}}
pub const FABRIC_CLIENTS: &[&[u8]] = &[\n{controls('stream')}];
pub const FABRIC_CALL_CLIENTS: &[&[u8]] = &[\n{controls('call')}];
pub const FABRIC_OPERATION_CLIENTS: &[&[u8]] = &[\n{controls('operation')}];
pub const FABRIC_SUPERVISION: &[(&[u8], u32)] = &[\n{supervision_rows}];
pub const FABRIC_SUBSCRIBERS: &[&[u8]] = &[\n{subscriber_rows}];
#[allow(dead_code)]
pub const FABRIC_MAX_ROUTES: usize = {limits['routes']};
#[allow(dead_code)]
pub const FABRIC_MAX_INGRESS_SOURCES: usize = {limits['ingressSources']};
pub const FABRIC_MAX_PUBLISHERS: usize = {limits['publishers']};
pub const FABRIC_MAX_SUBSCRIBERS: usize = {limits['subscribers']};
#[allow(dead_code)]
pub const FABRIC_MAX_CLIENTS: usize = {limits['clients']};
#[allow(dead_code)]
pub const FABRIC_MAX_SERVERS: usize = {limits['servers']};
pub const FABRIC_MAX_SAMPLE_BYTES: usize = {limits['sampleBytes']};
#[allow(dead_code)]
pub const FABRIC_MAX_QUEUE_DEPTH: usize = {limits['queueDepth']};
#[allow(dead_code)]
pub const FABRIC_MAX_HISTORY_DEPTH: usize = {limits['historyDepth']};
pub const FABRIC_MAX_EVENT_DEPTH: usize = {limits['eventDepth']};
pub const FABRIC_MAX_RETAINED_SAMPLES: usize = {limits['retainedSamples']};
pub const FABRIC_MAX_RETRIES: u8 = {limits['retries']};
pub const FABRIC_MAX_IN_FLIGHT_CALLS: usize = {limits['inFlightCalls']};
pub const FABRIC_MAX_IN_FLIGHT_OPERATIONS: usize = {limits['inFlightOperations']};
pub const FABRIC_MAX_BUFFER_PAGES: usize = {limits['bufferPages']};
pub const FABRIC_MAX_BUFFERS: usize = {limits['buffers']};
#[allow(dead_code)]
pub const FABRIC_MAX_MAPPINGS: usize = {limits['mappings']};
#[allow(dead_code)]
pub const FABRIC_MAX_LOANS: usize = {limits['loans']};
pub const FABRIC_MAX_CAPABILITY_SLOTS: usize = {limits['capabilitySlots']};
pub const FABRIC_REQUIRED_CAPABILITY_SLOTS: usize = {artifact['requiredCapabilitySlots']};
pub const FABRIC_FRAME_CAPACITY: usize = {artifact['frameCapacity']};
pub const FABRIC_COPY_PAGES: usize = {artifact['copyPages']};
/// No request/response route of this class exists in the resolved graph.
pub const FABRIC_DEADLINE_ABSENT: u64 = u64::MAX;
pub const FABRIC_CALL_DEADLINE_NS: u64 = {deadline('parameters')};
pub const FABRIC_OPERATION_DEADLINE_NS: u64 = {deadline('navigation')};
pub const FABRIC_FIRST_CONTROL_SLOT: u32 = {artifact['firstControlSlot']};
'''


def write_resolved_profile(output: Path, resolved: ResolvedFabricProfile) -> tuple[Path, Path, Path]:
    profile_path = output / "data-fabric-profile.zti"
    rust_path = output / "data-fabric-profile.rs"
    schemas_path = output / "normalized-interface-schemas.bin"
    profile_path.write_text(_zti_value(resolved.artifact) + "\n", encoding="utf-8")
    rust_path.write_text(render_fabric_profile_rust(resolved), encoding="utf-8")
    schemas_path.write_bytes(build_normalized_schema_artifact(resolved.schemas))
    return profile_path, rust_path, schemas_path




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
    )
    return header + schema_records + route_records + participant_records + hop_records


def component_target_dir(root: Path, target_profile: TargetProfile, name: str) -> Path:
    """Keep profiles sharing one Cargo target from reusing executable outputs."""
    return root / target_profile.name / name


def component_executable(built: Path, name: str, target_profile: TargetProfile) -> Path:
    """Where cargo wrote one component's executable.

    The rust-sel4 target specifications set `"exe-suffix": ".elf"`, so a binary
    named `init` is written as `init.elf`. Resolving it here rather than at each
    call site keeps the suffix a property of the target, which is what it is.
    """
    if is_json_target(target_profile):
        return built / f"{name}.elf"
    return built / name


def is_json_target(target_profile: TargetProfile) -> bool:
    """Whether this profile's Cargo target is a JSON specification file.

    A JSON target is not merely a different triple: cargo needs `-Z
    json-target-spec`, has no prebuilt `core`/`alloc` so it needs `build-std`,
    names its output directory by the file *stem* rather than the path, and is
    not matched by the per-triple `rustflags` in `components/.cargo/config.toml`.
    Each of those is handled below.
    """
    return target_profile.cargo_target.endswith(".json")


def cargo_target_argument(target_profile: TargetProfile) -> str:
    """What `--target` receives: an absolute path for a JSON spec, else the
    triple verbatim."""
    if is_json_target(target_profile):
        return str(ROOT / target_profile.cargo_target)
    return target_profile.cargo_target


def cargo_target_directory_name(target_profile: TargetProfile) -> str:
    """The directory cargo writes artifacts into.

    For a JSON specification this is the file stem, not the path — the single
    most common way a JSON target silently breaks a build script that assumed
    the two were the same string.
    """
    if is_json_target(target_profile):
        return Path(target_profile.cargo_target).stem
    return target_profile.cargo_target


def sel4_component_environment(environment: dict[str, str]) -> dict[str, str]:
    """Add what a `slime-components` build for the seL4 profile needs.

    `slime-rt`'s seL4 transport compiles against the installed libsel4, whose
    bindings `sel4-sys` generates with bindgen at build time, so the prefix and
    libclang must both be present and named. The toolchain is pinned by
    `sel4/pins.toml` because `build-std` requires the matching `rust-src`.
    Mirrors `scripts/build/build-sel4.py::cargo_environment`, which is the
    working precedent for building against these pins.
    """
    pins_path = ROOT / "sel4" / "pins.toml"
    if not pins_path.is_file():
        fail(f"missing pin manifest: {pins_path.relative_to(ROOT)}")
    import tomllib

    pins = tomllib.loads(pins_path.read_text(encoding="utf-8"))
    environment["RUSTUP_TOOLCHAIN"] = pins["rust_sel4"]["toolchain"]
    prefix = ROOT / "build" / "sel4-prefix"
    if not (prefix / "libsel4" / "include" / "kernel" / "gen_config.json").is_file():
        fail(
            f"no installed seL4 prefix at {prefix.relative_to(ROOT)}; "
            "run `just sel4_qemu_image_check` first"
        )
    environment["SEL4_PREFIX"] = str(prefix)
    if not environment.get("LIBCLANG_PATH"):
        fail(
            "LIBCLANG_PATH is unset, so bindgen cannot generate the libsel4 bindings; "
            "enter the pinned shell with `nix develop` or export LIBCLANG_PATH"
        )
    return environment


def build_rust_components(
    generation_number: int,
    profile_path: Path,
    target_profile: TargetProfile,
    recovery: bool = False,
    candidate_identity: bytes | None = None,
    components: set[str] | None = None,
    binding_slots: dict[str, int] | None = None,
    role_bindings: dict[str, int] | None = None,
) -> Path:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = str(generation_number)
    if environment.get("SLIME_BOOT_SELECTION_FAIL") == "1":
        environment["SLIME_BOOT_SELECTION_FAIL"] = "1"
    else:
        environment.pop("SLIME_BOOT_SELECTION_FAIL", None)
    environment["SLIME_DATA_FABRIC_PROFILE"] = str(profile_path)
    # The components are compiled before the generation is assembled, so they
    # cannot read the layout resource out of it. Emit the same table as Rust
    # here and hand `build.rs` the path, the way the fabric profile already
    # travels. Per generation number, so each component build addresses the
    # slots its own generation declares, and narrowed to the selected boot
    # profile's component set (B11) so `init.rs` reads the same slots the kernel
    # will place.
    layout_path = profile_path.parent / f"boot-layout-{generation_number}.rs"
    layout_path.write_text(
        render_boot_layout_rust(generation_number, components, binding_slots, role_bindings),
        encoding="utf-8",
    )
    environment["SLIME_BOOT_LAYOUT"] = str(layout_path)
    environment["SLIME_TARGET_PROFILE"] = target_profile.name
    if candidate_identity is None and os.environ.get("SLIME_TRANSFER_RECEIVER") == "1":
        environment["SLIME_TRANSFER_RECEIVER"] = "1"
    else:
        environment.pop("SLIME_TRANSFER_RECEIVER", None)
    if candidate_identity is not None and os.environ.get("SLIME_TRANSFER_ACTIVATE") == "1":
        environment["SLIME_TRANSFER_ACTIVATE"] = "1"
    else:
        environment.pop("SLIME_TRANSFER_ACTIVATE", None)
    if environment.get("SLIME_FABRIC_QOS_CHECK") == "1":
        environment["SLIME_FABRIC_QOS_CHECK"] = "1"
    else:
        environment.pop("SLIME_FABRIC_QOS_CHECK", None)
    if environment.get("SLIME_FABRIC_CALL_CHECK") == "1":
        environment["SLIME_FABRIC_CALL_CHECK"] = "1"
    else:
        environment.pop("SLIME_FABRIC_CALL_CHECK", None)
    if environment.get("SLIME_FABRIC_OPERATION_CHECK") == "1":
        environment["SLIME_FABRIC_OPERATION_CHECK"] = "1"
    else:
        environment.pop("SLIME_FABRIC_OPERATION_CHECK", None)
    if environment.get("SLIME_FABRIC_VISIBILITY_CHECK") == "1":
        environment["SLIME_FABRIC_VISIBILITY_CHECK"] = "1"
    else:
        environment.pop("SLIME_FABRIC_VISIBILITY_CHECK", None)
    if environment.get("SLIME_FABRIC_PROXY_EARLY_EXIT") == "1":
        environment["SLIME_FABRIC_PROXY_EARLY_EXIT"] = "1"
    else:
        environment.pop("SLIME_FABRIC_PROXY_EARLY_EXIT", None)
    # P5.3.1. Set by `build_sel4_generation` for the channel graph and popped
    # for every other build, so which scenario `init.rs` compiles in is decided
    # by the manifest being built rather than by whatever is in the caller's
    # shell. Scrubbed here for the same reason every flag above is: an inherited
    # value would silently change a different generation's components.
    if environment.get("SLIME_SEL4_CHANNEL_CHECK") == "1":
        environment["SLIME_SEL4_CHANNEL_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_CHANNEL_CHECK", None)
    # P5.3.2, on the same rule as the flag above.
    if environment.get("SLIME_SEL4_LOAN_CHECK") == "1":
        environment["SLIME_SEL4_LOAN_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_LOAN_CHECK", None)
    # P5.3.3, on the same rule again.
    if environment.get("SLIME_SEL4_SPAWN_CHECK") == "1":
        environment["SLIME_SEL4_SPAWN_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_SPAWN_CHECK", None)
    # P5.3.4, likewise.
    if environment.get("SLIME_SEL4_SAMPLE_CHECK") == "1":
        environment["SLIME_SEL4_SAMPLE_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_SAMPLE_CHECK", None)
    # P5.5.2, likewise.
    if environment.get("SLIME_SEL4_STREAM_CHECK") == "1":
        environment["SLIME_SEL4_STREAM_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_STREAM_CHECK", None)
    # B16's supervision plane, likewise.
    if environment.get("SLIME_SEL4_SUPERVISION_CHECK") == "1":
        environment["SLIME_SEL4_SUPERVISION_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_SUPERVISION_CHECK", None)
    # B38 task-arena and root-CSlot reclamation plane.
    if environment.get("SLIME_SEL4_RECLAMATION_CHECK") == "1":
        environment["SLIME_SEL4_RECLAMATION_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_RECLAMATION_CHECK", None)
    # B22's channel-crossing plane, likewise.
    if environment.get("SLIME_SEL4_CROSSING_CHECK") == "1":
        environment["SLIME_SEL4_CROSSING_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_CROSSING_CHECK", None)
    # P5.4.6's call plane, likewise. Its own flag rather than the oracle's
    # `SLIME_FABRIC_CALL_CHECK`: the two planes share the broker but not init's
    # composition, so one flag would make the seL4 generation walk the x86
    # boot layout.
    if environment.get("SLIME_SEL4_CALL_CHECK") == "1":
        environment["SLIME_SEL4_CALL_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_CALL_CHECK", None)
    # P5.4.7's operation plane. Two flags, on the QoS row's rule: the seL4 flag
    # selects init's composition while the oracle's `SLIME_FABRIC_OPERATION_CHECK`
    # keeps `fabric-service` and the five participants byte-identical with the
    # x86 plane. `init.rs` requires the seL4 flag to be absent before it takes
    # the oracle branch, so generation 20 cannot walk generation 15's layout.
    if environment.get("SLIME_SEL4_OPERATION_CHECK") == "1":
        environment["SLIME_SEL4_OPERATION_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_OPERATION_CHECK", None)
    # P5.4.8's visibility plane, on the operation row's rule: the seL4 flag
    # composes the plane while the oracle's `SLIME_FABRIC_VISIBILITY_CHECK`
    # selects the unmodified visibility broker and the five participants.
    if environment.get("SLIME_SEL4_VISIBILITY_CHECK") == "1":
        environment["SLIME_SEL4_VISIBILITY_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_VISIBILITY_CHECK", None)
    # P5.4.9's full-graph boot, on the same rule. Its oracle counterpart is
    # `SLIME_FABRIC_BOOT_CHECK`, which every participant reads through
    # `fabric_boot::active`.
    if environment.get("SLIME_SEL4_BOOT_CHECK") == "1":
        environment["SLIME_SEL4_BOOT_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_BOOT_CHECK", None)
    # P5.4.2c's storage plane, on the same rule.
    if environment.get("SLIME_SEL4_STORAGE_CHECK") == "1":
        environment["SLIME_SEL4_STORAGE_CHECK"] = "1"
    else:
        environment.pop("SLIME_SEL4_STORAGE_CHECK", None)
    if recovery:
        environment["SLIME_RECOVERY_IMAGE"] = "1"
    if environment.get("SLIME_GENERATION_CMD_CHECK") == "1" and candidate_identity is not None:
        environment["SLIME_GENERATION_CANDIDATE"] = candidate_identity.hex()
    # P5.3.1: the two seL4 graphs are both generation 1, so keying only on the
    # number would give them one Cargo target directory — and since they differ
    # by a compile-time `option_env!` in `init.rs` rather than by any input
    # Cargo tracks, the second build would silently reuse the first's `init.elf`
    # and boot the wrong scenario.
    sel4_manifest = os.environ.get("SLIME_SEL4_MANIFEST")
    if recovery:
        target_name = "recovery"
    elif sel4_manifest is not None and sel4_manifest != "sel4":
        target_name = f"{sel4_manifest}-{generation_number}"
    elif candidate_identity is None and os.environ.get("SLIME_TRANSFER_RECEIVER") == "1":
        target_name = f"generation-{generation_number}-transfer-receiver"
    elif candidate_identity is not None and os.environ.get("SLIME_TRANSFER_ACTIVATE") == "1":
        target_name = f"generation-{generation_number}-transfer-activate"
    else:
        target_name = f"generation-{generation_number}"
    target_dir = component_target_dir(COMPONENTS_TARGET_DIR, target_profile, target_name)
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    command = [
        "cargo",
        "build",
        "--release",
        "--target",
        cargo_target_argument(target_profile),
        "-p",
        "slime-components",
    ]
    if is_json_target(target_profile):
        features = ["sel4"]
        # The Dango command profile is generated from a manifest, and it must be
        # *this* generation's: the profile's executable slots are spawn-grant
        # positions, so a profile built from the oracle's manifest would name
        # slots this generation never grants.
        command_manifest = sel4_manifest or "sel4"
        environment["SLIME_COMMAND_PROFILE_MANIFEST"] = f"{command_manifest}.zti"
        # GPT validation and the object store come from `boot-contracts/gpt`,
        # which needs an allocator. `extern crate alloc` in a dependency makes
        # every binary in the crate require a `#[global_allocator]`, so the
        # feature is enabled only for the build whose components declare a heap.
        if components is not None and any(
            name in components
            for name in (
                "sel4-store-probe",
                "sel4-rollback-probe",
                "sel4-recovery-probe",
                "sel4-generation-manager",
                "sel4-filesystem-service",
                "sel4-transfer-probe",
            )
        ):
            features.append("store")
        command += ["--no-default-features", "--features", ",".join(features)]
        # Build exactly the binaries this generation declares, rather than every
        # binary in the crate. The fabric components are compiled against a
        # generated C8 profile this target has no graph for, so building them
        # would fail on constants that describe routes the generation does not
        # declare. Naming the binaries keeps the build's contents equal to the
        # manifest's, which is the same property the boot layout already has.
        if components is None:
            fail("seL4 component builds must name the components to build")
        for component in sorted(components):
            command += ["--bin", component]
        command += [
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ]
        # `components/.cargo/config.toml` keys `rustflags` by triple, so a JSON
        # target inherits none of them. Passing the determinism-relevant ones
        # explicitly keeps the link reproducible instead of silently dropping
        # them. `-T` and the load base are deliberately absent: a component here
        # is an ordinary seL4 ELF task at its own link addresses.
        environment["RUSTFLAGS"] = " ".join(
            [
                "-C link-arg=--build-id=none",
                f"--remap-path-prefix={ROOT}=.",
            ]
        )
        environment = sel4_component_environment(environment)
    else:
        # B12: `components/.cargo/config.toml` carried a hardcoded
        # `--remap-path-prefix` naming one developer's checkout. Any other
        # checkout made it a no-op — or worse, when the stale literal was a
        # *prefix* of the real path it rewrote the leading portion and left the
        # remainder, mangling recorded paths instead of normalizing them.
        #
        # Appended through `--config` rather than `RUSTFLAGS`, which would
        # *replace* the config's rustflags and silently drop the
        # relocation-model, code-model, and link-arg settings the x86 link
        # depends on. The JSON-target branch above can set `RUSTFLAGS` freely
        # because a JSON target inherits none of those to begin with.
        remap = f"--remap-path-prefix={ROOT}=."
        command += [
            "--config",
            f'target.{target_profile.cargo_target}.rustflags=["{remap}"]',
        ]
    subprocess.run(
        command,
        cwd=ROOT / "components",
        env=environment,
        check=True,
    )
    return target_dir / cargo_target_directory_name(target_profile) / "release"


def elf_component_image(name: str, elf: Path, stack_bytes: int, profile: TargetProfile) -> bytes:
    """Wrap a native ELF in the target-qualification header (P5.2).

    The seL4 profile carries the executable whole rather than re-basing it onto
    a fixed component load base: `slime-root` loads it with a real ELF loader at
    the addresses it links to. The header is byte-identical in layout to the
    segment-carrying revision, so `boot_contracts::component_image::admit`
    qualifies both by the same offsets and stage-0-style wrong-target rejection
    still applies before any byte is mapped. `segment_count` is zero because the
    body has no Slime segment table, and `entry_offset` is likewise zero: the
    entry point lives in the ELF header, where the loader reads it.
    """
    data = elf.read_bytes()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail(f"{name}: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 2 or machine != profile.elf_machine:
        fail(f"{name}: not a static executable for target {profile.name}")
    if len(data) > MAX_COMPONENT_IMAGE_BYTES:
        fail(f"{name}: image exceeds the component image bound")
    header = COMPONENT_IMAGE_HEADER.pack(
        COMPONENT_IMAGE_ELF_MAGIC,
        COMPONENT_IMAGE_ELF_VERSION,
        COMPONENT_IMAGE_ELF_HEADER_LEN,
        COMPONENT_IMAGE_KERNEL_ABI,
        profile.architecture,
        profile.abi,
        profile.page_profile,
        0,
        0,
        0,
        stack_bytes,
        profile.id,
        profile.required_features,
    )
    return header + data


def component_image(name: str, elf: Path, stack_bytes: int, profile: TargetProfile) -> bytes:
    if is_json_target(profile):
        return elf_component_image(name, elf, stack_bytes, profile)
    data = elf.read_bytes()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail(f"{name}: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 2 or machine != profile.elf_machine:
        fail(f"{name}: not a static executable for target {profile.name}")
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
    if not 1 <= len(segments) <= COMPONENT_IMAGE_MAX_SEGMENTS or segments[0][0] != profile.component_base or entry < profile.component_base:
        fail(f"{name}: invalid component load layout")
    records = bytearray()
    payload = bytearray()
    previous_end = 0
    entry_offset = entry - profile.component_base
    entry_ok = False
    total_pages = 0
    for vaddr, offset, filesz, memsz, elf_flags in segments:
        if filesz > memsz or vaddr % profile.page_bytes or vaddr < previous_end or offset + filesz > len(data):
            fail(f"{name}: invalid or overlapping segment")
        flags = (COMPONENT_SEGMENT_FLAG_EXEC if elf_flags & 1 else 0) | (COMPONENT_SEGMENT_FLAG_WRITE if elf_flags & 2 else 0)
        if flags == COMPONENT_SEGMENT_FLAG_EXEC | COMPONENT_SEGMENT_FLAG_WRITE:
            fail(f"{name}: writable executable segment")
        relative = vaddr - profile.component_base
        entry_ok |= bool(flags & COMPONENT_SEGMENT_FLAG_EXEC and relative <= entry_offset < relative + memsz)
        records += COMPONENT_IMAGE_SEGMENT.pack(relative, memsz, len(payload), filesz, flags, 0)
        payload += data[offset : offset + filesz]
        previous_end = vaddr + memsz
        total_pages += -(-memsz // profile.page_bytes)
    if not entry_ok or total_pages * profile.page_bytes > MAX_COMPONENT_IMAGE_BYTES:
        fail(f"{name}: invalid entry or image size")
    header = COMPONENT_IMAGE_HEADER.pack(
        COMPONENT_IMAGE_MAGIC,
        COMPONENT_IMAGE_VERSION,
        COMPONENT_IMAGE_HEADER.size,
        COMPONENT_IMAGE_KERNEL_ABI,
        profile.architecture,
        profile.abi,
        profile.page_profile,
        entry_offset,
        len(segments),
        0,
        stack_bytes,
        profile.id,
        profile.required_features,
    )
    return header + records + payload

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


def validate_acyclic(instances: list[dict]) -> None:
    graph = {instance["name"]: instance["dependencies"] for instance in instances}
    for name, dependencies in graph.items():
        if name in dependencies or len(set(dependencies)) != len(dependencies):
            fail(f"instance {name}: invalid dependencies")
        for dependency in dependencies:
            if dependency not in graph:
                fail(f"instance {name}: missing dependency {dependency}")
    active: set[str] = set()
    complete: set[str] = set()

    def visit(name: str) -> None:
        if name in complete:
            return
        if name in active:
            fail("instance dependency cycle")
        active.add(name)
        for dependency in graph[name]:
            visit(dependency)
        active.remove(name)
        complete.add(name)

    for name in graph:
        visit(name)


PLAN_NONE = 0xFFFFFFFF
GRANT_POLICY_ONLY = 1
SERVICE_ROOT_DISPATCH = 1
KERNEL_OBJECT_CNODE = 1
KERNEL_OBJECT_VSPACE = 2
KERNEL_OBJECT_TCB = 3
KERNEL_OBJECT_FRAME = 4
KERNEL_OBJECT_ENDPOINT = 5
KERNEL_OBJECT_PAGE_TABLE = 6
CAP_RIGHT_ALL = (1 << 64) - 1


def build_sel4_plan(
    manifest: dict,
    instances: list[dict],
    executables: list[dict],
    grants: list[dict],
    grant_rights: list[int],
    instance_index: dict[str, int],
    executable_index: dict[str, int],
    string_offset,
) -> tuple[bytes, ...]:
    """Materialize the required v5 process and authority plan.

    The builder owns this expansion because it has the authenticated manifest
    and the exact executable catalogue in hand. Counts and references are
    checked here, before a byte is emitted; the decoder repeats the checks at
    the trust boundary.
    """
    process_records = bytearray()
    thread_records = bytearray()
    kernel_records = bytearray()
    mapping_records = bytearray()
    cap_records = bytearray()
    service_records = bytearray()
    schedule_records = bytearray()
    fault_records = bytearray()
    spawn_records = bytearray()
    quota_records = bytearray()
    object_index: dict[tuple[str, str], int] = {}

    # Every declared instance is a process in the plan. A child instance is
    # constructed by its owner rather than by root, but its CSpace, VSpace,
    # TCB, IPC buffer, fault endpoint, schedule, and quota are all fixed here:
    # B39's whole point is that the generation proves the object plan before
    # anything activates, and an owner-spawned process consumes kernel objects
    # exactly as a root-autostart one does.
    planned_instances = list(instances)
    if not planned_instances or len(planned_instances) > MAX_PROCESSES:
        fail("seL4 process plan count exceeds bound")

    for process, instance in enumerate(planned_instances):
        name = instance["name"]
        quota = process
        cspace = len(object_index)
        object_index[(name, "cspace")] = cspace
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:cspace"), KERNEL_OBJECT_CNODE, process, 6, 1, PLAN_NONE, 0
            )
        )
        vspace = len(object_index)
        object_index[(name, "vspace")] = vspace
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:vspace"), KERNEL_OBJECT_VSPACE, process, 12, 1, PLAN_NONE, 0
            )
        )
        tcb = len(object_index)
        object_index[(name, "tcb")] = tcb
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:tcb"), KERNEL_OBJECT_TCB, process, 11, 1, PLAN_NONE, 0
            )
        )
        ipc = len(object_index)
        object_index[(name, "ipc-buffer")] = ipc
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:ipc-buffer"), KERNEL_OBJECT_FRAME, process, 12, 1, PLAN_NONE, 0
            )
        )
        fault_endpoint = len(object_index)
        object_index[(name, "fault-endpoint")] = fault_endpoint
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:fault-endpoint"), KERNEL_OBJECT_ENDPOINT, process, 4, 1, PLAN_NONE, 0
            )
        )

        thread = process
        schedule = process
        fault = process
        thread_records.extend(
            GENERATION_THREAD.pack(
                string_offset(f"{name}:main"), process, tcb, schedule, fault, ipc, 0, 0, 0
            )
        )
        process_records.extend(
            GENERATION_PROCESS.pack(
                string_offset(name), instance_index[name], cspace, vspace, thread, quota, 0
            )
        )
        schedule_records.extend(
            GENERATION_SCHEDULE.pack(
                string_offset(f"{name}:schedule"), thread, PLAN_NONE, 100, 100, 0, 0, 0
            )
        )
        fault_records.extend(
            GENERATION_FAULT_POLICY.pack(
                string_offset(f"{name}:fault"), thread, PLAN_NONE, fault_endpoint, process + 1, 1
            )
        )
        service_records.extend(
            GENERATION_SERVICE_BINDING.pack(
                process, SERVICE_ROOT_DISPATCH, 1, fault_endpoint, 1, process + 1, 0
            )
        )
        cap_records.extend(
            GENERATION_CAP_BINDING.pack(process, 2, tcb, CAP_RIGHT_ALL, 0, PLAN_NONE, 0)
        )
        cap_records.extend(
            GENERATION_CAP_BINDING.pack(process, 3, fault_endpoint, 1, process + 1, PLAN_NONE, 0)
        )
        quota_records.extend(
            GENERATION_RESOURCE_QUOTA.pack(
                string_offset(f"{name}:quota"), process, 1, 1, 2, 0, 2, 4, 6, 0, 64, 1 << 20, 0, 0
            )
        )

    process_for_instance = {
        instance["name"]: index for index, instance in enumerate(planned_instances)
    }
    for grant_index, (grant, rights) in enumerate(zip(grants, grant_rights, strict=True)):
        source_process = process_for_instance[grant["source"]]
        bound = next(
            (binding for binding in instances[instance_index[grant["source"]]]["bindings"] if binding["grant"] == grant["name"]),
            None,
        )
        if bound is None:
            fail(f"authority-bearing grant {grant['name']} has no concrete binding")
        if rights & RIGHT["exec"]:
            target = executable_index[grant["target"]]
            spawn_records.extend(
                GENERATION_SPAWN_TEMPLATE.pack(
                    string_offset(grant["name"]), target, source_process, source_process, source_process, source_process, 1, 0
                )
            )
            cap_records.extend(
                GENERATION_CAP_BINDING.pack(source_process, bound["slot"], object_index[(grant["source"], "tcb")], rights, 0, grant_index, 0)
            )
        elif rights & (RIGHT["send"] | RIGHT["recv"]):
            endpoint = len(object_index)
            object_index[(grant["name"], "endpoint")] = endpoint
            kernel_records.extend(
                GENERATION_KERNEL_OBJECT.pack(
                    string_offset(f"{grant['name']}:endpoint"), KERNEL_OBJECT_ENDPOINT, source_process, 4, 1, PLAN_NONE, 0
                )
            )
            cap_records.extend(
                GENERATION_CAP_BINDING.pack(source_process, bound["slot"], endpoint, rights, 0, grant_index, 0)
            )
        else:
            cap_records.extend(
                GENERATION_CAP_BINDING.pack(source_process, bound["slot"], object_index[(grant["source"], "tcb")], rights, 0, grant_index, GRANT_POLICY_ONLY)
            )
    # Minted bindings: a capability the owner creates at runtime and hands to
    # an instance it owns at spawn. Sorted by name so the section is canonical,
    # and validated here so an unsatisfiable declaration fails before output.
    minted_records = bytearray()
    seen_holder_slots: set[tuple[int, int]] = set()
    for minted in sorted(manifest.get("mintedBindings", []), key=lambda entry: entry["name"]):
        owner = instance_index.get(minted["owner"])
        holder = instance_index.get(minted["holder"])
        if owner is None or holder is None:
            fail(f"minted binding {minted['name']}: unknown owner or holder")
        if instances[holder]["owner"] != minted["owner"]:
            fail(f"minted binding {minted['name']}: holder is not owned by its minter")
        slot = minted["slot"]
        if not isinstance(slot, int) or not 0 <= slot < 64:
            fail(f"minted binding {minted['name']}: slot outside capability table")
        if (holder, slot) in seen_holder_slots:
            fail(f"minted binding {minted['name']}: duplicate holder slot")
        seen_holder_slots.add((holder, slot))
        rights = 0
        for right in minted["rights"]:
            if right not in RIGHT:
                fail(f"minted binding {minted['name']}: unknown right {right}")
            rights |= RIGHT[right]
        if rights == 0 or rights & RIGHT["exec"]:
            fail(f"minted binding {minted['name']}: invalid rights")
        minted_records.extend(
            GENERATION_MINTED_BINDING.pack(
                string_offset(minted["name"]), owner, holder, slot, rights, 0
            )
        )


    counts = (
        len(process_records) // GENERATION_PROCESS.size,
        len(thread_records) // GENERATION_THREAD.size,
        len(kernel_records) // GENERATION_KERNEL_OBJECT.size,
        len(mapping_records) // GENERATION_MAPPING.size,
        len(cap_records) // GENERATION_CAP_BINDING.size,
        len(service_records) // GENERATION_SERVICE_BINDING.size,
        len(schedule_records) // GENERATION_SCHEDULE.size,
        len(fault_records) // GENERATION_FAULT_POLICY.size,
        len(spawn_records) // GENERATION_SPAWN_TEMPLATE.size,
        len(quota_records) // GENERATION_RESOURCE_QUOTA.size,
        len(minted_records) // GENERATION_MINTED_BINDING.size,
    )
    limits = (
        MAX_PROCESSES, MAX_THREADS, MAX_KERNEL_OBJECTS, MAX_MAPPINGS, MAX_CAP_BINDINGS,
        MAX_SERVICE_BINDINGS, MAX_SCHEDULES, MAX_FAULT_POLICIES, MAX_SPAWN_TEMPLATES,
        MAX_RESOURCE_QUOTAS, MAX_MINTED_BINDINGS,
    )
    if any(count > limit for count, limit in zip(counts, limits, strict=True)):
        fail("seL4 execution plan count exceeds bound")
    return (
        process_records, thread_records, kernel_records, mapping_records, cap_records,
        service_records, schedule_records, fault_records, spawn_records, quota_records,
        minted_records, counts,
    )




def layout_executables(manifest: dict) -> set[str]:
    """Executables the initial graph addresses through its boot slot table."""
    initial = {instance["name"] for instance in manifest["instances"]}
    names = {instance["executable"] for instance in manifest["instances"]}
    names.update(
        grant["target"]
        for grant in manifest["grants"]
        if grant["source"] in initial and "exec" in grant["rights"]
    )
    return names


def build_generation(manifest: dict, payloads: dict[str, bytes], parent: bytes | None, number: int, profile: TargetProfile) -> bytes:
    declared_layout_executables = layout_executables(manifest)
    if "boot-layout" in {object_["id"] for object_ in manifest["objects"]}:
        payloads = dict(payloads)
        payloads["boot-layout"] = build_boot_layout(number, fail, declared_layout_executables)
    objects = unique_sorted(manifest["objects"], "id", "object ids")
    executables = unique_sorted(manifest["executables"], "name", "executable names")
    instances = unique_sorted(manifest["instances"], "name", "instance names")
    grants = sorted(manifest["grants"], key=lambda grant: (grant["name"], grant["source"], grant["target"]))
    states = unique_sorted(manifest["state"], "name", "state names")
    if len({(grant["name"], grant["source"], grant["target"]) for grant in grants}) != len(grants):
        fail("grant identities must be unique")
    if not 1 <= len(objects) <= MAX_OBJECTS or not 1 <= len(executables) <= MAX_EXECUTABLES or not 1 <= len(instances) <= MAX_INSTANCES or len(grants) > MAX_GRANTS or len(states) > MAX_STATES:
        fail("manifest count exceeds bound")
    validate_acyclic(instances)
    object_index = {object_["id"]: index for index, object_ in enumerate(objects)}
    executable_index = {executable["name"]: index for index, executable in enumerate(executables)}
    instance_index = {instance["name"]: index for index, instance in enumerate(instances)}
    grant_index = {grant["name"]: index for index, grant in enumerate(grants)}
    if len(grant_index) != len(grants):
        fail("grant names must be unique")
    if manifest["target"] != profile.name:
        fail(f"manifest target {manifest['target']!r} does not match resolved profile {profile.name!r}")
    bootstrap = instance_index.get(manifest["bootstrapInstance"])
    if bootstrap is None:
        fail("bootstrapInstance must name an instance")

    strings = bytearray()
    offsets: dict[str, int] = {}
    def string_offset(value: str) -> int:
        if value in offsets:
            return offsets[value]
        encoded = value.encode("utf-8")
        if len(encoded) > MAX_STRING_BYTES:
            fail("string exceeds bound")
        offset = len(strings)
        strings.extend(struct.pack("<H", len(encoded)))
        strings.extend(encoded)
        offsets[value] = offset
        if len(strings) > MAX_STRING_TABLE_BYTES:
            fail("string table exceeds bound")
        return offset

    target_offset = string_offset(manifest["target"])
    boot_action_offset = string_offset(manifest["bootAction"])
    for object_ in objects: string_offset(object_["id"])
    for executable in executables: string_offset(executable["name"])
    for instance in instances: string_offset(instance["name"])
    for grant in grants: string_offset(grant["name"])
    for state in states: string_offset(state["name"])

    grant_rights: list[int] = []
    for grant in grants:
        rights = 0
        for right in grant["rights"]:
            if right not in RIGHT:
                fail(f"unsupported right {right}")
            rights |= RIGHT[right]
        transferable = int(bool(grant["transferable"]))
        rights |= RIGHT_TRANSFER if transferable else 0
        if rights == 0 or rights & ~RIGHT_ALL:
            fail(f"invalid rights for {grant['name']}")
        grant_rights.append(rights)

    expected_bindings: dict[str, set[str]] = {name: set() for name in instance_index}
    for grant, rights in zip(grants, grant_rights, strict=True):
        source = instance_index.get(grant["source"])
        if source is None:
            fail(f"grant source missing: {grant['name']}")
        if rights & RIGHT["exec"]:
            expected_bindings[grant["source"]].add(grant["name"])
            if executable_index.get(grant["target"]) is None:
                fail(f"executable grant target missing: {grant['name']}")
        else:
            target = instance_index.get(grant["target"])
            if target is None:
                fail(f"grant target missing: {grant['name']}")
            if rights & (RIGHT["send"] | RIGHT["recv"]):
                expected_bindings[grant["source"]].add(grant["name"])
            expected_bindings[grant["target"]].add(grant["name"])

    dependency_records = bytearray()
    binding_records = bytearray()
    instance_rows: list[tuple] = []
    dependency_count = 0
    binding_count = 0
    required_from_instances: set[str] = set()
    for instance in instances:
        executable = executable_index.get(instance["executable"])
        if executable is None:
            fail(f"instance {instance['name']}: missing executable")
        owner = instance["owner"]
        if owner == "root":
            owner_kind, owner_index = 0, 0
        else:
            owner_kind, owner_index = 1, instance_index.get(owner, -1)
            if owner_index < 0 or owner == instance["name"]:
                fail(f"instance {instance['name']}: invalid owner")
        autostart = instance["autostart"]
        if not isinstance(autostart, bool):
            fail(f"instance {instance['name']}: invalid autostart")
        dependencies = sorted(instance["dependencies"])
        if autostart:
            for dependency in dependencies:
                depended = instances[instance_index[dependency]]
                if depended["owner"] == "root" and not depended["autostart"]:
                    fail(f"instance {instance['name']}: autostart dependency is inactive")
        dependency_start = dependency_count
        for dependency in dependencies:
            dependency_records += GENERATION_DEPENDENCY.pack(instance_index[dependency])
            dependency_count += 1
        declared = instance["bindings"]
        names = [binding["grant"] for binding in declared]
        slots = [binding["slot"] for binding in declared]
        if len(set(names)) != len(names) or len(set(slots)) != len(slots):
            fail(f"instance {instance['name']}: duplicate binding grant or slot")
        if any(not isinstance(slot, int) or not 0 <= slot < 64 for slot in slots):
            fail(f"instance {instance['name']}: binding slot outside capability table")
        expected = expected_bindings[instance["name"]]
        extra = set(names) - expected
        for name in extra:
            grant = grants[grant_index[name]] if name in grant_index else None
            if grant is None or grant["source"] not in (instance["name"], owner):
                fail(f"instance {instance['name']}: binding names unrelated grant")
            rights = grant_rights[grant_index[name]]
            delegated_to_instance = grant["target"] == instance["name"]
            delegated_from_owner = grant["source"] == owner
            delegated_to_owned_instance = any(
                child["owner"] == instance["name"] and child["name"] == grant["target"]
                for child in instances
            )
            delegated_to_owned_executable = bool(rights & RIGHT["exec"]) and any(
                child["owner"] == instance["name"] and child["executable"] == grant["target"]
                for child in instances
            )
            if not delegated_from_owner and not delegated_to_instance and not delegated_to_owned_instance and not delegated_to_owned_executable:
                fail(f"instance {instance['name']}: binding names unrelated grant")
        if not expected.issubset(names):
            fail(f"instance {instance['name']}: bindings do not close over related grants")
        binding_start = binding_count
        for binding in sorted(declared, key=lambda binding: binding["slot"]):
            grant = grant_index.get(binding["grant"])
            if grant is None:
                fail(f"instance {instance['name']}: binding names unknown grant")
            binding_records += GENERATION_BINDING.pack(grant, binding["slot"])
            binding_count += 1
        if instance["health"] not in ("required", "optional"):
            fail(f"instance {instance['name']}: invalid health")
        health = int(instance["health"] == "required")
        if health:
            required_from_instances.add(instance["name"])
        instance_rows.append((string_offset(instance["name"]), executable, owner_kind, owner_index, int(autostart), dependency_start, len(dependencies), binding_start, len(declared), health))
    if dependency_count > MAX_DEPENDENCIES or binding_count > MAX_BINDINGS:
        fail("dependency or binding count exceeds bound")

    health = manifest["health"]
    required = sorted(health["requiredInstances"])
    if health["bootAttempts"] <= 0 or len(required) > MAX_HEALTH_INSTANCES or len(set(required)) != len(required) or set(required) != required_from_instances:
        fail("invalid health policy")

    object_records = bytearray()
    executable_records = bytearray()
    instance_records = bytearray()
    grant_records = bytearray()
    state_records = bytearray()
    (
        process_records,
        thread_records,
        kernel_object_records,
        mapping_records,
        cap_binding_records,
        service_binding_records,
        schedule_records,
        fault_policy_records,
        spawn_template_records,
        resource_quota_records,
        minted_binding_records,
        plan_counts,
    ) = build_sel4_plan(
        manifest,
        instances,
        executables,
        grants,
        grant_rights,
        instance_index,
        executable_index,
        string_offset,
    )
    health_records = bytearray()
    blobs = bytearray()
    plan_bytes = sum(
        len(records)
        for records in (
            process_records,
            thread_records,
            kernel_object_records,
            mapping_records,
            cap_binding_records,
            service_binding_records,
            schedule_records,
            fault_policy_records,
            spawn_template_records,
            resource_quota_records,
            minted_binding_records,
        )
    )
    payload_start = (
        GENERATION_HEADER.size
        + len(objects) * GENERATION_OBJECT.size
        + len(executables) * GENERATION_EXECUTABLE.size
        + len(instances) * GENERATION_INSTANCE.size
        + len(dependency_records)
        + len(binding_records)
        + len(grants) * GENERATION_GRANT.size
        + len(states) * GENERATION_STATE.size
        + len(required) * GENERATION_HEALTH.size
        + plan_bytes
        + len(strings)
    )
    for object_ in objects:
        if object_["kind"] not in KIND:
            fail(f"unsupported object kind {object_['kind']}")
        payload = payloads.get(object_["id"])
        if payload is None or len(payload) > MAX_OBJECT_PAYLOAD_BYTES:
            fail(f"missing or oversized payload for {object_['id']}")
        object_records += GENERATION_OBJECT.pack(
            string_offset(object_["id"]),
            KIND[object_["kind"]],
            payload_start + len(blobs),
            len(payload),
            sha256(payload),
        )
        blobs += payload
    for executable in executables:
        object_ = object_index.get(executable["object"])
        if object_ is None or objects[object_]["kind"] not in ("bootstrap", "component"):
            fail(f"executable {executable['name']}: invalid object")
        if executable["role"] not in ROLE:
            fail(f"executable {executable['name']}: unsupported role")
        spawn_budget = executable["spawnBudget"]
        if not isinstance(spawn_budget, int) or not 0 <= spawn_budget <= MAX_SPAWN_BUDGET:
            fail(f"executable {executable['name']}: invalid spawn budget")
        executable_records += GENERATION_EXECUTABLE.pack(
            string_offset(executable["name"]), object_, ROLE[executable["role"]], spawn_budget
        )
    bootstrap_instance = instances[bootstrap]
    bootstrap_executable = executables[executable_index[bootstrap_instance["executable"]]]
    if bootstrap_instance["owner"] != "root" or not bootstrap_instance["autostart"] or bootstrap_executable["role"] != "init" or objects[object_index[bootstrap_executable["object"]]]["kind"] != "bootstrap":
        fail("bootstrap instance must be root-owned autostart init/bootstrap")
    for row in instance_rows: instance_records += GENERATION_INSTANCE.pack(*row)
    for grant, rights in zip(grants, grant_rights, strict=True):
        source = instance_index[grant["source"]]
        target = executable_index[grant["target"]] if rights & RIGHT["exec"] else instance_index[grant["target"]]
        grant_records += GENERATION_GRANT.pack(string_offset(grant["name"]), source, target, rights, int(bool(grant["transferable"])))
    for state in states:
        owner = instance_index.get(state["owner"])
        if owner is None or state["schemaVersion"] <= 0 or state["policy"] not in POLICY:
            fail(f"invalid state {state['name']}")
        state_records += GENERATION_STATE.pack(string_offset(state["name"]), owner, state["schemaVersion"], POLICY[state["policy"]])
    for name in required: health_records += GENERATION_HEALTH.pack(instance_index[name])

    object_offset = GENERATION_HEADER.size
    executable_offset = object_offset + len(object_records)
    instance_offset = executable_offset + len(executable_records)
    dependency_offset = instance_offset + len(instance_records)
    binding_offset = dependency_offset + len(dependency_records)
    grant_offset = binding_offset + len(binding_records)
    state_offset = grant_offset + len(grant_records)
    health_offset = state_offset + len(state_records)
    process_offset = health_offset + len(health_records)
    thread_offset = process_offset + len(process_records)
    kernel_object_offset = thread_offset + len(thread_records)
    mapping_offset = kernel_object_offset + len(kernel_object_records)
    cap_binding_offset = mapping_offset + len(mapping_records)
    service_binding_offset = cap_binding_offset + len(cap_binding_records)
    schedule_offset = service_binding_offset + len(service_binding_records)
    fault_policy_offset = schedule_offset + len(schedule_records)
    spawn_template_offset = fault_policy_offset + len(fault_policy_records)
    resource_quota_offset = spawn_template_offset + len(spawn_template_records)
    minted_binding_offset = resource_quota_offset + len(resource_quota_records)
    string_table_offset = minted_binding_offset + len(minted_binding_records)
    actual_payload_offset = string_table_offset + len(strings)
    if actual_payload_offset != payload_start:
        fail("internal payload offset mismatch")
    total_len = actual_payload_offset + len(blobs)
    if total_len > MAX_GENERATION_BYTES:
        fail("generation exceeds bound")
    header = GENERATION_HEADER.pack(
        GENERATION_MAGIC, GENERATION_VERSION, GENERATION_HEADER.size, 0, bytes(32), number,
        parent or bytes(32), target_offset, boot_action_offset, bootstrap, health["bootAttempts"], len(objects),
        len(executables), len(instances), dependency_count, binding_count, len(grants),
        len(states), len(required), *plan_counts, 0, object_offset, executable_offset,
        instance_offset, dependency_offset, binding_offset, grant_offset, state_offset,
        health_offset, process_offset, thread_offset, kernel_object_offset, mapping_offset,
        cap_binding_offset, service_binding_offset, schedule_offset, fault_policy_offset,
        spawn_template_offset, resource_quota_offset, minted_binding_offset,
        string_table_offset, len(strings),
        actual_payload_offset, total_len,
    )
    generation = bytearray(
        header + object_records + executable_records + instance_records + dependency_records
        + binding_records + grant_records + state_records + health_records + process_records
        + thread_records + kernel_object_records + mapping_records + cap_binding_records
        + service_binding_records + schedule_records + fault_policy_records
        + spawn_template_records + resource_quota_records + minted_binding_records
        + strings + blobs
    )
    generation[24:56] = generation_identity(generation)
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


SELECTOR_GENERATION_BYTES = 8 * 1024 * 1024


def build_bootstore(generations: list[bytes]) -> bytes:
    if any(len(generation) > SELECTOR_GENERATION_BYTES for generation in generations):
        fail(f"generation exceeds selector ceiling ({SELECTOR_GENERATION_BYTES} bytes)")
    release_sequences = [index + 1 for index in range(len(generations))]
    pending_sequence = os.environ.get("SLIME_PENDING_RELEASE_SEQUENCE")
    if pending_sequence is not None:
        release_sequences[-1] = int(pending_sequence)
    boot_bundle_hex = os.environ.get("SLIME_BOOT_BUNDLE_IDENTITY")
    try:
        boot_bundle = bytes.fromhex(boot_bundle_hex) if boot_bundle_hex is not None else None
    except ValueError:
        fail("SLIME_BOOT_BUNDLE_IDENTITY must be a nonzero 32-byte hex digest")
    if boot_bundle is not None and (len(boot_bundle) != 32 or boot_bundle == bytes(32)):
        fail("SLIME_BOOT_BUNDLE_IDENTITY must be a nonzero 32-byte hex digest")
    entries = sorted(
        (
            generation[24:56],
            generation,
            build_release(generation, release_sequences[index], boot_bundle_identity=boot_bundle),
        )
        for index, generation in enumerate(generations)
    )
    entries.sort(key=lambda item: item[0])
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




def bootstrap_binding_projection(manifest: dict) -> tuple[dict[str, int], dict[str, int]]:
    """Project explicit bootstrap bindings onto the compile-time slot API."""
    bootstrap = manifest["bootstrapInstance"]
    instance = next(item for item in manifest["instances"] if item["name"] == bootstrap)
    grants_by_name = {grant["name"]: grant for grant in manifest["grants"]}
    binding_slots: dict[str, int] = {}
    role_bindings: dict[str, int] = {}
    right_roles = {
        "endpointCreate": "endpoint-factory",
        "bufferCreate": "shared-buffer-factory",
        "inputRead": "input",
    }
    for binding in instance["bindings"]:
        grant = grants_by_name[binding["grant"]]
        if "exec" in grant["rights"]:
            binding_slots[grant["target"]] = binding["slot"]
        elif set(grant["rights"]) & {"send", "recv"}:
            binding_slots[grant["name"]] = binding["slot"]
        for right, role in right_roles.items():
            if right in grant["rights"]:
                role_bindings[role] = binding["slot"]
    channel_aliases = {"spawn-service-rpc": "service-spawn"}
    for source, alias in channel_aliases.items():
        if source in binding_slots:
            binding_slots[alias] = binding_slots[source]
    return binding_slots, role_bindings


def build_sel4_generation(output: Path, manifest: dict, target_profile: TargetProfile) -> None:
    """Build the `aarch64-sel4-qemu-virt` generation (P5.2).

    This is the product generation path. seL4 is the kernel, so the generation
    carries the pinned external-kernel identity required by the format but no
    custom-kernel executable. Recovery, storage, and generation management run
    as userspace planes selected by their manifests.

    A fabric graph is *conditional* rather than absent (P5.5.2). Four of the
    five seL4 manifests declare none, and for those the C8 resolution has
    nothing to resolve — that was true of every seL4 manifest until the stream
    plane arrived. `sel4-stream.zti` declares one, because `fabric-service`
    reads its route table, participant list, and control-slot base out of the
    generated profile at compile time: a graph it cannot resolve is a component
    that does not build, not one that runs without routes.

    What is shared is what matters: the same `build_generation` encoder, the
    same boot-layout resource, the same shared-buffer budget encoding, and the
    same digest-authenticated object closure. The generation this writes is a
    generation in exactly the sense every other one is.
    """
    # P5.5.2: a manifest that declares a fabric graph resolves it through the
    # same function every x86 profile uses, so a seL4 route identity, QoS row,
    # and control-slot base are folded from the same schemas and the same
    # validation rather than from a second implementation. A manifest that
    # declares none gets the empty profile the four earlier seL4 graphs get.
    resolved_profile = None
    profile_path = output / "sel4-fabric-profile.rs"
    if manifest.get("fabricGraph"):
        interfaces = validate_interface_schemas(manifest["interfaceSchemas"])
        resolved_profile = resolve_fabric_profile(
            manifest, interfaces, manifest["fabricGraph"]["profiles"][0]["name"]
        )
        profile_path.write_text(
            render_fabric_profile_rust(resolved_profile), encoding="utf-8"
        )
    else:
        profile_path.write_text("", encoding="utf-8")
    # P5.3.1: the channel graph's `init` needs its scenario compiled in, and
    # `init.rs` selects that with `option_env!`. Set from the manifest being
    # built rather than inherited, so the flag and the graph cannot disagree;
    # `build_rust_components` pops it for every other build.
    selected = os.environ.get("SLIME_SEL4_MANIFEST")
    #
    # A manifest may name more than one flag. P5.4.5's QoS plane is the stream
    # driver plus a clock, so it sets `SLIME_SEL4_STREAM_CHECK` — which selects
    # that driver in `init.rs` — *and* the oracle's own
    # `SLIME_FABRIC_QOS_CHECK`, which is what `fabric-service`,
    # `fabric-publisher-b`, and `fabric-subscriber-b` read to select their QoS
    # behaviour. Reusing the oracle's flag is what keeps those three components
    # byte-identical between the two seL4 planes; `init.rs::qos_plane` requires
    # both, so the x86 QoS generation cannot walk this composition.
    wanted: set[str] = set()
    declared: set[str] = set()
    for manifest_name, flags in (
        ("sel4-channel", ("SLIME_SEL4_CHANNEL_CHECK",)),
        ("sel4-loan", ("SLIME_SEL4_LOAN_CHECK",)),
        ("sel4-spawn", ("SLIME_SEL4_SPAWN_CHECK",)),
        ("sel4-sample", ("SLIME_SEL4_SAMPLE_CHECK",)),
        ("sel4-stream", ("SLIME_SEL4_STREAM_CHECK",)),
        ("sel4-qos", ("SLIME_SEL4_STREAM_CHECK", "SLIME_FABRIC_QOS_CHECK")),
        ("sel4-supervision", ("SLIME_SEL4_SUPERVISION_CHECK",)),
        ("sel4-reclamation", ("SLIME_SEL4_RECLAMATION_CHECK",)),
        ("sel4-crossing", ("SLIME_SEL4_CROSSING_CHECK",)),
        # P5.4.6: its own flag rather than the oracle's
        # `SLIME_FABRIC_CALL_CHECK`. The call *broker* is the same code on both
        # planes — `fabric-service` selects it on either flag, which is what
        # keeps them from diverging — but init's composition differs, because
        # this plane mints its control channels instead of reading them from
        # the base boot layout. Sharing one flag made generation 18 walk
        # generation 14's layout.
        ("sel4-call", ("SLIME_SEL4_CALL_CHECK",)),
        # P5.4.7: the seL4 flag composes the plane in `init.rs`, and the
        # oracle's flag selects the unmodified operation broker and
        # participants — the property this gate exists to demonstrate.
        (
            "sel4-operation",
            ("SLIME_SEL4_OPERATION_CHECK", "SLIME_FABRIC_OPERATION_CHECK"),
        ),
        # P5.4.8, on the operation row's rule.
        (
            "sel4-visibility",
            ("SLIME_SEL4_VISIBILITY_CHECK", "SLIME_FABRIC_VISIBILITY_CHECK"),
        ),
        # P5.4.9, on the same rule. Every participant's full-graph behaviour is
        # selected by the oracle's `SLIME_FABRIC_BOOT_CHECK` through
        # `fabric_boot::active`; only init's composition is seL4's.
        ("sel4-boot", ("SLIME_SEL4_BOOT_CHECK", "SLIME_FABRIC_BOOT_CHECK")),
        # P5.4.2c: its own flag, and no oracle counterpart. The x86 storage
        # probe reads through `buffer_phys`, an ambient pointer the retired
        # kernel dereferences; the seL4 payload crosses in the transfer window,
        # so the two components do not share a body.
        ("sel4-storage", ("SLIME_SEL4_STORAGE_CHECK",)),
        # P5.4.2c's second half, and likewise no oracle counterpart: M5.4 policy
        # runs in userspace here, where the oracle keeps it in `store_service`.
        ("sel4-store", ("SLIME_SEL4_STORE_CHECK",)),
        # M5.6, likewise userspace: the transition model is `boot_contracts`.
        ("sel4-rollback", ("SLIME_SEL4_ROLLBACK_CHECK",)),
        # M5.9, likewise.
        ("sel4-recovery", ("SLIME_SEL4_RECOVERY_PLANE_CHECK",)),
        # M6.5, P5.4.3: the generation service moves to userspace too.
        ("sel4-generation", ("SLIME_SEL4_GENERATION_CHECK",)),
        # M6.3: the directory capability mechanism the root now owns.
        ("sel4-directory", ("SLIME_SEL4_DIRECTORY_CHECK",)),
        # M6.3's other half: the filesystem service over that mechanism.
        ("sel4-filesystem", ("SLIME_SEL4_FILESYSTEM_CHECK",)),
        # M6.4: a Dango session over the scripted key source.
        ("sel4-dango", ("SLIME_SEL4_DANGO_CHECK",)),
        # The input mechanism M6.4 sits on, gated on its own.
        ("sel4-input", ("SLIME_SEL4_INPUT_CHECK",)),
        # M6.6: a chooser handing one narrowed view to a requester.
        ("sel4-powerbox", ("SLIME_SEL4_POWERBOX_CHECK",)),
        # M6.7: a generation crossing a persistence boundary.
        ("sel4-transfer", ("SLIME_SEL4_TRANSFER_CHECK",)),
    ):
        # Set-then-scrub in one pass would let a later row pop a flag an earlier
        # row set: `sel4-qos` and `sel4-stream` share
        # `SLIME_SEL4_STREAM_CHECK`, so iterating the table and popping for
        # every non-selected manifest cleared the stream plane's own flag. The
        # selected manifest's flags are collected first and every other flag in
        # the table is removed, so a flag two manifests share survives for the
        # one that asked for it.
        if selected == manifest_name:
            wanted.update(flags)
        declared.update(flags)
    for flag in declared:
        if flag in wanted:
            os.environ[flag] = "1"
        else:
            os.environ.pop(flag, None)
    executable_names = {executable["name"] for executable in manifest["executables"]}
    binding_slots, role_bindings = bootstrap_binding_projection(manifest)
    built = build_rust_components(
        manifest["generation"],
        profile_path,
        target_profile,
        candidate_identity=None,
        components=executable_names,
        binding_slots=binding_slots,
        role_bindings=role_bindings,
    )
    payloads: dict[str, bytes] = {}
    object_ids = {object_["id"] for object_ in manifest["objects"]}
    # seL4 is external to the generation-v4 object closure; there is no
    # kernel-object header field or synthetic marker payload.
    # The authenticated C8.2 graph, byte-identical to what an x86 generation
    # carries for the same declaration. `slime-root` does not read it — the
    # fabric is userspace policy and the root knows nothing of routes — but it
    # is part of the object closure the root re-checks, so a graph the builder
    # resolved and then failed to carry would fail admission rather than boot
    # with an unauthenticated one.
    if resolved_profile is not None:
        if "fabric-graph" not in object_ids:
            fail("fabricGraph declared without a fabric-graph resource object")
        payloads["fabric-graph"] = resolved_profile.graph_bytes
    elif "fabric-graph" in object_ids:
        fail("fabric-graph resource object declared without a fabricGraph")
    if "shared-buffer-budget" in object_ids:
        payloads["shared-buffer-budget"] = build_shared_buffer_budget(
            manifest.get("sharedBufferBudget", [])
        )
    for executable in manifest["executables"]:
        stack = executable.get("stackBytes", COMPONENT_DEFAULT_STACK_BYTES)
        if (
            not isinstance(stack, int)
            or stack <= 0
            or stack % target_profile.page_bytes
            or stack > COMPONENT_MAX_STACK_BYTES
        ):
            fail(f"executable {executable['name']}: invalid stack")
        if executable["object"] not in object_ids:
            fail(f"executable {executable['name']}: missing object")
        payloads[executable["object"]] = component_image(
            executable["name"],
            component_executable(built, executable["name"], target_profile),
            stack,
            target_profile,
        )

    generation = build_generation(manifest, payloads, None, manifest["generation"], target_profile)
    # Build the compatibility alias independently from the same resolved inputs.
    # Determinism checks can then detect hidden mutable builder state instead of
    # comparing a file copied from the bytes beside it.
    generation_one = build_generation(
        manifest, payloads, None, manifest["generation"], target_profile
    )
    bootstore = build_bootstore([generation])
    (output / "generation.bin").write_bytes(generation)
    (output / "generation-1.bin").write_bytes(generation_one)
    (output / "boot-store.bin").write_bytes(bootstore)
    print(f"Built seL4 generation {generation[24:56].hex()} target={target_profile.name}")
    print(f"Built boot-store.bin ({len(bootstore)} bytes)")


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: build-generation.py <output-dir>")
    output = Path(sys.argv[1]).resolve()
    manifest = load_manifest()
    # The manifest names the profile a generation is built for. The optional
    # override rewrites that declaration before the closed profile lookup, so
    # downstream admission still sees one authoritative target value.
    requested_target = os.environ.get("SLIME_TARGET_PROFILE")
    if requested_target:
        manifest["target"] = requested_target
    requested_generation = os.environ.get("SLIME_GENERATION_NUMBER")
    if requested_generation is not None:
        try:
            generation_number = int(requested_generation)
        except ValueError:
            fail("SLIME_GENERATION_NUMBER must be a positive integer")
        if generation_number <= 0:
            fail("SLIME_GENERATION_NUMBER must be a positive integer")
        manifest["generation"] = generation_number
    target_profile = resolve_target_profile(manifest.get("target"))
    if manifest["formatVersion"] != 1:
        fail("unsupported source formatVersion")
    if target_profile.name != SEL4_TARGET_PROFILE:
        fail("custom-kernel generation builds were retired with P5; select a seL4 manifest")
    output.mkdir(parents=True, exist_ok=True)
    build_sel4_generation(output, manifest, target_profile)


if __name__ == "__main__":
    main()
