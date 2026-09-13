#!/usr/bin/env python3
"""Every generation this repository builds is v5, and nothing still writes v4.

B50's exit condition asks that "every fixture uses v5". The wire format cut
over at `8745d18`, and the v4 binding generator is still on disk under
`contracts/generation/v4/` because the format's history is part of the
contract. What must not survive is a *producer*: a manifest the builder still
encodes as v4, or a second `GENERATION_VERSION` that some path selects.

Checked by building, not by reading. A manifest can declare whatever it likes
in `formatVersion` -- that field is the *manifest* schema's version, not the
wire format's, and the two are easy to confuse. The authority is the magic and
version word in the bytes the root actually decodes.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(ROOT / "scripts" / "lib"))
sys.path.insert(0, str(ROOT / "scripts" / "build"))

EXPECTED_MAGIC = b"SLIMEG5\0"
EXPECTED_VERSION = 5


class SelectionError(ValueError):
    pass


def fail(message: str) -> None:
    print(f"generation v5 check: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_builder():
    import importlib.util

    path = ROOT / "scripts" / "build" / "build-generation.py"
    spec = importlib.util.spec_from_file_location("build_generation_v5", path)
    if spec is None or spec.loader is None:
        fail(f"cannot load {path.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["build_generation_v5"] = module
    spec.loader.exec_module(module)
    return module


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard-index", type=int, default=0)
    parser.add_argument("--shard-count", type=int, default=1)
    parser.add_argument("--list-manifests", action="store_true")
    parser.add_argument("--check-controls", action="store_true", help=argparse.SUPPRESS)
    return parser.parse_args(argv)


def select_manifests(
    inventory: Iterable[str], shard_index: int, shard_count: int
) -> tuple[list[str], list[str]]:
    manifests = sorted(inventory)
    if not manifests:
        raise SelectionError("the builder declares no seL4 manifests")
    if shard_count <= 0:
        raise SelectionError("--shard-count must be positive")
    if shard_index < 0:
        raise SelectionError("--shard-index must not be negative")
    if shard_index >= shard_count:
        raise SelectionError("--shard-index must be less than --shard-count")
    if shard_count > len(manifests):
        raise SelectionError(f"--shard-count {shard_count} exceeds the {len(manifests)} manifests")
    selected = manifests[shard_index::shard_count]
    if not selected:
        raise SelectionError(f"shard {shard_index} of {shard_count} selects no manifests")
    return manifests, selected


def check_single_version(builder) -> None:
    """One version constant, and it is the one the decoder expects.

    A second constant is how a v4 path survives a cutover: the format stays
    described in one place while some manifest quietly selects the other.
    """
    if builder.GENERATION_VERSION != EXPECTED_VERSION:
        fail(
            f"builder writes generation version {builder.GENERATION_VERSION}, "
            f"expected {EXPECTED_VERSION}"
        )
    magic = builder.GENERATION_MAGIC
    if isinstance(magic, str):
        magic = magic.encode()
    if magic != EXPECTED_MAGIC:
        fail(f"builder writes magic {magic!r}, expected {EXPECTED_MAGIC!r}")


def built_header(manifest: str, slisp_mapping: str | None) -> tuple[bytes, int]:
    """Build one manifest and return the magic and version it encodes."""
    with tempfile.TemporaryDirectory() as directory:
        environment = os.environ.copy()
        environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
        environment["SLIME_SEL4_MANIFEST"] = manifest
        command = [
            sys.executable,
            str(ROOT / "scripts" / "build" / "build-generation.py"),
        ]
        if slisp_mapping is not None:
            command += ["--external-component", slisp_mapping]
        command.append(directory)
        result = subprocess.run(
            command,
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            fail(f"{manifest}: build failed: {result.stderr.strip().splitlines()[-1:]}")
        blob = Path(directory) / "generation.bin"
        if not blob.is_file():
            fail(f"{manifest}: build produced no generation.bin")
        return read_header(blob, manifest)


def read_header(blob: Path, manifest: str) -> tuple[bytes, int]:
    with blob.open("rb") as stream:
        header = stream.read(12)
    if len(header) != 12:
        fail(f"{manifest}: truncated generation header ({len(header)} of 12 bytes)")
    return header[:8], int.from_bytes(header[8:12], "little")


def build_slisp(directory: str) -> str:
    slisp = Path(directory) / "slisp.elf"
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts" / "build" / "build-c-component.py"),
            str(ROOT / "components" / "slisp" / "slisp.c"),
            str(ROOT / "components" / "slisp" / "main.c"),
            str(slisp),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        fail(f"Slisp build failed: {result.stderr.strip().splitlines()[-1:]}")
    return f"slisp-external={slisp}"


def run_check(
    args: argparse.Namespace,
    *,
    builder=None,
    header_builder=built_header,
    slisp_builder=build_slisp,
) -> None:
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if builder is None:
        builder = load_builder()
    check_single_version(builder)
    try:
        manifests, selected = select_manifests(
            builder.SEL4_MANIFESTS, args.shard_index, args.shard_count
        )
    except SelectionError as error:
        fail(str(error))

    if args.list_manifests:
        for manifest in selected:
            print(manifest)
        return

    with tempfile.TemporaryDirectory() as directory:
        slisp_mapping = slisp_builder(directory) if "sel4" in selected else None
        for position, manifest in enumerate(selected, 1):
            print(
                f"generation v5 check: [{position}/{len(selected)}] building {manifest}",
                flush=True,
            )
            magic, version = header_builder(
                manifest,
                slisp_mapping if manifest == "sel4" else None,
            )
            if magic != EXPECTED_MAGIC or version != EXPECTED_VERSION:
                fail(
                    f"{manifest} encodes magic {magic!r} version {version}, "
                    f"expected {EXPECTED_MAGIC!r} version {EXPECTED_VERSION}"
                )
    label = EXPECTED_MAGIC.rstrip(b"\0").decode()
    if args.shard_count == 1:
        scope = f"all {len(manifests)} seL4 manifests"
    else:
        scope = (
            f"shard {args.shard_index + 1}/{args.shard_count}: "
            f"{len(selected)} of {len(manifests)} seL4 manifests"
        )
    print(
        f"generation v5 check: {scope} encode {label} version {EXPECTED_VERSION}",
        flush=True,
    )


def check_controls(*, announce: bool) -> None:
    for size in range(1, 13):
        inventory = {f"manifest-{index:02}" for index in range(size)}
        for count in range(1, size + 1):
            shards = [select_manifests(inventory, index, count)[1] for index in range(count)]
            flattened = [manifest for shard in shards for manifest in shard]
            assert sorted(flattened) == sorted(inventory)
            assert len(flattened) == len(set(flattened))

    inventory = ["zeta", "alpha", "sel4", "middle"]
    full, selected = select_manifests(inventory, 0, 1)
    assert full == sorted(inventory)
    assert selected == full
    for index, count in [(-1, 1), (1, 1), (0, 0), (0, -1), (0, 5)]:
        try:
            select_manifests(inventory, index, count)
        except SelectionError:
            pass
        else:
            raise AssertionError(f"accepted invalid shard {index}/{count}")
    try:
        select_manifests([], 0, 1)
    except SelectionError:
        pass
    else:
        raise AssertionError("accepted an empty manifest inventory")

    class Builder:
        GENERATION_VERSION = EXPECTED_VERSION
        GENERATION_MAGIC = EXPECTED_MAGIC
        SEL4_MANIFESTS = inventory

    def no_build(*_args):
        raise AssertionError("list mode attempted a build")

    output = StringIO()
    with redirect_stdout(output):
        run_check(
            parse_args(["--shard-index", "1", "--shard-count", "2", "--list-manifests"]),
            builder=Builder,
            header_builder=no_build,
            slisp_builder=no_build,
        )
    assert output.getvalue().splitlines() == sorted(inventory)[1::2]

    default_builds: list[str] = []

    def record_default(manifest: str, _mapping: str | None) -> tuple[bytes, int]:
        default_builds.append(manifest)
        return EXPECTED_MAGIC, EXPECTED_VERSION

    with redirect_stdout(StringIO()):
        run_check(
            parse_args([]),
            builder=Builder,
            header_builder=record_default,
            slisp_builder=lambda _directory: "slisp-external=/tmp/slisp.elf",
        )
    assert default_builds == sorted(inventory)

    built: list[tuple[str, str | None]] = []
    slisp_mapping = "slisp-external=/tmp/slisp.elf"

    def record_build(manifest: str, mapping: str | None) -> tuple[bytes, int]:
        built.append((manifest, mapping))
        return EXPECTED_MAGIC, EXPECTED_VERSION

    with redirect_stdout(StringIO()):
        run_check(
            parse_args(["--shard-index", "0", "--shard-count", "2"]),
            builder=Builder,
            header_builder=record_build,
            slisp_builder=lambda _directory: slisp_mapping,
        )
    relevant = sorted(inventory)[0::2]
    assert [manifest for manifest, _mapping in built] == relevant
    assert built == [
        (manifest, slisp_mapping if manifest == "sel4" else None) for manifest in relevant
    ]

    def bad_header(_manifest: str, _mapping: str | None) -> tuple[bytes, int]:
        return b"BADMAGIC", EXPECTED_VERSION

    try:
        with redirect_stdout(StringIO()), redirect_stderr(StringIO()):
            run_check(
                parse_args(["--shard-index", "1", "--shard-count", "2"]),
                builder=Builder,
                header_builder=bad_header,
                slisp_builder=no_build,
            )
    except SystemExit as error:
        assert error.code == 1
    else:
        raise AssertionError("accepted a bad built generation header")
    with tempfile.TemporaryDirectory() as directory:
        blob = Path(directory) / "generation.bin"
        valid = EXPECTED_MAGIC + EXPECTED_VERSION.to_bytes(4, "little")
        blob.write_bytes(valid + b"payload")
        assert read_header(blob, "control") == (EXPECTED_MAGIC, EXPECTED_VERSION)
        for length in range(12):
            blob.write_bytes(valid[:length])
            error_output = StringIO()
            try:
                with redirect_stderr(error_output):
                    read_header(blob, "control")
            except SystemExit as error:
                assert error.code == 1
                assert "truncated generation header" in error_output.getvalue()
            else:
                raise AssertionError(f"accepted a {length}-byte generation header")
    if announce:
        print("generation v5 shard controls: passed")


def main(argv: list[str] | None = None) -> None:
    args = parse_args(argv)
    if args.check_controls:
        check_controls(announce=True)
        return
    run_check(args)


if __name__ == "__main__":
    main()
