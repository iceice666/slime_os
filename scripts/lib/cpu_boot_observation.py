"""P6.6's physical CPU-boot observation record.

The emulated planes reconstruct their evidence by re-running QEMU. A physical
boot cannot be re-run from an artifact, so what this module owns is the
record's *shape*: which facts a physical claim must carry, how they bind to the
exact medium `framework_media` approved, and which of them a host can check
rather than take on trust.

The split matters. `hash_protected_region` and `describe_disk` read real
devices and are evidence. Machine and firmware fields are operator-supplied and
cannot be derived — so they are bounded, required, and recorded verbatim rather
than validated into looking authoritative. `validate_record` refuses a record
whose checkable parts disagree with the medium, the target profile, or the
declared internal device.

Nothing here launches an emulator, and the contract carries no field that could
say a run was emulated: the record is only writable by the physical gate.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn

import cpu_boot_observation_contract as contract

ROOT = Path(__file__).resolve().parents[2]

CHUNK_SIZE = 1024 * 1024

DIGEST_PATTERN = re.compile(r"\A[0-9a-f]{64}\Z")

#: Fields every record must carry. Compared against `sorted(record)`, so these
#: are in Python's own string order rather than a reading order.
TOP_LEVEL_FIELDS = (
    "bootMediaIdentity",
    "bootMediaImageSha256",
    "boots",
    "formatVersion",
    "identity",
    "kind",
    "machine",
    "observedOn",
    "operator",
    "protectedRegion",
    "targetProfile",
    "writeTarget",
)

WRITE_TARGET_FIELDS = ("devicePath", "model", "removable", "serial", "sha256", "sizeBytes")
MACHINE_FIELDS = ("cpu", "firmwareVendor", "firmwareVersion", "product", "secureBoot", "vendor")
PROTECTED_REGION_FIELDS = (
    "devicePath",
    "model",
    "regionBytes",
    "serial",
    "sha256After",
    "sha256Before",
)
BOOT_FIELDS = (
    "coldStart",
    "displayBitsPerPixel",
    "displayHeight",
    "displayWidth",
    "generationIdentityPrefix",
    "generationNumber",
    "index",
    "keyboardUsed",
    "outcome",
    "panelSha256",
    "recordLines",
    "targetProfile",
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(CHUNK_SIZE):
            digest.update(chunk)
    return digest.hexdigest()


def record_identity(record: dict[str, object]) -> str:
    """The canonical digest of a record, excluding its own `identity`.

    Same construction as the boot-media identity: a domain-separated digest
    over the compact, key-sorted JSON encoding, so two hosts that agree on the
    facts agree on the digest.
    """
    material = dict(record)
    material.pop("identity", None)
    encoded = json.dumps(material, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"
    return hashlib.sha256(contract.IDENTITY_DOMAIN + encoded).hexdigest()


def _sysfs_block(name: str, fail: Callable[[str], NoReturn]) -> Path:
    path = Path("/sys/class/block") / name
    if not path.is_dir():
        fail(f"{path} is not a block device")
    return path


def _read_sys(path: Path, fail: Callable[[str], NoReturn]) -> str:
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def describe_disk(device: Path, fail: Callable[[str], NoReturn]) -> dict[str, object]:
    """The host's own description of a whole block device.

    Read from sysfs rather than accepted from the operator: the device path, its
    model, and its serial are what bind a record to one physical disk, and a
    typed-in serial proves nothing about what was actually written or read.
    """
    if device.parent != Path("/dev"):
        fail(f"expected a device directly under /dev, got {device}")
    name = device.name
    sysfs = _sysfs_block(name, fail)
    if (sysfs / "partition").exists():
        fail(f"{device} is a partition; name the whole disk")
    model_path = sysfs / "device" / "model"
    serial_path = sysfs / "device" / "serial"
    return {
        "devicePath": str(device),
        "model": _read_sys(model_path, fail) if model_path.is_file() else "",
        "serial": _read_sys(serial_path, fail) if serial_path.is_file() else "",
        "sizeBytes": int(_read_sys(sysfs / "size", fail)) * 512,
        "removable": _read_sys(sysfs / "removable", fail) == "1",
    }


def hash_protected_region(
    device: Path,
    fail: Callable[[str], NoReturn],
    *,
    length: int = contract.COMPARISON_REGION_BYTES,
) -> str:
    """Digest the first `length` bytes of `device`, bypassing the page cache.

    The cache eviction is load-bearing rather than tidy. The point of the pair
    of digests is to observe what is *on the disk* across a reboot; a cached
    read taken after the boot could reproduce the pre-boot bytes from memory
    and report a match that the platter does not support.
    """
    if device.parent != Path("/dev"):
        fail(f"expected a device directly under /dev, got {device}")
    digest = hashlib.sha256()
    remaining = length
    try:
        with device.open("rb", buffering=0) as handle:
            if hasattr(os, "posix_fadvise") and hasattr(os, "POSIX_FADV_DONTNEED"):
                os.posix_fadvise(handle.fileno(), 0, length, os.POSIX_FADV_DONTNEED)
            while remaining:
                chunk = handle.read(min(CHUNK_SIZE, remaining))
                if not chunk:
                    fail(f"short read hashing {device}: {remaining} bytes short of {length}")
                digest.update(chunk)
                remaining -= len(chunk)
    except PermissionError as error:
        fail(f"permission denied reading {device}: run through sudo ({error})")
    return digest.hexdigest()


def _require_digest(value: object, label: str, fail: Callable[[str], NoReturn]) -> str:
    if not isinstance(value, str) or not DIGEST_PATTERN.fullmatch(value):
        fail(f"{label} must be a lowercase SHA-256 digest")
    return value


def _require_text(value: object, label: str, fail: Callable[[str], NoReturn]) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    if len(value.encode("utf-8")) > contract.MAX_TEXT_BYTES:
        fail(f"{label} exceeds {contract.MAX_TEXT_BYTES} bytes")
    return value


def _validate_boot(
    boot: object,
    index: int,
    generation_prefix: str,
    fail: Callable[[str], NoReturn],
) -> None:
    if not isinstance(boot, dict) or tuple(sorted(boot)) != BOOT_FIELDS:
        fail(f"boot {index} has missing or unknown fields")
    if boot["index"] != index:
        fail(f"boot {index} is recorded out of order")
    if boot["outcome"] not in contract.BOOT_OUTCOMES:
        fail(f"boot {index} outcome {boot['outcome']!r} is not a declared outcome")
    # Every boot in a satisfying record is a cold start without input: a warm
    # reboot does not prove the firmware selects the medium from power-on, and a
    # keypress would contradict the milestone's no-input requirement.
    if boot["coldStart"] is not True:
        fail(f"boot {index} is not recorded as a cold start")
    if boot["keyboardUsed"] is not False:
        fail(f"boot {index} records keyboard input, which the milestone excludes")
    if boot["targetProfile"] != contract.TARGET_PROFILE:
        fail(f"boot {index} names target profile {boot['targetProfile']!r}")
    lines = boot["recordLines"]
    if not isinstance(lines, list) or not lines:
        fail(f"boot {index} records no panel lines")
    if len(lines) > contract.MAX_RECORD_LINES:
        fail(f"boot {index} records more than {contract.MAX_RECORD_LINES} panel lines")
    for line in lines:
        _require_text(line, f"boot {index} panel line", fail)
    # The panel is the evidence channel, so the fields the record claims must be
    # the fields the panel showed. A record whose prose disagrees with its own
    # rendered lines is not usable evidence.
    rendered = "\n".join(lines)
    if f"TARGET {contract.TARGET_PROFILE.upper()}" not in rendered.upper():
        fail(f"boot {index} panel lines do not name the target profile")
    prefix = boot["generationIdentityPrefix"]
    if not isinstance(prefix, str) or not re.fullmatch(r"[0-9A-F]{16}", prefix):
        fail(f"boot {index} generation identity prefix must be 16 uppercase hex digits")
    if prefix not in rendered.upper():
        fail(f"boot {index} panel lines do not show the recorded generation identity")
    # And the generation the panel showed must be the generation inside the
    # approved medium, which is what makes the boot evidence about *these*
    # bytes rather than about some other build of the same source.
    if prefix != generation_prefix:
        fail(
            f"boot {index} shows generation {prefix}, but the approved medium "
            f"carries {generation_prefix}"
        )
    for field in ("displayWidth", "displayHeight", "displayBitsPerPixel", "generationNumber"):
        value = boot[field]
        if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
            fail(f"boot {index} {field} must be a positive integer")
    panel = boot["panelSha256"]
    if panel != "":
        _require_digest(panel, f"boot {index} panel digest", fail)


def validate_record(
    record_path: Path,
    media_record: dict[str, object],
    generation_prefix: str,
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    """Load and check an observation record against the approved medium.

    Refuses before any claim is reported: a malformed, self-inconsistent, or
    medium-mismatched record is worse than a missing one, because it looks like
    evidence.
    """
    if not record_path.is_file():
        fail(f"missing CPU-boot observation record {record_path}")
    if record_path.stat().st_size > contract.MAX_RECORD_BYTES:
        fail(f"observation record exceeds {contract.MAX_RECORD_BYTES} bytes")
    try:
        record = json.loads(record_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse observation record {record_path}: {error}")
    if not isinstance(record, dict):
        fail("observation record must be an object")
    if tuple(sorted(record)) != TOP_LEVEL_FIELDS:
        fail("observation record has missing or unknown top-level fields")
    if record["formatVersion"] != contract.FORMAT_VERSION or record["kind"] != contract.KIND:
        fail("unsupported observation record")
    if record["identity"] != record_identity(record):
        fail("observation identity digest does not match the record")
    if record["targetProfile"] != contract.TARGET_PROFILE:
        fail(f"observation names target profile {record['targetProfile']!r}")
    _require_text(record["operator"], "operator", fail)
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", str(record["observedOn"])):
        fail("observedOn must be an ISO date")

    # The medium binding: the record is evidence about exactly the image the
    # media gate approved and QEMU booted, not about a rebuild of it.
    media_image = media_record.get("image")
    if not isinstance(media_image, dict):
        fail("boot-media record carries no image identity")
    if record["bootMediaIdentity"] != media_record.get("identity"):
        fail("observation does not name the approved boot-media identity")
    if record["bootMediaImageSha256"] != media_image.get("sha256"):
        fail("observation does not name the approved boot-media image digest")

    target = record["writeTarget"]
    if not isinstance(target, dict) or tuple(sorted(target)) != WRITE_TARGET_FIELDS:
        fail("observation write target has missing or unknown fields")
    if target["removable"] is not True:
        fail("observation write target is not recorded as removable")
    # The digest read back from the removable device must be the image's own, or
    # the machine booted something other than the approved bytes.
    if _require_digest(target["sha256"], "write target digest", fail) != media_image.get("sha256"):
        fail("the written removable device does not carry the approved image bytes")
    if not isinstance(target["sizeBytes"], int) or target["sizeBytes"] < media_image.get("bytes", 0):
        fail("observation write target is smaller than the image")
    _require_text(target["devicePath"], "write target path", fail)

    machine = record["machine"]
    if not isinstance(machine, dict) or tuple(sorted(machine)) != MACHINE_FIELDS:
        fail("observation machine has missing or unknown fields")
    for field in ("vendor", "product", "cpu", "firmwareVendor", "firmwareVersion"):
        _require_text(machine[field], f"machine {field}", fail)
    if not isinstance(machine["secureBoot"], bool):
        fail("machine secureBoot must be a boolean")

    region = record["protectedRegion"]
    if not isinstance(region, dict) or tuple(sorted(region)) != PROTECTED_REGION_FIELDS:
        fail("observation protected region has missing or unknown fields")
    if region["regionBytes"] != contract.COMPARISON_REGION_BYTES:
        fail(
            f"protected region is {region['regionBytes']} bytes, but the contract "
            f"fixes {contract.COMPARISON_REGION_BYTES}"
        )
    before = _require_digest(region["sha256Before"], "protected region pre-boot digest", fail)
    after = _require_digest(region["sha256After"], "protected region post-boot digest", fail)
    if before != after:
        fail(
            "the protected internal-storage region changed across the observed "
            f"boots: {before} before, {after} after"
        )
    _require_text(region["devicePath"], "protected region path", fail)
    # The protected device and the boot medium must be different devices, or the
    # no-write claim is about the medium the machine booted from.
    if region["devicePath"] == target["devicePath"]:
        fail("the protected region names the same device as the boot medium")
    if region["serial"] and region["serial"] == target["serial"]:
        fail("the protected region names the same serial as the boot medium")

    boots = record["boots"]
    if not isinstance(boots, list):
        fail("observation records no boots")
    if len(boots) != contract.REQUIRED_BOOTS:
        fail(
            f"observation records {len(boots)} boots, but the contract requires "
            f"exactly {contract.REQUIRED_BOOTS}"
        )
    for index, boot in enumerate(boots):
        _validate_boot(boot, index, generation_prefix, fail)
    if any(boot["outcome"] != "ready" for boot in boots):
        outcomes = ", ".join(str(boot["outcome"]) for boot in boots)
        fail(f"not every observed boot reached ready: {outcomes}")
    return record


def build_record(
    *,
    operator: str,
    observed_on: str,
    media_record: dict[str, object],
    write_target: dict[str, object],
    machine: dict[str, object],
    protected_region: dict[str, object],
    boots: list[dict[str, object]],
) -> dict[str, object]:
    """Assemble a record and stamp its canonical identity."""
    media_image = media_record["image"]
    record: dict[str, object] = {
        "formatVersion": contract.FORMAT_VERSION,
        "kind": contract.KIND,
        "identity": "",
        "targetProfile": contract.TARGET_PROFILE,
        "observedOn": observed_on,
        "operator": operator,
        "bootMediaIdentity": media_record["identity"],
        "bootMediaImageSha256": media_image["sha256"],
        "writeTarget": write_target,
        "machine": machine,
        "protectedRegion": protected_region,
        "boots": boots,
    }
    record["identity"] = record_identity(record)
    return record
