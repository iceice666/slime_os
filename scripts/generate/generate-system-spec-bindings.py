#!/usr/bin/env python3

"""Render `contracts/system-spec/v1`'s host constants.

The contract's own `gen_python.zt` is the renderer; this script runs it into a
staging directory, compares, and writes atomically, so `--check` can fail on
drift without leaving a partially written binding behind. Mirrors
`generate-interface-schema-bindings.py`, which is the working precedent for a
Python-only binding in this repository.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import fcntl
import os
import subprocess
import tempfile
from contextlib import contextmanager
from pathlib import Path

from harness import ROOT
from zutai_cli import STDLIB, binary

CONTRACT_GENERATOR = ROOT / "contracts" / "system-spec" / "v1" / "gen_python.zt"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "system_spec_contract.py"
LOCK_PATH = Path(tempfile.gettempdir()) / "slime-system-spec-bindings.lock"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-system-spec-bindings-") as temporary:
        staging = Path(temporary)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_SYSTEM_SPEC_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(CONTRACT_GENERATOR)],
            cwd=ROOT,
            env=environment,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if process.returncode != 0:
            raise SystemExit(
                "system-spec binding generator failed: "
                f"{(process.stderr or process.stdout).strip()}"
            )
        output = staging / "system_spec_contract.py"
        if not output.is_file():
            raise SystemExit("system-spec binding generator wrote no output")
        return output.read_text(encoding="utf-8")


@contextmanager
def _output_lock():
    with LOCK_PATH.open("w", encoding="utf-8") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        yield


def _write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, delete=False, suffix=".tmp"
    )
    temporary = Path(handle.name)
    try:
        with handle:
            handle.write(contents)
        temporary.replace(path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if the bindings are stale")
    arguments = parser.parse_args()
    rendered = render()
    with _output_lock():
        if arguments.check:
            existing = PYTHON_OUTPUT.read_text(encoding="utf-8") if PYTHON_OUTPUT.is_file() else ""
            if existing != rendered:
                raise SystemExit(
                    f"{PYTHON_OUTPUT.relative_to(ROOT)} is stale; "
                    "run python3 scripts/generate/generate-system-spec-bindings.py"
                )
            print(f"{PYTHON_OUTPUT.relative_to(ROOT)} is current")
            return
        _write_atomic(PYTHON_OUTPUT, rendered)
        print(f"Generated {PYTHON_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
