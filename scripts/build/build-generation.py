#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))
# This script's own directory, for sibling modules when imported by host checks.
_sys.path.insert(0, str(_Path(__file__).resolve().parent))

import argparse
import copy
import json
import os
import struct
import subprocess
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
    FABRIC_GRAPH_FRAME_CAPACITY,
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
    FABRIC_GRAPH_LIMIT_TRACE_DEPTH,
    FABRIC_GRAPH_TRACE_OVERFLOW_SATURATE,
    FABRIC_GRAPH_TRACE_TERMINAL_RESERVE,
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
    MAX_FABRIC_GRAPH_ROLE_PARTICIPANTS,
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
    GENERATION_RIGHT_ALL,
    GENERATION_RIGHT_BY_MANIFEST_NAME,
    GENERATION_RIGHT_TRANSFER,
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
    GENERATION_NOTIFICATION_GRANT,
    GENERATION_NOTIFICATION_BINDING,
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
    MAX_NOTIFICATION_GRANTS,
    MAX_NOTIFICATION_BINDINGS,
    MAX_RESOURCE_QUOTAS,
    SHARED_BUFFER_BUDGET_ENTRY,
    SHARED_BUFFER_BUDGET_HEADER,
    SHARED_BUFFER_BUDGET_HEADER_BYTES,
    SHARED_BUFFER_BUDGET_ENTRY_BYTES,
    SHARED_BUFFER_BUDGET_MAGIC,
    SHARED_BUFFER_BUDGET_VERSION,
    MAX_SHARED_BUFFER_BUDGET_HOLDERS,
    PRIVATE_MEMORY_BUDGET_ENTRY,
    PRIVATE_MEMORY_BUDGET_HEADER,
    PRIVATE_MEMORY_BUDGET_HEADER_BYTES,
    PRIVATE_MEMORY_BUDGET_ENTRY_BYTES,
    PRIVATE_MEMORY_BUDGET_MAGIC,
    PRIVATE_MEMORY_BUDGET_VERSION,
    MAX_PRIVATE_MEMORY_BUDGET_HOLDERS,
    PRIVATE_MEMORY_ROOT_REGION_PAGES,
    PRIVATE_MEMORY_ROOT_TOTAL_PAGES,
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
from boot_layout import build_boot_layout, layout_from_manifest
from fabric_trace_contract import (
    FABRIC_TRACE_MAX_DEPTH,
    FABRIC_TRACE_OVERFLOW_SATURATE,
    FABRIC_TRACE_TERMINAL_RESERVE,
)
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
# Additional seL4 manifests carry distinct authenticated boot actions and
# generation-derived component tables while sharing the same target profile.
SEL4_MANIFESTS = {
    "sel4": SEL4_SOURCE,
    # RP2: the demo-scoped slice — one generation that both launches the
    # product component graph and runs the bounded data path.
    "sel4-demo": ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-demo.zti",
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
    # C10.2: one executable declared twice, as a granted holder and an omitted
    # one, against a generation-declared private-memory budget.
    "sel4-private-memory": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-private-memory.zti",
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
    # 48 instances: the admitted ceiling, so the graph that boots is the
    # largest one admission will accept (B49).
    "sel4-stress": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-stress.zti",
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
    "sel4-traffic": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-traffic.zti",
    "sel4-matrix": ROOT
    / "contracts"
    / "generation"
    / "v1"
    / "fixtures"
    / "sel4-matrix.zti",
    # C8.12's negative arm shares this manifest (B62): one `telemetry-alt`
    # publisher is weakened to BEST_EFFORT against its RELIABLE subscriber
    # through a declared per-variant QoS override, rather than by a second
    # 1069-line copy. The builder emits the incompatible graph — pairwise QoS is
    # not a shape property — and `slime-root` refuses it at admission, which is
    # where that rule lives.
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
# The components whose crates declare `boot-contracts/gpt` and `slime-rt/heap`,
# and so link a `#[global_allocator]`. CP3 scoped that allocator to exactly
# these six by moving the feature declaration into their own manifests; this set
# is what keeps the *build* honest about it, because Cargo unifies features
# across every package named in one invocation, so a plain component compiled
# alongside one of these would silently gain the heap too.
#
# Declared here rather than read from the manifests because it must be the
# builder's own statement of the split: `just component_crate_split_check`
# compares it against what the crates actually declare, so a component gaining
# an allocator without moving groups is a gate failure rather than a silent
# regrouping.
STORE_COMPONENTS = frozenset(
    {
        "sel4-filesystem-service",
        "sel4-generation-manager",
        "sel4-recovery-probe",
        "sel4-rollback-probe",
        "sel4-store-probe",
        "sel4-transfer-probe",
    }
)
# C10.3's private-region allocator, scoped the same way and for the same
# reason: `slime-rt/private-heap` registers a *different*
# `#[global_allocator]`, mutually exclusive with the store plane's, so its
# consumers need a third build group rather than joining either of the other
# two. `just component_crate_split_check` compares this against what the crates
# declare.
PRIVATE_HEAP_COMPONENTS = frozenset({"private-heap-probe"})
PAGE_SIZE = 4096
KIND = {"kernel": 1, "bootstrap": 2, "component": 3, "resource": 4}
ROLE = {"init": 1, "service": 2, "driver": 3, "application": 4}
# Rights numbering is generated-contract truth. Both the manifest-spelling table
# and `RIGHT_ALL` come from `contracts/generation/v5/schema.zt` via
# `boot_contracts`, so the builder, the root, and the oracle cannot disagree
# about which bit a right is or which bits exist. `RIGHT_ALL` is the union of
# the named bits rather than a bit-width mask, which is what closes B57's hole
# at bit 17.
RIGHT = GENERATION_RIGHT_BY_MANIFEST_NAME
RIGHT_TRANSFER = GENERATION_RIGHT_TRANSFER
RIGHT_ALL = GENERATION_RIGHT_ALL

CAPABILITY_KIND = {
    "endpoint": 1,
    "executable": 2,
    "sharedBufferFactory": 3,
    "block": 4,
    "directory": 5,
    "input": 6,
    "supervision": 7,
    "sharedBuffer": 8,
    "loan": 9,
}


SUPERVISION_NAME_SUFFIX = "-supervision"
# The `minted:` resolve string a component builds for a supervision handle, and
# the stack buffer it builds it in. `fabric-service.rs`'s `supervision_slot_for`
# formats `minted:<component>-supervision` into a fixed 64-byte array, because a
# `no_std` component has no allocator; a name that overflowed it would be a
# runtime `fail()` on a real boot. Bounding it here makes that a build failure
# instead, which is the only place the two can be kept in agreement.
SUPERVISION_RESOLVE_PREFIX = "minted:"
SUPERVISION_RESOLVE_NAME_BYTES = 64


def validate_supervision_binding_names(manifest: dict, instances: list) -> None:
    """A supervision binding is named for the task it supervises.

    The `minted:` resolve axis lets a component ask the root which of its slots
    holds a named binding, which is only usable if the name means the same thing
    in every generation that declares it. Three conventions were in the fixtures
    at once — `fabric-publisher-supervision`,
    `fabric-service-supervision-publisher`, and
    `fabric-service-call-client-supervision` — so a component could not ask by
    name without a manifest-specific alias table, which is exactly the
    compile-time coupling B70 exists to remove. Naming the handle for the
    *supervised* task is the one choice that is a property of the graph rather
    than of which manifest happens to declare it: `fabric-service` and
    `fabric-call-worker` both supervise `fabric-call-client`, and under this rule
    both name that handle `fabric-call-client-supervision`.

    Asserted here rather than merely applied, so a fourth convention is a build
    failure rather than a name a component silently cannot resolve. Scoped to
    `supervision`, the one kind whose object is a task identity; other minted
    kinds name channels and have no supervised instance to be named for.

    Instance names may be prefixes of one another — `fabric-op-client-b` and
    `fabric-op-client-b-restart` both exist, each with its own handle — so the
    check strips the suffix and looks up the whole remainder, rather than
    searching for a known instance name inside the string. A substring search
    would resolve `fabric-op-client-b-restart-supervision` to the wrong task.

    The owner clause is what makes the name *answerable*. `resolve_minted_slot`
    scopes to the calling holder, so the name only has to be unique per holder —
    but a handle can only exist if its minter owns the task it names, which the
    holder-ownership check below already requires of the holder. Requiring the
    supervised instance to be owned by the same minter keeps the two halves
    consistent: a name pointing at a task its minter cannot supervise would pass
    the string check and fail at spawn.
    """
    owners = {instance["name"]: instance["owner"] for instance in instances}
    for minted in manifest.get("mintedBindings", []):
        if minted["capabilityKind"] != "supervision":
            continue
        name = minted["name"]
        if not name.endswith(SUPERVISION_NAME_SUFFIX):
            fail(
                f"minted binding {name}: a supervision binding is named "
                f"<supervised-instance>{SUPERVISION_NAME_SUFFIX}"
            )
        supervised = name[: -len(SUPERVISION_NAME_SUFFIX)]
        if supervised not in owners:
            fail(
                f"minted binding {name}: names no declared instance "
                f"({supervised!r} is not an instance in this generation)"
            )
        if supervised == minted["holder"]:
            fail(f"minted binding {name}: holder would supervise itself")
        if owners[supervised] != minted["owner"]:
            fail(
                f"minted binding {name}: supervises an instance owned by "
                f"{owners[supervised]!r}, but is minted by {minted['owner']!r}"
            )
        resolve_bytes = len((SUPERVISION_RESOLVE_PREFIX + name).encode("utf-8"))
        if resolve_bytes > SUPERVISION_RESOLVE_NAME_BYTES:
            fail(
                f"minted binding {name}: its resolve string is {resolve_bytes} "
                f"bytes, over the {SUPERVISION_RESOLVE_NAME_BYTES}-byte buffer a "
                "component formats it in"
            )


def validate_capability_rights(name: str, kind: str, rights: int) -> None:
    masks = {
        "endpoint": RIGHT["send"] | RIGHT["recv"] | RIGHT_TRANSFER,
        "executable": RIGHT["exec"] | RIGHT["spawn"] | RIGHT_TRANSFER,
        "sharedBufferFactory": RIGHT["bufferCreate"] | RIGHT_TRANSFER,
        "block": RIGHT["blockRead"] | RIGHT["blockWrite"],
        "directory": (
            RIGHT["directoryRead"]
            | RIGHT["directoryWrite"]
            | RIGHT["directoryList"]
            | RIGHT["directoryDerive"]
            | RIGHT_TRANSFER
        ),
        "input": RIGHT["inputRead"],
        "supervision": RIGHT["supervise"] | RIGHT_TRANSFER,
        "sharedBuffer": (
            RIGHT["bufferWrite"] | RIGHT["bufferMap"] | RIGHT["bufferLoan"] | RIGHT_TRANSFER
        ),
        "loan": RIGHT["bufferWrite"] | RIGHT["bufferMap"] | RIGHT_TRANSFER,
    }
    required = {
        "endpoint": RIGHT["send"] | RIGHT["recv"],
        "executable": RIGHT["exec"] | RIGHT["spawn"],
        "sharedBufferFactory": RIGHT["bufferCreate"],
        "block": RIGHT["blockRead"] | RIGHT["blockWrite"],
        "directory": (
            RIGHT["directoryRead"]
            | RIGHT["directoryWrite"]
            | RIGHT["directoryList"]
            | RIGHT["directoryDerive"]
        ),
        "input": RIGHT["inputRead"],
        "supervision": RIGHT["supervise"],
        "sharedBuffer": RIGHT["bufferWrite"] | RIGHT["bufferMap"] | RIGHT["bufferLoan"],
        "loan": RIGHT["bufferMap"],
    }
    mask = masks.get(kind)
    if mask is None:
        fail(f"{name}: unknown capability kind {kind!r}")
    if rights == 0 or rights & ~mask or rights & required[kind] == 0:
        fail(f"{name}: rights do not match capability kind {kind}")
    if kind == "executable" and rights & (RIGHT["exec"] | RIGHT["spawn"]) != (
        RIGHT["exec"] | RIGHT["spawn"]
    ):
        fail(f"{name}: executable capability requires exec and spawn")
    if kind == "input" and rights != RIGHT["inputRead"]:
        fail(f"{name}: input capability has an exact inputRead right")
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


def fail(message: str) -> None:
    raise SystemExit(message)
def parse_external_components(values: list[str]) -> dict[str, Path]:
    """Parse the explicit external implementation-name to ELF-path mapping."""
    mappings: dict[str, Path] = {}
    for value in values:
        name, separator, raw_path = value.partition("=")
        if not separator or not name or not raw_path:
            fail("--external-component must be <implementation-name>=<elf-path>")
        if name in mappings:
            fail(f"duplicate external component mapping for {name!r}")
        path = Path(raw_path).expanduser().resolve()
        if not path.is_file():
            fail(f"external component {name!r}: no regular ELF file at {path}")
        mappings[name] = path
    return mappings


def component_specs_for_manifest(
    manifest: dict, component_spec_root: Path | None = None
) -> dict[str, dict]:
    """Resolve specs for executables the component-spec corpus declares.

    Verification-only fixture binaries predate CP0 and are intentionally not
    component-platform declarations. They stay on the workspace path; a spec,
    when present, is authoritative and may select the external path.
    """
    from component_spec import ComponentSpecError, admit_specs, spec_paths

    try:
        paths = spec_paths(component_spec_root) if component_spec_root is not None else None
        admitted = admit_specs(paths)
    except ComponentSpecError as error:
        fail(f"component specification corpus refused: {error}")
    specs: dict[str, dict] = {}
    for executable in manifest["executables"]:
        name = executable["name"]
        matches = [
            entry.spec
            for entry in admitted
            if entry.name == name
            or (
                entry.spec["implementation"]["provider"] != "undeclared"
                and entry.spec["implementation"]["binary"] == name
            )
        ]
        if len(matches) > 1:
            fail(f"executable {name!r}: component specification identity is ambiguous")
        if matches:
            specs[name] = matches[0]
    return specs


def resolve_component_sources(
    manifest: dict,
    external_components: dict[str, Path],
    component_spec_root: Path | None = None,
) -> tuple[dict[str, dict], set[str]]:
    import component_spec_contract
    specs = component_specs_for_manifest(manifest, component_spec_root)

    manifest_names = {executable["name"] for executable in manifest["executables"]}
    workspace = manifest_names - set(specs)
    expected_external: set[str] = set()
    for executable_name, spec in specs.items():
        implementation = spec["implementation"]
        provider = implementation["provider"]
        binary_name = implementation["binary"]
        if provider == component_spec_contract.PROVIDER_WORKSPACE:
            workspace.add(binary_name)
        elif provider == component_spec_contract.PROVIDER_EXTERNAL:
            expected_external.add(binary_name)
        else:
            fail(f"executable {executable_name!r}: implementation is undeclared")
    supplied = set(external_components)
    missing = sorted(expected_external - supplied)
    if missing:
        fail(f"missing external component ELF mapping(s): {missing}")
    unused = sorted(supplied - expected_external)
    if unused:
        fail(f"external component mapping(s) not declared external: {unused}")
    return specs, workspace


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
    return assign_declared_slots(json.loads(output))


def assign_declared_slots(manifest: dict) -> dict:
    """Fill in every omitted `slot`, deterministically and per namespace.

    A manifest may omit `slot` on an instance binding, a minted binding, or a
    notification binding; this assigns the lowest free number in that holder's
    namespace, taking omitted entries in grant-name order so the result is a
    function of the manifest alone. Explicit slots are never moved and are
    reserved before any assignment, so a manifest that pins every number --
    which the byte-frozen boot-layout fixtures do -- encodes exactly as before.

    The namespaces are separate because the runtime regions are. Capability
    bindings and minted bindings share one per holder, since both land in the
    child's capability table and the decoder refuses a duplicate there.
    Notification bindings are their own, relative to the native notification
    region, so a notification at 0 and a capability at 0 do not collide.
    """

    def fill(entries: list, holder_of, limit: int) -> None:
        taken: dict[str, set[int]] = {}
        for entry in entries:
            slot = entry.get("slot")
            if slot is not None:
                taken.setdefault(holder_of(entry), set()).add(slot)
        pending = [entry for entry in entries if entry.get("slot") is None]
        for entry in sorted(pending, key=lambda item: item.get("name") or item.get("grant") or ""):
            holder = holder_of(entry)
            used = taken.setdefault(holder, set())
            slot = next((n for n in range(limit) if n not in used), None)
            if slot is None:
                fail(f"declared slots for {holder} exhaust the {limit}-slot namespace")
            used.add(slot)
            entry["slot"] = slot

    for instance in manifest.get("instances", []):
        # One namespace per holder, shared by both kinds, matching the decoder.
        shared = list(instance.get("bindings", [])) + [
            minted
            for minted in manifest.get("mintedBindings", [])
            if minted["holder"] == instance["name"]
        ]
        fill(shared, lambda _entry, name=instance["name"]: name, 32)
    fill(
        manifest.get("notificationBindings", []),
        lambda entry: entry["holder"],
        15,
    )
    return manifest


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


def private_memory_holder_identity(name: str) -> bytes:
    """Stable per-holder identity, matching `boot_contracts::private_memory_budget`.

    Its domain tag is this contract's own, not `holder_identity`'s: an identity
    computed for one budget must never be replayable as a valid identity in the
    other, since the two bound unrelated mechanisms.
    """
    encoded = name.encode("utf-8")
    return sha256(
        b"slime-private-memory-holder-v1" + struct.pack("<H", len(encoded)) + encoded
    )


def build_private_memory_budget(holders: list[dict]) -> bytes:
    """Encode the C10.2 private-memory budget resource object.

    Entries are sorted by holder identity and must be unique: the decoder
    rejects an unsorted or duplicated table, so the sort here is part of the
    format rather than a convenience. A component absent from the table gets no
    quota at all (deny by default), so omission is meaningful, not a default.
    """
    if len(holders) > MAX_PRIVATE_MEMORY_BUDGET_HOLDERS:
        fail("private-memory budget exceeds holder bound")
    entries = []
    for holder in holders:
        identity = private_memory_holder_identity(holder["holder"])
        quota = holder["pageQuota"]
        if not isinstance(quota, int) or isinstance(quota, bool) or not 0 <= quota <= 0xFFFFFFFF:
            fail(f"private-memory budget: invalid pageQuota for {holder['holder']}")
        entries.append((identity, quota))
    entries.sort(key=lambda entry: entry[0])
    identities = {entry[0] for entry in entries}
    if len(identities) != len(entries):
        fail("private-memory budget: duplicate holder")
    total_len = (
        PRIVATE_MEMORY_BUDGET_HEADER_BYTES + len(entries) * PRIVATE_MEMORY_BUDGET_ENTRY_BYTES
    )
    header = PRIVATE_MEMORY_BUDGET_HEADER.pack(
        PRIVATE_MEMORY_BUDGET_MAGIC,
        PRIVATE_MEMORY_BUDGET_VERSION,
        PRIVATE_MEMORY_BUDGET_HEADER_BYTES,
        0,
        len(entries),
        total_len,
    )
    return header + b"".join(PRIVATE_MEMORY_BUDGET_ENTRY.pack(*entry) for entry in entries)


def validated_private_memory_quotas(holders: list[dict]) -> dict[str, dict]:
    """Mirror `PrivateMemoryBudget::validate_against` on the build side.

    Both arms, so a manifest error fails the build rather than producing a
    generation that only fails at boot: the per-holder reservation bound, and
    B8's aggregate rule that every declared holder must be able to sit at its
    ceiling simultaneously. The ceilings come from the contract's published
    `regionPages`/`totalPages`, which `slime-root/src/private_memory.rs` pins
    against its own constants, so there is one source for both readers.
    """
    if len(holders) > MAX_PRIVATE_MEMORY_BUDGET_HOLDERS:
        fail("private-memory budget exceeds holder bound")
    by_name: dict[str, dict] = {}
    total = 0
    for holder in holders:
        name = holder["holder"]
        if name in by_name:
            fail(f"private-memory budget: duplicate holder {name}")
        quota = holder["pageQuota"]
        if (
            not isinstance(quota, int)
            or isinstance(quota, bool)
            or not 0 <= quota <= PRIVATE_MEMORY_ROOT_REGION_PAGES
        ):
            fail(f"private-memory budget: invalid pageQuota for {name}")
        total += quota
        by_name[name] = holder
    if total > PRIVATE_MEMORY_ROOT_TOTAL_PAGES:
        fail("private-memory budget: aggregate pageQuota exceeds the root ceiling")
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
    # Same rule as the shared-buffer budget above: a profile that does not
    # declare an instance carries no quota for it, so the resource a boot
    # profile emits names only holders that profile actually launches.
    if manifest.get("privateMemoryBudget") is not None:
        resolved["privateMemoryBudget"] = [
            entry for entry in manifest["privateMemoryBudget"] if entry["holder"] in kept
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
    target_profile: TargetProfile,
    recovery: bool = False,
    candidate_identity: bytes | None = None,
    components: set[str] | None = None,
) -> Path:
    environment = {
        key: value
        for key, value in os.environ.items()
        if key != "SLIME_TRANSFER_ACTIVATE"
        and not (
            key.endswith("_CHECK")
            and (key.startswith("SLIME_FABRIC_") or key.startswith("SLIME_SEL4_"))
        )
    }
    if os.environ.get("SLIME_BOOT_SELECTION_FAIL") == "1":
        environment["SLIME_BOOT_SELECTION_FAIL"] = "1"
    else:
        environment.pop("SLIME_BOOT_SELECTION_FAIL", None)
    environment["SLIME_TARGET_PROFILE"] = target_profile.name
    # Product graph selectors are deliberately absent: generated manifest data
    # selects component behavior. Validation-only injection controls remain.
    if environment.get("SLIME_FABRIC_PROXY_EARLY_EXIT") == "1":
        environment["SLIME_FABRIC_PROXY_EARLY_EXIT"] = "1"
    else:
        environment.pop("SLIME_FABRIC_PROXY_EARLY_EXIT", None)
    if environment.get("SLIME_FABRIC_STREAM_EARLY_EXIT") == "1":
        environment["SLIME_FABRIC_STREAM_EARLY_EXIT"] = "1"
    else:
        environment.pop("SLIME_FABRIC_STREAM_EARLY_EXIT", None)
    if recovery:
        environment["SLIME_RECOVERY_IMAGE"] = "1"
    if environment.get("SLIME_GENERATION_CMD_CHECK") == "1" and candidate_identity is not None:
        environment["SLIME_GENERATION_CANDIDATE"] = candidate_identity.hex()
    # Keep separate target directories for distinct manifests because their
    # generated layout and profile inputs intentionally produce distinct images.
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
    base = [
        "cargo",
        "build",
        "--release",
        "--target",
        cargo_target_argument(target_profile),
    ]
    # CP3: one package per component, so the build names packages rather than
    # `--bin` targets of one crate. Each invocation is a *group* of packages
    # sharing a feature set, not one call per component: 52 cargo startups per
    # plane across 29 fixtures is a real cost, and grouping keeps it at today's.
    invocations: list[list[str]] = []
    if is_json_target(target_profile):
        # No command-profile manifest is exported: `spawn-service` and `dango`
        # resolve their commands, launch contexts, request endpoint, and spawn
        # budget from the authenticated generation at runtime (B70), so nothing
        # in a component build reads a fixture any more.
        # Build exactly the components this generation declares, rather than
        # every component crate. The fabric components are compiled against a
        # generated C8 profile this target has no graph for, so building them
        # would fail on constants that describe routes the generation does not
        # declare. Naming the packages keeps the build's contents equal to the
        # manifest's, which is the same property the boot layout already has.
        if components is None:
            fail("seL4 component builds must name the components to build")
        # CP3: the allocator is scoped to the components that actually need it,
        # by declaring `boot-contracts/gpt` and `slime-rt/heap` in those six
        # crates' own manifests. Grouping the build by that split is what makes
        # the scoping real: Cargo unifies features across every package in one
        # invocation, so building a store component alongside a plain one would
        # switch `#[global_allocator]` on for the plain one too — measured, not
        # assumed. Separate invocations keep the plain group's `slime-rt`
        # heap-free.
        #
        # C10.3 adds a third group rather than widening either: its
        # `private-heap` allocator is mutually exclusive with the store plane's
        # (`slime-rt/lib.rs` refuses both with a `compile_error!`, since
        # `#[global_allocator]` is one symbol per link), so unifying the two
        # feature sets in one invocation would fail to compile rather than
        # silently over-link.
        store = sorted(name for name in components if name in STORE_COMPONENTS)
        private_heap = sorted(name for name in components if name in PRIVATE_HEAP_COMPONENTS)
        plain = sorted(
            name
            for name in components
            if name not in STORE_COMPONENTS and name not in PRIVATE_HEAP_COMPONENTS
        )
        for group in (plain, store, private_heap):
            if not group:
                continue
            command = list(base)
            for component in group:
                command += ["-p", f"slime-component-{component}"]
            command += [
                "-Z",
                "json-target-spec",
                "-Z",
                "build-std=core,alloc,compiler_builtins",
                "-Z",
                "build-std-features=compiler-builtins-mem",
            ]
            invocations.append(command)
        # `components/.cargo/config.toml` keys `rustflags` by triple, so a JSON
        # target inherits none of them. Passing the determinism-relevant ones
        # explicitly keeps the link reproducible instead of silently dropping
        # them. `-T` and the load base are deliberately absent: a component here
        # is an ordinary seL4 ELF task at its own link addresses.
        environment["RUSTFLAGS"] = " ".join(
            [
                "-C link-arg=--build-id=none",
                f"--remap-path-prefix={target_dir}=./target/components/{target_profile.name}/{target_name}",
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
        # Guarded like the JSON branch above. Before CP3 `-p slime-components`
        # sat in the shared prefix, so both branches were package-scoped
        # unconditionally; building the `-p` list from `components` means an
        # empty set would leave a bare `cargo build` that resolves the root
        # workspace and builds every default member for a bare-metal target.
        # Unreachable today — `build_sel4_generation` is the only caller — which
        # is why it is a guard rather than a fix.
        if not components:
            fail("component builds must name the components to build")
        command = list(base)
        for component in sorted(components):
            command += ["-p", f"slime-component-{component}"]
        command += [
            "--config",
            f'target.{target_profile.cargo_target}.rustflags=["{remap}"]',
        ]
        invocations.append(command)
    for command in invocations:
        subprocess.run(
            command,
            cwd=ROOT / "components",
            env=environment,
            check=True,
        )
    return target_dir / cargo_target_directory_name(target_profile) / "release"


def _elf64_load_segments(
    name: str, data: bytes, profile: TargetProfile
) -> tuple[int, list[tuple[int, int, int, int, int]]]:
    """Validate the ELF shape shared by the component wrappers."""
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1 or data[6] != 1:
        fail(f"{name}: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 2 or machine != profile.elf_machine:
        fail(f"{name}: not a static executable for target {profile.name}")
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    if phentsize != 56 or phnum == 0:
        fail(f"{name}: invalid program header table")
    segments: list[tuple[int, int, int, int, int]] = []
    for index in range(phnum):
        header = phoff + index * phentsize
        if header < phoff or header > len(data) - 56:
            fail(f"{name}: truncated program header")
        p_type, p_flags = struct.unpack_from("<II", data, header)
        p_offset, p_vaddr, _, p_filesz, p_memsz = struct.unpack_from(
            "<QQQQQ", data, header + 8
        )
        if p_type == 1 and p_memsz:
            if (
                p_filesz > p_memsz
                or p_offset > len(data)
                or p_filesz > len(data) - p_offset
            ):
                fail(f"{name}: malformed load segment")
            segments.append((p_vaddr, p_offset, p_filesz, p_memsz, p_flags))
    if not segments:
        fail(f"{name}: no loadable segment")
    return entry, segments


def _admit_sel4_elf(
    name: str, data: bytes, stack_bytes: int, profile: TargetProfile
) -> bytes:
    """Apply the canonical component and root-loader checks before signing."""
    if len(data) > MAX_COMPONENT_IMAGE_BYTES:
        fail(f"{name}: image exceeds the component image bound")
    entry, segments = _elf64_load_segments(name, data, profile)
    page_flags: dict[int, int] = {}
    start: int | None = None
    end = 0
    entry_ok = False
    mapped_bytes = 0
    for vaddr, _offset, _filesz, memsz, elf_flags in segments:
        if vaddr > (1 << 64) - memsz:
            fail(f"{name}: malformed load segment")
        segment_end = vaddr + memsz
        segment_pages = -(-memsz // profile.page_bytes)
        segment_mapped = segment_pages * profile.page_bytes
        if mapped_bytes > MAX_COMPONENT_IMAGE_BYTES - segment_mapped:
            fail(f"{name}: mapped component image exceeds the component image bound")
        mapped_bytes += segment_mapped
        start = vaddr if start is None else min(start, vaddr)
        end = max(end, segment_end)
        entry_ok |= bool(elf_flags & 1 and vaddr <= entry < segment_end)
    if not entry_ok:
        fail(f"{name}: entry point is not executable")
    if start is None:
        fail(f"{name}: no loadable segment")
    footprint_start = start - start % profile.page_bytes
    footprint_end = -(-end // profile.page_bytes) * profile.page_bytes
    pages = (footprint_end - footprint_start) // profile.page_bytes + 2
    if footprint_start == 0 or footprint_end + 2 * profile.page_bytes > 1 << 40:
        fail(f"{name}: component image footprint is out of range")
    if pages > 512:
        fail(f"{name}: component image footprint exceeds 512 pages")
    for vaddr, _offset, _filesz, memsz, elf_flags in segments:
        segment_end = vaddr + memsz
        first_page = vaddr // profile.page_bytes
        last_page = -(-segment_end // profile.page_bytes)
        for page in range(first_page, last_page):
            page_flags[page] = page_flags.get(page, 0) | elf_flags
    if any(flags & 2 and flags & 1 for flags in page_flags.values()):
        fail(f"{name}: writable executable page")
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


def elf_component_image(
    name: str, elf: Path | bytes, stack_bytes: int, profile: TargetProfile
) -> bytes:
    """Wrap one immutable native ELF after host/root-equivalent admission."""
    data = elf if isinstance(elf, bytes) else elf.read_bytes()
    return _admit_sel4_elf(name, data, stack_bytes, profile)


def component_image(
    name: str, elf: Path | bytes, stack_bytes: int, profile: TargetProfile
) -> bytes:
    if is_json_target(profile):
        return elf_component_image(name, elf, stack_bytes, profile)
    data = elf if isinstance(elf, bytes) else elf.read_bytes()
    entry, segments = _elf64_load_segments(name, data, profile)
    segments = [segment for segment in segments if segment[3]]
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
# One below the root task's own priority, matching `slime-root`'s
# `task::CHILD_PRIORITY`. A child at or above the root cannot be preempted by
# the service loop, so this is a ceiling as well as a default.
DEFAULT_CHILD_PRIORITY = 254
GRANT_POLICY_ONLY = 1
# Grant flags. No bit is defined: `GRANT_MINTED` named a send/recv grant whose
# object its source created at runtime, which the native cutover made
# impossible — an endpoint is a generation-owned seL4 Endpoint the root
# materializes — so B50 deleted the concept rather than leaving a flag nothing
# can set.
GRANT_FLAGS_NONE = 0
# Endpoint and notification slots are relative to distinct 31-entry child
# CSpace regions. The receiver slot occupies the last CSpace entry, so 31 is a
# count, never a legal relative slot.
MAX_DECLARED_NATIVE_SLOT = 31
# Must match `slime-root::task::CHILD_CNODE_SIZE_BITS`; the admitted v5 quota
# and the CNode object are derived from this one value.
CHILD_CNODE_SIZE_BITS = 7
SERVICE_LIFECYCLE = 1
SERVICE_SPAWN = 2
SERVICE_SUPERVISION = 3
SERVICE_CAPABILITY_TRANSFER = 4
SERVICE_SHARED_BUFFER = 5
SERVICE_DIRECTORY = 6
SERVICE_INPUT = 7
SERVICE_BLOCK = 8
SERVICE_CONSOLE = 9
# Fixed userspace ABI slots. Several typed mechanisms share the root transport
# endpoint at slot 1; the service discriminant states the authority carried.
ROOT_SERVICE_SLOT = 1
CONSOLE_SERVICE_SLOT = 32
SERVICE_BY_CAPABILITY_KIND = {
    "sharedBufferFactory": SERVICE_SHARED_BUFFER,
    "sharedBuffer": SERVICE_SHARED_BUFFER,
    "loan": SERVICE_SHARED_BUFFER,
    "directory": SERVICE_DIRECTORY,
    "input": SERVICE_INPUT,
    "block": SERVICE_BLOCK,
    "supervision": SERVICE_SUPERVISION,
}
KERNEL_OBJECT_CNODE = 1
KERNEL_OBJECT_VSPACE = 2
KERNEL_OBJECT_TCB = 3
KERNEL_OBJECT_FRAME = 4
KERNEL_OBJECT_ENDPOINT = 5
KERNEL_OBJECT_PAGE_TABLE = 6
KERNEL_OBJECT_NOTIFICATION = 7
NOTIFICATION_ROLE_SIGNAL = 1
NOTIFICATION_ROLE_WAIT = 2
CAP_RIGHT_ALL = (1 << 64) - 1

def declared_services(
    instance: dict,
    executable: dict,
    grants_by_name: dict[str, dict],
    minted_bindings: list[dict],
    shared_buffer_holders: set[str],
) -> set[int]:
    services = {SERVICE_LIFECYCLE, SERVICE_CONSOLE}
    if executable["role"] == "init" or executable["spawnBudget"] > 0:
        # Spawn returns a supervision capability. A caller that can acquire
        # one must also be allowed to release it, even when no endpoint or
        # separately transferable grant happens to imply the table service.
        services.update({SERVICE_SPAWN, SERVICE_SUPERVISION, SERVICE_CAPABILITY_TRANSFER})
    if instance["name"] in shared_buffer_holders:
        # A receiver may map and return a loan created by another process even
        # though no persistent loan capability appears in its manifest. The
        # authenticated per-holder budget is the declaration that authorizes
        # that receiver-side shared-buffer mechanism.
        services.add(SERVICE_SHARED_BUFFER)
    capability_declarations = [
        grants_by_name[binding["grant"]] for binding in instance["bindings"]
    ] + [
        minted
        for minted in minted_bindings
        if minted["holder"] == instance["name"]
    ]
    for declaration in capability_declarations:
        service = SERVICE_BY_CAPABILITY_KIND.get(declaration["capabilityKind"])
        if service is not None:
            services.add(service)
        if declaration["capabilityKind"] == "executable":
            services.add(SERVICE_SPAWN)
        # A declared endpoint is both the carrier used by capability delegation
        # and the source of received capabilities that `cap_drop` releases. The
        # transport therefore needs the narrow transfer service even when the
        # endpoint itself is not re-delegatable.
        if declaration["capabilityKind"] == "endpoint" or declaration["transferable"]:
            services.add(SERVICE_CAPABILITY_TRANSFER)
    return services


def build_sel4_plan(
    manifest: dict,
    instances: list[dict],
    grants: list[dict],
    grant_rights: list[int],
    instance_index: dict[str, int],
    executable_index: dict[str, int],
    string_offset,
    # Pages each executable's image occupies, keyed by object id, so a
    # process's declared frame count covers what the loader actually maps
    # (B49).
    image_pages: dict[str, int],
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
    grants_by_name = {grant["name"]: grant for grant in grants}
    executables_by_name = {entry["name"]: entry for entry in manifest["executables"]}

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
        # Named once so the CSpace object and the quota's `cslot_count` cannot
        # disagree about how many slots the child has. Six bits, matching
        # `slime-root`'s `task::CHILD_CNODE_SIZE_BITS`: fixed declared slots
        # end at 32 and the native mirrored regions fill the remaining CSpace.
        cnode_size_bits = CHILD_CNODE_SIZE_BITS
        cspace = len(object_index)
        object_index[(name, "cspace")] = cspace
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:cspace"),
                KERNEL_OBJECT_CNODE,
                process,
                cnode_size_bits,
                1,
                PLAN_NONE,
                0,
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

        # Indices into the thread, schedule, and fault tables. These used to be
        # `= process`, which held only while every process had exactly one
        # thread and all four tables grew in lockstep. Counting them lets a
        # process declare more without the tables silently misaligning (B47).
        thread = len(thread_records) // GENERATION_THREAD.size
        schedule = len(schedule_records) // GENERATION_SCHEDULE.size
        fault = len(fault_records) // GENERATION_FAULT_POLICY.size
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
        # Priority is the instance's to declare. Absent, it is the root's
        # default: one below the root's own, so the service loop always
        # preempts a runnable child.
        #
        # Bounded here as well as in the root, because a manifest is the wrong
        # place to learn that a number was silently clamped. `budget_us` and
        # `period_us` stay zero until MCS is admitted -- seL4 without it has no
        # notion of either, and writing a figure the kernel cannot enforce
        # would make the record say more than the system does.
        priority = instance.get("priority", DEFAULT_CHILD_PRIORITY)
        if not isinstance(priority, int) or isinstance(priority, bool):
            fail(f"instance {name}: invalid priority")
        if not 0 <= priority <= DEFAULT_CHILD_PRIORITY:
            fail(
                f"instance {name}: priority {priority} outside 0..={DEFAULT_CHILD_PRIORITY}; "
                "a child at or above the root's priority can stall the service loop"
            )
        # A worker's priority is its own to declare, defaulting to its main
        # thread's (B48). Declaring it *below* the main thread is the case that
        # matters: it lets one component hold a busy thread without stalling
        # its own IPC, which is what "a budget-exhausting client cannot starve
        # an unrelated service" reduces to under a priority-only scheduler.
        worker_priority = instance.get("workerPriority", priority)
        if not isinstance(worker_priority, int) or isinstance(worker_priority, bool):
            fail(f"instance {name}: invalid workerPriority")
        if not 0 <= worker_priority <= DEFAULT_CHILD_PRIORITY:
            fail(
                f"instance {name}: workerPriority {worker_priority} outside "
                f"0..={DEFAULT_CHILD_PRIORITY}"
            )
        schedule_records.extend(
            GENERATION_SCHEDULE.pack(
                string_offset(f"{name}:schedule"),
                thread,
                PLAN_NONE,
                priority,
                priority,
                0,
                0,
                0,
            )
        )
        fault_records.extend(
            GENERATION_FAULT_POLICY.pack(
                string_offset(f"{name}:fault"), thread, PLAN_NONE, fault_endpoint, process + 1, 1
            )
        )
        console_endpoint = len(object_index)
        object_index[(name, "console-endpoint")] = console_endpoint
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{name}:console-endpoint"), KERNEL_OBJECT_ENDPOINT, process, 4, 1, PLAN_NONE, 0
            )
        )
        for service in sorted(
            declared_services(
                instance,
                executables_by_name[instance["executable"]],
                grants_by_name,
                manifest.get("mintedBindings", []),
                {entry["holder"] for entry in manifest.get("sharedBufferBudget", [])},
            )
        ):
            if service == SERVICE_CONSOLE:
                slot, endpoint = CONSOLE_SERVICE_SLOT, console_endpoint
            else:
                slot, endpoint = ROOT_SERVICE_SLOT, fault_endpoint
            service_records.extend(
                GENERATION_SERVICE_BINDING.pack(
                    process, service, slot, endpoint, RIGHT["send"], process + 1, 0
                )
            )
        cap_records.extend(
            GENERATION_CAP_BINDING.pack(process, 2, tcb, CAP_RIGHT_ALL, 0, PLAN_NONE, 0)
        )
        cap_records.extend(
            GENERATION_CAP_BINDING.pack(process, 3, fault_endpoint, 1, process + 1, PLAN_NONE, 0)
        )
        # Counted from the objects this loop just declared for the process,
        # not guessed. The row used to be a literal `1, 1, 2, 0, 2, 4, 6, ...`
        # that no builder derived and no root read; `frame_count=2` and
        # `mapping_count=6` in particular described no plan (B49).
        #
        # One CNode, one VSpace, and two endpoints — the fault endpoint and the
        # console endpoint — plus one TCB and one IPC-buffer frame *per thread*,
        # which is what the loop below declares for the extra threads (B47).
        # The image's own frames and page tables are not here: they are mapped
        # by the root from its own untyped when it loads the ELF, so they
        # belong to the root's accounting rather than the child's declared plan.
        thread_total = 1 + instance.get("extraThreads", 0)
        # The image's own frames, from the payload the loader will map. The
        # root allocates one frame capability per page out of its own CSlots,
        # so leaving them out understated a process's cost by an order of
        # magnitude: the 48-instance stress plane declared 6 slots per instance
        # and consumed 81 (B49).
        executable_object = next(
            (
                e["object"]
                for e in manifest["executables"]
                if e["name"] == instance["executable"]
            ),
            None,
        )
        image_frame_count = image_pages.get(executable_object, 0)
        process_objects = {
            "cnode": 1,
            "vspace": 1,
            "tcb": thread_total,
            # One IPC-buffer/window pair per thread, plus the image itself.
            "frame": thread_total + image_frame_count,
            "endpoint": 2 + sum(
                1
                for grant in grants
                if grant["capabilityKind"] == "endpoint"
                and grant["source"] == name
            ),
            # Each static notification object is owned once, by its declared
            # signal source; wait holders receive capabilities to that object.
            "notification": sum(
                1
                for grant in manifest.get("notificationGrants", [])
                if grant["source"] == name
            ),
        }
        quota_records.extend(
            GENERATION_RESOURCE_QUOTA.pack(
                string_offset(f"{name}:quota"),
                process,
                process_objects["cnode"],
                process_objects["tcb"],
                process_objects["endpoint"],
                process_objects["notification"],
                process_objects["frame"],
                process_objects["vspace"],
                0,
                0,
                # CSlots the child's own CNode holds, from the same size the
                # CSpace object above was given.
                1 << cnode_size_bits,
                0,
                0,
                0,
            )
        )

        # Extra threads, if the instance declares any (B47). Each gets its own
        # TCB, IPC buffer, fault endpoint, fault policy, and schedule, and
        # shares this process's CSpace and VSpace -- which is exactly what
        # makes it a second *thread* rather than a second process.
        #
        # Appended after the main thread's records so the indices above stay
        # the ones the process names, and counted from the tables themselves
        # so a plan with several multi-threaded processes still lines up.
        extra_threads = instance.get("extraThreads", 0)
        if not isinstance(extra_threads, int) or isinstance(extra_threads, bool):
            fail(f"instance {name}: invalid extraThreads")
        if extra_threads < 0:
            fail(f"instance {name}: extraThreads {extra_threads} is negative")
        for extra in range(extra_threads):
            label = f"{name}:thread{extra + 1}"
            extra_tcb = len(object_index)
            object_index[(name, f"tcb{extra + 1}")] = extra_tcb
            kernel_records.extend(
                GENERATION_KERNEL_OBJECT.pack(
                    string_offset(f"{label}:tcb"), KERNEL_OBJECT_TCB, process, 11, 1, PLAN_NONE, 0
                )
            )
            extra_ipc = len(object_index)
            object_index[(name, f"ipc-buffer{extra + 1}")] = extra_ipc
            kernel_records.extend(
                GENERATION_KERNEL_OBJECT.pack(
                    string_offset(f"{label}:ipc-buffer"),
                    KERNEL_OBJECT_FRAME,
                    process,
                    12,
                    1,
                    PLAN_NONE,
                    0,
                )
            )
            extra_fault_endpoint = len(object_index)
            object_index[(name, f"fault-endpoint{extra + 1}")] = extra_fault_endpoint
            kernel_records.extend(
                GENERATION_KERNEL_OBJECT.pack(
                    string_offset(f"{label}:fault-endpoint"),
                    KERNEL_OBJECT_ENDPOINT,
                    process,
                    4,
                    1,
                    PLAN_NONE,
                    0,
                )
            )
            extra_schedule = len(schedule_records) // GENERATION_SCHEDULE.size
            schedule_records.extend(
                GENERATION_SCHEDULE.pack(
                    string_offset(f"{label}:schedule"),
                    len(thread_records) // GENERATION_THREAD.size,
                    PLAN_NONE,
                    worker_priority,
                    worker_priority,
                    0,
                    0,
                    0,
                )
            )
            extra_fault = len(fault_records) // GENERATION_FAULT_POLICY.size
            fault_records.extend(
                GENERATION_FAULT_POLICY.pack(
                    string_offset(f"{label}:fault"),
                    len(thread_records) // GENERATION_THREAD.size,
                    PLAN_NONE,
                    extra_fault_endpoint,
                    process + 1,
                    1,
                )
            )
            thread_records.extend(
                GENERATION_THREAD.pack(
                    string_offset(label),
                    process,
                    extra_tcb,
                    extra_schedule,
                    extra_fault,
                    extra_ipc,
                    0,
                    0,
                    0,
                )
            )

    process_for_instance = {
        instance["name"]: index for index, instance in enumerate(planned_instances)
    }
    for grant_index, (grant, rights) in enumerate(zip(grants, grant_rights, strict=True)):
        # A grant materializes in whichever instance declares a binding for it.
        # An `exec` or channel grant is bound by its source; a delegated
        # authority such as `bufferCreate` is bound only by its target, which is
        # the instance that actually holds the capability.
        holder = next(
            (
                name
                for name in (grant["source"], grant["target"])
                if name in instance_index
                and any(
                    binding["grant"] == grant["name"]
                    for binding in instances[instance_index[name]]["bindings"]
                )
            ),
            None,
        )
        if holder is None:
            fail(f"authority-bearing grant {grant['name']} has no concrete binding")
        source_process = process_for_instance[holder]
        bound = next(
            binding
            for binding in instances[instance_index[holder]]["bindings"]
            if binding["grant"] == grant["name"]
        )
        if grant["capabilityKind"] == "executable":
            target = executable_index[grant["target"]]
            spawn_records.extend(
                GENERATION_SPAWN_TEMPLATE.pack(
                    string_offset(grant["name"]), target, source_process, source_process, source_process, source_process, 1, 0
                )
            )
            cap_records.extend(
                GENERATION_CAP_BINDING.pack(source_process, bound["slot"], object_index[(grant["source"], "tcb")], rights, 0, grant_index, 0)
            )
        elif grant["capabilityKind"] == "endpoint":
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
    notification_grant_records = bytearray()
    notification_binding_records = bytearray()
    notification_grants = sorted(manifest.get("notificationGrants", []), key=lambda grant: grant["name"])
    notification_index = {grant["name"]: index for index, grant in enumerate(notification_grants)}
    if len(notification_index) != len(notification_grants):
        fail("notification grant names must be unique")
    bindings_by_grant: dict[str, list[dict]] = {name: [] for name in notification_index}
    seen_notification_slots: set[tuple[int, int]] = set()
    for binding in manifest.get("notificationBindings", []):
        grant = notification_index.get(binding["grant"])
        holder = instance_index.get(binding["holder"])
        role = {"signal": NOTIFICATION_ROLE_SIGNAL, "wait": NOTIFICATION_ROLE_WAIT}.get(binding["role"])
        slot = binding["slot"]
        if grant is None or holder is None or role is None:
            fail("notification binding names unknown grant, holder, or role")
        if not isinstance(slot, int) or isinstance(slot, bool) or not 0 <= slot < MAX_DECLARED_NATIVE_SLOT:
            fail(f"notification binding {binding['grant']}: relative slot outside 0..30")
        if (holder, slot) in seen_notification_slots:
            fail(f"notification binding {binding['grant']}: duplicate holder slot")
        seen_notification_slots.add((holder, slot))
        bindings_by_grant[binding["grant"]].append(binding)
        notification_binding_records.extend(
            GENERATION_NOTIFICATION_BINDING.pack(grant, holder, slot, role, 0)
        )
    for grant in notification_grants:
        source = instance_index.get(grant["source"])
        target = instance_index.get(grant["target"])
        if source is None or target is None or source == target:
            fail(f"notification grant {grant['name']}: invalid endpoints")
        bindings = bindings_by_grant[grant["name"]]
        # One waiter, and at least the declared source signalling it. Several
        # signallers are the point of a Notification: a waiter blocked on one
        # object learns which of them spoke from the badge, which is the only
        # way a broker can wait on a whole peer set at once. `source` names the
        # edge the grant is *for*; any additional signaller must still be a
        # declared instance, and each gets its own badge bit from its slot.
        waiters = [b for b in bindings if b["role"] == "wait"]
        signals = [b for b in bindings if b["role"] == "signal"]
        if len(waiters) != 1 or instance_index[waiters[0]["holder"]] != target:
            fail(f"notification grant {grant['name']}: requires exactly one target wait binding")
        if not signals or source not in {instance_index[b["holder"]] for b in signals}:
            fail(f"notification grant {grant['name']}: requires a source signal binding")
        object_ = len(object_index)
        object_index[(grant["name"], "notification")] = object_
        kernel_records.extend(
            GENERATION_KERNEL_OBJECT.pack(
                string_offset(f"{grant['name']}:notification"),
                KERNEL_OBJECT_NOTIFICATION,
                process_for_instance[grant["source"]],
                4,
                1,
                PLAN_NONE,
                0,
            )
        )
        notification_grant_records.extend(
            GENERATION_NOTIFICATION_GRANT.pack(
                string_offset(grant["name"]), source, target, object_, 0
            )
        )
    if len(notification_grants) > MAX_NOTIFICATION_GRANTS or len(manifest.get("notificationBindings", [])) > MAX_NOTIFICATION_BINDINGS:
        fail("notification topology count exceeds bound")

    # Minted bindings: a capability the owner creates at runtime and hands to
    # an instance it owns at spawn. Sorted by name so the section is canonical,
    # and validated here so an unsatisfiable declaration fails before output.
    minted_records = bytearray()
    seen_holder_slots: set[tuple[int, int]] = set()
    validate_supervision_binding_names(manifest, instances)
    for minted in sorted(manifest.get("mintedBindings", []), key=lambda entry: entry["name"]):
        owner = instance_index.get(minted["owner"])
        holder = instance_index.get(minted["holder"])
        if owner is None or holder is None:
            fail(f"minted binding {minted['name']}: unknown owner or holder")
        if instances[holder]["owner"] != minted["owner"]:
            fail(f"minted binding {minted['name']}: holder is not owned by its minter")
        slot = minted["slot"]
        if not isinstance(slot, int) or isinstance(slot, bool) or not 0 <= slot < 32:
            fail(f"minted binding {minted['name']}: logical slot outside 0..31")
        if (holder, slot) in seen_holder_slots:
            fail(f"minted binding {minted['name']}: duplicate holder slot")
        seen_holder_slots.add((holder, slot))
        rights = RIGHT_TRANSFER if minted["transferable"] else 0
        for right in minted["rights"]:
            if right not in RIGHT:
                fail(f"minted binding {minted['name']}: unknown right {right}")
            rights |= RIGHT[right]
        kind = minted["capabilityKind"]
        validate_capability_rights(f"minted binding {minted['name']}", kind, rights)
        minted_records.extend(
            GENERATION_MINTED_BINDING.pack(
                string_offset(minted["name"]),
                owner,
                holder,
                slot,
                rights,
                0,
                CAPABILITY_KIND[kind],
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
        len(notification_grant_records) // GENERATION_NOTIFICATION_GRANT.size,
        len(notification_binding_records) // GENERATION_NOTIFICATION_BINDING.size,
    )
    limits = (
        MAX_PROCESSES, MAX_THREADS, MAX_KERNEL_OBJECTS, MAX_MAPPINGS, MAX_CAP_BINDINGS,
        MAX_SERVICE_BINDINGS, MAX_SCHEDULES, MAX_FAULT_POLICIES, MAX_SPAWN_TEMPLATES,
        MAX_RESOURCE_QUOTAS, MAX_MINTED_BINDINGS, MAX_NOTIFICATION_GRANTS,
        MAX_NOTIFICATION_BINDINGS,
    )
    if any(count > limit for count, limit in zip(counts, limits, strict=True)):
        fail("seL4 execution plan count exceeds bound")
    return (
        process_records, thread_records, kernel_records, mapping_records, cap_records,
        service_records, schedule_records, fault_records, spawn_records, quota_records,
        minted_records, notification_grant_records, notification_binding_records, counts,
    )




def layout_executables(manifest: dict) -> set[str]:
    """Executables the initial graph addresses through its boot slot table."""
    initial = {instance["name"] for instance in manifest["instances"]}
    names = {instance["executable"] for instance in manifest["instances"]}
    names.update(
        grant["target"]
        for grant in manifest["grants"]
        if grant["source"] in initial and grant["capabilityKind"] == "executable"
    )
    return names


def build_generation(manifest: dict, payloads: dict[str, bytes], parent: bytes | None, number: int, profile: TargetProfile) -> bytes:
    if "boot-layout" in {object_["id"] for object_ in manifest["objects"]}:
        payloads = dict(payloads)
        # Derived from the manifest's own `InstanceBinding` records rather than
        # from a second, static statement of the same thing (B71). The root
        # places the bootstrap component's capabilities from those bindings, so
        # this is the only derivation that cannot drift from what boots — the
        # static table had `spawn-service` at 4 where the root placed 5, and
        # nothing noticed until CP2's query read the resource's content.
        payloads["boot-layout"] = build_boot_layout(
            number,
            fail,
            entries=layout_from_manifest(manifest, RIGHT, RIGHT_TRANSFER),
        )
    objects = unique_sorted(manifest["objects"], "id", "object ids")
    executables = unique_sorted(manifest["executables"], "name", "executable names")

    # Pages each executable's image occupies, read back from the payload the
    # loader will actually map (B49). The root allocates one frame capability
    # per page from its own CSlots, so a quota that omitted them understated a
    # process's cost by an order of magnitude -- the 48-instance stress plane
    # declared 6 slots per instance and consumed 81.
    image_pages: dict[str, int] = {}
    for object_id, payload in payloads.items():
        if len(payload) < COMPONENT_IMAGE_HEADER.size:
            continue
        # The seL4 profile carries the whole ELF after the qualification
        # header rather than a re-based segment table, so the pages come from
        # that ELF's own program headers -- the same LOAD segments
        # `child_vspace` will map.
        if payload[:8] != COMPONENT_IMAGE_ELF_MAGIC:
            continue
        elf = payload[COMPONENT_IMAGE_ELF_HEADER_LEN:]
        if len(elf) < 64 or elf[:4] != b"\x7fELF":
            continue
        phoff = struct.unpack_from("<Q", elf, 0x20)[0]
        phentsize, phnum = struct.unpack_from("<HH", elf, 0x36)
        pages = 0
        for index in range(phnum):
            at = phoff + index * phentsize
            if at + 56 > len(elf):
                fail(f"object {object_id}: truncated program header")
            p_type = struct.unpack_from("<I", elf, at)[0]
            if p_type != 1:
                continue
            memsz = struct.unpack_from("<Q", elf, at + 0x28)[0]
            pages += -(-memsz // profile.page_bytes)
        image_pages[object_id] = pages
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
        validate_capability_rights(f"grant {grant['name']}", grant["capabilityKind"], rights)
        grant_rights.append(rights)

    expected_bindings: dict[str, set[str]] = {name: set() for name in instance_index}
    for grant, _rights in zip(grants, grant_rights, strict=True):
        source = instance_index.get(grant["source"])
        if source is None:
            fail(f"grant source missing: {grant['name']}")
        if grant["capabilityKind"] == "executable":
            expected_bindings[grant["source"]].add(grant["name"])
            if executable_index.get(grant["target"]) is None:
                fail(f"executable grant target missing: {grant['name']}")
        else:
            target = instance_index.get(grant["target"])
            if target is None:
                fail(f"grant target missing: {grant['name']}")
            # An endpoint's two ends are both declared, so the source binds it
            # as well as the target. Every other kind lands only in its target.
            if grant["capabilityKind"] == "endpoint":
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
        if any(not isinstance(slot, int) or isinstance(slot, bool) or not 0 <= slot < 32 for slot in slots):
            fail(f"instance {instance['name']}: logical binding slot outside 0..31")
        for binding in declared:
            grant = grants[grant_index[binding["grant"]]] if binding["grant"] in grant_index else None
            if grant is not None and set(grant["rights"]) & {"send", "recv"} and binding["slot"] >= MAX_DECLARED_NATIVE_SLOT:
                fail(f"instance {instance['name']}: endpoint-relative slot outside 0..30")
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
            delegated_to_owned_executable = grant["capabilityKind"] == "executable" and any(
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
        notification_grant_records,
        notification_binding_records,
        plan_counts,
    ) = build_sel4_plan(
        manifest,
        instances,
        grants,
        grant_rights,
        instance_index,
        executable_index,
        string_offset,
        image_pages,
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
            notification_grant_records,
            notification_binding_records,
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
        target = executable_index[grant["target"]] if grant["capabilityKind"] == "executable" else instance_index[grant["target"]]
        grant_records += GENERATION_GRANT.pack(
            string_offset(grant["name"]),
            source,
            target,
            rights,
            int(bool(grant["transferable"])),
            GRANT_FLAGS_NONE,
            CAPABILITY_KIND[grant["capabilityKind"]],
        )
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
    notification_grant_offset = minted_binding_offset + len(minted_binding_records)
    notification_binding_offset = notification_grant_offset + len(notification_grant_records)
    string_table_offset = notification_binding_offset + len(notification_binding_records)
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
        notification_grant_offset, notification_binding_offset,
        string_table_offset, len(strings),
        actual_payload_offset, total_len,
    )
    generation = bytearray(
        header + object_records + executable_records + instance_records + dependency_records
        + binding_records + grant_records + state_records + health_records + process_records
        + thread_records + kernel_object_records + mapping_records + cap_binding_records
        + service_binding_records + schedule_records + fault_policy_records
        + spawn_template_records + resource_quota_records + minted_binding_records
        + notification_grant_records + notification_binding_records + strings + blobs
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


# Must equal `slime-root/src/boot_selector.rs`'s `SELECTOR_GENERATION_BYTES`.
# Lowered from 8 MiB with that constant: the selector's buffer is `.bss`, so
# every page of it costs a root CSlot before the root runs, and 8 MiB spent
# ~2048 of the root CNode's 4096 — enough that the selector refused generations
# every other image admits. See that constant for the measurement.
SELECTOR_GENERATION_BYTES = 4 * 1024 * 1024


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
    kind_roles = {
        "sharedBufferFactory": "shared-buffer-factory",
        "input": "input",
    }
    for binding in instance["bindings"]:
        grant = grants_by_name[binding["grant"]]
        if "exec" in grant["rights"]:
            binding_slots[grant["target"]] = binding["slot"]
        elif set(grant["rights"]) & {"send", "recv"}:
            binding_slots[grant["name"]] = binding["slot"]
        role = kind_roles.get(grant["capabilityKind"])
        if role is not None:
            role_bindings[role] = binding["slot"]
    return binding_slots, role_bindings


def build_sel4_generation(
    output: Path,
    manifest: dict,
    target_profile: TargetProfile,
    external_components: dict[str, Path] | None = None,
    component_spec_root: Path | None = None,
) -> None:
    """Build the `aarch64-sel4-qemu-virt` generation (P5.2).

    This is the product generation path. seL4 is the kernel, so the generation
    carries the pinned external-kernel identity required by the format but no
    custom-kernel executable. Recovery, storage, and generation management run
    as userspace planes selected by their manifests.

    A fabric graph is conditional rather than absent: a graph that declares one
    resolves the same authenticated profile the userspace fabric consumes.
    """

    # P5.5.2: a manifest that declares a fabric graph resolves it through the
    # same function every x86 profile uses, so a seL4 route identity, QoS row,
    # and control-slot base are folded from the same schemas and the same
    # validation rather than from a second implementation.
    #
    # The resolution produces the authenticated resource bytes below and
    # nothing a component compiles against: B70's per-plane Rust profile is
    # gone, so no image is parameterized by which plane built it.
    resolved_profile = None
    if manifest.get("fabricGraph"):
        interfaces = validate_interface_schemas(manifest["interfaceSchemas"])
        resolved_profile = resolve_fabric_profile(
            manifest, interfaces, manifest["fabricGraph"]["profiles"][0]["name"]
        )
    import component_spec_contract

    external_components = external_components or {}
    component_specs, workspace_binaries = resolve_component_sources(
        manifest, external_components, component_spec_root
    )
    built = build_rust_components(
        manifest["generation"],
        target_profile,
        candidate_identity=None,
        components=workspace_binaries,
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
    # C10.2's private-memory budget. Validated here rather than only encoded:
    # the root refuses an over-declared or over-committed budget at admission,
    # so checking the same two rules on the build side is what makes builder/root
    # drift a build failure instead of a boot failure.
    declared_private_memory = manifest.get("privateMemoryBudget") or []
    if "private-memory-budget" in object_ids:
        validated_private_memory_quotas(declared_private_memory)
        payloads["private-memory-budget"] = build_private_memory_budget(
            declared_private_memory
        )
    elif declared_private_memory:
        # A quota nothing carries is a promise the generation cannot keep: the
        # root reads the ceiling from the resource object, so a manifest
        # declaring holders without the object would boot with every one of
        # them denied and no indication why.
        fail("privateMemoryBudget declared without a private-memory-budget resource object")
    for executable in manifest["executables"]:
        stack = executable.get("stackBytes", COMPONENT_DEFAULT_STACK_BYTES)
        if (
            not isinstance(stack, int)
            or stack <= 0
            or stack % target_profile.page_bytes
            or stack > COMPONENT_MAX_STACK_BYTES
        ):
            fail(f"executable {executable['name']}: invalid stack")
        specification = component_specs.get(executable["name"])
        if specification is None:
            binary_name = executable["name"]
            provider = "workspace-fixture"
            elf = component_executable(built, binary_name, target_profile)
        else:
            implementation = specification["implementation"]
            binary_name = implementation["binary"]
            provider = implementation["provider"]
            if provider == component_spec_contract.PROVIDER_WORKSPACE:
                elf = component_executable(built, binary_name, target_profile)
            else:
                elf = external_components[binary_name]
                source_path = elf
                try:
                    data = elf.read_bytes()
                except OSError as error:
                    fail(f"external component {binary_name!r}: cannot read {elf}: {error}")
                actual = sha256(data).hex()
                expected = implementation["contentHash"]
                if actual != expected:
                    fail(
                        f"external component {binary_name!r}: SHA-256 {actual} "
                        f"does not match declared {expected}"
                    )
                elf = data
                display_path = source_path
        if provider != component_spec_contract.PROVIDER_EXTERNAL:
            display_path = elf
        print(
            f"Component source: executable={executable['name']} "
            f"implementation={binary_name} provider={provider} path={display_path}"
        )
        payloads[executable["object"]] = component_image(
            executable["name"], elf, stack, target_profile
        )

    # RP2: qualify one named executable for a *different* admitted target, so a
    # boot gate can observe the root refusing a wrong-target component image
    # before mapping any of its bytes rather than only proving it host-side.
    #
    # Deliberately narrow. The name must be a declared executable and the
    # profile must be an admitted one, so this cannot fabricate a target the
    # contract does not define; only the qualification header changes, and the
    # generation stays otherwise valid so the refusal cannot be an unrelated
    # admission error wearing a wrong-target label. Validation-only, in the same
    # family as `SLIME_BOOT_SELECTION_FAIL`.
    injection = os.environ.get("SLIME_WRONG_TARGET_EXECUTABLE")
    if injection:
        name, _, profile_name = injection.partition("=")
        if not name or not profile_name:
            fail("SLIME_WRONG_TARGET_EXECUTABLE must be <executable>=<target-profile>")
        wrong = TARGET_PROFILES_BY_NAME.get(profile_name)
        if wrong is None:
            fail(f"SLIME_WRONG_TARGET_EXECUTABLE: unknown target {profile_name!r}")
        if wrong.name == target_profile.name:
            fail("SLIME_WRONG_TARGET_EXECUTABLE: the injected target is this generation's own")
        declared = {executable["name"]: executable for executable in manifest["executables"]}
        if name not in declared:
            fail(f"SLIME_WRONG_TARGET_EXECUTABLE: {name!r} is not a declared executable")
        executable = declared[name]
        stack = executable.get("stackBytes", COMPONENT_DEFAULT_STACK_BYTES)
        # The body stays the AArch64 ELF this workspace built; only the
        # qualification header names the other profile. That is exactly the
        # artifact `TargetProfile::admit` must reject, and keeping the body valid
        # means a passing refusal cannot come from a malformed ELF instead.
        header = COMPONENT_IMAGE_HEADER.pack(
            COMPONENT_IMAGE_ELF_MAGIC,
            COMPONENT_IMAGE_ELF_VERSION,
            COMPONENT_IMAGE_ELF_HEADER_LEN,
            COMPONENT_IMAGE_KERNEL_ABI,
            wrong.architecture,
            wrong.abi,
            wrong.page_profile,
            0,
            0,
            0,
            stack,
            wrong.id,
            wrong.required_features,
        )
        body = payloads[executable["object"]][COMPONENT_IMAGE_HEADER.size :]
        payloads[executable["object"]] = header + body
        print(f"Injected wrong-target qualification: {name} as {wrong.name}")

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
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--external-component",
        action="append",
        default=[],
        metavar="NAME=ELF",
        help="supply one externally built component ELF by implementation name",
    )
    parser.add_argument(
        "--component-spec-root",
        type=Path,
        help="load component specifications from this directory",
    )
    parser.add_argument("output_dir")
    arguments = parser.parse_args()
    output = Path(arguments.output_dir).resolve()
    external_components = parse_external_components(arguments.external_component)
    component_spec_root = (
        arguments.component_spec_root.resolve()
        if arguments.component_spec_root is not None
        else None
    )
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
    # B62: one declared fabric limit, overridden per build variant.
    #
    # `sel4-traffic`, `sel4-fault`, and `sel4-saturation` were three 1882-line
    # fixtures differing in exactly two fields: the generation number (already
    # overridable above) and, for saturation, `inFlightOperations`. Copying an
    # entire manifest to change one integer is how a fixture goes stale against a
    # derivation rule it restates but no longer matches, which is B55's first
    # defect. `.zti` is immediate mode by design — no imports, no composition —
    # so the delta is expressed here rather than in the data.
    #
    # Deliberately narrow: one declared limit by name, validated against the
    # limits the manifest already declares, so this cannot introduce a limit the
    # schema does not know or silently create a graph field.
    requested_limit = os.environ.get("SLIME_FABRIC_LIMIT_OVERRIDE")
    if requested_limit:
        name, _, raw = requested_limit.partition("=")
        if not _ or not name:
            fail("SLIME_FABRIC_LIMIT_OVERRIDE must be <limit>=<value>")
        limits = manifest.get("fabricGraph", {}).get("limits")
        if not isinstance(limits, dict) or name not in limits:
            fail(f"SLIME_FABRIC_LIMIT_OVERRIDE names undeclared limit {name!r}")
        try:
            value = int(raw)
        except ValueError:
            fail("SLIME_FABRIC_LIMIT_OVERRIDE value must be an integer")
        if value <= 0:
            fail("SLIME_FABRIC_LIMIT_OVERRIDE value must be positive")
        limits[name] = value
    # B62: one declared participant QoS field, overridden per build variant.
    #
    # `sel4-matrix-unsatisfiable` was a 1069-line copy of `sel4-matrix` flipping
    # exactly one participant's `reliability` so admission must refuse the
    # resulting incompatible pair. The negative control is that single field, so
    # the copy carried 1068 lines of agreement that nothing checked stayed in
    # agreement.
    #
    # Addressed by route, component, and field so it cannot silently retarget:
    # every part must resolve against what the manifest already declares.
    requested_qos = os.environ.get("SLIME_FABRIC_QOS_OVERRIDE")
    if requested_qos:
        parts = requested_qos.split(":")
        if len(parts) != 4 or not all(parts):
            fail("SLIME_FABRIC_QOS_OVERRIDE must be <route>:<component>:<field>:<value>")
        route_name, component, field, value = parts
        routes = [
            route
            for route in manifest.get("fabricGraph", {}).get("routes", [])
            if route.get("name") == route_name
        ]
        if len(routes) != 1:
            fail(f"SLIME_FABRIC_QOS_OVERRIDE names undeclared route {route_name!r}")
        members = [
            member
            for member in routes[0]["participants"]
            if member.get("component") == component
        ]
        if len(members) != 1:
            fail(
                f"SLIME_FABRIC_QOS_OVERRIDE names {component!r}, which is not a "
                f"unique participant of {route_name!r}"
            )
        if field not in members[0]:
            fail(f"SLIME_FABRIC_QOS_OVERRIDE names undeclared field {field!r}")
        members[0][field] = value
    target_profile = resolve_target_profile(manifest.get("target"))
    if manifest["formatVersion"] != 1:
        fail("unsupported source formatVersion")
    if target_profile.name != SEL4_TARGET_PROFILE:
        fail("custom-kernel generation builds were retired with P5; select a seL4 manifest")
    output.mkdir(parents=True, exist_ok=True)
    build_sel4_generation(
        output,
        manifest,
        target_profile,
        external_components,
        component_spec_root,
    )


if __name__ == "__main__":
    main()
