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

TARGET = "x86_64-qemu-virtio"
COMPONENTS_WORKSPACE = ROOT / "components" / "Cargo.toml"
COMPONENTS_ELF_DIR = ROOT / "components" / "target" / "x86_64-unknown-none" / "release"
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

# Component image format constants (contracts/component/v1). Mirrored by the
# kernel decoder and by scripts/check-generation.py; drift is caught by
# `just generation_check`.
IMAGE_MAGIC = b"SLIMECMP"
IMAGE_FORMAT_VERSION = 1
IMAGE_KERNEL_ABI = 1
IMAGE_HEADER = struct.Struct("<8sIIIIHHI")
IMAGE_SEGMENT = struct.Struct("<IIIIHH")
IMAGE_BASE = 0x400000  # must match ENTRY_VA in kernel/src/task/mod.rs
PAGE_SIZE = 4096
MAX_IMAGE_BYTES = 16 * 1024 * 1024
MAX_STACK_BYTES = 1024 * 1024
DEFAULT_STACK_BYTES = 16384
SEGMENT_WRITE = 1
SEGMENT_EXEC = 2


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


def component_image(name: str, elf: Path, stack_bytes: int) -> bytes:
    """Convert a statically linked component ELF into a component image.

    Reads the ELF program headers directly (stdlib only) so the conversion is
    deterministic and dependency-free. Enforces the same rules the kernel
    decoder validates, so a built image can never be rejected at boot.
    """
    data = elf.read_bytes()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail(f"{name}: not a 64-bit little-endian ELF")
    elf_type, machine = struct.unpack_from("<HH", data, 16)
    if elf_type != 2 or machine != 62:
        fail(f"{name}: not a static x86-64 executable")
    entry = struct.unpack_from("<Q", data, 24)[0]
    phoff = struct.unpack_from("<Q", data, 32)[0]
    _, phentsize, phnum = struct.unpack_from("<HHH", data, 52)

    segments = []
    for index in range(phnum):
        p_type, p_flags = struct.unpack_from("<II", data, phoff + index * phentsize)
        p_offset, p_vaddr, _, p_filesz, p_memsz = struct.unpack_from(
            "<QQQQQ", data, phoff + index * phentsize + 8
        )
        if p_type != 1 or p_memsz == 0:  # PT_LOAD with content only
            continue
        segments.append((p_vaddr, p_offset, p_filesz, p_memsz, p_flags))
    segments.sort()
    if not 1 <= len(segments) <= 16:
        fail(f"{name}: {len(segments)} loadable segments outside 1..=16")
    if segments[0][0] != IMAGE_BASE:
        fail(f"{name}: link base {segments[0][0]:#x} is not {IMAGE_BASE:#x}")
    if entry < IMAGE_BASE:
        fail(f"{name}: entry point below link base")
    entry_offset = entry - IMAGE_BASE

    records = bytearray()
    payload = bytearray()
    previous_end = 0
    total_pages = 0
    entry_ok = False
    for vaddr, offset, filesz, memsz, elf_flags in segments:
        if filesz > memsz or vaddr % PAGE_SIZE or vaddr < previous_end:
            fail(f"{name}: invalid or overlapping segment at {vaddr:#x}")
        # ELF PF_X=1 / PF_W=2 -> image EXEC/WRITE flag bits.
        flags = (SEGMENT_EXEC if elf_flags & 1 else 0) | (SEGMENT_WRITE if elf_flags & 2 else 0)
        if flags & (SEGMENT_WRITE | SEGMENT_EXEC) == (SEGMENT_WRITE | SEGMENT_EXEC):
            fail(f"{name}: segment at {vaddr:#x} is both writable and executable")
        relative = vaddr - IMAGE_BASE
        if flags & SEGMENT_EXEC and relative <= entry_offset < relative + memsz:
            entry_ok = True
        records += IMAGE_SEGMENT.pack(relative, memsz, len(payload), filesz, flags, 0)
        payload += data[offset : offset + filesz]
        previous_end = vaddr + memsz
        total_pages += -(-memsz // PAGE_SIZE)
    if not entry_ok:
        fail(f"{name}: entry point outside executable segments")
    if total_pages * PAGE_SIZE > MAX_IMAGE_BYTES:
        fail(f"{name}: image footprint exceeds {MAX_IMAGE_BYTES} bytes")

    header = IMAGE_HEADER.pack(
        IMAGE_MAGIC,
        IMAGE_FORMAT_VERSION,
        IMAGE_HEADER.size,
        IMAGE_KERNEL_ABI,
        entry_offset,
        len(segments),
        0,
        stack_bytes,
    )
    return header + bytes(records) + bytes(payload)


def build_rust_components() -> None:
    # cargo discovers .cargo/config.toml by walking up from the *current
    # directory*, not from --manifest-path, so this must run with cwd inside
    # components/ to pick up components/.cargo/config.toml (target and
    # linker flags).
    subprocess.run(
        ["cargo", "build", "--release"],
        cwd=COMPONENTS_WORKSPACE.parent,
        check=True,
    )


def build_component(name: str, stack_bytes: int) -> bytes:
    return component_image(name, COMPONENTS_ELF_DIR / name, stack_bytes)


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
    generation_number = int(os.environ.get("SLIME_GENERATION_NUMBER") or manifest["generation"])

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

    build_rust_components()

    payloads: dict[str, bytes] = {manifest["kernelObject"]: b""}
    for component in components:
        if component["object"] not in object_by_id:
            fail(f"missing object for component {component['name']}")
        stack_bytes = component.get("stackBytes", DEFAULT_STACK_BYTES)
        if (
            not isinstance(stack_bytes, int)
            or stack_bytes <= 0
            or stack_bytes % PAGE_SIZE
            or stack_bytes > MAX_STACK_BYTES
        ):
            fail(f"component {component['name']}: invalid stackBytes {stack_bytes}")
        payloads[component["object"]] = build_component(component["name"], stack_bytes)

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
        generation_number,
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
        generation_number,
        digest,
        len(objects),
        len(components),
        len(grants),
        bootstrap,
        0,
    )
    generation = header + body
    (output / "generation.bin").write_bytes(generation)
    print(
        f"Built generation {generation_number} "
        f"({len(generation)} bytes, sha256:{digest.hex()})"
    )


if __name__ == "__main__":
    main()
