#!/usr/bin/env python3
"""Every seL4 manifest the generation builder declares still builds.

This is the only gate that builds the whole manifest corpus: a plane gate
builds its own composition, and `system_image_builder_check` builds one
selected closure unless run with `--exhaustive`. A manifest whose generation no
longer builds is otherwise found only when someone next runs its plane.

`--shard-index`/`--shard-count` split the sorted corpus round-robin so CI can
build it on parallel runners; shard `i` of `n` builds `manifests[i::n]`, so the
shards together build every manifest exactly once.

Before any real build, the controls run the same acceptance path against a
stand-in builder that fails, writes no `generation.bin`, or writes an empty
one, and require each to be refused, so a pass is evidence of bytes rather
than of a builder that exited zero.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import subprocess
import sys
import tempfile
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILDER = ROOT / "scripts" / "build" / "build-generation.py"

sys.path.insert(0, str(ROOT / "scripts" / "lib"))
sys.path.insert(0, str(ROOT / "scripts" / "build"))


def fail(message: str) -> None:
    print(f"generation corpus check: {message}", file=sys.stderr)
    raise SystemExit(1)


def manifests() -> list[str]:
    spec = importlib.util.spec_from_file_location("build_generation_corpus", BUILDER)
    if spec is None or spec.loader is None:
        fail(f"cannot load {BUILDER.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["build_generation_corpus"] = module
    spec.loader.exec_module(module)
    names = sorted(module.SEL4_MANIFESTS)
    if not names:
        fail("the builder declares no seL4 manifests")
    return names


def select(names: list[str], index: int, count: int) -> list[str]:
    if count <= 0 or not 0 <= index < count:
        fail(f"shard {index} of {count} is not a valid shard")
    if count > len(names):
        fail(f"--shard-count {count} exceeds the {len(names)} manifests")
    return names[index::count]


def run(command: list[str], what: str, environment: dict[str, str] | None = None) -> None:
    result = subprocess.run(
        command, cwd=ROOT, env=environment, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        fail(f"{what}: build failed: {result.stderr.strip().splitlines()[-1:]}")


def build_slisp(directory: Path) -> str:
    """The `sel4` manifest embeds Slisp as an external component."""
    slisp = directory / "slisp.elf"
    run(
        [
            sys.executable,
            str(ROOT / "scripts" / "build" / "build-c-component.py"),
            str(ROOT / "components" / "slisp" / "slisp.c"),
            str(ROOT / "components" / "slisp" / "main.c"),
            str(slisp),
        ],
        "Slisp",
    )
    return f"slisp-external={slisp}"


def build(manifest: str, slisp_mapping: str | None, builder: list[str] | None = None) -> None:
    with tempfile.TemporaryDirectory() as directory:
        environment = os.environ | {
            "SLIME_TARGET_PROFILE": "aarch64-sel4-qemu-virt",
            "SLIME_SEL4_MANIFEST": manifest,
        }
        command = builder or [sys.executable, str(BUILDER)]
        if slisp_mapping is not None:
            command += ["--external-component", slisp_mapping]
        run([*command, directory], manifest, environment)
        blob = Path(directory) / "generation.bin"
        if not blob.is_file() or blob.stat().st_size == 0:
            fail(f"{manifest}: build produced no generation.bin")


# Stand-in builders: each receives the output directory as its last argument.
CONTROLS = {
    "builder exits non-zero": "raise SystemExit(1)",
    "no generation.bin": "pass",
    "empty generation.bin": "import sys, pathlib; (pathlib.Path(sys.argv[-1]) / 'generation.bin').write_bytes(b'')",
}
ACCEPTED = "import sys, pathlib; (pathlib.Path(sys.argv[-1]) / 'generation.bin').write_bytes(b'G')"


def check_controls() -> None:
    for label, body in CONTROLS.items():
        try:
            with redirect_stderr(StringIO()):
                build("control", None, [sys.executable, "-c", body])
        except SystemExit:
            continue
        fail(f"control accepted: {label}")
    build("control", None, [sys.executable, "-c", ACCEPTED])
    print(f"generation corpus check: {len(CONTROLS)} controls refused", flush=True)


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard-index", type=int, default=0)
    parser.add_argument("--shard-count", type=int, default=1)
    parser.add_argument("--list-manifests", action="store_true")
    args = parser.parse_args(argv)
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    names = manifests()
    selected = select(names, args.shard_index, args.shard_count)
    if args.list_manifests:
        print("\n".join(selected))
        return
    check_controls()
    with tempfile.TemporaryDirectory() as directory:
        slisp_mapping = build_slisp(Path(directory)) if "sel4" in selected else None
        for position, manifest in enumerate(selected, 1):
            print(f"generation corpus check: [{position}/{len(selected)}] building {manifest}", flush=True)
            build(manifest, slisp_mapping if manifest == "sel4" else None)
    scope = (
        f"all {len(names)}"
        if args.shard_count == 1
        else f"shard {args.shard_index + 1}/{args.shard_count}: {len(selected)} of {len(names)}"
    )
    print(f"generation corpus check: {scope} seL4 manifests built", flush=True)


if __name__ == "__main__":
    main()
