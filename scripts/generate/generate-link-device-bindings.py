#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from zutai_cli import STDLIB, binary

from harness import ROOT

GENERATOR = ROOT / "contracts" / "link-device" / "v1" / "schema.zt"
OUTPUT = ROOT / "components" / "proto" / "src" / "link_device.rs"
INVALID_SCHEMA = "INVALID_LINK_DEVICE_SCHEMA"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-link-device-bindings-") as temporary:
        staging = Path(temporary)
        staged = staging / "components" / "proto" / "src" / "link_device.rs"
        staged.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_LINK_DEVICE_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(GENERATOR)],
            cwd=ROOT,
            env=environment,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if process.returncode != 0:
            sys.stderr.write(process.stdout)
            sys.stderr.write(process.stderr)
            raise SystemExit(process.returncode)
        if not staged.exists():
            raise SystemExit("link-device generator did not write bindings")
        generated = staged.read_text(encoding="utf-8")
        if INVALID_SCHEMA in generated:
            raise SystemExit("link-device schema reflection/layout validation failed")
        return generated


def format_rust(source: str) -> str:
    process = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    return process.stdout


def write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(contents, encoding="utf-8")
    temporary.replace(path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    generated = format_rust(render())
    if arguments.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit(
                "generated link-device bindings are stale; run "
                "`python3 scripts/generate/generate-link-device-bindings.py`"
            )
        print("LinkDevice protocol bindings are current")
        return
    write_atomic(OUTPUT, generated)
    print(f"Generated {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
