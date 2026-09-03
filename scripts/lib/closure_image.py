"""Build a plane image by closure identity, for a checker that used a plane flag.

CP15 cuts every QEMU checker over from `build-sel4.py --<plane>-plane` to the
generic closure builder. The two differ in more than spelling:

- a plane flag names *behavior the caller chose*, and nothing in the resulting
  image records which flag produced it, so two callers passing different flags
  produce indistinguishable artifacts;
- a closure identity names *the inputs*, is recorded in the build result beside
  the image, and is independently re-resolved from repository state before the
  build runs — so a stale or hand-edited input is a refusal rather than a
  silently different image.

This module is the migration seam. A checker asks for a closure by name and
receives a built image path plus the build-result record, with the identity
verified against repository state rather than trusted from the record. It exists
so the cutover is one shared mechanism instead of 45 near-identical subprocess
calls, per this repository's verification-code discipline.

Deliberately not here: QEMU invocation, marker ordering, disk fixtures, and
fault injection. Those are the test-run contract's and each plane gate's, and
folding them in would rebuild the closure/test-run separation CP11 drew.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from harness import ROOT
from system_image_closure import compile_closure, resolve_closure

CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"
BUILDER = ROOT / "scripts" / "build" / "build-system-image.py"
BUILD_ROOT = ROOT / "build" / "closure"


class ClosureImageError(RuntimeError):
    """A closure could not be resolved or built."""


@dataclass(frozen=True)
class BuiltImage:
    """A built plane image and the record proving what it was built from."""

    name: str
    identity: str
    image: Path
    root: Path
    loader: Path
    generation: Path
    output: Path
    build_result: dict

    def digest(self) -> str:
        """The packaged image's recorded SHA-256."""
        image = self.build_result.get("image")
        if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
            raise ClosureImageError(f"{self.name}: the build result records no image digest")
        return image["sha256"]


def closure_path(name: str) -> Path:
    path = CLOSURE_ROOT / f"{name}.zti"
    if not path.is_file():
        raise ClosureImageError(
            f"no closure named {name!r}; expected {path.relative_to(ROOT)}"
        )
    return path


def closure_identity(name: str) -> str:
    """The identity of a closure, compiled from its committed bytes."""
    return compile_closure(closure_path(name)).identity.hex()


def verify_resolves(name: str) -> str:
    """Re-resolve a closure from repository state and return its identity.

    The generator writes identities from repository state and the resolver
    independently re-reads every named path, so a closure emitted from a dirty
    tree fails here rather than producing an image nobody can reproduce. This is
    what keeps a generated closure from being a circular claim, so it runs
    before every build rather than being trusted from the record.
    """
    path = closure_path(name)
    try:
        resolve_closure(path)
    except Exception as error:  # noqa: BLE001 - re-raised with the closure named
        raise ClosureImageError(f"{name}: closure does not resolve: {error}") from error
    return compile_closure(path).identity.hex()


def build(name: str, *, output: Path | None = None, reuse: bool = True) -> BuiltImage:
    """Build `name`'s image through the closure builder and return its artifacts.

    `reuse` returns an existing build whose recorded closure identity still
    matches the current one. That check is the whole value of reuse: an image
    built from a superseded closure is exactly the stale artifact a plane flag
    would have booted silently, so a mismatch rebuilds instead of being
    accepted.
    """
    identity = verify_resolves(name)
    destination = output if output is not None else BUILD_ROOT / name

    if reuse and destination.is_dir():
        existing = _read_result(destination)
        if existing is not None and existing.get("closureIdentity") == identity:
            return _built(name, identity, destination, existing)

    # The builder refuses a non-empty output directory, which is what stops a
    # stale artifact from being reported as this build's result. So a
    # superseded build must be removed rather than built over: reaching here
    # means either there was no previous build or its recorded identity no
    # longer matches, and in both cases nothing in the directory may survive
    # into the new one.
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    command = [sys.executable, str(BUILDER), str(closure_path(name)), str(destination)]
    print(f"[closure build] {name} {identity[:12]}", flush=True)
    process = subprocess.run(
        command, cwd=ROOT, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    )
    if process.returncode != 0:
        tail = "\n".join(process.stdout.strip().splitlines()[-20:])
        raise ClosureImageError(f"{name}: closure build failed:\n{tail}")

    result = _read_result(destination)
    if result is None:
        raise ClosureImageError(f"{name}: the closure build wrote no build-result.json")
    recorded = result.get("closureIdentity")
    if recorded != identity:
        raise ClosureImageError(
            f"{name}: the build recorded closure {recorded} but the repository resolves {identity}"
        )
    return _built(name, identity, destination, result)


def _read_result(output: Path) -> dict | None:
    path = output / "build-result.json"
    if not path.is_file():
        return None
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def _built(name: str, identity: str, output: Path, result: dict) -> BuiltImage:
    image = output / "image.elf"
    if not image.is_file():
        raise ClosureImageError(f"{name}: the closure build produced no image at {image}")
    return BuiltImage(
        name=name,
        identity=identity,
        image=image,
        root=output / "root.elf",
        loader=output / "loader.elf",
        generation=output / "generation",
        output=output,
        build_result=result,
    )
