"""Canonical recovery-index encoder used by fixture builders.

This hand-written module sits beside generated ``boot_contracts`` constants;
it is currently consumed by ``build-store-fixture.py``. Product generations
are assembled independently by ``build-generation.py``, so changes here must
continue to mirror the Rust decoder rather than claiming a shared product
encoding path.
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
    if len(target_generation) != 32 or target_generation == bytes(32):
        raise ValueError("target generation must be a nonzero 32-byte identity")
    if len(generation_root) != 32 or generation_root == bytes(32):
        raise ValueError("generation root must be a nonzero 32-byte identity")
    if len(state_entries) > MAX_RECOVERY_STATE_OBJECTS:
        raise ValueError("recovery state closure exceeds bound")
    entries = sorted(
        (
            (binding_identity(name), identity, schema)
            for name, identity, schema in state_entries
        ),
        key=lambda entry: entry[0],
    )
    if any(len(identity) != 32 or identity == bytes(32) or schema <= 0 for _, identity, schema in entries):
        raise ValueError("invalid recovery state entry")
    if any(left[0] == right[0] for left, right in zip(entries, entries[1:], strict=False)):
        raise ValueError("duplicate recovery state binding")
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


def _test_validation_parity() -> None:
    nonzero = bytes([1]) * 32
    arguments = (nonzero, nonzero, 1, 0, 2, 3)
    for entries in [
        [("same", nonzero, 1), ("same", bytes([2]) * 32, 1)],
        [("zero-root", bytes(32), 1)],
    ]:
        try:
            build_recovery_index(
                arguments[0],
                arguments[1],
                arguments[2],
                arguments[3],
                entries,
                arguments[4],
                arguments[5],
            )
        except ValueError:
            pass
        else:
            raise AssertionError("invalid recovery index input was accepted")
