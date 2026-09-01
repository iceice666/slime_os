"""Hand-written host-side helpers shared by the check/build scripts.

The wire-format constants live in generated ``boot_contracts``/``fs_contracts``
modules (source of truth: ``contracts/*/schema.zt``). This module holds the host
constants and subprocess boilerplate shared by product build and check scripts.
"""

from __future__ import annotations

import hashlib
import importlib.util
import subprocess
import sys
import tomllib
from collections.abc import Callable
from pathlib import Path
from types import ModuleType
from typing import NoReturn


# ``scripts/lib`` is two levels below the repository root.
ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
BUILD_SCRIPTS = SCRIPTS / "build"
CHECK_SCRIPTS = SCRIPTS / "check"
GENERATE_SCRIPTS = SCRIPTS / "generate"
LIB_SCRIPTS = SCRIPTS / "lib"

# The generation-manifest contract. `fixtures/` holds the two schema-conformance
# fixtures `check-valid.zt` and `check-invalid.zt` decode; `compositions/` holds
# the product and plane manifests `build-generation.py` encodes. Separate from
# `contracts/generation/v{2..5}`, which is the boot-time *binary* format.
GENERATION_CONTRACT = ROOT / "contracts" / "generation-manifest" / "v1"
GENERATION_FIXTURES = GENERATION_CONTRACT / "fixtures"
GENERATION_COMPOSITIONS = GENERATION_CONTRACT / "compositions"


# Bound each boot so a wedged guest (e.g. a stack-overflow reboot loop) fails
# loudly instead of hanging the check forever.
BOOT_TIMEOUT_SECONDS = 600

# Logical block size for every disposable fixture image and on-disk layout.
SECTOR_SIZE = 512


def load_script(name: str, relative_path: str) -> ModuleType:
    """Import a script whose hyphenated filename is not a module name."""
    path = SCRIPTS / relative_path
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_qemu(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    cwd: Path = ROOT,
    allow_failure: bool = False,
    timeout: int | None = BOOT_TIMEOUT_SECONDS,
    echo: str = "always",
) -> str:
    """Run a bounded guest/tool subprocess with combined stdout+stderr.

    ``echo`` controls when captured output is streamed to this process's stdout:
    ``"always"`` before returning, ``"on-error"`` only when the command fails,
    ``"never"`` leaves it to the caller. A ``timeout`` of ``None`` disables the
    bound. On timeout the captured output is streamed and ``SystemExit`` is
    raised; on a non-allowed failure ``SystemExit(returncode)`` is raised.
    """
    try:
        process = subprocess.run(
            arguments,
            cwd=cwd,
            env=environment,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        output = error.output or ""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        sys.stdout.write(output)
        raise SystemExit(
            f"command timed out after {timeout}s (wedged guest?): {arguments}"
        ) from error
    failed = process.returncode != 0 and not allow_failure
    if echo == "always" or (echo == "on-error" and failed):
        sys.stdout.write(process.stdout)
    if failed:
        raise SystemExit(process.returncode)
    return process.stdout


# seL4 gates default to the established AArch64 reference profile. Cross-target
# gates pass their own section explicitly; the section remains part of every
# validation error so a missing RV64 pin cannot be misreported as an ARM one.
QEMU_PROFILE_SECTION = "qemu_arm_virt"


def load_qemu_profile(
    fail: Callable[[str], NoReturn],
    pins_path: Path | None = None,
    section: str = QEMU_PROFILE_SECTION,
) -> dict[str, object]:
    """Load one exact QEMU profile table from ``sel4/pins.toml``.

    Refuses a missing file or section rather than returning an empty mapping: a
    gate that read no profile would boot QEMU with defaults and still claim it
    had honoured the pins.
    """
    path = pins_path or ROOT / "sel4" / "pins.toml"
    if not path.is_file():
        fail(f"missing pin manifest: {path.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {path.relative_to(ROOT)}: {error}")
    profile = pins.get(section)
    if not isinstance(profile, dict):
        fail(f"{path.relative_to(ROOT)} declares no [{section}] profile")
    return profile


def profile_text(
    profile: dict[str, object],
    key: str,
    fail: Callable[[str], NoReturn],
    section: str = QEMU_PROFILE_SECTION,
) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [{section}].{key} must be non-empty text")
    return value


def profile_integer(
    profile: dict[str, object],
    key: str,
    fail: Callable[[str], NoReturn],
    section: str = QEMU_PROFILE_SECTION,
) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [{section}].{key} must be an integer")
    return value


def qemu_kernel_arguments(
    qemu_binary: str,
    image_path: Path,
    fail: Callable[[str], NoReturn],
) -> list[str]:
    """Return the platform-correct QEMU arguments for one packaged image.

    QEMU's RISC-V ``-kernel`` path enters the ELF load base, but the rust-sel4
    loader entry is inside that first segment. Load the image as ELF, then place
    a two-instruction PC-relative jump at the load base so OpenSBI reaches the
    declared entry without rewriting the packaged artifact.
    """
    if qemu_binary != "qemu-system-riscv64":
        return ["-kernel", str(image_path)]

    data = image_path.read_bytes()
    if len(data) < 64 or data[:4] != b"\x7fELF" or data[4:6] != b"\x02\x01":
        fail(f"{image_path.relative_to(ROOT)} is not a little-endian ELF64 image")
    entry = int.from_bytes(data[24:32], "little")
    program_offset = int.from_bytes(data[32:40], "little")
    program_size = int.from_bytes(data[54:56], "little")
    program_count = int.from_bytes(data[56:58], "little")
    load_base: int | None = None
    for index in range(program_count):
        offset = program_offset + index * program_size
        header = data[offset : offset + program_size]
        if len(header) != program_size:
            fail(f"{image_path.relative_to(ROOT)} has a truncated program-header table")
        if int.from_bytes(header[0:4], "little") == 1:
            load_base = int.from_bytes(header[24:32], "little")
            break
    if load_base is None:
        fail(f"{image_path.relative_to(ROOT)} has no loadable segment")
    delta = entry - load_base
    upper = (delta + 0x800) >> 12
    lower = delta - (upper << 12)
    if not (-(1 << 19) <= upper < (1 << 19) and -(1 << 11) <= lower < (1 << 11)):
        fail(f"RISC-V loader entry delta {delta:#x} is outside the two-instruction shim")
    auipc_t0 = ((upper & 0xFFFFF) << 12) | (5 << 7) | 0x17
    jalr_zero_t0 = ((lower & 0xFFF) << 20) | (5 << 15) | 0x67
    shim = image_path.with_name(f".{image_path.name}.entry.bin")
    shim.write_bytes(auipc_t0.to_bytes(4, "little") + jalr_zero_t0.to_bytes(4, "little"))
    return [
        "-kernel",
        str(image_path),
        "-device",
        f"loader,file={shim},addr={load_base:#x},force-raw=on",
    ]


def sha256_file(path: Path, fail: Callable[[str], NoReturn]) -> str:
    """A built artifact's digest, refused rather than defaulted when absent.

    Gates compare this against the identity manifest the build wrote, so a
    missing file must fail here instead of producing a digest of nothing that
    then mismatches with a confusing message.
    """
    if not path.is_file():
        fail(f"missing artifact: {path}")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()
