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
from harness import ROOT, load_script

builder = load_script("build_generation_profile", "build/build-generation.py")


def fail(message: str) -> None:
    raise SystemExit(f"data fabric profile check: {message}")


def rejected(label: str, mutate, *, profile: str = "default") -> None:
    manifest = copy.deepcopy(MANIFEST)
    mutate(manifest)
    try:
        builder.resolve_fabric_profile(manifest, INTERFACES, profile)
    except SystemExit:
        return
    except (KeyError, TypeError, ValueError, struct.error) as error:
        fail(f"{label} bypassed a builder check: {type(error).__name__}: {error}")
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

first = builder.resolve_fabric_profile(MANIFEST, INTERFACES, "default")
second = builder.resolve_fabric_profile(copy.deepcopy(MANIFEST), INTERFACES, "default")
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
    for row in first.artifact["participants"]:
        expected = f'(b"{row["component"]}", "{row["route"]}", "{row["interface"]}", {row["direction"]})'
        if expected not in rust:
            fail("Rust participant table diverges from the canonical profile")
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


def frame_layout_too_small(manifest: dict) -> None:
    for route in manifest["fabricGraph"]["routes"]:
        for participant in route["participants"]:
            if participant["direction"] == "subscribe":
                participant["historyDepth"] = 16
    manifest["fabricGraph"]["limits"]["historyDepth"] = 16


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
    ("frame layout above generated table", frame_layout_too_small),
):
    rejected(label, mutate)

rejected("unknown profile", lambda _manifest: None, profile="missing")

visibility = builder.resolve_fabric_profile(MANIFEST, INTERFACES, "visibility")
if visibility.graph_bytes == first.graph_bytes:
    fail("named visibility profile did not change authenticated graph authority")
if visibility.artifact["name"] != "visibility":
    fail("resolved artifact lost its selected profile name")

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

fallback_profile = ROOT / "components/bins/src/default_fabric_profile.rs"
if fallback_profile.read_text(encoding="utf-8") != builder.render_fabric_profile_rust(first):
    fail("checked-in default userspace profile is stale")

print("typed fabric profile, resources, and deterministic schema corpus: ok")
