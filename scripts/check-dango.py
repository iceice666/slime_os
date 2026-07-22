#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MARKERS = [
    "[dango] native runtime ready",
    "dango> $(sysinfo)",
    "resolved:profile",
    "[sysinfo] command=sysinfo args=0 env=0 cwd=none stdin=none",
    "[sysinfo] spawned through profile",
    "spawn-request:accepted",
    "result:exit:0",
    "dango> (with-env {MODE=ci} (with-cwd docs (with-stdin data $(echo ok)))",
    "[echo-agent] command=echo args=1 env=1 cwd=explicit stdin=explicit",
    "echo-agent{tool=echo,value=ok,env=MODE=ci}",
    "spawn-request:accepted",
    "result:exit:0",
    "dango> $(inject)",
    "resolve-denied",
    "dango> $(echo a b c)",
    "parse-error",
    "[dango] interactive session closed",
]


def run() -> str:
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "7"
    environment["SLIME_DANGO_CHECK"] = "1"
    process = subprocess.run(
        ["cargo", "run", "--release", "--", "-display", "none"],
        cwd=ROOT / "kernel",
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        raise SystemExit(process.returncode)
    cursor = 0
    for marker in MARKERS:
        position = process.stdout.find(marker, cursor)
        if position < 0:
            raise SystemExit(f"dango transcript is missing or out of order at: {marker}")
        cursor = position + len(marker)
    return process.stdout


def transcript(output: str) -> str:
    lines = output.splitlines()
    start = next(index for index, line in enumerate(lines) if MARKERS[0] in line)
    end = next(index for index, line in enumerate(lines[start:], start) if MARKERS[-1] in line)
    return "\n".join(lines[start : end + 1])


def main() -> None:
    first = transcript(run())
    second = transcript(run())
    if first != second:
        raise SystemExit("dango scripted transcript is not deterministic")
    print("dango native runtime check: ok")


if __name__ == "__main__":
    main()
