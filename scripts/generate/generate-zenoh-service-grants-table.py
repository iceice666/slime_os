#!/usr/bin/env python3

"""Render the wire records of `contracts/zenoh-service-grants-table/v1` as the Rust file the root and the Zenoh service share.

The layouts live in `contracts/zenoh-service-grants-table/v1/schema.zt`. The grants table exam
(`check-zenoh-profile0.py --slice grants-table`) renders the same text with `zenoh_grants_table_exam.render_bindings` and
requires the committed `components/proto/src/zenoh_service_grants_table_wire.rs` to equal it byte for byte, so this script
and the exam cannot disagree about a field offset. Run `just zenoh_service_grants_table_gen` to write the file and
`--check` to compare it without writing.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
from pathlib import Path

from harness import ROOT
import zenoh_grants_table_exam


def write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_bytes(contents.encode("utf-8"))
    temporary.replace(path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    try:
        rendered = zenoh_grants_table_exam.render_bindings()
    except zenoh_grants_table_exam.TableExamError as error:
        raise SystemExit(str(error)) from error
    output = zenoh_grants_table_exam.GENERATED
    if arguments.check:
        if not output.exists() or output.read_bytes() != rendered.encode("utf-8"):
            raise SystemExit(f"generated {output.relative_to(ROOT)} is stale; run `just zenoh_service_grants_table_gen`")
        print("Zenoh service grants table wire records are current")
        return
    write_atomic(output, rendered)
    print(f"Generated {output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
