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


# The pinned QEMU machine profile every seL4 plane gate boots against. Before
# B63 each gate carried its own `load_pins`/`profile_text`/`profile_integer` —
# 33, 31, and 31 copies, differing only in the wording of their refusals. A pin
# reader duplicated 33 times is 33 chances for one gate to accept a profile the
# others reject, which is the opposite of what pinning is for.
#
# `fail` is passed in rather than imported: each gate raises `SystemExit` with
# its own prefix (`seL4 boot plane check: …`), and that prefix is how a failure
# in a suite of 30-odd gates is attributable. These helpers borrow it instead of
# imposing one.
QEMU_PROFILE_SECTION = "qemu_arm_virt"


def load_qemu_profile(
    fail: Callable[[str], NoReturn], pins_path: Path | None = None
) -> dict[str, object]:
    """The `[qemu_arm_virt]` table of `sel4/pins.toml`.

    Refuses a missing file or a missing section rather than returning an empty
    mapping: a gate that read no profile would boot QEMU with defaults and still
    claim it had honoured the pins.
    """
    path = pins_path or ROOT / "sel4" / "pins.toml"
    if not path.is_file():
        fail(f"missing pin manifest: {path.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {path.relative_to(ROOT)}: {error}")
    profile = pins.get(QEMU_PROFILE_SECTION)
    if not isinstance(profile, dict):
        fail(f"{path.relative_to(ROOT)} declares no [{QEMU_PROFILE_SECTION}] profile")
    return profile


def profile_text(
    profile: dict[str, object], key: str, fail: Callable[[str], NoReturn]
) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [{QEMU_PROFILE_SECTION}].{key} must be non-empty text")
    return value


def profile_integer(
    profile: dict[str, object], key: str, fail: Callable[[str], NoReturn]
) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [{QEMU_PROFILE_SECTION}].{key} must be an integer")
    return value


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
