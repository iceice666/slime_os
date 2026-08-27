#!/usr/bin/env python3
"""Exercise the bounded Slisp core on the host and as an external seL4 component."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from component_spec import admit_specs  # noqa: E402
from harness import load_qemu_profile, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
BUILD_COMPONENT = ROOT / "scripts" / "build" / "build-c-component.py"
BUILD_IMAGE = ROOT / "scripts" / "build" / "build-sel4.py"
SLISP_ROOT = ROOT / "components" / "slisp"
IMAGE = ROOT / "build" / "slime-sel4-slisp.elf"
IDENTITY = ROOT / "build" / "slime-sel4-slisp.identity.json"
IMPLEMENTATION = "slisp-external"
READY = "[slisp] repl done"
TERMINAL = "SLIME_GRAPH HEALTHY generation=43 required=1 live=0 completed=1 failed=0"
TIMEOUT = 240


def fail(message: str) -> NoReturn:
    raise SystemExit(f"Slisp core check: {message}")

def zti(value: object, indent: int = 0) -> str:
    padding = " " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        if not value:
            return "[]"
        return "[\n" + "".join(
            f"{padding}  {zti(item, indent + 2)};\n" for item in value
        ) + padding + "]"
    if isinstance(value, dict):
        return "{\n" + "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n"
            for key, item in value.items()
        ) + padding + "}"
    raise TypeError(type(value))


def write_specs(root: Path, digest: str) -> None:
    root.mkdir()
    for entry in admit_specs():
        (root / f"{entry.name}.zti").write_text(zti(entry.spec) + "\n", encoding="utf-8")
    slisp = {
        "formatVersion": 1,
        "name": "slisp",
        "componentType": "init",
        "version": "1.0.0",
        "owner": "root",
        "purpose": "Runs the bounded pure S-expression Slisp reader and lexical evaluator as a non-Rust Slime component.",
        "implementation": {
            "provider": "external",
            "binary": IMPLEMENTATION,
            "contentHash": digest,
        },
        "provides": ["supervision"],
        "requires": ["input"],
        "interfaces": [],
        "dependencies": [],
        "communication": {"semantic": "none", "qos": []},
        "configuration": [],
        "lifecycle": ["Initialize", "Start", "Ready", "Running", "Stop", "Error"],
        "runtime": {
            "executionEnvironment": "aarch64-sel4-qemu-virt",
            "resource": {
                "stackBytes": 65536,
                "spawnBudget": 0,
                "extraThreads": 0,
                "bufferBytePages": 0,
                "bufferCount": 0,
                "mappingCount": 0,
                "loanCount": 0,
                "privatePageQuota": 0,
            },
            "devices": ["input"],
        },
        "health": "required",
        "compatibility": {
            "platform": "aarch64-sel4-qemu-virt",
            "interface": "contracts/interface-schema/v1",
            "dependency": "none",
            "resource": "atMost",
            "runtime": "exact",
            "qos": "none",
        },
        "test": {
            "testCondition": "the slisp_core_check gate runs host vectors and drives the external Slisp REPL",
            "expectedResult": "the reader, lexical closures, persistent definitions, evaluation, and structural refusals behave identically",
            "passFailCriteria": READY,
            "requiredTestEnvironment": "slisp_core_check",
        },
    }
    (root / "slisp.zti").write_text(zti(slisp) + "\n", encoding="utf-8")


def run_host_vectors(output: Path) -> None:
    compiler = shutil.which("cc")
    if compiler is None:
        fail("cc is not on PATH")
    result = subprocess.run(
        [
            compiler,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            str(SLISP_ROOT / "slisp.c"),
            str(SLISP_ROOT / "host_main.c"),
            "-o",
            str(output),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        fail(f"host build failed: {result.stderr.strip()}")
    result = subprocess.run([str(output)], cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode != 0 or result.stdout.strip() != (
        "Slisp core: 15 behavior vectors passed\n"
        "Slisp session: persistent define passed\n"
        "Slisp effects: spawn selection passed"
    ):
        fail(f"host behavior vectors failed: {result.stderr.strip() or result.stdout.strip()}")

def build() -> None:
    with tempfile.TemporaryDirectory(prefix="slisp-core-check-") as temporary:
        root = Path(temporary)
        run_host_vectors(root / "slisp-host")
        elf = root / "slisp.elf"
        component = subprocess.run(
            [
                sys.executable,
                str(BUILD_COMPONENT),
                str(SLISP_ROOT / "slisp.c"),
                str(SLISP_ROOT / "main.c"),
                str(elf),
            ],
            cwd=ROOT,
            check=False,
        )
        if component.returncode != 0 or not elf.is_file():
            fail("Slisp component build failed")
        digest = hashlib.sha256(elf.read_bytes()).hexdigest()
        specs = root / "specs"
        write_specs(specs, digest)
        image = subprocess.run(
            [
                sys.executable,
                str(BUILD_IMAGE),
                "--slisp-plane",
                "--component-spec-root",
                str(specs),
                "--external-component",
                f"{IMPLEMENTATION}={elf}",
            ],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
        )
        if image.returncode != 0 or not IMAGE.is_file() or not IDENTITY.is_file():
            fail("Slisp image build failed")
    identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
    if identity.get("variant") != "slisp":
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    process = subprocess.Popen(
        [
            qemu,
            "-machine",
            str(profile["machine"]),
            "-cpu",
            str(profile["cpu"]),
            "-smp",
            str(profile["cpus"]),
            "-m",
            f"size={profile['memory_mib']}M",
            "-nographic",
            "-serial",
            "mon:stdio",
            "-kernel",
            str(IMAGE),
        ],
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(re.escape(TERMINAL) + r"|SLIME_ROOT FATAL|SLIME_GRAPH FAIL")
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    if timed_out:
        fail("QEMU timed out")
    return "\n".join(lines)


def main() -> None:
    build()
    transcript = boot(load_qemu_profile(fail))
    markers = (
        "Slisp",
        "slisp> ",
        "=> 40",
        "=> 42",
        "! arity",
        READY,
        TERMINAL,
    )
    for marker in markers:
        if marker not in transcript:
            print(transcript)
            fail(f"missing marker {marker!r}")
    for marker in ("! input", "SLIME_ROOT FATAL", "SLIME_GRAPH FAIL"):
        if marker in transcript:
            print(transcript)
            fail(f"failure marker {marker!r}")
    print(
        "Slisp core check: host vectors and the freestanding seL4 REPL proved "
        "the bounded reader, lexical evaluator, persistent definitions, and refusals"
    )


if __name__ == "__main__":
    main()
