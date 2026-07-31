#!/usr/bin/env python3

"""B10 host-side agreement check for the boot-layout resource.

The resource the host builder emits must describe the layout the kernel
resolves. Until the kernel consumes it, nothing at boot would notice a
disagreement, and once it does consume it a disagreement costs a full QEMU
cycle to find. This check compares the two host-side, in under a second:

- Every generation the emitter knows encodes and decodes cleanly, with its
  header carrying the number it was built for.
- Each emitted layout agrees slot-for-slot with the blessed fixture in
  `contracts/boot-layout/v1/fixtures/`, which records what the kernel actually
  resolved before the resource existed.

Slot 9 is compared by slot, label, and rights but not by object kind. The
fixture records the kind the kernel resolved on the run that captured it
(`block-device` with a drive attached, `object-store` without), while the
resource names the *role* — which is precisely the distinction the resource
exists to abstract.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))
_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "build"))

from boot_contracts import (
    BOOT_LAYOUT_ENTRY,
    BOOT_LAYOUT_ENTRY_BYTES,
    BOOT_LAYOUT_HEADER,
    BOOT_LAYOUT_HEADER_BYTES,
    BOOT_LAYOUT_MAGIC,
    BOOT_LAYOUT_VERSION,
)
from boot_layout import (
    ROLE,
    build_boot_layout,
    channel_identity,
    component_identity,
    layout_for,
    render_rust as render_boot_layout_rust,
)
from harness import ROOT

FIXTURES = ROOT / "contracts" / "boot-layout" / "v1" / "fixtures"

# Fixture stem -> the generation number that boots it. Mirrors `PROFILES` in
# check-boot-layout.py; the storage fixtures differ only in attached hardware,
# so several map to the same number.
FIXTURE_GENERATIONS = {
    "default": 1,
    "storage-read": 1,
    "storage-write": 2,
    "storage-fault": 3,
    "storage-store": 4,
    "directory": 6,
    "dango": 7,
    "generation-commands": 8,
    "powerbox": 9,
    "sample-plane": 10,
    "fabric-authority": 11,
    "fabric-stream": 12,
    "fabric-qos": 13,
    "fabric-call": 14,
    "fabric-operation": 15,
    "fabric-visibility": 16,
    "fabric-boot": 17,
    "bootstate": 99,
}

# What the kernel puts in the storage slot when the platform enumerates no
# block device: a read-only object store, standing in for the block capability
# the resource declares. `bootstrap.rs` holds the same fallback.
NO_DISK_FALLBACK = ("object-store", 0x1000)


def fail(message: str) -> None:
    raise SystemExit(f"boot layout resource: {message}")


def decode(blob: bytes) -> tuple[int, list[tuple[int, int, bytes, int]]]:
    """Decode an emitted resource the way `boot_layout::BootLayout` does."""
    if len(blob) < BOOT_LAYOUT_HEADER_BYTES:
        fail("resource shorter than its header")
    (magic, version, header_size, flags, number, count, total) = BOOT_LAYOUT_HEADER.unpack(
        blob[:BOOT_LAYOUT_HEADER_BYTES]
    )
    if magic != BOOT_LAYOUT_MAGIC:
        fail("bad magic")
    if version != BOOT_LAYOUT_VERSION or header_size != BOOT_LAYOUT_HEADER_BYTES:
        fail("unsupported version")
    if flags != 0:
        fail("unknown required flags")
    if total != len(blob) or total != BOOT_LAYOUT_HEADER_BYTES + count * BOOT_LAYOUT_ENTRY_BYTES:
        fail("declared length disagrees with the encoded entries")
    entries = []
    previous = -1
    for index in range(count):
        offset = BOOT_LAYOUT_HEADER_BYTES + index * BOOT_LAYOUT_ENTRY_BYTES
        identity, slot, role, rights = BOOT_LAYOUT_ENTRY.unpack(
            blob[offset : offset + BOOT_LAYOUT_ENTRY_BYTES]
        )
        if slot <= previous:
            fail(f"entries are not in ascending unique slot order at slot {slot}")
        previous = slot
        entries.append((slot, role, identity, rights))
    return number, entries


def fixture_rows(stem: str) -> list[tuple[int, str, str, int]]:
    """The (slot, kind, label, rights) rows the kernel emitted for this profile."""
    rows = []
    for line in (FIXTURES / f"{stem}.layout").read_text().splitlines():
        parts = line.split()
        if len(parts) == 5 and parts[1].isdigit():
            rows.append((int(parts[1]), parts[2], parts[3], int(parts[4], 16)))
    return rows


def expected_identity(role: str, label: str | None) -> bytes:
    if label is None:
        return bytes(32)
    return component_identity(label) if role == "executable" else channel_identity(label)


def check_generation(number: int) -> None:
    """Encode and decode one generation's resource, and check its header."""
    blob = build_boot_layout(number, fail)
    decoded_number, entries = decode(blob)
    if decoded_number != number:
        fail(f"generation {number} resource carries number {decoded_number}")
    declared = layout_for(number)
    if len(entries) != len(declared):
        fail(f"generation {number}: encoded {len(entries)} entries, declared {len(declared)}")
    for (slot, role, identity, rights), (want_slot, want_role, want_label, want_rights) in zip(
        entries, declared, strict=True
    ):
        if slot != want_slot or rights != want_rights or role != ROLE[want_role]:
            fail(f"generation {number} slot {slot}: encoded entry disagrees with the table")
        if identity != expected_identity(want_role, want_label):
            fail(f"generation {number} slot {slot}: identity disagrees with its label")


def check_fixture(stem: str, number: int) -> None:
    """Compare one generation's emitted layout against what the kernel resolved."""
    declared = {slot: (role, label, rights) for slot, role, label, rights in layout_for(number)}
    observed = fixture_rows(stem)
    if len(observed) != len(declared):
        fail(f"{stem}: kernel resolved {len(observed)} slots, resource declares {len(declared)}")
    for slot, kind, label, rights in observed:
        if slot not in declared:
            fail(f"{stem}: kernel filled slot {slot}, resource declares nothing there")
        role, want_label, want_rights = declared[slot]
        observed_label = None if label == "-" else label
        if observed_label != want_label:
            fail(f"{stem} slot {slot}: kernel label {observed_label!r}, resource {want_label!r}")
        # The storage capability is the one slot whose resolution the host
        # cannot predict. The resource declares the authority a present block
        # device carries; when the platform enumerates none, the kernel
        # substitutes a read-only object store. Both kind and rights therefore
        # differ legitimately, and only the fallback shape is accepted.
        if role == "storage-capability":
            if kind == "block-device":
                if rights != want_rights:
                    fail(
                        f"{stem} slot {slot}: block device rights {rights:#x}, "
                        f"resource {want_rights:#x}"
                    )
            elif (kind, rights) != NO_DISK_FALLBACK:
                fail(
                    f"{stem} slot {slot}: no block device, but the kernel resolved "
                    f"{kind!r} {rights:#x} rather than the {NO_DISK_FALLBACK} fallback"
                )
            continue
        if rights != want_rights:
            fail(f"{stem} slot {slot}: kernel rights {rights:#x}, resource {want_rights:#x}")
        if kind != kind_for(role):
            fail(f"{stem} slot {slot}: kernel kind {kind!r}, resource role {role!r}")


def kind_for(role: str) -> str:
    """The dump kind a role resolves to, where the mapping is unambiguous."""
    return {
        "endpoint-client": "endpoint",
        "endpoint-service": "endpoint",
        "directory": "directory",
    }.get(role, role)


def check_component_fallback() -> None:
    """The checked-in slot table must match what the emitter renders.

    `components/bins/build.rs` falls back to it when `SLIME_BOOT_LAYOUT` is
    unset, which is every plain `cargo build`. If it drifts, userspace compiles
    against slots the kernel no longer places, and only a QEMU boot would say
    so.
    """
    path = ROOT / "components" / "bins" / "src" / "default_boot_layout.rs"
    expected = render_boot_layout_rust(1)
    if path.read_text() != expected:
        fail(f"{path.relative_to(ROOT)} is stale; regenerate it from layout_for(1)")


def main() -> None:
    check_component_fallback()
    print("boot layout resource: component fallback table is current")
    numbers = sorted(set(FIXTURE_GENERATIONS.values()))
    for number in numbers:
        check_generation(number)
    print(f"boot layout resource: {len(numbers)} generations encode and decode")

    # `build-generation.py` builds two generations from one manifest, and the
    # layout resource must be recomputed for each. Every generation's resource
    # therefore differs from generation 1's, if only in the header number —
    # which is what makes a builder that emitted one into both detectable.
    baseline = build_boot_layout(1, fail)
    for number in numbers:
        if number != 1 and build_boot_layout(number, fail) == baseline:
            fail(f"generation 1 and {number} encode identical resources")

    for stem, number in sorted(FIXTURE_GENERATIONS.items()):
        check_fixture(stem, number)
    print(f"boot layout resource: {len(FIXTURE_GENERATIONS)} fixtures agree with the resource")
    print("boot layout resource check: ok")


if __name__ == "__main__":
    main()
