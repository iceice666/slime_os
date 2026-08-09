#!/usr/bin/env python3
"""Build the M5.4 GPT + object store fixture images.

Every variant shares one layout: a 2048-sector raw image with a protective
MBR, primary and backup GPT copies, and a single Slime OS object-store
partition (type GUID "SLIMEOSSTOREGPT!") at LBA 40..2014. The store carries
genesis superblock slot B (sequence 1, empty), committed slot A (sequence 2,
one seeded object), and the seeded record. Fault variants corrupt exactly one
structure so the guest must recover or reject per the documented rules.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import hashlib
import struct
import zlib
from pathlib import Path

from harness import SECTOR_SIZE

SECTOR = SECTOR_SIZE
CAPACITY = 2048
ENTRY_COUNT = 128
ENTRY_SIZE = 128
FIRST_USABLE = 34
LAST_USABLE = 2014
PRIMARY_ENTRIES_LBA = 2
BACKUP_HEADER_LBA = CAPACITY - 1
BACKUP_ENTRIES_LBA = BACKUP_HEADER_LBA - (ENTRY_COUNT * ENTRY_SIZE) // SECTOR
STORE_FIRST = 40
STORE_LAST = LAST_USABLE
PARTITION_SECTORS = STORE_LAST - STORE_FIRST + 1

DISK_GUID = b"SLIMEOSDISKGUID!"
STORE_TYPE_GUID = b"SLIMEOSSTOREGPT!"
SEEDED_TYPE = 1
SEEDED_PAYLOAD_LEN = 512
SEEDED_RECORD_SECTORS = 2
from boot_contracts import (
    STORE_FORMAT_VERSION as FORMAT_VERSION,
    STORE_RECORD,
    STORE_RECORD_AREA_START as RECORD_AREA_START,
    STORE_RECORD_CONTENT_HASH_OFFSET,
    STORE_RECORD_FORMAT_VERSION_OFFSET,
    STORE_RECORD_HEADER_SIZE_OFFSET,
    STORE_RECORD_MAGIC as RECORD_MAGIC,
    STORE_RECORD_OBJ_TYPE_OFFSET,
    STORE_RECORD_PAYLOAD_LEN_OFFSET,
    STORE_SUPERBLOCK_APPEND_LBA_OFFSET,
    STORE_SUPERBLOCK_CRC32_OFFSET,
    STORE_SUPERBLOCK_FORMAT_VERSION_OFFSET,
    STORE_SUPERBLOCK_HEADER_SIZE_OFFSET,
    STORE_SUPERBLOCK_MAGIC as SUPERBLOCK_MAGIC,
    STORE_SUPERBLOCK_OBJECT_COUNT_OFFSET,
    STORE_SUPERBLOCK_PARTITION_SECTORS_OFFSET,
    STORE_SUPERBLOCK_RECORD_AREA_START_OFFSET,
    STORE_SUPERBLOCK_SEQUENCE_OFFSET,
)
from recovery_index import binding_identity, build_recovery_index, sha256
from boot_contracts import (
    RELEASE_BYTES,
    TRANSFER_HEADER,
    TRANSFER_HEADER_BYTES,
    TRANSFER_HEADER_HASH_END,
    TRANSFER_HEADER_HASH_OFFSET,
    TRANSFER_MAGIC,
    TRANSFER_OBJECT,
    TRANSFER_OBJECT_FLAG_PAYLOAD as OBJECT_FLAG_PAYLOAD,
    TRANSFER_STATE,
    TRANSFER_STATE_FLAG_READ_ONLY as STATE_FLAG_READ_ONLY,
    TRANSFER_STATE_FLAG_TRAVEL as STATE_FLAG_TRAVEL,
    TRANSFER_VERSION,
)

HEADER_SIZE = STORE_RECORD.size

MESSAGE = b"Slime OS M5.4 object store fixture\n"

VARIANTS = [
    "happy",
    "gpt-primary-damaged",
    "gpt-conflict",
    "superblock-newest-damaged",
    "superblock-both-damaged",
    "interrupted-append",
    # P5.4.2c's recovery plane: both BootState slots corrupt and a valid
    # recovery index naming the seeded object as the state closure. The store
    # itself is the happy one, because reconstruction must verify real objects.
    "recovery",
    # P5.4.2c's recovery layout plus a transfer manifest, for M6.7's source
    # device. The receiver uses the `happy` variant: it needs a validated
    # partition for its BootState slots and nothing else.
    "transfer",
]

# The BootState slots and the recovery index, partition-relative. Above the
# store's record area so the two structures share a partition without
# overlapping; kept in step with the probes that read them.
STATE_SLOT_A = 1024
STATE_SLOT_B = 1025
RECOVERY_INDEX_LBA = 1026
RECOVERY_INDEX_SECTORS = 4
RECOVERY_TARGET = bytes([0x55]) * 32
RECOVERY_GENERATION_ROOT = bytes([0x66]) * 32
RECOVERY_RELEASE_SEQUENCE = 3
RECOVERY_BINDING = "recovered-state"
RECOVERY_SCHEMA_VERSION = 1

# M6.7: where the source carries its transfer manifest, and what it carries.
TRANSFER_MANIFEST_LBA = 1030
TRANSFER_MANIFEST_SECTORS = 16
TRANSFER_GENERATION = bytes([0x77]) * 32
TRANSFER_GENERATION_ROOT = bytes([0x88]) * 32
TRANSFER_RELEASE_SEQUENCE = 5
TRANSFER_OBJECT_TYPE = 1
TRANSFER_PAYLOAD = b"Slime OS M6.7 transferred object\n"
TRANSFER_STATE_BINDING = "transferred-state"


def seeded_payload() -> bytes:
    data = bytearray(SEEDED_PAYLOAD_LEN)
    data[: len(MESSAGE)] = MESSAGE
    for index in range(len(MESSAGE), SEEDED_PAYLOAD_LEN):
        data[index] = (index * 37 + 11) & 0xFF
    return bytes(data)


def gpt_header(
    current_lba: int, backup_lba: int, entries_lba: int, entries_crc: int, disk_guid: bytes
) -> bytes:
    header = bytearray(SECTOR)
    struct.pack_into("<8s", header, 0, b"EFI PART")
    struct.pack_into("<I", header, 8, 0x00010000)
    struct.pack_into("<I", header, 12, 92)
    struct.pack_into("<Q", header, 24, current_lba)
    struct.pack_into("<Q", header, 32, backup_lba)
    struct.pack_into("<Q", header, 40, FIRST_USABLE)
    struct.pack_into("<Q", header, 48, LAST_USABLE)
    struct.pack_into("<16s", header, 56, disk_guid)
    struct.pack_into("<Q", header, 72, entries_lba)
    struct.pack_into("<I", header, 80, ENTRY_COUNT)
    struct.pack_into("<I", header, 84, ENTRY_SIZE)
    struct.pack_into("<I", header, 88, entries_crc)
    crc = zlib.crc32(bytes(header[:92]))
    struct.pack_into("<I", header, 16, crc)
    return bytes(header)


def gpt_entries() -> bytes:
    table = bytearray(ENTRY_COUNT * ENTRY_SIZE)
    struct.pack_into("<16s", table, 0, STORE_TYPE_GUID)
    struct.pack_into("<16s", table, 16, b"SLIMEOSSTOREINST")
    struct.pack_into("<Q", table, 32, STORE_FIRST)
    struct.pack_into("<Q", table, 40, STORE_LAST)
    return bytes(table)


def superblock(sequence: int, append_lba: int, object_count: int) -> bytes:
    sector = bytearray(SECTOR)
    struct.pack_into("<8s", sector, 0, SUPERBLOCK_MAGIC)
    struct.pack_into("<I", sector, STORE_SUPERBLOCK_FORMAT_VERSION_OFFSET, FORMAT_VERSION)
    struct.pack_into("<I", sector, STORE_SUPERBLOCK_HEADER_SIZE_OFFSET, HEADER_SIZE)
    struct.pack_into("<Q", sector, STORE_SUPERBLOCK_SEQUENCE_OFFSET, sequence)
    struct.pack_into("<Q", sector, STORE_SUPERBLOCK_APPEND_LBA_OFFSET, append_lba)
    struct.pack_into("<I", sector, STORE_SUPERBLOCK_OBJECT_COUNT_OFFSET, object_count)
    struct.pack_into("<Q", sector, STORE_SUPERBLOCK_RECORD_AREA_START_OFFSET, RECORD_AREA_START)
    struct.pack_into("<Q", sector, STORE_SUPERBLOCK_PARTITION_SECTORS_OFFSET, PARTITION_SECTORS)
    crc = zlib.crc32(bytes(sector[:STORE_SUPERBLOCK_CRC32_OFFSET]))
    struct.pack_into("<I", sector, STORE_SUPERBLOCK_CRC32_OFFSET, crc)
    return bytes(sector)


def record(obj_type: int, payload: bytes) -> bytes:
    digest = hashlib.sha256(payload).digest()
    header = bytearray(HEADER_SIZE)
    struct.pack_into("<8s", header, 0, RECORD_MAGIC)
    struct.pack_into("<I", header, STORE_RECORD_FORMAT_VERSION_OFFSET, FORMAT_VERSION)
    struct.pack_into("<I", header, STORE_RECORD_HEADER_SIZE_OFFSET, HEADER_SIZE)
    struct.pack_into("<I", header, STORE_RECORD_OBJ_TYPE_OFFSET, obj_type)
    struct.pack_into("<Q", header, STORE_RECORD_PAYLOAD_LEN_OFFSET, len(payload))
    struct.pack_into("<32s", header, STORE_RECORD_CONTENT_HASH_OFFSET, digest)
    data = bytes(header) + payload
    data += bytes(-len(data) % SECTOR)
    return data


def place(image: bytearray, lba: int, data: bytes) -> None:
    image[lba * SECTOR : lba * SECTOR + len(data)] = data


def recovery_index(state_object: bytes) -> bytes:
    """A recovery index naming one state object: the store's seeded record.

    Built with the same `build_recovery_index` the generation builder uses, so
    the fixture and the product encode one format rather than two.
    """
    return build_recovery_index(
        RECOVERY_TARGET,
        RECOVERY_GENERATION_ROOT,
        RECOVERY_RELEASE_SEQUENCE,
        0,
        [(RECOVERY_BINDING, state_object, RECOVERY_SCHEMA_VERSION)],
        STORE_FIRST + RECORD_AREA_START,
        STORE_LAST,
    )


def transfer_manifest() -> bytes:
    """A one-object, one-state transfer manifest.

    Encoded here rather than by `build-transfer.py` because that script builds
    from a *pair of real generations*, and this fixture needs only a
    well-formed record for the seL4 plane to verify: the properties under test
    are the self-excluding digest, the object closure's content hashes, and the
    travel flags, none of which need a real generation behind them.
    """
    payload = TRANSFER_PAYLOAD
    digest = sha256(payload)
    state_root = sha256(b"transferred-state-root")
    # `<32s32sIII4x`: binding, state root, schema version, policy, flags.
    states = TRANSFER_STATE.pack(
        binding_identity(TRANSFER_STATE_BINDING),
        state_root,
        1,
        0,
        STATE_FLAG_TRAVEL | STATE_FLAG_READ_ONLY,
    )
    release = bytes(RELEASE_BYTES)
    metadata = b"sel4-transfer-fixture"
    object_offset = TRANSFER_HEADER_BYTES
    state_offset = object_offset + TRANSFER_OBJECT.size
    release_offset = state_offset + len(states)
    metadata_offset = release_offset + len(release)
    payload_offset = metadata_offset + len(metadata)
    total = payload_offset + len(payload)
    # `<32sQQII8x`: digest, length, payload offset, kind, flags. The offset is
    # absolute within the manifest, and must be nonzero exactly when the
    # payload flag is set.
    objects = TRANSFER_OBJECT.pack(
        digest, len(payload), payload_offset, TRANSFER_OBJECT_TYPE, OBJECT_FLAG_PAYLOAD
    )
    # Field order is the generated layout's, read from
    # `TRANSFER_HEADER_*_OFFSET` rather than guessed: magic, version, header
    # size, required flags, generation, parent, source state root, authority
    # manifest, release sequence, generation length, reserved, object count,
    # state count, and then the six section offsets, the metadata length, the
    # payload offset, the total length, a reserved tail, and the digest.
    header = TRANSFER_HEADER.pack(
        TRANSFER_MAGIC,
        TRANSFER_VERSION,
        TRANSFER_HEADER_BYTES,
        0,
        TRANSFER_GENERATION,
        bytes(32),
        state_root,
        TRANSFER_GENERATION_ROOT,
        TRANSFER_RELEASE_SEQUENCE,
        0,
        0,
        1,
        1,
        object_offset,
        state_offset,
        release_offset,
        metadata_offset,
        len(metadata),
        payload_offset,
        total,
        0,
        bytes(32),
    )
    body = bytearray(header + objects + states + release + metadata + payload)
    # The digest excludes its own field, which is why a tampered byte anywhere
    # else is caught: hash the record with that window zeroed, then write it in.
    hasher = hashlib.sha256()
    hasher.update(bytes(body[:TRANSFER_HEADER_HASH_OFFSET]))
    hasher.update(bytes(32))
    hasher.update(bytes(body[TRANSFER_HEADER_HASH_END:]))
    body[TRANSFER_HEADER_HASH_OFFSET:TRANSFER_HEADER_HASH_END] = hasher.digest()
    return bytes(body)


def build(variant: str) -> bytearray:
    image = bytearray(CAPACITY * SECTOR)

    # Protective MBR: one 0xEE entry spanning the disk plus the signature.
    struct.pack_into("<B", image, 446 + 4, 0xEE)
    struct.pack_into("<I", image, 446 + 8, 1)
    struct.pack_into("<I", image, 446 + 12, min(CAPACITY - 1, 0xFFFFFFFF))
    struct.pack_into("<H", image, 510, 0xAA55)

    entries = gpt_entries()
    entries_crc = zlib.crc32(entries)
    primary = gpt_header(1, BACKUP_HEADER_LBA, PRIMARY_ENTRIES_LBA, entries_crc, DISK_GUID)
    backup_guid = DISK_GUID if variant != "gpt-conflict" else b"SLIMEOSOTHERGUID"
    backup = gpt_header(BACKUP_HEADER_LBA, 1, BACKUP_ENTRIES_LBA, entries_crc, backup_guid)

    if variant == "gpt-primary-damaged":
        primary = bytes([primary[0] ^ 0xFF]) + primary[1:]

    place(image, 1, primary)
    place(image, PRIMARY_ENTRIES_LBA, entries)
    place(image, BACKUP_ENTRIES_LBA, entries)
    place(image, BACKUP_HEADER_LBA, backup)

    # Object store genesis: slot B sequence 1 (empty), slot A sequence 2 with
    # the seeded object committed; the record lives at record area start.
    seeded = seeded_payload()
    place(image, STORE_FIRST + 0, superblock(2, RECORD_AREA_START + SEEDED_RECORD_SECTORS, 1))
    place(image, STORE_FIRST + 1, superblock(1, RECORD_AREA_START, 0))
    place(image, STORE_FIRST + RECORD_AREA_START, record(SEEDED_TYPE, seeded))

    if variant == "superblock-newest-damaged":
        damaged = bytearray(superblock(2, RECORD_AREA_START + SEEDED_RECORD_SECTORS, 1))
        damaged[60] ^= 0xFF
        place(image, STORE_FIRST + 0, bytes(damaged))
    elif variant == "superblock-both-damaged":
        for slot in (0, 1):
            sector = bytearray(image[(STORE_FIRST + slot) * SECTOR : (STORE_FIRST + slot + 1) * SECTOR])
            sector[60] ^= 0xFF
            place(image, STORE_FIRST + slot, bytes(sector))
    elif variant == "interrupted-append":
        # A partial, uncommitted record at the append offset: valid magic but
        # truncated garbage. The committed append_lba still excludes it.
        garbage = bytearray(SECTOR)
        struct.pack_into("<8s", garbage, 0, RECORD_MAGIC)
        struct.pack_into("<I", garbage, 8, FORMAT_VERSION)
        struct.pack_into("<I", garbage, 24, 0xFFFF_FFFF)
        place(image, STORE_FIRST + RECORD_AREA_START + SEEDED_RECORD_SECTORS, bytes(garbage))

    if variant == "recovery":
        # Both BootState slots corrupt: not merely absent, but present-and-bad,
        # so selection must refuse rather than treat the region as empty.
        for slot in (STATE_SLOT_A, STATE_SLOT_B):
            corrupt = bytearray(SECTOR)
            corrupt[:8] = b"SLIMEBS\0"
            corrupt[8:12] = struct.pack("<I", 1)
            corrupt[64:96] = bytes([0xEE]) * 32
            place(image, STORE_FIRST + slot, bytes(corrupt))
        index = recovery_index(sha256(seeded))
        if len(index) > RECOVERY_INDEX_SECTORS * SECTOR:
            raise SystemExit("recovery index exceeds its reserved sectors")
        padded = index + bytes(RECOVERY_INDEX_SECTORS * SECTOR - len(index))
        place(image, STORE_FIRST + RECOVERY_INDEX_LBA, padded)

    if variant == "transfer":
        manifest = transfer_manifest()
        if len(manifest) > TRANSFER_MANIFEST_SECTORS * SECTOR:
            raise SystemExit("transfer manifest exceeds its reserved sectors")
        padded = manifest + bytes(TRANSFER_MANIFEST_SECTORS * SECTOR - len(manifest))
        place(image, STORE_FIRST + TRANSFER_MANIFEST_LBA, padded)

    return image


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("image", type=Path)
    parser.add_argument("variant", choices=VARIANTS)
    arguments = parser.parse_args()
    image = build(arguments.variant)
    arguments.image.write_bytes(image)
    print(
        f"Built {arguments.image} variant={arguments.variant} "
        f"({len(image)} bytes, seeded sha256:{hashlib.sha256(seeded_payload()).hexdigest()})"
    )


if __name__ == "__main__":
    main()
