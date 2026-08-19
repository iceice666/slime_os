#!/usr/bin/env python3

"""C8.9 typed full-profile and resource-bound closure gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import os
import struct
import subprocess
import tempfile

from boot_contracts import (
    FABRIC_GRAPH_CHANNEL_QUEUE_DEPTH,
    MAX_NORMALIZED_SCHEMAS,
    MAX_NORMALIZED_SCHEMAS_ARTIFACT_BYTES,
    NORMALIZED_SCHEMAS_ENTRY,
    NORMALIZED_SCHEMAS_HEADER,
    NORMALIZED_SCHEMAS_HEADER_BYTES,
    NORMALIZED_SCHEMAS_MAGIC,
    NORMALIZED_SCHEMAS_VERSION,
)
from fabric_trace_contract import (
    FABRIC_TRACE_MAX_DEPTH,
    FABRIC_TRACE_OVERFLOW_SATURATE,
    FABRIC_TRACE_TERMINAL_RESERVE,
)
from harness import ROOT, load_script

builder = load_script("build_generation_profile", "build/build-generation.py")


# B11: `default` is now the product boot profile and declares no verification
# scaffolding, so the negative cases below — which crowd participants, exhaust
# the fabric holder's quota, and interpose through a probe — are written against
# the profile that declares the full participant set. `default` gets its own
# determinism and resolution coverage through the all-profiles loop at the end.
SCAFFOLDING_PROFILE = "test"

def fail(message: str) -> None:
    raise SystemExit(f"data fabric profile check: {message}")


def rejected(label: str, mutate, *, profile: str = SCAFFOLDING_PROFILE) -> None:
    manifest = copy.deepcopy(MANIFEST)
    # A mutator may alter builder module state rather than the manifest — the
    # fixed-shape worker bound lives there, not in the graph. Snapshot and restore
    # it so one negative case cannot leak into the next.
    shapes = copy.deepcopy(builder.FABRIC_WORKER_WAIT_SHAPES)
    # The outer `finally` covers mutation as well as resolution, so a mutator that
    # edits module state and then raises cannot leak into the next case. Only the
    # resolve is allowed to answer "rejected": a `SystemExit` out of `mutate` is a
    # broken mutator, and swallowing it here would make the case pass vacuously.
    try:
        mutate(manifest)
        try:
            builder.resolve_fabric_profile(manifest, INTERFACES, profile)
        except SystemExit:
            return
        except (KeyError, TypeError, ValueError, struct.error) as error:
            fail(f"{label} bypassed a builder check: {type(error).__name__}: {error}")
    finally:
        builder.FABRIC_WORKER_WAIT_SHAPES.clear()
        builder.FABRIC_WORKER_WAIT_SHAPES.update(shapes)
    fail(f"{label} was accepted")


def zti_check(path: _Path) -> None:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(builder.STDLIB)
    environment["SLIME_DATA_FABRIC_PROFILE_PATH"] = str(path)
    process = subprocess.run(
        [str(builder.binary()), "run", "contracts/data-fabric-profile/v1/check.zt"],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0 or not process.stdout.startswith("#valid"):
        fail(f"resolved profile failed its Zutai contract: {process.stdout.strip()}")


MANIFEST = builder.load_manifest()
INTERFACES = builder.validate_interface_schemas(MANIFEST["interfaceSchemas"])

first = builder.resolve_fabric_profile(MANIFEST, INTERFACES, SCAFFOLDING_PROFILE)
second = builder.resolve_fabric_profile(
    copy.deepcopy(MANIFEST), INTERFACES, SCAFFOLDING_PROFILE
)
if first.graph_bytes != second.graph_bytes or first.artifact != second.artifact:
    fail("identical source did not produce identical resolved graph/profile values")

with tempfile.TemporaryDirectory(prefix="slime-data-fabric-profile-") as temporary:
    left = _Path(temporary) / "left"
    right = _Path(temporary) / "right"
    left.mkdir()
    right.mkdir()
    left_paths = builder.write_resolved_profile(left, first)
    right_paths = builder.write_resolved_profile(right, second)
    for left_path, right_path in zip(left_paths, right_paths, strict=True):
        if left_path.read_bytes() != right_path.read_bytes():
            fail(f"{left_path.name} is not byte deterministic")
    zti_check(left_paths[0])
    rust = left_paths[1].read_text(encoding="utf-8")
    # The participant *table* retired with B70/CP2 -- a component reads the graph
    # for what it used to compile in. What this still has to catch is the failure
    # the table check caught: rendered Rust silently disagreeing with the canonical
    # profile. `FABRIC_QOS` and `FABRIC_VISIBILITY` are rendered one row per
    # participant from the same list, so each must carry every declared participant
    # and carry exactly as many rows as there are participants.
    #
    # Both halves are load-bearing, and each was observed failing without the other.
    # Searching the whole file for a `(b"component", "route", ` prefix proves
    # nothing, because `FABRIC_NOTIFICATION_BINDINGS` renders that same prefix and
    # keeps the substring alive after `FABRIC_QOS` and `FABRIC_VISIBILITY` have lost
    # the row -- so the search is scoped to one table body at a time. Membership
    # alone is likewise not enough: a duplicated participant covers for a dropped
    # one, which the row count catches and the substring search does not.
    def table_body(name):
        opening = f"pub const {name}"
        start = rust.find(opening)
        if start < 0:
            fail(f"rendered Rust omitted the {name} table")
        start = rust.find("&[", rust.find("= ", start))
        end = rust.find("];", start)
        if start < 0 or end < 0:
            fail(f"rendered Rust {name} table is not delimited as expected")
        return rust[start + 2 : end]

    for name in ("FABRIC_QOS", "FABRIC_VISIBILITY"):
        body = table_body(name)
        rows = [line for line in body.splitlines() if line.strip()]
        if len(rows) != len(first.artifact["participants"]):
            fail(
                f"rendered Rust {name} has {len(rows)} rows for "
                f"{len(first.artifact['participants'])} declared participants"
            )
        for row in first.artifact["participants"]:
            expected = f'(b"{row["component"]}", "{row["route"]}", '
            if expected not in body:
                fail(f"rendered Rust {name} diverges from the canonical profile participants")
    for entry in first.artifact["limits"]:
        if f" = {entry['value']};" not in rust:
            fail(f"Rust profile omitted the {entry['name']} limit value")

    schema_bytes = left_paths[2].read_bytes()
    header = NORMALIZED_SCHEMAS_HEADER.unpack_from(schema_bytes)
    magic, version, header_size, required_flags, count, total_len = header
    if (
        magic != NORMALIZED_SCHEMAS_MAGIC
        or version != NORMALIZED_SCHEMAS_VERSION
        or header_size != NORMALIZED_SCHEMAS_HEADER_BYTES
        or required_flags != 0
        or total_len != len(schema_bytes)
        or count != len(first.schemas)
    ):
        fail("normalized schema artifact header is invalid")
    cursor = NORMALIZED_SCHEMAS_HEADER_BYTES
    identities = []
    lengths = []
    for _ in range(count):
        identity, normalized_len, reserved = NORMALIZED_SCHEMAS_ENTRY.unpack_from(schema_bytes, cursor)
        cursor += NORMALIZED_SCHEMAS_ENTRY.size
        if reserved != 0:
            fail("normalized schema artifact has nonzero reserved data")
        identities.append(identity)
        lengths.append(normalized_len)
    if identities != sorted(identities) or identities != [interface.identity for interface in first.schemas]:
        fail("normalized schema entries are not in schema-identity order")
    for interface, normalized_len in zip(first.schemas, lengths, strict=True):
        payload = schema_bytes[cursor : cursor + normalized_len]
        cursor += normalized_len
        if payload != interface.normalized:
            fail("normalized schema payload differs from the admitted bytes")
    if cursor != len(schema_bytes):
        fail("normalized schema artifact has trailing or missing bytes")
    if count > MAX_NORMALIZED_SCHEMAS or len(schema_bytes) > MAX_NORMALIZED_SCHEMAS_ARTIFACT_BYTES:
        fail("normalized schema artifact exceeds its generated bounds")


def duplicate_profile(manifest: dict) -> None:
    manifest["fabricGraph"]["profiles"].append(copy.deepcopy(manifest["fabricGraph"]["profiles"][0]))


def unknown_profile_target(manifest: dict) -> None:
    manifest["fabricGraph"]["profiles"][0]["interpositions"] = [
        {"route": "missing", "participant": "fabric-subscriber", "chain": ["fabric-intruder"]}
    ]


def ambiguous_profile_target(manifest: dict) -> None:
    route = manifest["fabricGraph"]["routes"][0]
    duplicate = copy.deepcopy(route["participants"][0])
    route["participants"].append(duplicate)
    manifest["fabricGraph"]["profiles"][0]["interpositions"] = [
        {"route": route["name"], "participant": duplicate["component"], "chain": ["fabric-intruder"]}
    ]


def malformed_profile_chain(manifest: dict) -> None:
    manifest["fabricGraph"]["profiles"][0]["interpositions"] = [
        {"route": "telemetry", "participant": "fabric-subscriber", "chain": []}
    ]


def insufficient_holder_pages(manifest: dict) -> None:
    holder = next(entry for entry in manifest["sharedBufferBudget"] if entry["holder"] == "fabric-service")
    holder["bytePages"] = manifest["fabricGraph"]["limits"]["bufferPages"] - 1


def insufficient_holder_buffers(manifest: dict) -> None:
    holder = next(entry for entry in manifest["sharedBufferBudget"] if entry["holder"] == "fabric-service")
    holder["bufferCount"] = manifest["fabricGraph"]["limits"]["buffers"] - 1
    holder["loanCount"] = min(holder["loanCount"], holder["bufferCount"])


def insufficient_holder_mappings(manifest: dict) -> None:
    holder = next(entry for entry in manifest["sharedBufferBudget"] if entry["holder"] == "fabric-service")
    holder["mappingCount"] = manifest["fabricGraph"]["limits"]["mappings"] - 1


def insufficient_holder_loans(manifest: dict) -> None:
    holder = next(entry for entry in manifest["sharedBufferBudget"] if entry["holder"] == "fabric-service")
    holder["loanCount"] = manifest["fabricGraph"]["limits"]["loans"] - 1


def queue_above_kernel(manifest: dict) -> None:
    manifest["fabricGraph"]["limits"]["queueDepth"] = FABRIC_GRAPH_CHANNEL_QUEUE_DEPTH + 1


def capability_layout_too_small(manifest: dict) -> None:
    manifest["fabricGraph"]["limits"]["capabilitySlots"] = first.artifact["requiredCapabilitySlots"] - 1


def worker_above_wait_bound(manifest: dict) -> None:
    """Crowd one worker's routes past its wake-source ceiling.

    C8.10 partitions the graph so every worker can block on all of its live
    sources at once. Adding subscribers to a route the stream worker already
    carries pushes that worker past the kernel bound, which must fail the build:
    a worker that cannot register its whole set would have to poll.

    Reaching that check at all takes care, because three unrelated guards sit in
    front of it and each rejects a careless mutation for its own reason — which
    would leave the wait bound untested while the case still looked green:

    * a participant naming an undeclared component is refused by graph encoding,
      so each addition needs a matching `components` entry;
    * `subscribers` is already exactly at its declared budget, so the mutator has
      to raise the very limit it is not testing;
    * the summed subscriber history is checked against the frame table, so the
      additions carry the shallowest history that still declares an edge.

    Two subscribers is the minimum that exceeds the bound: the stream worker sits
    at 8 of 9, so one more would only reach it.
    """
    graph = manifest["fabricGraph"]
    telemetry = next(route for route in graph["routes"] if route["name"] == "telemetry")
    template = next(
        member for member in telemetry["participants"] if member["direction"] == "subscribe"
    )
    crowd = 2
    for index in range(crowd):
        component = f"fabric-crowd-{index}"
        # v5 splits `components` into the executable catalogue and the
        # instances built from it. A synthetic participant needs both: the
        # route names an instance, and an instance names an executable.
        manifest["executables"].append(
            {
                "name": component,
                "object": "sha256:fabric-observer",
                "role": "application",
                "spawnBudget": 0,
                "commandProfile": [],
            }
        )
        manifest["instances"].append(
            {
                "name": component,
                "executable": component,
                "owner": "init",
                "autostart": True,
                "dependencies": ["fabric-service"],
                "health": "optional",
                "bindings": [],
            }
        )
        extra = dict(template)
        extra["component"] = component
        extra["historyDepth"] = 1
        extra["retainedDepth"] = 0
        telemetry["participants"].append(extra)
    graph["limits"]["subscribers"] += crowd


def worker_shape_above_wait_bound(manifest: dict) -> None:
    """Raise a fixed-shape worker's declared peak past the kernel bound.

    The request/response workers park across fixed slot arrays rather than the
    graph, so crowding a route cannot move their peak — the only way one drifts
    over the bound is the broker growing its own set. Mutating the declared shape
    is the closest stand-in, and it proves the ceiling is enforced for fixed-shape
    workers too rather than only for the graph-derived one.
    """
    builder.FABRIC_WORKER_WAIT_SHAPES["call"] = {
        "graphDerived": False,
        "peak": builder.MAX_FABRIC_GRAPH_INGRESS_SOURCES + 1,
    }


def frame_layout_too_small(manifest: dict) -> None:
    for route in manifest["fabricGraph"]["routes"]:
        for participant in route["participants"]:
            if participant["direction"] == "subscribe":
                participant["historyDepth"] = 16
    manifest["fabricGraph"]["limits"]["historyDepth"] = 16


def trace_depth_above_ceiling(manifest: dict) -> None:
    manifest["fabricGraph"]["traceDepth"] = FABRIC_TRACE_MAX_DEPTH + 1


def trace_depth_below_reservation(manifest: dict) -> None:
    """A sink whose whole depth is the terminal reservation records nothing.

    The reservation exists so a full sink can still say the trace ended. A depth
    at or below it leaves no slot for ordinary evidence, so such a generation
    declares a sink that cannot hold a single event — a build failure, not a
    boot-time surprise.
    """
    manifest["fabricGraph"]["traceDepth"] = FABRIC_TRACE_TERMINAL_RESERVE


def trace_overflow_unknown(manifest: dict) -> None:
    manifest["fabricGraph"]["traceOverflow"] = "dropOldest"


def trace_depth_not_an_integer(manifest: dict) -> None:
    manifest["fabricGraph"]["traceDepth"] = "16"


for label, mutate in (
    ("duplicate profile", duplicate_profile),
    ("unknown profile target", unknown_profile_target),
    ("ambiguous profile target", ambiguous_profile_target),
    ("malformed profile chain", malformed_profile_chain),
    ("insufficient fabric page quota", insufficient_holder_pages),
    ("insufficient fabric buffer quota", insufficient_holder_buffers),
    ("insufficient fabric mapping quota", insufficient_holder_mappings),
    ("insufficient fabric loan quota", insufficient_holder_loans),
    ("queue above kernel bound", queue_above_kernel),
    ("capability layout above declaration", capability_layout_too_small),
    ("route worker above wait bound", worker_above_wait_bound),
    ("fixed-shape worker above wait bound", worker_shape_above_wait_bound),
    ("frame layout above generated table", frame_layout_too_small),
    ("trace depth above contract ceiling", trace_depth_above_ceiling),
    ("trace depth below terminal reservation", trace_depth_below_reservation),
    ("unknown trace overflow discipline", trace_overflow_unknown),
    ("non-integer trace depth", trace_depth_not_an_integer),
):
    rejected(label, mutate)

# Each wait-bound mutator must be rejected *by the wait bound*, not by one of the
# unrelated guards standing in front of it. Neutralizing the ceiling and requiring
# the same manifest to resolve is what tells the two apart: a mutator that still
# fails here was never testing this bound, and would have kept passing after the
# check it names was deleted outright.
for label, mutate in (
    ("route worker above wait bound", worker_above_wait_bound),
    ("fixed-shape worker above wait bound", worker_shape_above_wait_bound),
):
    ceiling = builder.MAX_FABRIC_GRAPH_INGRESS_SOURCES
    shapes = copy.deepcopy(builder.FABRIC_WORKER_WAIT_SHAPES)
    probe = copy.deepcopy(MANIFEST)
    mutate(probe)
    builder.MAX_FABRIC_GRAPH_INGRESS_SOURCES = 1 << 30
    try:
        builder.resolve_fabric_profile(probe, INTERFACES, "default")
    except SystemExit as error:
        fail(f"{label} is rejected by {error!s}, not by the wait bound it names")
    finally:
        builder.MAX_FABRIC_GRAPH_INGRESS_SOURCES = ceiling
        builder.FABRIC_WORKER_WAIT_SHAPES.clear()
        builder.FABRIC_WORKER_WAIT_SHAPES.update(shapes)

# Same discipline for the trace-sink ceiling: neutralizing it must make the very
# manifest that was refused resolve cleanly. Without this, a depth mutator that
# happened to trip some earlier guard would keep the case green after the ceiling
# it names was deleted.
for label, mutate in (
    ("trace depth above contract ceiling", trace_depth_above_ceiling),
    ("trace depth below terminal reservation", trace_depth_below_reservation),
):
    ceiling = builder.FABRIC_TRACE_MAX_DEPTH
    reserve = builder.FABRIC_TRACE_TERMINAL_RESERVE
    probe = copy.deepcopy(MANIFEST)
    mutate(probe)
    builder.FABRIC_TRACE_MAX_DEPTH = 1 << 30
    builder.FABRIC_TRACE_TERMINAL_RESERVE = 0
    try:
        builder.resolve_fabric_profile(probe, INTERFACES, "default")
    except SystemExit as error:
        fail(f"{label} is rejected by {error!s}, not by the trace-sink bound it names")
    finally:
        builder.FABRIC_TRACE_MAX_DEPTH = ceiling
        builder.FABRIC_TRACE_TERMINAL_RESERVE = reserve

# The resolved profile must actually carry the sink the graph declared, and the
# Rust a component compiles must state the same two numbers. A bound validated at
# build time but absent from the artifact would leave every worker sizing its sink
# from a default nothing checked.
if first.artifact["traceDepth"] != MANIFEST["fabricGraph"]["traceDepth"]:
    fail("resolved profile dropped the declared trace-sink depth")
if first.artifact["traceOverflow"] != FABRIC_TRACE_OVERFLOW_SATURATE:
    fail("resolved profile did not map the declared overflow discipline")
profile_rust = builder.render_fabric_profile_rust(first)
# The whole declaration, not the name and the value independently: `= 1;` occurs
# for several unrelated slot constants, so checking the two substrings separately
# would pass even if the overflow constant rendered a different number entirely.
for name, kind, value in (
    ("FABRIC_TRACE_DEPTH", "usize", first.artifact["traceDepth"]),
    ("FABRIC_TRACE_OVERFLOW", "u32", first.artifact["traceOverflow"]),
):
    if f"pub const {name}: {kind} = {value};" not in profile_rust:
        fail(f"Rust profile does not declare {name} as {value}")

rejected("unknown profile", lambda _manifest: None, profile="missing")

visibility = builder.resolve_fabric_profile(MANIFEST, INTERFACES, "visibility")
if visibility.graph_bytes == first.graph_bytes:
    fail("named visibility profile did not change authenticated graph authority")
if visibility.artifact["name"] != "visibility":
    fail("resolved artifact lost its selected profile name")

# Every declared profile must resolve, including one no generation selects yet.
# An unresolvable profile is a latent boot failure rather than dead text: it stays
# green until the generation that selects it is written, which is exactly when a
# stale interposition chain or dropped participant is most expensive to find.
#
# `unified` is excluded, and the exclusion is a structural fact rather than a
# waiver. B55 gave each plane's control grants a per-plane holder: under
# `unified` they must terminate at `fabric-call-worker`/`fabric-op-worker`,
# because a bounded route worker authenticates a client by the control endpoint
# the request arrived on and no one can hand a worker that endpoint afterwards.
# Every other profile has no worker instance at all and its controls terminate
# at `fabric-service`. A manifest carries *one* grant list, so a manifest
# declaring both kinds of profile cannot satisfy both rules -- and `valid.zti`
# is precisely that: the reference manifest for the single-broker profiles,
# which is why its grants target `fabric-service`.
#
# The real full-graph fixtures (`sel4-boot.zti`, `sel4-traffic.zti`,
# and the traffic composition the fault/saturation variants share) declare
# `unified` *alone* and target
# the workers, so they resolve it correctly -- and `just sel4_boot_check`,
# `sel4_traffic_check`, `sel4_fault_check`, and `sel4_saturation_check` all boot
# it, which is stronger evidence than resolving it here would be. Sweeping it
# out of this reference manifest asserts a contradiction, not a property.
SINGLE_BROKER_PROFILES = tuple(
    profile
    for profile in builder.declared_fabric_profiles(MANIFEST)
    if profile != builder.UNIFIED_FABRIC_PROFILE
)
if not SINGLE_BROKER_PROFILES:
    fail("the reference manifest declares no single-broker fabric profile to resolve")
for profile in SINGLE_BROKER_PROFILES:
    resolved = builder.resolve_fabric_profile(MANIFEST, INTERFACES, profile)
    if resolved.artifact["name"] != profile:
        fail(f"resolved artifact lost its selected profile name: {profile}")
    for worker in resolved.artifact["workers"]:
        if worker["waitSources"] > builder.MAX_FABRIC_GRAPH_INGRESS_SOURCES:
            fail(f"profile {profile} worker {worker['name']} exceeds its wake-source ceiling")

# The Rust decoder is the second reader of the schema artifact. Run its tests
# so a layout or rule drift between the builder and decoder fails this gate.
subprocess.run(
    [
        "cargo",
        "test",
        "--quiet",
        "--lib",
        "-p",
        "boot-contracts",
        "normalized_interface_schemas",
    ],
    cwd=ROOT,
    check=True,
)

# The checked-in fallback is what a plain `cargo build` compiles against, so it
# carries the product boot profile (B11) — the same profile
# `default_boot_layout.rs` renders.
fallback_profile = ROOT / "components/bins/src/default_fabric_profile.rs"
product = builder.resolve_fabric_profile(copy.deepcopy(MANIFEST), INTERFACES, "default")
if fallback_profile.read_text(encoding="utf-8") != builder.render_fabric_profile_rust(product):
    fail("checked-in product userspace profile is stale")

print("typed fabric profile, resources, and deterministic schema corpus: ok")
