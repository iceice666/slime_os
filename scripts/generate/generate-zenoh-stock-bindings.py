#!/usr/bin/env python3

"""Render the captured stock-peer batches as the Rust constants the `zenoh_stock::` tests read.

The captured bytes live in `scripts/lib/zenoh_stock_peer.py`, which the stock-peer exam
(`check-zenoh-profile0.py --slice stock`) also pins. This script writes them as one record of
`contracts/zenoh-stock-fixtures/v1`, lets that contract's `gen_rust.zt` decode and render it, and
formats the result, so the tests and the exam cannot disagree about a byte.
"""

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
import zenoh_stock_peer

GENERATOR = ROOT / "contracts" / "zenoh-stock-fixtures" / "v1" / "gen_rust.zt"
RUST_OUTPUT = ROOT / "components" / "lib" / "src" / "zenoh_stock" / "fixtures.rs"
INVALID = "INVALID_ZENOH_STOCK_FIXTURES"


def render(source: str | None = None) -> str:
    with tempfile.TemporaryDirectory(prefix="slime-zenoh-stock-bindings-") as temporary:
        staging = Path(temporary)
        record = staging / "fixtures.zti"
        record.write_text(zenoh_stock_peer.fixture_zti() if source is None else source, encoding="utf-8")
        staged = staging / "components" / "lib" / "src" / "zenoh_stock" / "fixtures.rs"
        staged.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_ZENOH_STOCK_BINDINGS_ROOT"] = str(staging)
        environment["SLIME_ZENOH_STOCK_FIXTURES_PATH"] = str(record)
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
            raise SystemExit("zenoh-stock-fixtures generator did not write bindings")
        rust = staged.read_text(encoding="utf-8")
        if INVALID in rust:
            raise SystemExit("zenoh-stock-fixtures record is refused by its contract")
        return rust


def corrupted_records(honest: str) -> list[tuple[str, str]]:
    """Records the contract's constants forbid, each of which the generator must refuse."""
    row = '    { name = "connects-01-init-syn"; hex = "'
    start = honest.index(row) + len(row)
    first = honest[start : honest.index('"', start)]

    def with_first(hexed: str) -> str:
        return honest[:start] + hexed + honest[start + len(first) :]

    cookie = zenoh_stock_peer.COOKIE.encode().hex()
    marker = "  connects = [\n"
    line_start = honest.index(row)
    return [
        ("a twelfth connects row", honest.replace(marker, marker + '    { name = "connects-12-extra"; hex = "04"; };\n', 1)),
        ("a connects row missing", honest[:line_start] + honest[honest.index("\n", line_start) + 1 :]),
        ("non-hex bytes", with_first("zz" + first[2:])),
        ("uppercase hex", with_first(first.upper())),
        ("odd-length hex", with_first(first + "0")),
        ("a batch over 512 bytes", with_first("00" * 513)),
        ("formatVersion 2", honest.replace("formatVersion = 1;", "formatVersion = 2;")),
        ("a cookie of another length", honest.replace(cookie, "00" * 28, 1)),
        ("a non-hex cookie", honest.replace(cookie, "zz" * 29, 1)),
        ("a name over 64 bytes", honest.replace('name = "connects-01-init-syn"', 'name = "' + "n" * 65 + '"', 1)),
    ]


def controls() -> None:
    honest = zenoh_stock_peer.fixture_zti()
    render(honest)
    for label, record in corrupted_records(honest):
        if record == honest:
            raise SystemExit(f"control '{label}' did not change the record")
        try:
            render(record)
        except SystemExit:
            continue
        raise SystemExit(f"generator rendered a record it must refuse: {label}")
    print(f"Zenoh stock fixture controls: {len(corrupted_records(honest))} corrupted records refused")


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
    parser.add_argument("--controls", action="store_true", help="prove the generator refuses corrupted records")
    arguments = parser.parse_args()
    if arguments.controls:
        controls()
        return
    generated = format_rust(render())
    if arguments.check:
        if not RUST_OUTPUT.exists() or RUST_OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit(
                "generated zenoh_stock/fixtures.rs is stale; run `just zenoh_stock_gen`"
            )
        print("Zenoh stock fixtures are current")
        return
    write_atomic(RUST_OUTPUT, generated)
    print(f"Generated {RUST_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
