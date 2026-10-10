"""Operator entrypoint for the pinned devloop, separate from grader helpers."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile

import devloop
from harness import ROOT


def main(arguments: list[str]) -> int:
    findings = devloop.check_toolchain()
    if findings:
        for finding in findings:
            print(f"devloop: {finding}")
        return 1
    environment = devloop._environment()
    # The pinned CLI captures adapter stderr. Relay only diagnostic locations
    # and capture errors, never observation stdout or recipe content.
    if arguments and arguments[0] == "gate":
        with tempfile.TemporaryDirectory(prefix="slime-devloop-relay-") as temporary:
            relay = os.path.join(temporary, "messages")
            environment["SLIME_DEVLOOP_DIAGNOSTIC_RELAY"] = relay
            finished = subprocess.run([devloop.executable(), *arguments], cwd=ROOT, env=environment)
            try:
                with open(relay, encoding="utf-8") as messages:
                    sys.stderr.write(messages.read(16384))
            except FileNotFoundError:
                pass
    else:
        finished = subprocess.run(
            [devloop.executable(), *(arguments or ["--help"])], cwd=ROOT, env=environment
        )
    return finished.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
