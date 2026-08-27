#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import subprocess
import tempfile
from pathlib import Path

from harness import ROOT

RUNTIME = ROOT / "components" / "runtime"
INCLUDE = RUNTIME / "include"
SEL4_INCLUDE = ROOT / "build" / "sel4-prefix" / "libsel4" / "include"
COMMON_FLAGS = (
    "--target=aarch64-none-elf",
    "-ffreestanding",
    "-fno-stack-protector",
    "-fno-builtin",
    "-nostdlib",
)


def run(command: list[str]) -> None:
    subprocess.run(command, cwd=ROOT, env=os.environ.copy(), check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description="Build one freestanding C Slime component")
    parser.add_argument("source", type=Path, nargs="+")
    parser.add_argument("output", type=Path)
    parser.add_argument("--cc", default=os.environ.get("CC", "clang"))
    arguments = parser.parse_args()
    sources = [source.resolve() for source in arguments.source]
    output = arguments.output.resolve()
    for source in sources:
        if not source.is_file():
            raise SystemExit(f"missing component source: {source}")
    if not SEL4_INCLUDE.is_dir():
        raise SystemExit("missing build/sel4-prefix; run `just sel4_qemu_image_check` first")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="slime-c-component-") as temporary:
        staging = Path(temporary)
        include_flags = ["-isystem", str(SEL4_INCLUDE), "-I", str(INCLUDE)]
        runtime_object = staging / "runtime.o"
        start_object = staging / "start.o"
        component_objects = [staging / f"component-{index}.o" for index in range(len(sources))]
        run(
            [
                arguments.cc,
                *COMMON_FLAGS,
                *include_flags,
                "-c",
                str(RUNTIME / "c" / "component_runtime.c"),
                "-o",
                str(runtime_object),
            ]
        )
        run(
            [
                arguments.cc,
                *COMMON_FLAGS,
                "-c",
                str(RUNTIME / "c" / "start-aarch64.S"),
                "-o",
                str(start_object),
            ]
        )
        for source, component_object in zip(sources, component_objects, strict=True):
            run(
                [
                    arguments.cc,
                    *COMMON_FLAGS,
                    *include_flags,
                    "-Os",
                    "-c",
                    str(source),
                    "-o",
                    str(component_object),
                ]
            )
        run(
            [
                arguments.cc,
                *COMMON_FLAGS,
                "-fuse-ld=lld",
                f"-Wl,-T,{RUNTIME / 'c' / 'component-aarch64.ld'}",
                "-Wl,--build-id=none",
                str(start_object),
                str(runtime_object),
                *[str(component_object) for component_object in component_objects],
                "-o",
                str(output),
            ]
        )
    print(output)


if __name__ == "__main__":
    main()
