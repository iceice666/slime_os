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
import os
import re
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
from harness import ROOT, load_script

SEL4_RESOLVER_STEMS = {
    "sel4-qos",
    "sel4-call",
    "sel4-operation",
    "sel4-visibility",
    "sel4-matrix",
    "sel4-boot",
    "sel4-storage",
    "sel4-store",
    "sel4-rollback",
    "sel4-recovery",
    "sel4-generation",
    "sel4-directory",
    "sel4-filesystem",
    "sel4-dango",
    "sel4-input",
    "sel4-powerbox",
    "sel4-transfer",
}
FIXTURES = ROOT / "contracts" / "boot-layout" / "v1" / "fixtures"

# The boot profiles are declared in the generation manifest, so this check reads
# them through the builder rather than restating the component sets (B11).
builder = load_script("build_generation", "build/build-generation.py")
MANIFEST = builder.load_manifest()

# Fixture stem -> (generation number, boot profile) used by the retained
# contract corpus; storage fixtures differ only in attached hardware.
#
# B11: every fixture below `product` boots a profile that declares verification
# scaffolding, which is why their slot tables are unchanged by that milestone.
# `product` is the boot the product ships -- no probes, no scenario doubles --
# and is the one layout the scaffolding profiles cannot speak for.
FIXTURE_PROFILES = {
    "default": (1, "test"),
    "product": (1, "default"),
    "storage-read": (1, "test"),
    "storage-write": (2, "test"),
    "storage-fault": (3, "test"),
    "storage-store": (4, "test"),
    "directory": (6, "test"),
    "dango": (7, "test"),
    "generation-commands": (8, "test"),
    "powerbox": (9, "test"),
    "sample-plane": (10, "test"),
    "fabric-authority": (11, "test"),
    "fabric-stream": (12, "test"),
    "fabric-qos": (13, "test"),
    "fabric-call": (14, "test"),
    "fabric-operation": (15, "test"),
    "fabric-visibility": (16, "visibility"),
    "fabric-boot": (17, "unified"),
    "bootstate": (99, "test"),
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
        # `>= 5`, not `== 5`: B26 appends a sixth `declared=0x…` field to a row
        # whose layout rights differ from the installed ones. Only the four
        # leading fields are this check's subject, so a row carrying the tail is
        # read rather than silently dropped — an exact-length filter would make
        # such a row vanish and this function would compare a short table
        # against a full one. Unreachable today, since `FIXTURE_PROFILES` names
        # only the nineteen x86 stems and the x86 dump does not emit the field;
        # widened here so porting it at P5.4.final does not have to remember.
        if len(parts) >= 5 and parts[1].isdigit():
            rows.append((int(parts[1]), parts[2], parts[3], int(parts[4], 16)))
    return rows


def expected_identity(role: str, label: str | None) -> bytes:
    if label is None:
        return bytes(32)
    return component_identity(label) if role == "executable" else channel_identity(label)


def profile_executables(profile: str) -> set[str]:
    """Executables the resolved initial graph addresses through boot slots."""
    resolved = builder.resolve_boot_profile(MANIFEST, profile)
    return builder.layout_executables(resolved)


def check_generation(number: int, profile: str) -> None:
    """Encode and decode one generation's resource, and check its header."""
    executables = profile_executables(profile)
    blob = build_boot_layout(number, fail, executables)
    decoded_number, entries = decode(blob)
    if decoded_number != number:
        fail(f"generation {number} resource carries number {decoded_number}")
    declared = layout_for(number, executables)
    if len(entries) != len(declared):
        fail(f"generation {number}: encoded {len(entries)} entries, declared {len(declared)}")
    for (slot, role, identity, rights), (want_slot, want_role, want_label, want_rights) in zip(
        entries, declared, strict=True
    ):
        if slot != want_slot or rights != want_rights or role != ROLE[want_role]:
            fail(f"generation {number} slot {slot}: encoded entry disagrees with the table")
        if identity != expected_identity(want_role, want_label):
            fail(f"generation {number} slot {slot}: identity disagrees with its label")


def check_fixture(stem: str, number: int, profile: str) -> None:
    """Compare one generation's emitted layout against what the kernel resolved."""
    declared = {
        slot: (role, label, rights)
        for slot, role, label, rights in layout_for(number, profile_executables(profile))
    }
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


def check_sel4_fixtures() -> int:
    """Cross-check seL4 replacement tables against the resolver fixtures."""
    previous_target = os.environ.get("SLIME_TARGET_PROFILE")
    previous_manifest = os.environ.get("SLIME_SEL4_MANIFEST")
    checked = 0
    try:
        os.environ["SLIME_TARGET_PROFILE"] = builder.SEL4_TARGET_PROFILE
        for manifest_name in sorted(builder.SEL4_MANIFESTS):
            stem = "sel4" if manifest_name == "sel4" else manifest_name
            if stem not in SEL4_RESOLVER_STEMS:
                continue
            os.environ["SLIME_SEL4_MANIFEST"] = manifest_name
            manifest = builder.load_manifest()
            fixture = FIXTURES / f"{stem}.layout"
            if not fixture.is_file():
                fail(f"missing seL4 layout fixture {fixture.relative_to(ROOT)}")
            executables = builder.layout_executables(manifest)
            declared = {
                slot: (role, label, rights)
                for slot, role, label, rights in layout_for(manifest["generation"], executables)
            }
            observed = fixture_rows(stem)
            if len(observed) != len(declared):
                fail(
                    f"{stem}: frozen layout has {len(observed)} rows, resolver expects "
                    f"{len(declared)}"
                )
            for slot, kind, label, rights in observed:
                expected = declared.get(slot)
                if expected is None:
                    fail(f"{stem}: frozen slot {slot} is absent from layout_for()")
                role, expected_label, expected_rights = expected
                observed_label = None if label == "-" else label
                if (observed_label, rights) != (expected_label, expected_rights):
                    fail(
                        f"{stem} slot {slot}: frozen {(observed_label, rights)!r}, "
                        f"resolver {(expected_label, expected_rights)!r}"
                    )
                if kind != kind_for(role):
                    fail(f"{stem} slot {slot}: frozen kind {kind!r}, resolver role {role!r}")
            checked += 1
    finally:
        if previous_target is None:
            os.environ.pop("SLIME_TARGET_PROFILE", None)
        else:
            os.environ["SLIME_TARGET_PROFILE"] = previous_target
        if previous_manifest is None:
            os.environ.pop("SLIME_SEL4_MANIFEST", None)
        else:
            os.environ["SLIME_SEL4_MANIFEST"] = previous_manifest
    return checked

def check_component_fallback() -> None:
    """The checked-in slot table must match what the emitter renders.

    `components/bins/build.rs` falls back to it when `SLIME_BOOT_LAYOUT` is
    unset, which is every plain `cargo build`. If it drifts, userspace compiles
    against slots the kernel no longer places, and only a QEMU boot would say
    so.
    """
    path = ROOT / "components" / "bins" / "src" / "default_boot_layout.rs"
    expected = render_boot_layout_rust(1, profile_executables("default"))
    if path.read_text() != expected:
        fail(
            f"{path.relative_to(ROOT)} is stale; regenerate it from the product "
            "profile's layout_for(1)"
        )


def check_bootstrap_binding_projection() -> None:
    """Compile-time semantic slots must equal the explicit bootstrap bindings."""
    prior_target = os.environ.get("SLIME_TARGET_PROFILE")
    prior_manifest = os.environ.get("SLIME_SEL4_MANIFEST")
    os.environ["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    os.environ["SLIME_SEL4_MANIFEST"] = "sel4"
    try:
        manifest = builder.load_manifest()
    finally:
        if prior_target is None:
            os.environ.pop("SLIME_TARGET_PROFILE", None)
        else:
            os.environ["SLIME_TARGET_PROFILE"] = prior_target
        if prior_manifest is None:
            os.environ.pop("SLIME_SEL4_MANIFEST", None)
        else:
            os.environ["SLIME_SEL4_MANIFEST"] = prior_manifest
    bindings, roles = builder.bootstrap_binding_projection(manifest)
    rendered = render_boot_layout_rust(
        manifest["generation"],
        {item["name"] for item in manifest["executables"]},
        bindings,
        roles,
    )
    expected = {
        "CONSOLE_SLOT": bindings["console"],
        "CONSOLE_OUTPUT_SLOT": bindings["console-output"],
        "SPAWN_SERVICE_SLOT": bindings["spawn-service"],
        "SPAWN_SERVICE_RPC_SLOT": bindings["spawn-service-rpc"],
        "SHARED_BUFFER_FACTORY_SLOT": roles["shared-buffer-factory"],
    }
    for constant, slot in expected.items():
        declaration = f"pub const {constant}: u32 = {slot};"
        if declaration not in rendered:
            fail(f"bootstrap binding projection did not emit {declaration}")


def check_derived_layout_agrees(stem: str) -> int:
    """B71: the resource, the constants, and the frozen layout are one table.

    Three readings of where the root places the bootstrap component's
    capabilities: the binary resource the root decodes, the Rust constants a
    component compiles against, and the `.layout` fixture recording what the
    root actually resolved. B71 was two of them disagreeing — `spawn-service` at
    slot 4 in the resource against 5 everywhere else — with nothing comparing
    them, because until CP2's runtime query nothing read the resource's content.

    Also refuses two differently-named constants sharing one slot. That was the
    second half of B71: a role the generation does not grant kept whatever slot
    the full static table gave it, so the component-graph plane's table declared
    `STORAGE_CAPABILITY_SLOT` as `ECHO_AGENT_SLOT`'s real 7 and
    `GENERATION_CONTROL_SLOT` as the real shared-buffer factory's 8. Both were
    silently unused; either would have handed out a live capability of the wrong
    kind. `_0`-suffixed aliases are the one legitimate repeat.
    """
    prior_target = os.environ.get("SLIME_TARGET_PROFILE")
    prior_manifest = os.environ.get("SLIME_SEL4_MANIFEST")
    os.environ["SLIME_TARGET_PROFILE"] = builder.SEL4_TARGET_PROFILE
    os.environ["SLIME_SEL4_MANIFEST"] = stem
    try:
        manifest = builder.load_manifest()
    finally:
        if prior_target is None:
            os.environ.pop("SLIME_TARGET_PROFILE", None)
        else:
            os.environ["SLIME_TARGET_PROFILE"] = prior_target
        if prior_manifest is None:
            os.environ.pop("SLIME_SEL4_MANIFEST", None)
        else:
            os.environ["SLIME_SEL4_MANIFEST"] = prior_manifest

    entries = builder.layout_from_manifest(manifest, builder.RIGHT, builder.RIGHT_TRANSFER)
    number = manifest["generation"]

    # The resource the root decodes.
    encoded_number, encoded = decode(build_boot_layout(number, fail, entries=entries))
    if encoded_number != number:
        fail(f"{stem}: resource carries generation {encoded_number}, manifest says {number}")
    resource = {slot: (role, rights) for slot, role, _identity, rights in encoded}

    # What the root actually resolved, frozen by `just sel4_boot_layout_check`.
    observed = fixture_rows(stem)
    if len(observed) != len(resource):
        fail(
            f"{stem}: the root resolved {len(observed)} slots, the derived resource "
            f"declares {len(resource)}"
        )
    for slot, kind, label, rights in observed:
        if slot not in resource:
            fail(f"{stem}: the root filled slot {slot}, the resource declares nothing there")
        role_wire, resource_rights = resource[slot]
        role = next(name for name, wire in ROLE.items() if wire == role_wire)
        if rights != resource_rights:
            fail(
                f"{stem} slot {slot}: the root resolved rights {rights:#x}, the resource "
                f"declares {resource_rights:#x}"
            )
        if kind != kind_for(role):
            fail(f"{stem} slot {slot}: the root resolved kind {kind!r}, the resource role {role!r}")
        observed_label = None if label == "-" else label
        if expected_identity(role, observed_label) != next(
            identity for entry_slot, _role, identity, _rights in encoded if entry_slot == slot
        ):
            fail(f"{stem} slot {slot}: identity disagrees with the label {observed_label!r}")

    # The constants a component compiles against, over the same derivation.
    bindings, _roles = builder.bootstrap_binding_projection(manifest)
    rendered = render_boot_layout_rust(
        number,
        {item["name"] for item in manifest["executables"]},
        bindings,
        entries=entries,
    )
    slots_by_name: dict[str, int] = {}
    for line in rendered.splitlines():
        # `u32` only, and only `*_SLOT`: `BOOT_LAYOUT_GENERATION` is a `u64`
        # generation number that shares this table but names no slot.
        match = re.fullmatch(r"pub const (\w+_SLOT(?:_\d+)?): u32 = (\d+);", line)
        if match:
            slots_by_name[match[1]] = int(match[2])
    for name, slot in slots_by_name.items():
        # A layout row's constant must name the slot the resource declares.
        if slot in resource:
            continue
        # Otherwise it must be a real binding of a kind that occupies no row —
        # an endpoint — rather than a stale number.
        if slot not in bindings.values():
            fail(
                f"{stem}: {name} is {slot}, which is neither a resource row nor a "
                "declared binding slot"
            )
    collisions: dict[int, list[str]] = {}
    for name, slot in sorted(slots_by_name.items()):
        collisions.setdefault(slot, []).append(name)
    for slot, names in sorted(collisions.items()):
        distinct = {name.removesuffix("_0") for name in names}
        if len(distinct) > 1:
            fail(f"{stem}: slot {slot} is declared by {', '.join(names)}")
    return len(resource)

def main() -> None:
    check_component_fallback()
    print("boot layout resource: component fallback table is current")
    check_bootstrap_binding_projection()
    print("boot layout resource: bootstrap binding projection is current")
    pairs = sorted(set(FIXTURE_PROFILES.values()))
    for number, profile in pairs:
        check_generation(number, profile)
    print(f"boot layout resource: {len(pairs)} generation/profile pairs encode and decode")

    # `build-generation.py` builds two generations from one manifest, and the
    # layout resource must be recomputed for each. Every generation's resource
    # therefore differs from generation 1's, if only in the header number —
    # which is what makes a builder that emitted one into both detectable.
    scaffolding = profile_executables("test")
    baseline = build_boot_layout(1, fail, scaffolding)
    for number, profile in pairs:
        if profile != "test":
            continue
        if number != 1 and build_boot_layout(number, fail, scaffolding) == baseline:
            fail(f"generation 1 and {number} encode identical resources")

    for stem, (number, profile) in sorted(FIXTURE_PROFILES.items()):
        check_fixture(stem, number, profile)
    print(f"boot layout resource: {len(FIXTURE_PROFILES)} fixtures agree with the resource")
    sel4_checked = check_sel4_fixtures()
    print(f"boot layout resource: {sel4_checked} seL4 fixtures agree with layout_for()")
    # B71. Every seL4 plane, not just the 17 `SEL4_RESOLVER_STEMS`: the two the
    # older arm skipped -- `sel4` and `sel4-spawn` -- are exactly the two whose
    # static table disagreed with what the root placed.
    derived_rows = 0
    derived_planes = 0
    for manifest_name in sorted(builder.SEL4_MANIFESTS):
        if not (FIXTURES / f"{manifest_name}.layout").is_file():
            continue
        derived_rows += check_derived_layout_agrees(manifest_name)
        derived_planes += 1
    print(
        f"boot layout resource: {derived_planes} seL4 planes agree across resource, "
        f"constants, and resolved layout ({derived_rows} rows), with no slot declared twice"
    )
    print("boot layout resource check: ok")


if __name__ == "__main__":
    main()
