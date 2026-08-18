#!/usr/bin/env python3

"""Derive `contracts/generation/v1` fixtures from their `system-spec` sources.

CP1's deliverable: the manifest sections that were hand-authored in parallel with
the component model are now generated from it. `--check` fails on drift, so a
fixture edited by hand instead of regenerated is a gate failure rather than a
silent fork — the same discipline `generate-interface-schema-bindings.py` and the
rest of the `scripts/generate/` family already apply.

Which fixture each system derives is declared by `scripts/lib/system_spec.py`'s
`DERIVED_GENERATION_FIXTURES`, so the table has one home and this script cannot
disagree with the gate about what it converts.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import json
import tempfile
from pathlib import Path

from component_spec import admit_specs, interface_catalogue
from harness import ROOT
from system_spec import (
    DERIVED_GENERATION_FIXTURES as DERIVED_FIXTURES,
    GENERATION_FIXTURES,
    compile_system,
    derive_manifest,
    system_paths,
)


def zti(value: object, indent: int = 0) -> str:
    """The `.zti` rendering the fixtures use: one field per line, two-space steps."""
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
        rows = "".join(f"{padding}  {zti(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def render() -> dict[Path, str]:
    catalogue = interface_catalogue()
    components = {entry.name: entry.spec for entry in admit_specs(catalogue=catalogue)}
    outputs: dict[Path, str] = {}
    for path in system_paths():
        fixture = DERIVED_FIXTURES.get(path.stem)
        if fixture is None:
            raise SystemExit(
                f"{path.name} derives no declared fixture; add it to "
                "check-system-spec.py's DERIVED_FIXTURES"
            )
        system = compile_system(path, components=components)
        outputs[GENERATION_FIXTURES / fixture] = zti(derive_manifest(system)) + "\n"
    return outputs


def write_atomic(path: Path, contents: str) -> None:
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
    parser.add_argument("--check", action="store_true", help="fail if a fixture is stale")
    arguments = parser.parse_args()
    outputs = render()
    if arguments.check:
        stale = [
            path
            for path, contents in outputs.items()
            if not path.is_file() or path.read_text(encoding="utf-8") != contents
        ]
        if stale:
            raise SystemExit(
                "stale derived generation fixture(s): "
                + ", ".join(str(path.relative_to(ROOT)) for path in stale)
                + "; run python3 scripts/generate/generate-generation-from-spec.py"
            )
        for path in outputs:
            print(f"{path.relative_to(ROOT)} is current")
        return
    for path, contents in outputs.items():
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
