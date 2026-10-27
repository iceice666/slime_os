#!/usr/bin/env python3

"""Render the constants of `contracts/zenoh-service-client/v1` as the Rust file the Zenoh service and its clients read.

The constants live in `contracts/zenoh-service-client/v1/schema.zt`. The client exam
(`check-zenoh-profile0.py --slice client`) renders the same text with `zenoh_client_exam.render_bindings`
and requires the committed `components/proto/src/zenoh_service_client.rs` to equal it byte for byte, so
this script and the exam cannot disagree about a constant. Run `just zenoh_service_client_gen` to write the
file and `--check` to compare it without writing.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
from pathlib import Path

from harness import ROOT
import zenoh_client_exam


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
        rendered = zenoh_client_exam.render_bindings()
    except zenoh_client_exam.ClientError as error:
        raise SystemExit(str(error)) from error
    output = zenoh_client_exam.GENERATED
    if arguments.check:
        if not output.exists() or output.read_bytes() != rendered.encode("utf-8"):
            raise SystemExit(f"generated {output.relative_to(ROOT)} is stale; run `just zenoh_service_client_gen`")
        print("Zenoh service client constants are current")
        return
    write_atomic(output, rendered)
    print(f"Generated {output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
