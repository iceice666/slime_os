#!/usr/bin/env python3
"""Build and boot one external C component against the generated runtime ABI."""

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
SOURCE = ROOT / "components" / "c-runtime-probe" / "main.c"
IMAGE = ROOT / "build" / "slime-sel4-c-runtime.elf"
IDENTITY = ROOT / "build" / "slime-sel4-c-runtime.identity.json"
IMPLEMENTATION = "c-runtime-probe-external"
READY = "[c-runtime-probe] C component ready"
TERMINAL = "SLIME_GRAPH HEALTHY generation=42 required=1 live=0 completed=1 failed=0"
TIMEOUT = 240


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 C runtime check: {message}")


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
    probe = {
        "formatVersion": 1,
        "name": "c-runtime-probe",
        "componentType": "init",
        "version": "1.0.0",
        "owner": "root",
        "purpose": "Proves a freestanding C executable can enter the generated Slime component ABI, write to the console endpoint, and exit through the lifecycle service.",
        "implementation": {
            "provider": "external",
            "binary": IMPLEMENTATION,
            "contentHash": digest,
        },
        "provides": ["supervision"],
        "requires": [],
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
            "devices": [],
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
            "testCondition": "the sel4_c_runtime_check gate builds and boots the external C component",
            "expectedResult": "the C component invokes generated ABI labels and exits cleanly",
            "passFailCriteria": READY,
            "requiredTestEnvironment": "sel4_c_runtime_check",
        },
    }
    (root / "c-runtime-probe.zti").write_text(zti(probe) + "\n", encoding="utf-8")


def build() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-c-runtime-check-") as temporary:
        root = Path(temporary)
        elf = root / "c-runtime-probe.elf"
        component = subprocess.run(
            [sys.executable, str(BUILD_COMPONENT), str(SOURCE), str(elf)],
            cwd=ROOT,
            check=False,
        )
        if component.returncode != 0 or not elf.is_file():
            fail("C component build failed")
        digest = hashlib.sha256(elf.read_bytes()).hexdigest()
        specs = root / "specs"
        write_specs(specs, digest)
        image = subprocess.run(
            [
                sys.executable,
                str(BUILD_IMAGE),
                "--c-runtime-plane",
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
            fail("C runtime image build failed")
    identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
    if identity.get("variant") != "c-runtime":
        fail(f"wrong image variant {identity.get('variant')!r}")
    if identity.get("target_profile") != "aarch64-sel4-qemu-virt":
        fail(f"wrong target profile {identity.get('target_profile')!r}")
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
    for marker in (READY, TERMINAL):
        if marker not in transcript:
            print(transcript)
            fail(f"missing marker {marker!r}")
    for marker in ("SLIME_ROOT FATAL", "SLIME_GRAPH FAIL"):
        if marker in transcript:
            print(transcript)
            fail(f"failure marker {marker!r}")
    print(
        "seL4 C runtime check: an external freestanding C ELF entered through the "
        "generated component ABI, wrote through the console endpoint, and exited cleanly"
    )


if __name__ == "__main__":
    main()
