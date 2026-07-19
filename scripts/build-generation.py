#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ZUTAI = ROOT / "deps" / "zutai" / "Cargo.toml"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
COMPONENT_SOURCES = ROOT / "components" / "src"

TARGET = "x86_64-qemu-virtio"
MAGIC = b"SLIMEGEN"
HEADER = struct.Struct("<8sIIQ32sHHHHH2x")
OBJECT = struct.Struct("<32sQII")
COMPONENT = struct.Struct("<IIBBH")
GRANT = struct.Struct("<IIIBBH")
RIGHT_READ = 1
RIGHT_WRITE = 2
RIGHT_TRANSFER = 4
KIND = {"kernel": 1, "bootstrap": 2, "component": 3, "resource": 4}
ROLE = {"init": 1, "service": 2, "driver": 3, "application": 4}
RIGHT = {"read": RIGHT_READ, "write": RIGHT_WRITE}


def fail(message: str) -> None:
    raise SystemExit(message)


def load_manifest() -> dict:
    env = os.environ.copy()
    env["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    output = subprocess.run(
        ["cargo", "run", "--manifest-path", str(ZUTAI), "-q", "-p", "zutai-cli", "--", "json", str(SOURCE)],
        cwd=ROOT,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    return json.loads(output)


def build_component(name: str, output: Path) -> bytes:
    source = COMPONENT_SOURCES / f"{name}.S"
    obj = output / f"{name}.o"
    binary = output / f"{name}.bin"
    subprocess.run(
        ["as", "--64", "-I", str(ROOT / "components" / "include"), "-o", obj, source],
        check=True,
    )
    subprocess.run(["objcopy", "-O", "binary", "-j", ".text", obj, binary], check=True)
    return binary.read_bytes()


def encode_string(value: str) -> bytes:
    data = value.encode("utf-8")
    if len(data) > 0xFFFF:
        fail(f"string too long: {value[:32]}")
    return struct.pack("<H", len(data)) + data


def main() -> None:
    if len(sys.argv) != 3:
        fail("usage: build-generation.py <kernel-elf> <output-dir>")
    kernel = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = load_manifest()

    if manifest["formatVersion"] != 1:
        fail("unsupported formatVersion")
    if manifest["target"] != TARGET:
        fail("unexpected target")

    components = manifest["components"]
    objects = manifest["objects"]
    grants = manifest["grants"]
    object_by_id = {obj["id"]: obj for obj in objects}
    if len(object_by_id) != len(objects):
        fail("object ids must be unique")
    component_index = {component["name"]: index for index, component in enumerate(components)}
    if len(component_index) != len(components):
        fail("component names must be unique")
    bootstrap = component_index.get(manifest["bootstrapComponent"])
    if bootstrap is None or components[bootstrap]["role"] != "init":
        fail("bootstrapComponent must name an init component")
    if object_by_id.get(manifest["kernelObject"], {}).get("kind") != "kernel":
        fail("kernelObject must name a kernel object")

    payloads: dict[str, bytes] = {manifest["kernelObject"]: b""}
    for component in components:
        if component["object"] not in object_by_id:
            fail(f"missing object for component {component['name']}")
        payloads[component["object"]] = build_component(component["name"], output)

    object_records = bytearray()
    blobs = bytearray()
    for obj in objects:
        payload = payloads.get(obj["id"])
        if payload is None:
            fail(f"missing payload for object {obj['id']}")
        digest = hashlib.sha256(payload).digest()
        object_records.extend(OBJECT.pack(digest, len(blobs), len(payload), KIND[obj["kind"]]))
        blobs.extend(payload)

    component_records = bytearray()
    strings = bytearray()
    for component in components:
        name_offset = len(strings)
        strings.extend(encode_string(component["name"]))
        object_index = next(index for index, obj in enumerate(objects) if obj["id"] == component["object"])
        component_records.extend(COMPONENT.pack(name_offset, object_index, ROLE[component["role"]], 0, 0))

    grant_records = bytearray()
    for grant in grants:
        source = component_index.get(grant["source"])
        target = component_index.get(grant["target"])
        if source is None or target is None:
            fail(f"grant endpoint missing: {grant['name']}")
        rights = 0
        for right in grant["rights"]:
            if right not in RIGHT:
                fail(f"unsupported right: {right}")
            rights |= RIGHT[right]
        if grant["transferable"]:
            rights |= RIGHT_TRANSFER
        name_offset = len(strings)
        strings.extend(encode_string(grant["name"]))
        grant_records.extend(GRANT.pack(name_offset, source, target, rights, 0, 0))

    header_without_hash = HEADER.pack(
        MAGIC,
        1,
        HEADER.size,
        manifest["generation"],
        bytes(32),
        len(objects),
        len(components),
        len(grants),
        bootstrap,
        0,
    )
    body = object_records + component_records + grant_records + strings + blobs
    digest = hashlib.sha256(header_without_hash + body).digest()
    header = HEADER.pack(
        MAGIC,
        1,
        HEADER.size,
        manifest["generation"],
        digest,
        len(objects),
        len(components),
        len(grants),
        bootstrap,
        0,
    )
    generation = header + body
    (output / "generation.bin").write_bytes(generation)
    print(f"Built generation {manifest['generation']} ({len(generation)} bytes, sha256:{digest.hex()})")


if __name__ == "__main__":
    main()
