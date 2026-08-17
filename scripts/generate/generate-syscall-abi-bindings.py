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
from zutai_cli import STDLIB, binary

from harness import ROOT

GENERATOR = ROOT / "contracts" / "syscall-abi" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "components" / "proto" / "src" / "syscall_abi.rs"
# `docs/syscall-abi.md`'s operation table is *verified* against the contract
# rather than generated from it: the doc's rows carry per-operation operand
# layouts and result conventions that the ABI declaration does not model, so
# generating the table would delete real documentation. Checking it instead
# still removes the manual coupling `AGENTS.md` invariant 4 describes — a
# renumbering that misses the doc now fails a gate instead of going unnoticed.
SYSCALL_ABI_DOC = ROOT / "docs" / "syscall-abi.md"
INVALID_SCHEMA = "INVALID_SYSCALL_ABI_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-syscall-abi-bindings-") as temporary:
        staging = Path(temporary)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_SYSCALL_ABI_BINDINGS_ROOT"] = str(staging)
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
        rust = staging / "syscall_abi.rs"
        markdown = staging / "syscall-abi.md"
        if not rust.exists() or not markdown.exists():
            raise SystemExit("syscall-abi generator did not write both bindings")
        rust_text = rust.read_text(encoding="utf-8")
        markdown_text = markdown.read_text(encoding="utf-8")
        if INVALID_SCHEMA in rust_text or INVALID_SCHEMA in markdown_text:
            raise SystemExit("syscall-abi schema validation failed")
        return rust_text, markdown_text


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


def declared_labels(markdown: str) -> dict[int, str]:
    """The `label -> operation` pairs the contract declared, from its own table."""
    labels: dict[int, str] = {}
    for row in markdown.splitlines():
        match = re.match(r"\|\s*(\d+)\s*\|\s*`([^`]+)`\s*\|\s*`([^`]+)`\s*\|", row)
        if match:
            labels[int(match.group(1))] = match.group(3)
    if not labels:
        raise SystemExit("syscall-abi generator produced no operation rows to check")
    return labels


def check_doc(expected: dict[int, str]) -> None:
    """Every contract label must appear in the doc's operation table, and the
    doc must not document a label the contract does not declare.

    The doc's rows carry operand and result columns the contract does not model,
    so only the label column is compared. A renumbering that misses the doc, or
    a doc row for a retired operation, fails here."""
    text = SYSCALL_ABI_DOC.read_text(encoding="utf-8")
    section = text.partition("## Root service operations")[2].partition("\n## ")[0]
    if not section:
        raise SystemExit(f"{SYSCALL_ABI_DOC.name} has no root service operations section")
    documented = {
        int(match.group(1))
        for match in re.finditer(r"^\|\s*(\d+)\s*\|", section, flags=re.MULTILINE)
    }
    missing = sorted(set(expected) - documented)
    if missing:
        detail = ", ".join(f"{label} (`{expected[label]}`)" for label in missing)
        raise SystemExit(
            f"{SYSCALL_ABI_DOC.name} does not document declared operations: {detail}"
        )
    extra = sorted(documented - set(expected))
    if extra:
        raise SystemExit(
            f"{SYSCALL_ABI_DOC.name} documents labels the contract does not declare: "
            f"{', '.join(str(label) for label in extra)}"
        )
    print(f"docs/syscall-abi.md documents all {len(expected)} declared operations")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    rust_text, markdown_text = render()
    rust_text = format_rust(rust_text)
    check_doc(declared_labels(markdown_text))
    if arguments.check:
        if not RUST_OUTPUT.exists() or RUST_OUTPUT.read_text(encoding="utf-8") != rust_text:
            raise SystemExit(
                f"generated {RUST_OUTPUT.relative_to(ROOT)} is stale; run `just syscall_abi_gen`"
            )
        print("Syscall ABI bindings are current")
        return
    write_atomic(RUST_OUTPUT, rust_text)
    print(f"Generated {RUST_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
