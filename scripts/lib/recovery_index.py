"""Build a recovery index (M5.9).

Hand-written, beside the generated `boot_contracts` constants it uses rather
than inside them: `scripts/lib/boot_contracts.py` carries an `@generated`
banner, so anything added there is erased by the next `just boot_gen`.

Shared by `scripts/build/build-generation.py`, which embeds an index as a
generation resource, and `scripts/build/build-store-fixture.py`, which seeds one
on a disk for the seL4 recovery plane. One encoder, so a fixture the gate builds
and an index the product builds cannot drift.
"""

from __future__ import annotations

import hashlib
import struct

from boot_contracts import (
    MAX_RECOVERY_STATE_OBJECTS,
    RECOVERY_INDEX_HEADER,
    RECOVERY_INDEX_MAGIC,
    RECOVERY_INDEX_VERSION,
    RECOVERY_STATE_ENTRY,
)


def sha256(data: bytes) -> bytes:
    return hashlib.sha256(data).digest()


def binding_identity(name: str) -> bytes:
    """Stable identity for a state binding, matching `boot_contracts::recovery`."""
    encoded = name.encode("utf-8")
    return sha256(b"slime-state-binding-v1" + struct.pack("<H", len(encoded)) + encoded)


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
        raise ValueError("recovery state closure exceeds bound")
    # Ascending by binding identity: the decoder enforces the order, so the
    # encoder must produce it rather than rely on declaration order.
    entries = sorted(
        (
            (binding_identity(name), identity, schema)
            for name, identity, schema in state_entries
        ),
        key=lambda entry: entry[0],
    )
    if any(identity == bytes(32) or schema <= 0 for _, identity, schema in entries):
        raise ValueError("invalid recovery state entry")
    encoded = b"".join(
        RECOVERY_STATE_ENTRY.pack(binding, identity, schema, bytes(4))
        for binding, identity, schema in entries
    )
    # A content-addressed root over every binding, so a closure that gained or
    # lost an entry cannot pass as the one the index was signed for.
    state_root = sha256(
        b"".join(
            binding + identity + struct.pack("<I", schema)
            for binding, identity, schema in entries
        )
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
