#!/usr/bin/env python3

"""Render `contracts/system-test-run/v1` host constants."""

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

CONTRACT_GENERATOR = ROOT / "contracts" / "system-test-run" / "v1" / "gen_python.zt"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "system_test_run_contract.py"
LOCK_PATH = Path(tempfile.gettempdir()) / "slime-system-test-run-bindings.lock"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-system-test-run-bindings-") as temporary:
        staging = Path(temporary)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_SYSTEM_TEST_RUN_BINDINGS_ROOT"] = str(staging)
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
                "system-test-run binding generator failed: "
                f"{(process.stderr or process.stdout).strip()}"
            )
        output = staging / "system_test_run_contract.py"
        if not output.is_file():
            raise SystemExit("system-test-run binding generator wrote no output")
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
                    "run python3 scripts/generate/generate-system-test-run-bindings.py"
                )
            print(f"{PYTHON_OUTPUT.relative_to(ROOT)} is current")
            return
        _write_atomic(PYTHON_OUTPUT, rendered)
        print(f"Generated {PYTHON_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
