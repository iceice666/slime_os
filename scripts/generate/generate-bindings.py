#!/usr/bin/env python3

"""Render, or with `--check` verify, the bindings named in `zutai_bindings.BINDINGS`.

    generate-bindings.py NAME...          write each binding's outputs
    generate-bindings.py --check NAME...  fail if any committed output is stale
    generate-bindings.py --check --all    every binding in the table
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import zutai_bindings  # noqa: E402


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("names", nargs="*", metavar="NAME")
    parser.add_argument("--all", action="store_true", help="every binding in the table")
    parser.add_argument("--check", action="store_true", help="fail if a committed output is stale")
    arguments = parser.parse_args(argv)
    if arguments.all == bool(arguments.names):
        parser.error("name one or more bindings, or pass --all")
    names = sorted(zutai_bindings.BINDINGS) if arguments.all else arguments.names
    if arguments.check:
        stale = [path for name in names for path in zutai_bindings.stale(name)]
        if stale:
            raise SystemExit(
                "stale generated bindings: "
                + ", ".join(stale)
                + "; run python3 scripts/generate/generate-bindings.py "
                + " ".join(names)
            )
        print(f"{len(names)} binding(s) current: {', '.join(names)}")
        return
    for name in names:
        for path in zutai_bindings.write(name):
            print(f"Generated {path}")


if __name__ == "__main__":
    main()
