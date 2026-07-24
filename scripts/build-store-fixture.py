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

import argparse
import hashlib
import struct
import zlib
from pathlib import Path

SECTOR = 512
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

HEADER_SIZE = STORE_RECORD.size

MESSAGE = b"Slime OS M5.4 object store fixture\n"

VARIANTS = [
    "happy",
    "gpt-primary-damaged",
    "gpt-conflict",
    "superblock-newest-damaged",
    "superblock-both-damaged",
    "interrupted-append",
]


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
