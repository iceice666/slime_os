#!/usr/bin/env python3

"""Render `contracts/composition-inventory/v1`'s host constants.

Same shape as `generate-system-spec-bindings.py`: the contract's own
`gen_python.zt` is the renderer, this script only runs it into
`scripts/lib/` and, under `--check`, refuses a stale committed copy.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import subprocess
import tempfile
from pathlib import Path

from harness import ROOT
from zutai_cli import STDLIB, binary

CONTRACT_GENERATOR = ROOT / "contracts" / "composition-inventory" / "v1" / "gen_python.zt"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "composition_inventory_contract.py"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-composition-inventory-") as staging_root:
        staging = Path(staging_root)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_COMPOSITION_INVENTORY_BINDINGS_ROOT"] = str(staging)
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
                "composition-inventory binding generator failed: "
                f"{(process.stderr or process.stdout).strip()}"
            )
        output = staging / "composition_inventory_contract.py"
        if not output.is_file():
            raise SystemExit("composition-inventory binding generator wrote no output")
        return output.read_text(encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if the committed copy is stale")
    arguments = parser.parse_args()
    rendered = render()
    if arguments.check:
        current = PYTHON_OUTPUT.read_text(encoding="utf-8") if PYTHON_OUTPUT.is_file() else ""
        if current != rendered:
            raise SystemExit(
                f"{PYTHON_OUTPUT.relative_to(ROOT)} is stale; "
                "run python3 scripts/generate/generate-composition-inventory-bindings.py"
            )
        print(f"{PYTHON_OUTPUT.relative_to(ROOT)} is current")
        return
    PYTHON_OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"Generated {PYTHON_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
