#!/usr/bin/env python3

"""Render `contracts/boot-media/v1`'s host constants."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

CONTRACT = ROOT / "contracts" / "boot-media" / "v1" / "schema.zt"
OUTPUT = ROOT / "scripts" / "lib" / "boot_media_contract.py"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-boot-media-bindings-") as temporary:
        root = Path(temporary)
        environment = dict(os.environ)
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_BOOT_MEDIA_BINDINGS_ROOT"] = str(root)
        process = subprocess.run(
            [str(binary()), "run", str(CONTRACT)],
            cwd=ROOT,
            env=environment,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if process.returncode != 0:
            raise SystemExit(
                "boot-media binding generation failed:\n"
                + process.stdout
                + process.stderr
            )
        generated = root / OUTPUT.name
        if not generated.is_file():
            raise SystemExit(f"boot-media contract produced no {generated}")
        return generated.read_text(encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    rendered = render()
    if arguments.check:
        if not OUTPUT.is_file() or OUTPUT.read_text(encoding="utf-8") != rendered:
            raise SystemExit(
                f"{OUTPUT.relative_to(ROOT)} is stale; run "
                "python3 scripts/generate/generate-boot-media-bindings.py"
            )
        print(f"{OUTPUT.relative_to(ROOT)} is current")
        return
    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"Generated {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
