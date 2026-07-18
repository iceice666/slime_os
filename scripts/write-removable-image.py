#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import os
import stat
from pathlib import Path

CHUNK_SIZE = 1024 * 1024


def fail(message: str) -> None:
    raise SystemExit(message)


def major_minor(device: int) -> str:
    return f"{os.major(device)}:{os.minor(device)}"


def block_device_id(path: str) -> str | None:
    if not path.startswith("/dev/"):
        return None
    try:
        info = os.stat(path)
    except OSError:
        return None
    if not stat.S_ISBLK(info.st_mode):
        return None
    return major_minor(info.st_rdev)


def block_name(device: Path) -> str:
    try:
        real = device.resolve(strict=True)
    except FileNotFoundError:
        fail(f"device not found: {device}")
    if real.parent != Path("/dev"):
        fail(f"device must resolve under /dev: {device}")
    try:
        mode = real.stat().st_mode
    except OSError as error:
        fail(f"cannot stat device {real}: {error}")
    if not stat.S_ISBLK(mode):
        fail(f"not a block device: {real}")
    return real.name


def sysfs_block(name: str) -> Path:
    path = Path("/sys/class/block") / name
    if not path.exists():
        fail(f"missing sysfs block entry: {path}")
    return path


def read_sys(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def mounted_devices() -> set[str]:
    mounted: set[str] = set()
    with Path("/proc/self/mountinfo").open(encoding="utf-8") as handle:
        for line in handle:
            fields = line.split()
            if len(fields) < 3:
                continue
            mounted.add(fields[2])
            if "-" in fields:
                separator = fields.index("-")
                if len(fields) > separator + 2:
                    source = fields[separator + 2]
                    source_id = block_device_id(source)
                    if source_id is not None:
                        mounted.add(source_id)
    return mounted


def dev_id(path: Path) -> str:
    raw = read_sys(path / "dev")
    return raw


def block_ids_with_holders(path: Path, seen: set[Path] | None = None) -> set[str]:
    if seen is None:
        seen = set()
    try:
        resolved = path.resolve(strict=True)
    except OSError:
        return set()
    if resolved in seen:
        return set()
    seen.add(resolved)

    ids = {dev_id(resolved)} if (resolved / "dev").exists() else set()
    holders = resolved / "holders"
    if holders.exists():
        for holder in holders.iterdir():
            ids.update(block_ids_with_holders(holder, seen))
    return ids


def disk_summary(sysfs: Path) -> str:
    sectors = int(read_sys(sysfs / "size"))
    size = sectors * 512
    model_path = sysfs / "device" / "model"
    model = read_sys(model_path) if model_path.exists() else sysfs.name
    return f"/dev/{sysfs.name} ({model}, {size} bytes)"


def confirm_destructive_write(device: Path, summary: str, assume_yes: bool) -> None:
    print(f"Target removable disk: {summary}", flush=True)
    if assume_yes:
        return
    if not os.isatty(0):
        fail("destructive write requires an interactive terminal or --yes")
    response = input(f"Type {device} to overwrite it: ")
    if response != str(device):
        fail("confirmation did not match target device")


def assert_safe_disk(name: str, assume_yes: bool) -> Path:
    sysfs = sysfs_block(name)
    device = Path("/dev") / name
    if (sysfs / "partition").exists():
        fail(f"refusing to write a partition; pass the whole removable disk, not /dev/{name}")
    if read_sys(sysfs / "removable") != "1":
        fail(f"refusing non-removable disk /dev/{name}")
    if read_sys(sysfs / "ro") != "0":
        fail(f"refusing read-only disk /dev/{name}")

    mounted = mounted_devices()
    blocked = []
    for child in [sysfs, *sorted(sysfs.glob(f"{name}*"))]:
        child_dev = child / "dev"
        if child_dev.exists() and block_ids_with_holders(child) & mounted:
            blocked.append(child.name)
    if blocked:
        fail(f"refusing mounted device(s): {', '.join(blocked)}")

    confirm_destructive_write(device, disk_summary(sysfs), assume_yes)
    return device


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(CHUNK_SIZE):
            digest.update(chunk)
    return digest.hexdigest()


def block_sha256(path: Path, length: int) -> str:
    digest = hashlib.sha256()
    remaining = length
    with path.open("rb", buffering=0) as handle:
        while remaining:
            chunk = handle.read(min(CHUNK_SIZE, remaining))
            if not chunk:
                fail(f"short read while verifying {path}")
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def drop_device_cache(path: Path, length: int) -> None:
    if not hasattr(os, "posix_fadvise") or not hasattr(os, "POSIX_FADV_DONTNEED"):
        return
    with path.open("rb", buffering=0) as handle:
        os.posix_fadvise(handle.fileno(), 0, length, os.POSIX_FADV_DONTNEED)


def write_image(image: Path, device: Path) -> None:
    total = image.stat().st_size
    written = 0
    try:
        with image.open("rb") as src, device.open("r+b", buffering=0) as dst:
            while chunk := src.read(CHUNK_SIZE):
                dst.write(chunk)
                written += len(chunk)
            dst.flush()
            os.fsync(dst.fileno())
    except PermissionError as error:
        fail(f"permission denied writing {device}: run through sudo ({error})")
    if written != total:
        fail(f"short write: wrote {written} of {total} bytes")


def main() -> None:
    parser = argparse.ArgumentParser(description="Safely write a Slime OS boot image to removable media.")
    parser.add_argument("image", type=Path)
    parser.add_argument("device", type=Path)
    parser.add_argument("--yes", action="store_true", help="confirm the destructive removable-disk write")
    args = parser.parse_args()

    if not args.image.is_file():
        fail(f"image not found: {args.image}")
    name = block_name(args.device)
    device = assert_safe_disk(name, args.yes)

    expected = file_sha256(args.image)
    write_image(args.image, device)
    drop_device_cache(device, args.image.stat().st_size)
    actual = block_sha256(device, args.image.stat().st_size)
    if actual != expected:
        fail(f"verification failed: image sha256 {expected}, device sha256 {actual}")
    print(f"Wrote {args.image} to {device} ({args.image.stat().st_size} bytes, sha256:{actual})")


if __name__ == "__main__":
    main()
