#!/usr/bin/env python3

"""Regenerate `contracts/system-image-closure/v1/inputs/sdk-release.json`.

The closure corpus needs an SDK release record naming the exact seL4 prefixes
this commit's `sel4/pins.toml` pins, for *every* profile the record advertises.
A consumer selecting a profile receives that profile's prefix archive, so a
profile whose kernel or kernel-configuration hash lags the pinned kernel hands
out a prefix the product no longer builds against. That is not hypothetical:
this file advertised the pre-19-bit-CNode RV64 kernel while `sel4/pins.toml`
pinned the rebuilt one, because the record was previously updated by hand and
only the AArch64 half was touched.

The record cannot be produced by `component_sdk.export` unmodified. That path
also exports a *system corpus*, and exporting one compiles a closure that
declares this very file as an input, so a real `systems` row cannot exist in
the file the closure reads. The row is stubbed here, exactly as the committed
record carries it, and every other field -- prefix identities, contract and
crate identities, and the compatibility axes derived from them -- is exported
normally, so it stays derived from the tree rather than hand-edited.

`export_prefix_asset` verifies each installed prefix against `sel4/pins.toml`
before packaging it, so a profile whose prefix has not been rebuilt for the
current pins is refused rather than recorded.

`--check` refuses a stale committed record without writing, which is what the
closure gates need; the bare invocation rewrites it.
"""

from __future__ import annotations

import argparse
import json
import shutil
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
import component_sdk_system  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402

OUTPUT = ROOT / "contracts" / "system-image-closure" / "v1" / "inputs" / "sdk-release.json"

# The published release this corpus tracks. The record describes the SDK a
# consumer resolves, so its version and source commit stay the published
# release's; what this script keeps current are the build inputs that release's
# profiles actually have in this tree.
VERSION = "3.1.0"
SOURCE_COMMIT = "c42ae22fc2178e906634879a07d51f7fdc1cd017"
SOURCE_REPOSITORY = "git@github.com:iceice666/slime_os"
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"

# Every profile the corpus advertises. Both are QEMU references this
# repository builds and pins. Adding one here without an installed prefix
# matching this commit's pins is a refusal, not a stale entry.
PROFILES = ("aarch64-sel4-qemu-virt", "riscv64-sel4-qemu-virt")

# The stubbed system row, for the reason in the module docstring.
# `component_sdk.export` derives `files` from the row's archive path, so the
# placeholder appears twice in the record and must be reproduced, not removed.
PLACEHOLDER_SYSTEM = {
    "name": "placeholder",
    "archive": "placeholder",
    "archiveHash": "0" * 64,
    "closureIdentity": "0" * 64,
    "testRunIdentity": "0" * 64,
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"closure SDK release refresh: {message}")


def render() -> str:
    """Export into a scratch tree and return the normalized release record."""
    original = component_sdk_system.export_asset
    component_sdk_system.export_asset = lambda *_, **__: dict(PLACEHOLDER_SYSTEM)
    staging = Path(tempfile.mkdtemp(prefix="slime-closure-sdk-release-"))
    try:
        destination = staging / "sdk"
        component_sdk.export(
            destination,
            version=VERSION,
            sdk_repository=SDK_REPOSITORY,
            profiles=PROFILES,
            source=ROOT,
            prefix_source=ROOT,
            commit=SOURCE_COMMIT,
            repository=SOURCE_REPOSITORY,
        )
        return (destination / "component-sdk-release.json").read_text(encoding="utf-8")
    except ComponentSdkError as error:
        fail(str(error))
    finally:
        component_sdk_system.export_asset = original
        shutil.rmtree(staging, ignore_errors=True)


def drifted_profiles(committed: str, rendered: str) -> list[str]:
    """Which profiles' prefix identities moved, for a precise refusal."""
    try:
        before = {
            entry["profile"]: entry["prefix"] for entry in json.loads(committed)["profiles"]
        }
        after = {entry["profile"]: entry["prefix"] for entry in json.loads(rendered)["profiles"]}
    except (KeyError, TypeError, json.JSONDecodeError):
        return []
    return sorted(name for name, prefix in after.items() if before.get(name) != prefix)


def main() -> None:
    parser = argparse.ArgumentParser(description="Refresh the closure corpus SDK release record")
    parser.add_argument(
        "--check",
        action="store_true",
        help="refuse a stale committed record instead of rewriting it",
    )
    arguments = parser.parse_args()

    rendered = render()
    relative = OUTPUT.relative_to(ROOT)
    script = Path(__file__).relative_to(ROOT)
    if arguments.check:
        if not OUTPUT.is_file():
            fail(f"{relative} does not exist; run python3 {script}")
        committed = OUTPUT.read_text(encoding="utf-8")
        if committed != rendered:
            drifted = drifted_profiles(committed, rendered)
            detail = f" (prefix drift: {', '.join(drifted)})" if drifted else ""
            fail(f"{relative} is stale{detail}; run python3 {script}")
        print(f"{relative} is current")
        return

    OUTPUT.write_text(rendered, encoding="utf-8")
    print(f"wrote {relative}")


if __name__ == "__main__":
    main()
