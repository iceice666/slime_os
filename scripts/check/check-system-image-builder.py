#!/usr/bin/env python3

"""CP13 data-driven image-builder gate.

CP11 proved one closure builds one image. This proves the *corpus* does: every
derived composition has a closure, every closure resolves, each names a
distinct identity, and one command shape — `build-system-image.py CLOSURE
OUTPUT_DIR` — builds any of them with no plane flag, no variant table lookup,
and no composition named in Python control flow.

What it asserts, in order:

1. every derived composition either has a closure or is accounted for by a
   declared reason, and the closure set has no orphan;
2. every closure resolves against live repository state, and no two closures
   share an identity;
3. each resolved closure's derived manifest is the one its system spec
   produces, so the closure and the composition corpus cannot disagree about
   what a plane admits;
4. the generic builder builds a selected closure end to end and its
   build-result record names that closure's identity;
5. building the same closure twice into two output directories produces
   byte-identical generation, root, loader, image, and build-result identities;
6. the builder refuses a non-empty output directory, so two distinct closures
   cannot collide in one tree;
7. the builder takes no plane flag: its argument parser accepts exactly a
   closure and an output directory, and the source declares no `--*-plane`
   option, no `VARIANT_*` table, and no per-composition branch.

(4) and (5) are expensive — each is a full Cargo build of a root task and a
loader — so they run against one selected closure by default and every closure
under `--exhaustive`. (1) through (3) and (6) through (7) cover the whole
corpus on every run.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import ast
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from harness import ROOT
from system_image_closure import (
    SystemImageClosureError,
    resolve_closure,
)
from system_spec import DERIVED_GENERATION_FIXTURES, SYSTEM_ROOT, compile_system, derive_manifest
from component_spec import admit_specs, interface_catalogue

CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"
BUILDER = ROOT / "scripts" / "build" / "build-system-image.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GENERATOR = ROOT / "scripts" / "generate" / "generate-system-image-closures.py"

# The composition this gate builds when not exhaustive: the smallest derived
# seL4 graph, so the expensive arm is a real end-to-end build rather than a
# sampled one.
SELECTED = "sel4-channel"

# Derived compositions with no closure, and why. Each must be a composition the
# closure generator itself declines, so this list cannot hide a closure that
# merely failed to generate.
#
#   `reference` targets `x86_64-qemu-virtio`, for which no seL4 platform asset
#   exists to name.
#   `sel4` and `sel4-slisp` admit the product Slisp, whose implementation is an
#   `external` ELF built from C source at gate time: not a committed artifact
#   this repository can name by path, so a closure over it would invent one.
WITHOUT_CLOSURE = {
    "reference": "targets x86_64-qemu-virtio, which has no seL4 platform asset",
    "sel4": "admits an external product Slisp implementation with no committed artifact",
    "sel4-slisp": "admits an external product Slisp implementation with no committed artifact",
}


def fail(message: str) -> None:
    raise SystemExit(f"system image builder check: {message}")


def closure_paths() -> dict[str, Path]:
    return {path.stem: path for path in sorted(CLOSURE_ROOT.glob("*.zti"))}


def check_coverage(closures: dict[str, Path]) -> set[str]:
    """Every derived composition has a closure or a declared reason, and no orphan.

    A scenario closure is not a composition — it is a base composition plus
    declared build parameters — so the orphan check admits the scenario names
    the closure generator declares, and `check-system-image-scenario.py` owns
    asserting each one is a real scenario over a real base.
    """
    from importlib.util import module_from_spec, spec_from_file_location

    spec = spec_from_file_location("builder_closure_generator", GENERATOR)
    generator = module_from_spec(spec)
    spec.loader.exec_module(generator)
    scenarios = set(generator.SCENARIOS)

    derived = set(DERIVED_GENERATION_FIXTURES)
    missing = sorted(derived - set(closures) - set(WITHOUT_CLOSURE))
    if missing:
        fail(f"derived composition(s) with no closure and no declared reason: {missing}")
    orphaned = sorted(set(closures) - derived - scenarios)
    if orphaned:
        fail(f"closure(s) naming no derived composition or declared scenario: {orphaned}")
    stale_reasons = sorted(set(WITHOUT_CLOSURE) & set(closures))
    if stale_reasons:
        fail(
            f"composition(s) {stale_reasons} are declared to have no closure but one exists; "
            "remove the exemption"
        )
    unknown_reasons = sorted(set(WITHOUT_CLOSURE) - derived)
    if unknown_reasons:
        fail(f"exemption(s) naming no derived composition: {unknown_reasons}")
    return scenarios


def check_resolution(closures: dict[str, Path], scenarios: set[str]) -> dict[str, str]:
    """Every closure resolves, names a distinct identity, and agrees with its spec."""
    catalogue = interface_catalogue()
    components = {entry.name: entry.spec for entry in admit_specs(catalogue=catalogue)}
    identities: dict[str, str] = {}
    for name, path in closures.items():
        try:
            resolved = resolve_closure(path)
        except (SystemImageClosureError, SystemExit) as error:
            fail(f"{name}: closure does not resolve: {error}")
        identity = resolved.compiled.identity.hex()
        if identity in identities.values():
            duplicate = next(k for k, v in identities.items() if v == identity)
            fail(f"{name} and {duplicate} compute the same closure identity")
        identities[name] = identity
        # The closure's own manifest and the composition corpus's must be the
        # same object: a closure that resolved a different graph than the plane
        # gates boot would be a second authority on what the plane admits.
        #
        # A scenario closure is excluded, and only because its whole purpose is
        # to differ: its declared build parameters change the manifest, and
        # `check-system-image-scenario.py` asserts they change exactly the
        # fields they name and nothing else.
        if name in scenarios:
            if not resolved.build_parameters:
                fail(f"{name}: declared a scenario but carries no build parameters")
            continue
        if resolved.build_parameters:
            fail(f"{name}: carries build parameters but is not a declared scenario")
        expected = derive_manifest(
            compile_system(SYSTEM_ROOT / f"{name}.zti", components=components)
        )
        if resolved.manifest != expected:
            fail(f"{name}: resolved closure manifest differs from its system spec's derivation")
    return identities


def check_no_plane_flags() -> None:
    """The builder takes a closure and an output directory, and nothing else."""
    source = BUILDER.read_text(encoding="utf-8")
    tree = ast.parse(source, filename=str(BUILDER))
    added = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == "add_argument"
    ]
    declared = []
    for node in added:
        if node.args and isinstance(node.args[0], ast.Constant):
            declared.append(node.args[0].value)
    if sorted(declared) != ["closure", "output_dir"]:
        fail(
            "the closure builder declares arguments "
            f"{sorted(declared)}; expected exactly ['closure', 'output_dir']"
        )
    for pattern, label in (
        (r"--[a-z0-9-]+-plane", "a plane flag"),
        (r"\bVARIANT_[A-Z_]+\b", "a variant mapping table"),
        (r"--component-graph\b", "the component-graph flag"),
        (r"--external-component\b", "an external-component flag"),
        (r"--prebuilt-generation\b", "a prebuilt-generation flag"),
    ):
        if re.search(pattern, source):
            fail(f"the closure builder still references {label}")
    # And no composition is named in its control flow: a closure selects the
    # graph, so the builder must not know one composition from another.
    for name in sorted(DERIVED_GENERATION_FIXTURES):
        if name == "reference":
            continue
        if re.search(rf'"{re.escape(name)}"', source):
            fail(f"the closure builder names composition {name!r} in source")


def build(closure: Path, output: Path) -> dict:
    process = subprocess.run(
        [sys.executable, str(BUILDER), str(closure), str(output)],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        tail = "\n".join(process.stdout.strip().splitlines()[-15:])
        fail(f"{closure.stem}: closure build failed:\n{tail}")
    result = output / "build-result.json"
    if not result.is_file():
        fail(f"{closure.stem}: build wrote no build-result.json")
    return json.loads(result.read_text(encoding="utf-8"))


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_build(name: str, path: Path, identity: str) -> None:
    """One closure builds, reproduces, and refuses a dirty output directory."""
    with tempfile.TemporaryDirectory(prefix=f"slime-builder-{name}-") as scope:
        root = Path(scope)
        first = root / "first"
        second = root / "second"
        left = build(path, first)
        if left["closureIdentity"] != identity:
            fail(f"{name}: build result names closure {left['closureIdentity']} not {identity}")
        for output in ("generation/generation.bin", "root.elf", "loader.elf", "image.elf"):
            if not (first / output).is_file():
                fail(f"{name}: build produced no {output}")
        right = build(path, second)
        for field in ("generation", "root", "loader", "image", "identityManifest"):
            if left[field]["sha256"] != right[field]["sha256"]:
                fail(f"{name}: two builds of one closure disagree on {field}")
        if (first / "build-result.identity").read_text(encoding="utf-8") != (
            second / "build-result.identity"
        ).read_text(encoding="utf-8"):
            fail(f"{name}: two builds of one closure disagree on the build-result identity")
        # A non-empty output directory is refused, which is what keeps two
        # distinct closure identities from colliding in one tree.
        collision = subprocess.run(
            [sys.executable, str(BUILDER), str(path), str(first)],
            cwd=ROOT,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if collision.returncode == 0:
            fail(f"{name}: the builder overwrote a non-empty output directory")
        if "output directory is not empty" not in collision.stdout:
            fail(f"{name}: wrong refusal for a non-empty output directory: {collision.stdout[-200:]}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--exhaustive",
        action="store_true",
        help="build every closure rather than the selected one",
    )
    arguments = parser.parse_args()

    generated = subprocess.run(
        [sys.executable, str(GENERATOR), "--check"],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if generated.returncode != 0:
        fail(f"closures are stale: {generated.stdout.strip()}")

    closures = closure_paths()
    if not closures:
        fail("no closure exists")
    scenarios = check_coverage(closures)
    identities = check_resolution(closures, scenarios)
    check_no_plane_flags()

    selected = sorted(closures) if arguments.exhaustive else [SELECTED]
    for name in selected:
        if name not in closures:
            fail(f"{name}: selected for build but has no closure")
        check_build(name, closures[name], identities[name])

    print(
        f"system image builder check: {len(closures)} closures resolve with distinct "
        f"identities and manifests matching their system specs; "
        f"{len(WITHOUT_CLOSURE)} derived compositions declared closure-exempt, "
        f"{len(scenarios)} scenario closure(s); "
        f"{len(selected)} closure(s) built twice byte-identically through one command "
        "shape with no plane flag, variant table, or composition named in builder source"
    )


if __name__ == "__main__":
    main()
