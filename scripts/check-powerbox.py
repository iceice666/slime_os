#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MARKERS = [
    "[powerbox] scripted check active",
    "[powerbox] chooser ready",
    "[powerbox-probe] manifest directory absent",
    "[powerbox] request kind=file purpose=Open the selected note",
    "[powerbox-provenance] event=1 gesture=select kind=file path=note rights=0x00080004 purpose=Open the selected note",
    "[powerbox-probe] selected single object received",
    "[powerbox] derive closure denied",
    "[powerbox-probe] derive closure enforced",
    "[powerbox] request kind=file purpose=Cancel this selection",
    "[powerbox] selection cancelled",
    "[powerbox-probe] cancellation minted nothing",
    "[powerbox-probe] done",
    "[init] powerbox scenario complete",
]


def run() -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "9"
    environment["SLIME_POWERBOX_CHECK"] = "1"
    process = subprocess.run(
        ["cargo", "run", "--release", "--", "-display", "none"],
        cwd=ROOT / "kernel",
        timeout=90,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        print(process.stdout, end="")
        raise SystemExit(process.returncode)
    cursor = 0
    for marker in MARKERS:
        position = process.stdout.find(marker, cursor)
        if position < 0:
            print(process.stdout, end="")
            raise SystemExit(f"powerbox transcript is missing or out of order at: {marker}")
        cursor = position + len(marker)
    if "[powerbox] chooser complete" not in process.stdout:
        print(process.stdout, end="")
        raise SystemExit("powerbox chooser did not complete")
    return process.stdout
def transcript(output: str) -> str:
    return "\n".join(
        line
        for line in output.splitlines()
        if any(marker in line for marker in MARKERS)
        or "[powerbox] chooser complete" in line
    )


def main() -> None:
    first = transcript(run())
    second = transcript(run())
    if first != second:
        raise SystemExit("powerbox scripted transcript is not deterministic")
    print("powerbox capability check: ok")


if __name__ == "__main__":
    main()
