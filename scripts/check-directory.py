#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MARKERS = [
    "[directory-probe] no-cap denied",
    "[directory-probe] scoped read ok",
    "[directory-probe] interrupted transition preserved root",
    "[directory-probe] root transition committed",
    "[directory-probe] derive narrowed",
    "[directory-probe] scoped boundary enforced",
    "[directory-probe] malformed rejected",
    "[directory-probe] done",
]


def run(image: Path) -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "6"
    process = subprocess.run(
        [
            "cargo",
            "run",
            "--release",
            "--",
            "-display",
            "none",
            "-drive",
            f"if=none,id=slime-storage,format=raw,cache=directsync,file={image}",
            "-device",
            "virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8",
        ],
        cwd=ROOT / "kernel",
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    sys.stdout.write(process.stdout)
    if process.returncode != 0:
        raise SystemExit(process.returncode)
    missing = [marker for marker in MARKERS if marker not in process.stdout]
    if missing:
        raise SystemExit(f"directory check missing markers: {missing}")
    return process.stdout


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: check-directory.py <image>")
    image = Path(sys.argv[1])
    subprocess.run([ROOT / "scripts" / "build-directory-fixture.py", image], check=True)
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    run(image)
    after_first = hashlib.sha256(image.read_bytes()).hexdigest()
    if after_first == before:
        raise SystemExit("directory namespace transition did not commit")
    run(image)
    if hashlib.sha256(image.read_bytes()).hexdigest() != after_first:
        raise SystemExit("idempotent directory transition rewrote the store")
    print("directory capability and namespace check: ok")


if __name__ == "__main__":
    main()
