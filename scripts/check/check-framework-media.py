#!/usr/bin/env python3

"""P6.5: deterministic Framework-qualified GPT/FAT32 removable media."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import boot_media_contract as contract  # noqa: E402
from framework_media import (  # noqa: E402
    FRAMEWORK_TARGET_PROFILE,
    load_record,
    validate_image,
)
from harness import ROOT, load_qemu_profile  # noqa: E402
from pc99_media import boot_media, qemu_command  # noqa: E402
from sel4_boot import report_transcript, run as run_boot  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-framework-media.py"
WRITER = ROOT / "scripts" / "build" / "write-removable-image.py"
IMAGE = ROOT / "build" / "framework-media" / contract.IMAGE_FILE_NAME
RECORD = ROOT / "build" / "framework-media" / contract.RECORD_FILE_NAME
TREE = ROOT / "build" / "media" / "framework13-ai300-graph"
SOURCE_MANIFEST = ROOT / "build" / "slime-sel4-graph-framework13-ai300.identity.json"
PINS = ROOT / "sel4" / "pins.toml"
BOOT_TIMEOUT_SECONDS = 120
TERMINAL = re.compile(r"slisp> ")
FAILURES = re.compile(
    r"SLIME_ROOT FATAL|SLIME_GRAPH FAIL|SLIME_GRAPH component exit .*status=-?[1-9]\d*|panicked at|aborted at"
)


def fail(message: str) -> None:
    raise SystemExit(f"Framework media check: {message}")


def run_builder(*arguments: str) -> None:
    command = [sys.executable, str(BUILDER), *arguments]
    print(f"[build] {' '.join(command)}", flush=True)
    process = subprocess.run(command, cwd=ROOT, check=False)
    if process.returncode != 0:
        fail(f"media builder failed with exit status {process.returncode}")


def boot_image() -> str:
    profile = load_qemu_profile(fail, PINS, "qemu_pc99")
    command = qemu_command(
        tree=TREE,
        profile=profile,
        fail=fail,
        vars_copy=ROOT / "build" / "framework-media" / ".qemu-vars.fd",
        raw_image=IMAGE,
    )
    transcript = run_boot(
        command,
        terminal=re.compile(TERMINAL.pattern + "|" + FAILURES.pattern),
        timeout=BOOT_TIMEOUT_SECONDS,
        fail=fail,
    )
    if FAILURES.search(transcript):
        report_transcript(transcript)
        fail(f"failure marker in raw-media boot: {FAILURES.search(transcript).group(0)!r}")
    if TERMINAL.search(transcript) is None:
        report_transcript(transcript)
        fail("raw-media boot did not reach the resident product graph")
    required = (
        r"SLIME_ROOT generation admitted number=1 executables=6 instances=6 grants=\d+",
        r"\[init\] product services resident",
        r"Slisp",
        r"slisp> ",
    )
    for pattern in required:
        if re.compile(pattern).search(transcript) is None:
            report_transcript(transcript)
            fail(f"raw-media boot is missing {pattern!r}")
    ready = re.search(r"SLIME_ROOT READY target_profile=([^\r\n]+)", transcript)
    if ready is None or ready.group(1).strip() != FRAMEWORK_TARGET_PROFILE:
        report_transcript(transcript)
        fail("raw-media boot did not report the Framework target profile")
    if "[slisp] resident input wait" in transcript:
        fail("Framework-qualified image unexpectedly compiled an input authority")
    return transcript

def expect_rejected(label: str, image: Path, record: Path) -> None:
    try:
        validate_image(image, record, fail=fail)
    except SystemExit:
        print(f"Framework media negative: {label} refused")
        return
    fail(f"negative control passed: {label}")


def mutate_image(source: Path, root: Path, label: str, mutate) -> tuple[Path, Path]:
    image = root / f"{label}.img"
    record = root / f"{label}.json"
    shutil.copyfile(source, image)
    data = bytearray(image.read_bytes())
    mutate(data)
    image.write_bytes(data)
    source_record = load_record(RECORD, fail)
    changed = copy.deepcopy(source_record)
    changed["image"]["path"] = image.relative_to(ROOT).as_posix() if image.is_relative_to(ROOT) else str(image)
    changed["image"]["bytes"] = len(data)
    changed["image"]["sha256"] = hashlib.sha256(data).hexdigest()
    from framework_media import record_identity

    changed["identity"] = record_identity(changed)
    record.write_text(json.dumps(changed, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return image, record


def negative_controls() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-framework-media-negative-", dir=ROOT / "build") as temporary:
        root = Path(temporary)
        missing = root / "missing.img"
        record_copy = root / "record.json"
        shutil.copyfile(RECORD, record_copy)
        expect_rejected("missing image", missing, record_copy)

        drift = root / "drift.img"
        drift_record = root / "drift.json"
        shutil.copyfile(IMAGE, drift)
        shutil.copyfile(RECORD, drift_record)
        with drift.open("r+b") as handle:
            handle.seek(contract.ESP_FIRST_LBA * contract.SECTOR_BYTES + 4096)
            byte = handle.read(1)
            handle.seek(-1, os.SEEK_CUR)
            handle.write(bytes([byte[0] ^ 0xFF]))
        expect_rejected("digest drift", drift, drift_record)

        wrong = load_record(RECORD, fail)
        wrong = copy.deepcopy(wrong)
        wrong["targetProfile"] = "x86_64-sel4-qemu-pc99"
        from framework_media import record_identity

        wrong["identity"] = record_identity(wrong)
        wrong_path = root / "wrong-target.json"
        wrong_path.write_text(json.dumps(wrong, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        expect_rejected("wrong target", IMAGE, wrong_path)

        malformed_image, malformed_record = mutate_image(
            IMAGE,
            root,
            "bad-gpt",
            lambda data: data.__setitem__(contract.PRIMARY_HEADER_LBA * contract.SECTOR_BYTES, 0),
        )
        expect_rejected("malformed GPT", malformed_image, malformed_record)

        def add_partition(data: bytearray) -> None:
            entries_offset = contract.PRIMARY_ENTRIES_LBA * contract.SECTOR_BYTES
            backup_offset = contract.BACKUP_ENTRIES_LBA * contract.SECTOR_BYTES
            entry = bytearray(contract.PARTITION_ENTRY_BYTES)
            struct.pack_into(
                "<16s16sQQQ72s",
                entry,
                0,
                bytes.fromhex(contract.ESP_TYPE_GUID_HEX),
                bytes.fromhex("534c494d454f53455854524130303031"),
                100,
                101,
                0,
                "extra".encode("utf-16-le").ljust(72, b"\0"),
            )
            second = contract.PARTITION_ENTRY_BYTES
            data[entries_offset + second : entries_offset + second * 2] = entry
            data[backup_offset + second : backup_offset + second * 2] = entry
            table_size = contract.PARTITION_ENTRIES * contract.PARTITION_ENTRY_BYTES
            crc = zlib.crc32(data[entries_offset : entries_offset + table_size])
            for header_lba in (contract.PRIMARY_HEADER_LBA, contract.BACKUP_HEADER_LBA):
                offset = header_lba * contract.SECTOR_BYTES
                struct.pack_into("<I", data, offset + 88, crc)
                struct.pack_into("<I", data, offset + 16, 0)
                struct.pack_into("<I", data, offset + 16, zlib.crc32(data[offset : offset + 92]))

        extra_image, extra_record = mutate_image(IMAGE, root, "extra-partition", add_partition)
        expect_rejected("unexpected partition", extra_image, extra_record)

        tree_copy = root / "tree"
        shutil.copytree(TREE, tree_copy)
        (tree_copy / "slime" / "kernel.elf").unlink()
        try:
            boot_media(tree_copy, profile=load_qemu_profile(fail, PINS, "qemu_pc99"), fail=fail)
        except SystemExit:
            print("Framework media negative: missing EFI file refused")
        else:
            fail("negative control passed: missing EFI file")

        (tree_copy / "slime" / "kernel.elf").write_bytes(b"wrong")
        (tree_copy / "unexpected").write_bytes(b"extra")
        try:
            boot_media(tree_copy, profile=load_qemu_profile(fail, PINS, "qemu_pc99"), fail=fail)
        except SystemExit:
            print("Framework media negative: malformed FAT tree refused")
        else:
            fail("negative control passed: malformed FAT tree")


def check_reproducible() -> None:
    first_image = IMAGE.read_bytes()
    first_record = RECORD.read_bytes()
    run_builder("--no-build")
    if IMAGE.read_bytes() != first_image or RECORD.read_bytes() != first_record:
        fail("two builds of the same EFI tree are not byte-identical")
    print("Framework media check: two raw GPT/FAT32 builds are byte-identical")


def check_writer_contract() -> None:
    spec = importlib.util.spec_from_file_location("slime_removable_writer", WRITER)
    if spec is None or spec.loader is None:
        fail("cannot import removable writer")
    writer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(writer)

    with tempfile.TemporaryDirectory(prefix="slime-writer-check-", dir=ROOT / "build") as temporary:
        root = Path(temporary)
        copied = root / "copy.img"
        copied.write_bytes(b"\0" * IMAGE.stat().st_size)
        writer.write_image(IMAGE, copied)
        if writer.verify_written_image(IMAGE, copied) != hashlib.sha256(IMAGE.read_bytes()).hexdigest():
            fail("writer read-back verification returned the wrong digest")

        short = root / "short.img"
        short.write_bytes(b"\0" * 1024)
        try:
            writer.block_sha256(short, 2048)
        except SystemExit:
            print("Framework media writer negative: short read refused")
        else:
            fail("writer accepted a short read")

        partial = root / "partial.img"
        partial.write_bytes(b"\0" * IMAGE.stat().st_size)
        original_open = Path.open

        class ShortWriter:
            def __init__(self, handle):
                self.handle = handle
                self.calls = 0

            def __enter__(self):
                return self

            def __exit__(self, *args):
                self.handle.close()

            def write(self, data):
                self.calls += 1
                if self.calls == 2:
                    return 0
                return self.handle.write(data[: max(1, len(data) // 2)])

            def flush(self):
                return self.handle.flush()

            def fileno(self):
                return self.handle.fileno()

        def patched_open(path, mode="r", *args, **kwargs):
            handle = original_open(path, mode, *args, **kwargs)
            if Path(path) == partial and "r+b" in mode:
                return ShortWriter(handle)
            return handle

        Path.open = patched_open
        try:
            try:
                writer.write_image(IMAGE, partial)
            except SystemExit:
                print("Framework media writer negative: short write refused")
            else:
                fail("writer accepted a short write")
        finally:
            Path.open = original_open

        def refused(label: str, **changes) -> None:
            state = {
                "partition": False,
                "removable": "1",
                "ro": "0",
                "mounted": set(),
            }
            state.update(changes)
            sysfs = root / label
            sysfs.mkdir()
            (sysfs / "dev").write_text("8:0")
            (sysfs / "size").write_text(str(contract.DISK_SECTORS * 2))
            (sysfs / "removable").write_text(str(state["removable"]))
            (sysfs / "ro").write_text(str(state["ro"]))
            (sysfs / "holders").mkdir()
            if state["partition"]:
                (sysfs / "partition").write_text("1")
            original_sysfs = writer.sysfs_block
            original_mounts = writer.mounted_devices
            original_device = writer.Path
            writer.sysfs_block = lambda _name: sysfs
            writer.mounted_devices = lambda: set(state["mounted"])
            try:
                writer.assert_safe_disk(label, True, confirm=False)
            except SystemExit:
                print(f"Framework media writer negative: {label} refused")
            else:
                fail(f"writer accepted unsafe target: {label}")
            finally:
                writer.sysfs_block = original_sysfs
                writer.mounted_devices = original_mounts
                writer.Path = original_device

        refused("partition", partition=True)
        refused("non-removable", removable="0")
        refused("read-only", ro="1")
        refused("mounted", mounted={"8:0"})


def main() -> None:
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    run_builder()
    validate_image(IMAGE, RECORD, fail=fail)
    check_reproducible()
    negative_controls()
    check_writer_contract()
    boot_image()
    print(
        "Framework media check: one Framework-qualified identity-bound raw image "
        "contains exactly one read-only ESP, boots under pinned QEMU/OVMF to the "
        "resident product graph, and is accepted only by the guarded removable writer"
    )


if __name__ == "__main__":
    main()
