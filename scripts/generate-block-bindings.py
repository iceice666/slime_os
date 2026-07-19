#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ZUTAI_MANIFEST = ROOT / "deps" / "zutai" / "Cargo.toml"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
GENERATOR = ROOT / "contracts" / "block" / "v1" / "gen_rust.zt"
RUST_OUTPUT = ROOT / "kernel" / "src" / "block_proto" / "gen.rs"
COMPONENT_OUTPUT = ROOT / "components" / "include" / "block_proto.inc"
INVALID_SCHEMA = "INVALID_BLOCK_SCHEMA"


def render() -> dict[str, str]:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [
            "cargo",
            "run",
            "--manifest-path",
            str(ZUTAI_MANIFEST),
            "-q",
            "-p",
            "zutai-cli",
            "--",
            "json",
            str(GENERATOR),
        ],
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

    try:
        generated = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"block generator returned invalid record envelope: {error}") from error
    if not isinstance(generated, dict):
        raise SystemExit("block generator did not return a binding record")
    if set(generated) != {"rust", "component"} or not all(
        isinstance(value, str) for value in generated.values()
    ):
        raise SystemExit("block generator returned malformed bindings")
    if INVALID_SCHEMA in generated.values():
        raise SystemExit("block schema reflection/layout validation failed")
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
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail when the checked-in Rust bindings are stale",
    )
    arguments = parser.parse_args()
    rendered = render()
    generated = {
        RUST_OUTPUT: format_rust(rendered["rust"]),
        COMPONENT_OUTPUT: rendered["component"],
    }

    if arguments.check:
        for path, contents in generated.items():
            if not path.exists():
                raise SystemExit(f"missing generated block bindings: {path.relative_to(ROOT)}")
            if path.read_text(encoding="utf-8") != contents:
                raise SystemExit("generated block bindings are stale; run `just block_gen`")
        print("Block protocol bindings are current")
        return

    for path, contents in generated.items():
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
