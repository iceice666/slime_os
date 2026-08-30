#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from harness import ROOT
from zutai_cli import STDLIB, binary

SCHEMA = ROOT / "contracts" / "component-runtime-abi" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "boot-contracts" / "src" / "generated" / "component_runtime_abi.rs"
C_OUTPUT = ROOT / "components" / "runtime" / "include" / "slime" / "component_runtime_abi.h"
# This contract owns the *console* half of the component ABI's label numbering,
# so it owns the doc check for that table. The syscall-abi generator checks only
# `## Root service operations`, which is why B83's console renumbering left the
# doc's rows wrong with every gate green.
ABI_DOC = ROOT / "docs" / "syscall-abi.md"
INVALID_SCHEMA = "INVALID_COMPONENT_RUNTIME_ABI_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-component-runtime-abi-") as temporary:
        staging = Path(temporary)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_COMPONENT_RUNTIME_ABI_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(SCHEMA)],
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
        rust = staging / "component_runtime_abi.rs"
        c = staging / "component_runtime_abi.h"
        if not rust.exists() or not c.exists():
            raise SystemExit("component-runtime-abi generator did not write Rust and C bindings")
        rust_text = rust.read_text(encoding="utf-8")
        c_text = c.read_text(encoding="utf-8")
        if INVALID_SCHEMA in rust_text or INVALID_SCHEMA in c_text:
            raise SystemExit("component-runtime-abi schema validation failed")
        return rust_text, c_text


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


def declared_console_labels(rust: str) -> dict[int, str]:
    """The `label -> operation` pairs the contract declared, from its own output.

    The generated `console_labels` module is the contract's numbering after
    rendering, so reading it here checks the doc against the same bytes every
    Rust consumer imports rather than against a second parse of the schema."""
    module = rust.partition("pub mod console_labels {")[2].partition("\n}")[0]
    if not module:
        raise SystemExit("component-runtime-abi bindings declare no console_labels module")
    labels = {
        int(match.group(2)): match.group(1)
        for match in re.finditer(
            r"pub const ([A-Z0-9_]+)\s*:\s*u64\s*=\s*(\d+)\s*;", module
        )
    }
    if not labels:
        raise SystemExit("component-runtime-abi bindings declare no console operations")
    return labels


def check_doc(expected: dict[int, str]) -> None:
    """Every console label must appear in the doc's console table, and the doc
    must not document a label the contract does not declare.

    B83 renumbered this table when `BLOCK_TRANSACT` was deleted and the doc kept
    the pre-cutover numbers for months, because the syscall-abi gate reads only
    the *root* service section. The doc spells an operation with spaces where the
    contract uses underscores, so names are compared in the contract's spelling
    and the label is compared exactly."""
    text = ABI_DOC.read_text(encoding="utf-8")
    section = text.partition("## Console service operations")[2].partition("\n## ")[0]
    if not section:
        raise SystemExit(f"{ABI_DOC.name} has no console service operations section")
    documented = {
        int(match.group(1)): match.group(2).replace(" ", "_")
        for match in re.finditer(
            r"^\|\s*(\d+)\s*\|\s*`([^`]+)`\s*\|", section, flags=re.MULTILINE
        )
    }
    missing = sorted(set(expected) - set(documented))
    if missing:
        detail = ", ".join(f"{label} (`{expected[label]}`)" for label in missing)
        raise SystemExit(f"{ABI_DOC.name} does not document console operations: {detail}")
    extra = sorted(set(documented) - set(expected))
    if extra:
        raise SystemExit(
            f"{ABI_DOC.name} documents console labels the contract does not declare: "
            f"{', '.join(str(label) for label in extra)}"
        )
    mismatched = sorted(
        label for label, name in expected.items() if documented[label] != name
    )
    if mismatched:
        detail = ", ".join(
            f"{label}: contract `{expected[label]}` vs doc `{documented[label]}`"
            for label in mismatched
        )
        raise SystemExit(f"{ABI_DOC.name} misnames console operations: {detail}")
    print(f"docs/syscall-abi.md documents all {len(expected)} console operations")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    rust_text, c_text = render()
    rust_text = format_rust(rust_text)
    check_doc(declared_console_labels(rust_text))
    outputs = ((RUST_OUTPUT, rust_text), (C_OUTPUT, c_text))
    if arguments.check:
        for path, contents in outputs:
            if not path.exists() or path.read_text(encoding="utf-8") != contents:
                raise SystemExit(
                    f"generated {path.relative_to(ROOT)} is stale; "
                    "run `just component_runtime_abi_gen`"
                )
        print("Component runtime ABI bindings are current")
        return
    for path, contents in outputs:
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
