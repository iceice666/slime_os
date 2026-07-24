#!/usr/bin/env python3

from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import hashlib
import json
import os
import subprocess
import stat
from pathlib import Path

from harness import RELEASE_KERNEL, ROOT, run_qemu

KERNEL = ROOT / "kernel"
DEFAULT_IMAGE = Path("/tmp/slime-os-framework-inventory.img")
DEFAULT_EVIDENCE = ROOT / "evidence" / "framework-inventory.jsonl"
REPORT_BEGIN = "[hw-report] begin version=1"
REPORT_END = "[hw-report] end version=1"
MAX_REPORT_BYTES = 256 * 1024
HASH_BYTES = 16 * 1024 * 1024


def fail(message: str) -> None:
    raise SystemExit(message)


def sha256_file(path: Path, limit: int | None = None) -> str:
    digest = hashlib.sha256()
    remaining = limit
    with path.open("rb", buffering=0) as handle:
        while remaining is None or remaining > 0:
            size = 1024 * 1024 if remaining is None else min(1024 * 1024, remaining)
            chunk = handle.read(size)
            if not chunk:
                break
            digest.update(chunk)
            if remaining is not None:
                remaining -= len(chunk)
    if limit is not None and remaining:
        fail(f"short read hashing {path}: missing {remaining} bytes")
    return digest.hexdigest()


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def machine_identity() -> dict[str, str]:
    dmi = Path("/sys/class/dmi/id")
    fields = {
        "sys_vendor": "sys_vendor",
        "product_name": "product_name",
        "product_version": "product_version",
        "product_uuid": "product_uuid",
        "bios_vendor": "bios_vendor",
        "bios_version": "bios_version",
        "bios_date": "bios_date",
    }
    result: dict[str, str] = {}
    for name, filename in fields.items():
        path = dmi / filename
        if path.exists():
            result[name] = read_text(path)
    if not result:
        fail("machine identity unavailable under /sys/class/dmi/id")
    return result


def storage_hashes(devices: list[Path]) -> list[dict[str, str | int]]:
    hashes: list[dict[str, str | int]] = []
    for device in devices:
        try:
            resolved = device.resolve(strict=True)
            info = resolved.stat()
        except OSError as error:
            fail(f"cannot resolve storage device {device}: {error}")
        if resolved.parent != Path("/dev") or not stat.S_ISBLK(info.st_mode):
            fail(f"storage device must resolve to a block device under /dev: {device}")
        sysfs = Path("/sys/dev/block") / f"{os.major(info.st_rdev)}:{os.minor(info.st_rdev)}"
        try:
            sysfs = sysfs.resolve(strict=True)
        except OSError as error:
            fail(f"storage sysfs identity unavailable for {resolved}: {error}")
        model_path = sysfs / "device" / "model"
        serial_path = sysfs / "device" / "serial"
        hashes.append(
            {
                "device": str(resolved),
                "major_minor": f"{os.major(info.st_rdev)}:{os.minor(info.st_rdev)}",
                "model": read_text(model_path) if model_path.exists() else "",
                "serial": read_text(serial_path) if serial_path.exists() else "",
                "offset": 0,
                "length": HASH_BYTES,
                "sha256": sha256_file(resolved, HASH_BYTES),
            }
        )
    return hashes


def build_image(image: Path) -> str:
    environment = os.environ.copy()
    environment["SLIME_FRAMEWORK_INVENTORY"] = "1"
    subprocess.run(
        ["cargo", "build", "--release"],
        cwd=KERNEL,
        env=environment,
        check=True,
    )
    subprocess.run(
        [
            str(KERNEL / "scripts" / "build-iso.sh"),
            str(RELEASE_KERNEL),
            str(image),
            "128",
        ],
        cwd=ROOT,
        env=environment,
        check=True,
    )
    return sha256_file(image)


def qemu_report(fixture: Path) -> str:
    environment = os.environ.copy()
    environment["SLIME_FRAMEWORK_INVENTORY"] = "1"
    environment["SLIME_FRAMEWORK_INVENTORY_QEMU"] = "1"
    output = run_qemu(
        [
            "cargo", "run", "--release", "--", "-display", "none",
            "-drive", f"if=none,id=inventory-nvme,format=raw,readonly=on,file={fixture}",
            "-device", "nvme,serial=inventory-nvme,drive=inventory-nvme",
        ],
        environment=environment,
        cwd=KERNEL,
        timeout=120,
        echo="on-error",
    )
    return extract_report(output)


def extract_report(output: str) -> str:
    start = output.find(REPORT_BEGIN)
    end = output.find(REPORT_END, start)
    if start < 0 or end < 0:
        print(output, end="")
        fail("bounded hardware report framing is missing")
    end += len(REPORT_END)
    report = output[start:end]
    if len(report.encode()) > MAX_REPORT_BYTES:
        fail("hardware report exceeds bound")
    if "[hw-report] input path=" not in report or "[hw-report] input_stage" not in report:
        fail("hardware report lacks input path/stages")
    return report


def append_record(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n"
    with path.open("a", encoding="utf-8") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        handle.write(line)
        handle.flush()
        os.fsync(handle.fileno())
        fcntl.flock(handle, fcntl.LOCK_UN)


def normalized_report(report: str) -> str:
    return "\n".join(line.strip() for line in report.splitlines() if line.strip())

def report_generation(report: str) -> str:
    prefix = "[hw-report] generation="
    for line in report.splitlines():
        if line.startswith(prefix):
            return line[len(prefix) :]
    fail("hardware report lacks generation identity")




def prepare_pending(path: Path, image: Path, image_sha256: str, devices: list[Path]) -> None:
    if path.exists():
        fail(f"pending evidence already exists: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    pending = {
        "format": "slime-framework-inventory-pending",
        "version": 1,
        "image": {"path": str(image.resolve()), "sha256": image_sha256},
        "expected_generation": report_generation(normalized_report(qemu_report(Path("/tmp/slime-os-inventory-nvme.img")))),
        "machine": machine_identity(),
        "storage_before": storage_hashes(devices),
    }
    with path.open("x", encoding="utf-8") as handle:
        json.dump(pending, handle, sort_keys=True, separators=(",", ":"))
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())


def main() -> None:
    parser = argparse.ArgumentParser(description="Build, verify, and record a bounded Framework hardware inventory")
    parser.add_argument("--image", type=Path, default=DEFAULT_IMAGE)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--serial-log", type=Path, help="captured physical serial transcript")
    parser.add_argument("--storage-device", action="append", type=Path, default=[])
    parser.add_argument("--prepare", action="store_true", help="record pre-boot storage hashes")
    parser.add_argument("--record", action="store_true", help="append evidence after the physical boot")
    parser.add_argument("--pending", type=Path, default=Path("/tmp/slime-framework-inventory-pending.json"))
    args = parser.parse_args()

    if args.record:
        if not args.pending.is_file():
            fail(f"pending evidence not found: {args.pending}; run --prepare before boot")
        pending = json.loads(args.pending.read_text(encoding="utf-8"))
        image = Path(str(pending.get("image", {}).get("path", "")))
        if not image.is_file():
            fail(f"prepared Framework image not found: {image}")
        image_sha256 = sha256_file(image)
    else:
        image_sha256 = build_image(args.image)
        fixture = Path("/tmp/slime-os-inventory-nvme.img")
        subprocess.run([str(ROOT / "scripts" / "build-storage-fixture.py"), str(fixture)], check=True)
        first = normalized_report(qemu_report(fixture))
        second = normalized_report(qemu_report(fixture))
        if first != second:
            fail("two unchanged QEMU boots produced different normalized topology reports")

    if args.prepare:
        if args.record or not args.storage_device:
            fail("--prepare requires storage devices and cannot be combined with --record")
        prepare_pending(args.pending, args.image, image_sha256, args.storage_device)
        print(f"Prepared Framework inventory evidence: {args.pending}")
        return
    if not args.record:
        print("Framework inventory deterministic check: ok")
        return
    if args.serial_log is None or args.storage_device:
        fail("--record requires --serial-log and uses devices saved by --prepare")
    pending = json.loads(args.pending.read_text(encoding="utf-8"))
    if pending.get("format") != "slime-framework-inventory-pending" or pending.get("version") != 1:
        fail("unsupported pending evidence format")
    if pending.get("image", {}).get("sha256") != image_sha256:
        fail("Framework image changed since --prepare")
    before = pending.get("storage_before")
    if not isinstance(before, list) or not before:
        fail("pending evidence has no storage hashes")
    report = normalized_report(extract_report(args.serial_log.read_text(encoding="utf-8")))
    if report_generation(report) != pending.get("expected_generation"):
        fail("physical report generation does not match the prepared image")
    after = storage_hashes([Path(str(entry["device"])) for entry in before])
    for previous, current in zip(before, after, strict=True):
        if previous["sha256"] != current["sha256"]:
            fail(f"storage comparison region changed: {previous['device']}")
    record = {
        "format": "slime-framework-inventory-evidence",
        "version": 1,
        "recorded_utc": dt.datetime.now(dt.UTC).isoformat(),
        "image": pending["image"],
        "machine": pending["machine"],
        "report": report,
        "storage_before": before,
        "storage_after": after,
    }
    append_record(args.evidence, record)
    args.pending.unlink()
    print(f"Appended Framework inventory evidence: {args.evidence}")


if __name__ == "__main__":
    main()
