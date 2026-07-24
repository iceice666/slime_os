"""Hand-written host-side helpers shared by the check/build scripts.

The wire-format constants live in the generated ``boot_contracts``/``fs_contracts``
modules (source of truth: ``contracts/*/schema.zt``). This module is the home for
the *host* constants and subprocess boilerplate those scripts otherwise copy: the
repository root, the release-kernel path, the bounded-boot timeout, the sector
size, the sibling-script loader, and the QEMU/tool run harness.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path
from types import ModuleType

# scripts/ lives directly under the repository root, so this resolves to the
# same root every script previously recomputed.
ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"

# The Justfile check targets build `--release`; the debug kernel's larger stack
# frames overflow the boot stack, so every host check embeds the release binary.
RELEASE_KERNEL = ROOT / "kernel" / "target" / "x86_64-unknown-none" / "release" / "slime_os-kernel"

# Bound each boot so a wedged guest (e.g. a stack-overflow reboot loop) fails
# loudly instead of hanging the check forever.
BOOT_TIMEOUT_SECONDS = 600

# Logical block size for every disposable fixture image and on-disk layout.
SECTOR_SIZE = 512


def load_script(name: str, filename: str) -> ModuleType:
    """Import a sibling script whose hyphenated filename is not a module name."""
    path = SCRIPTS / filename
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
