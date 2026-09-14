"""Deterministic GPT/FAT32 media for the Framework x86-64 boot path."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import struct
import subprocess
import zlib
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn

import boot_media_contract as contract

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_EFI_PATHS = (
    "EFI/BOOT/BOOTX64.EFI",
    "boot/grub/grub.cfg",
    "slime/kernel.elf",
    "slime/slime-root.elf",
)
FRAMEWORK_TARGET_PROFILE = "x86_64-sel4-framework13-ai300"
ESP_ATTRIBUTES = 1 << 60  # read-only, until a storage milestone owns writable media


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _tool(name: str, fail: Callable[[str], NoReturn]) -> str:
    path = shutil.which(name)
    if path is None:
        fail(f"{name} is not on PATH; enter `nix develop`")
    return path


def _guid_bytes(text: str, label: str, fail: Callable[[str], NoReturn]) -> bytes:
    try:
        value = bytes.fromhex(text)
    except ValueError as error:
        fail(f"boot-media contract {label} is not hexadecimal: {error}")
    if len(value) != 16:
        fail(f"boot-media contract {label} must encode 16 bytes")
    return value


def _partition_name(fail: Callable[[str], NoReturn]) -> bytes:
    encoded = contract.PARTITION_NAME.encode("utf-16-le")
    if len(encoded) > 72:
        fail("boot-media partition name exceeds GPT's 36-code-unit field")
    return encoded.ljust(72, b"\0")


def _protective_mbr() -> bytes:
    sector = bytearray(contract.SECTOR_BYTES)
    struct.pack_into("<B", sector, 446 + 4, 0xEE)
    struct.pack_into("<I", sector, 446 + 8, 1)
    struct.pack_into("<I", sector, 446 + 12, min(contract.DISK_SECTORS - 1, 0xFFFFFFFF))
    struct.pack_into("<H", sector, 510, 0xAA55)
    return bytes(sector)


def _gpt_entries(fail: Callable[[str], NoReturn]) -> bytes:
    table = bytearray(contract.PARTITION_ENTRIES * contract.PARTITION_ENTRY_BYTES)
    struct.pack_into(
        "<16s16sQQQ72s",
        table,
        0,
        _guid_bytes(contract.ESP_TYPE_GUID_HEX, "ESP type GUID", fail),
        _guid_bytes(contract.ESP_GUID_HEX, "ESP GUID", fail),
        contract.ESP_FIRST_LBA,
        contract.ESP_LAST_LBA,
        ESP_ATTRIBUTES,
        _partition_name(fail),
    )
    return bytes(table)


def _gpt_header(
    *,
    current_lba: int,
    backup_lba: int,
    entries_lba: int,
    entries_crc: int,
    fail: Callable[[str], NoReturn],
) -> bytes:
    header = bytearray(contract.SECTOR_BYTES)
    struct.pack_into("<8sII", header, 0, b"EFI PART", 0x00010000, 92)
    struct.pack_into("<QQQQ", header, 24, current_lba, backup_lba, 34, contract.BACKUP_ENTRIES_LBA - 1)
    struct.pack_into("<16sQIII", header, 56, _guid_bytes(contract.DISK_GUID_HEX, "disk GUID", fail), entries_lba, contract.PARTITION_ENTRIES, contract.PARTITION_ENTRY_BYTES, entries_crc)
    struct.pack_into("<I", header, 16, zlib.crc32(bytes(header[:92])))
    return bytes(header)


def _write_at(image: Path, lba: int, data: bytes) -> None:
    with image.open("r+b") as handle:
        handle.seek(lba * contract.SECTOR_BYTES)
        handle.write(data)


def _run(command: list[str], environment: dict[str, str], fail: Callable[[str], NoReturn]) -> bytes:
    try:
        process = subprocess.run(
            command,
            cwd=ROOT,
            env=environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        fail(f"cannot run {command[0]}: {error}")
    if process.returncode != 0:
        fail(
            f"{' '.join(command)} failed with exit status {process.returncode}:\n"
            + process.stdout.decode(errors="replace")
            + process.stderr.decode(errors="replace")
        )
    return process.stdout


def _mtools_environment() -> dict[str, str]:
    environment = dict(os.environ)
    environment["SOURCE_DATE_EPOCH"] = str(contract.SOURCE_DATE_EPOCH)
    environment["TZ"] = "UTC"
    return environment


def _copy_tree_to_esp(image: Path, tree: Path, fail: Callable[[str], NoReturn]) -> None:
    offset = contract.ESP_FIRST_LBA * contract.SECTOR_BYTES
    image_spec = f"{image}@@{offset}"
    environment = _mtools_environment()
    _run(
        [
            _tool("mformat", fail),
            "-i",
            image_spec,
            "-F",
            "-h",
            str(contract.FAT_HEADS),
            "-s",
            str(contract.FAT_TRACK_SECTORS),
            "-T",
            str(contract.FAT_SECTORS),
            "-H",
            str(contract.ESP_FIRST_LBA),
            "-N",
            hex(contract.FAT_VOLUME_ID),
            "-v",
            contract.FAT_VOLUME_LABEL,
            "::",
        ],
        environment,
        fail,
    )
    for child in sorted(tree.iterdir(), key=lambda path: path.name):
        _run(
            [_tool("mcopy", fail), "-i", image_spec, "-s", str(child), "::/"],
            environment,
            fail,
        )


def _normalized_json(value: dict[str, object]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def record_identity(record: dict[str, object]) -> str:
    material = dict(record)
    material.pop("identity", None)
    return hashlib.sha256(contract.IDENTITY_DOMAIN + _normalized_json(material)).hexdigest()


def _write_record(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def build_image(
    image: Path,
    record_path: Path,
    *,
    tree: Path,
    tree_record: dict[str, object],
    source_manifest: Path,
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    if not tree.is_dir():
        fail(f"missing EFI tree {tree}")
    if not source_manifest.is_file():
        fail(f"missing source identity manifest {source_manifest}")
    if contract.ESP_LAST_LBA - contract.ESP_FIRST_LBA + 1 != contract.FAT_SECTORS:
        fail("boot-media contract ESP and FAT sizes disagree")
    if contract.BACKUP_HEADER_LBA + 1 != contract.DISK_SECTORS:
        fail("boot-media contract backup header is not the final sector")
    if contract.BACKUP_ENTRIES_LBA + (
        contract.PARTITION_ENTRIES * contract.PARTITION_ENTRY_BYTES // contract.SECTOR_BYTES
    ) != contract.BACKUP_HEADER_LBA:
        fail("boot-media contract backup entry table does not precede the backup header")

    image.parent.mkdir(parents=True, exist_ok=True)
    image.write_bytes(b"")
    with image.open("wb") as handle:
        handle.truncate(contract.DISK_SECTORS * contract.SECTOR_BYTES)
    entries = _gpt_entries(fail)
    entries_crc = zlib.crc32(entries)
    _write_at(image, 0, _protective_mbr())
    _write_at(
        image,
        contract.PRIMARY_HEADER_LBA,
        _gpt_header(
            current_lba=contract.PRIMARY_HEADER_LBA,
            backup_lba=contract.BACKUP_HEADER_LBA,
            entries_lba=contract.PRIMARY_ENTRIES_LBA,
            entries_crc=entries_crc,
            fail=fail,
        ),
    )
    _write_at(image, contract.PRIMARY_ENTRIES_LBA, entries)
    _write_at(image, contract.BACKUP_ENTRIES_LBA, entries)
    _write_at(
        image,
        contract.BACKUP_HEADER_LBA,
        _gpt_header(
            current_lba=contract.BACKUP_HEADER_LBA,
            backup_lba=contract.PRIMARY_HEADER_LBA,
            entries_lba=contract.BACKUP_ENTRIES_LBA,
            entries_crc=entries_crc,
            fail=fail,
        ),
    )
    _copy_tree_to_esp(image, tree, fail)

    record: dict[str, object] = {
        "formatVersion": contract.FORMAT_VERSION,
        "kind": contract.KIND,
        "targetProfile": FRAMEWORK_TARGET_PROFILE,
        "image": {
            "path": image.relative_to(ROOT).as_posix(),
            "bytes": image.stat().st_size,
            "sha256": sha256_file(image),
        },
        "sourceManifest": {
            "path": source_manifest.relative_to(ROOT).as_posix(),
            "sha256": sha256_file(source_manifest),
        },
        "efiTree": {
            "sha256": tree_record["tree_sha256"],
            "files": [
                {"path": path, **identity}
                for path, identity in sorted(tree_record["files"].items())
            ],
        },
        "disk": {
            "sectorBytes": contract.SECTOR_BYTES,
            "sectors": contract.DISK_SECTORS,
            "diskGuid": contract.DISK_GUID_HEX,
            "partitionCount": 1,
            "partitions": [
                {
                    "typeGuid": contract.ESP_TYPE_GUID_HEX,
                    "uniqueGuid": contract.ESP_GUID_HEX,
                    "firstLba": contract.ESP_FIRST_LBA,
                    "lastLba": contract.ESP_LAST_LBA,
                    "attributes": ESP_ATTRIBUTES,
                    "name": contract.PARTITION_NAME,
                    "filesystem": "fat32",
                    "volumeId": contract.FAT_VOLUME_ID,
                    "volumeLabel": contract.FAT_VOLUME_LABEL,
                    "writableProductState": False,
                }
            ],
        },
    }
    record["identity"] = record_identity(record)
    _write_record(record_path, record)
    return record


def load_record(path: Path, fail: Callable[[str], NoReturn]) -> dict[str, object]:
    if not path.is_file():
        fail(f"missing boot-media identity record {path}")
    if path.stat().st_size > contract.MAX_RECORD_BYTES:
        fail(f"boot-media identity record exceeds {contract.MAX_RECORD_BYTES} bytes")
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse boot-media identity record {path}: {error}")
    if not isinstance(record, dict):
        fail("boot-media identity record must be an object")
    if record.get("formatVersion") != contract.FORMAT_VERSION or record.get("kind") != contract.KIND:
        fail("unsupported boot-media identity record")
    if record.get("targetProfile") != FRAMEWORK_TARGET_PROFILE:
        fail(f"boot-media target profile is {record.get('targetProfile')!r}, not {FRAMEWORK_TARGET_PROFILE!r}")
    identity = record.get("identity")
    if not isinstance(identity, str) or identity != record_identity(record):
        fail("boot-media identity digest does not match the record")
    _validate_record_shape(record, fail)
    return record

def _require_digest(value: object, label: str, fail: Callable[[str], NoReturn]) -> str:
    if not isinstance(value, str) or len(value) != contract.MAX_DIGEST_BYTES:
        fail(f"boot-media identity {label} must be a SHA-256 digest")
    try:
        bytes.fromhex(value)
    except ValueError:
        fail(f"boot-media identity {label} is not hexadecimal")
    return value


def _validate_record_shape(record: dict[str, object], fail: Callable[[str], NoReturn]) -> None:
    allowed = {
        "disk",
        "efiTree",
        "formatVersion",
        "identity",
        "image",
        "kind",
        "sourceManifest",
        "targetProfile",
    }
    if set(record) != allowed:
        fail("boot-media identity has missing or unknown top-level fields")
    image = record.get("image")
    if not isinstance(image, dict) or set(image) != {"path", "bytes", "sha256"}:
        fail("boot-media identity has an invalid image record")
    if not isinstance(image.get("path"), str) or len(image["path"].encode()) > contract.MAX_PATH_BYTES:
        fail("boot-media identity image path is invalid")
    if image.get("bytes") != contract.DISK_SECTORS * contract.SECTOR_BYTES:
        fail("boot-media identity image size is not canonical")
    _require_digest(image.get("sha256"), "image.sha256", fail)

    source = record.get("sourceManifest")
    if not isinstance(source, dict) or set(source) != {"path", "sha256"}:
        fail("boot-media identity has an invalid source manifest record")
    if not isinstance(source.get("path"), str) or len(source["path"].encode()) > contract.MAX_PATH_BYTES:
        fail("boot-media identity source manifest path is invalid")
    _require_digest(source.get("sha256"), "sourceManifest.sha256", fail)

    disk = record.get("disk")
    expected_partition = {
        "typeGuid": contract.ESP_TYPE_GUID_HEX,
        "uniqueGuid": contract.ESP_GUID_HEX,
        "firstLba": contract.ESP_FIRST_LBA,
        "lastLba": contract.ESP_LAST_LBA,
        "attributes": ESP_ATTRIBUTES,
        "name": contract.PARTITION_NAME,
        "filesystem": "fat32",
        "volumeId": contract.FAT_VOLUME_ID,
        "volumeLabel": contract.FAT_VOLUME_LABEL,
        "writableProductState": False,
    }
    expected_disk = {
        "sectorBytes": contract.SECTOR_BYTES,
        "sectors": contract.DISK_SECTORS,
        "diskGuid": contract.DISK_GUID_HEX,
        "partitionCount": 1,
        "partitions": [expected_partition],
    }
    if disk != expected_disk:
        fail("boot-media identity disk layout is not canonical")

    tree = record.get("efiTree")
    if not isinstance(tree, dict) or set(tree) != {"sha256", "files"}:
        fail("boot-media identity has an invalid EFI tree record")
    _require_digest(tree.get("sha256"), "efiTree.sha256", fail)
    files = tree.get("files")
    if not isinstance(files, list) or len(files) != len(REQUIRED_EFI_PATHS):
        fail("boot-media identity must name exactly the required EFI files")
    observed_paths = []
    for entry in files:
        if not isinstance(entry, dict) or set(entry) != {"path", "bytes", "sha256"}:
            fail("boot-media identity contains an invalid EFI file record")
        path = entry.get("path")
        if not isinstance(path, str) or len(path.encode()) > contract.MAX_PATH_BYTES:
            fail("boot-media identity contains an invalid EFI path")
        if not isinstance(entry.get("bytes"), int) or isinstance(entry.get("bytes"), bool) or entry["bytes"] < 0:
            fail(f"boot-media identity has an invalid byte count for {path}")
        _require_digest(entry.get("sha256"), f"efiTree.files[{path}].sha256", fail)
        observed_paths.append(path)
    if tuple(observed_paths) != tuple(sorted(REQUIRED_EFI_PATHS)):
        fail("boot-media identity EFI file set is not canonical")


def _read_exact(handle, offset: int, length: int, label: str, fail: Callable[[str], NoReturn]) -> bytes:
    handle.seek(offset)
    data = handle.read(length)
    if len(data) != length:
        fail(f"short read for {label}: got {len(data)} of {length} bytes")
    return data


def _parse_header(data: bytes, expected_lba: int, fail: Callable[[str], NoReturn]) -> dict[str, int | bytes]:
    if data[:8] != b"EFI PART":
        fail(f"GPT header at LBA {expected_lba} has bad magic")
    revision, size, stored_crc = struct.unpack_from("<III", data, 8)
    if revision != 0x00010000 or size != 92:
        fail(f"GPT header at LBA {expected_lba} has unsupported revision or size")
    checked = bytearray(data[:size])
    struct.pack_into("<I", checked, 16, 0)
    if zlib.crc32(checked) != stored_crc:
        fail(f"GPT header at LBA {expected_lba} has a bad CRC")
    current, backup, first_usable, last_usable = struct.unpack_from("<QQQQ", data, 24)
    disk_guid, entries_lba, count, entry_size, entries_crc = struct.unpack_from("<16sQIII", data, 56)
    if current != expected_lba:
        fail(f"GPT header at LBA {expected_lba} names current LBA {current}")
    return {
        "backup": backup,
        "first_usable": first_usable,
        "last_usable": last_usable,
        "disk_guid": disk_guid,
        "entries_lba": entries_lba,
        "count": count,
        "entry_size": entry_size,
        "entries_crc": entries_crc,
    }


def _fat_file(image: Path, relative: str, fail: Callable[[str], NoReturn]) -> bytes:
    offset = contract.ESP_FIRST_LBA * contract.SECTOR_BYTES
    return _run(
        [_tool("mtype", fail), "-i", f"{image}@@{offset}", f"::/{relative}"],
        _mtools_environment(),
        fail,
    )


def validate_image(
    image: Path,
    record_path: Path,
    *,
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    record = load_record(record_path, fail)
    if not image.is_file():
        fail(f"missing boot-media image {image}")
    expected_image = record.get("image")
    if not isinstance(expected_image, dict):
        fail("boot-media identity records no image")
    if expected_image.get("path") != image.relative_to(ROOT).as_posix():
        fail("boot-media identity names a different image path")
    if expected_image.get("bytes") != image.stat().st_size or expected_image.get("sha256") != sha256_file(image):
        fail("boot-media image bytes do not match the identity record")
    if image.stat().st_size != contract.DISK_SECTORS * contract.SECTOR_BYTES:
        fail("boot-media image size does not match the contract")

    with image.open("rb") as handle:
        mbr = _read_exact(handle, 0, contract.SECTOR_BYTES, "protective MBR", fail)
        if mbr[510:512] != b"\x55\xaa" or mbr[446 + 4] != 0xEE:
            fail("boot-media image has no valid protective MBR")
        primary = _parse_header(
            _read_exact(handle, contract.PRIMARY_HEADER_LBA * contract.SECTOR_BYTES, contract.SECTOR_BYTES, "primary GPT header", fail),
            contract.PRIMARY_HEADER_LBA,
            fail,
        )
        backup = _parse_header(
            _read_exact(handle, contract.BACKUP_HEADER_LBA * contract.SECTOR_BYTES, contract.SECTOR_BYTES, "backup GPT header", fail),
            contract.BACKUP_HEADER_LBA,
            fail,
        )
        expected_guid = _guid_bytes(contract.DISK_GUID_HEX, "disk GUID", fail)
        if primary["disk_guid"] != expected_guid or backup["disk_guid"] != expected_guid:
            fail("GPT disk GUID does not match the contract")
        if primary["backup"] != contract.BACKUP_HEADER_LBA or backup["backup"] != contract.PRIMARY_HEADER_LBA:
            fail("GPT headers do not point at each other")
        table_bytes = contract.PARTITION_ENTRIES * contract.PARTITION_ENTRY_BYTES
        primary_entries = _read_exact(handle, int(primary["entries_lba"]) * contract.SECTOR_BYTES, table_bytes, "primary GPT entries", fail)
        backup_entries = _read_exact(handle, int(backup["entries_lba"]) * contract.SECTOR_BYTES, table_bytes, "backup GPT entries", fail)
    if primary_entries != backup_entries:
        fail("primary and backup GPT entry tables differ")
    if zlib.crc32(primary_entries) != primary["entries_crc"] or zlib.crc32(backup_entries) != backup["entries_crc"]:
        fail("GPT entry-table CRC does not match")

    entries = []
    for index in range(contract.PARTITION_ENTRIES):
        offset = index * contract.PARTITION_ENTRY_BYTES
        raw = primary_entries[offset : offset + contract.PARTITION_ENTRY_BYTES]
        if raw[:16] == b"\0" * 16:
            continue
        type_guid, unique_guid, first_lba, last_lba, attributes, raw_name = struct.unpack_from("<16s16sQQQ72s", raw)
        entries.append((type_guid.hex(), unique_guid.hex(), first_lba, last_lba, attributes, raw_name.decode("utf-16-le").rstrip("\0")))
    expected_entry = (
        contract.ESP_TYPE_GUID_HEX,
        contract.ESP_GUID_HEX,
        contract.ESP_FIRST_LBA,
        contract.ESP_LAST_LBA,
        ESP_ATTRIBUTES,
        contract.PARTITION_NAME,
    )
    if entries != [expected_entry]:
        fail(f"boot-media partition table is {entries!r}, expected one read-only ESP")

    efi_tree = record["efiTree"]
    files = efi_tree["files"]
    digest = hashlib.sha256()
    for expected in files:
        relative = expected["path"]
        data = _fat_file(image, relative, fail)
        observed = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        if observed != {"bytes": expected["bytes"], "sha256": expected["sha256"]}:
            fail(f"ESP file {relative} does not match the identity record")
        digest.update(relative.encode("utf-8"))
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
    if digest.hexdigest() != efi_tree["sha256"]:
        fail("ESP tree digest does not match the identity record")
    return record


def qemu_disk_arguments(image: Path) -> list[str]:
    return ["-drive", f"format=raw,readonly=on,file={image}"]
