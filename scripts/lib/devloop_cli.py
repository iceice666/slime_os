"""Operator entrypoint for the pinned devloop, separate from grader helpers."""

from __future__ import annotations

import subprocess
import sys

import devloop
from harness import ROOT


def main(arguments: list[str]) -> int:
    findings = devloop.check_toolchain()
    if findings:
        for finding in findings:
            print(f"devloop: {finding}")
        return 1
    finished = subprocess.run(
        [devloop.executable(), *(arguments or ["--help"])],
        cwd=ROOT,
        env=devloop._environment(),
    )
    return finished.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
