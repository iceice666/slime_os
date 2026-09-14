#!/usr/bin/env python3

"""Build P6.5's Framework-qualified deterministic GPT/FAT32 image."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import boot_media_contract as contract  # noqa: E402
from framework_media import build_image  # noqa: E402
from harness import ROOT, load_qemu_profile  # noqa: E402
from pc99_media import boot_media  # noqa: E402

BUILD_SEL4 = ROOT / "scripts" / "build" / "build-sel4.py"
PLATFORM = "framework13-ai300"
TREE = ROOT / "build" / "media" / f"{PLATFORM}-graph"
SOURCE_MANIFEST = ROOT / "build" / f"slime-sel4-graph-{PLATFORM}.identity.json"
OUTPUT_DIR = ROOT / "build" / "framework-media"
IMAGE = OUTPUT_DIR / contract.IMAGE_FILE_NAME
RECORD = OUTPUT_DIR / contract.RECORD_FILE_NAME
PINS = ROOT / "sel4" / "pins.toml"


def fail(message: str) -> None:
    raise SystemExit(f"Framework media build: {message}")


def build_source() -> None:
    command = [
        sys.executable,
        str(BUILD_SEL4),
        "--component-graph",
        "--platform",
        PLATFORM,
        "--skip-pin-check",
    ]
    print(f"[build] {' '.join(command)}", flush=True)
    process = subprocess.run(command, cwd=ROOT, check=False)
    if process.returncode != 0:
        fail(f"Framework-qualified component graph build failed with exit status {process.returncode}")


def check_source_manifest() -> dict[str, object]:
    if not SOURCE_MANIFEST.is_file():
        fail(f"missing {SOURCE_MANIFEST.relative_to(ROOT)}")
    try:
        manifest = json.loads(SOURCE_MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {SOURCE_MANIFEST.relative_to(ROOT)}: {error}")
    if manifest.get("kind") != "slime-sel4-image-identity":
        fail("source manifest is not a Slime seL4 identity")
    if manifest.get("platform") != PLATFORM:
        fail(f"source manifest platform is {manifest.get('platform')!r}, not {PLATFORM!r}")
    if manifest.get("target_profile") != "x86_64-sel4-framework13-ai300":
        fail("source manifest is not Framework-target-qualified")
    if manifest.get("variant") != "graph" or manifest.get("component_graph") is not True:
        fail("source manifest does not identify the resident product graph")
    if manifest.get("boot_route") != "multiboot2" or manifest.get("image") is not None:
        fail("source manifest does not identify the shared Multiboot2 route")
    media = manifest.get("media")
    if not isinstance(media, dict):
        fail("source manifest records no EFI tree")
    observed = boot_media(
        TREE,
        profile=load_qemu_profile(fail, PINS, "qemu_pc99"),
        fail=fail,
    )
    if observed["tree_sha256"] != media.get("tree_sha256") or observed["files"] != media.get("files"):
        fail("Framework EFI tree does not match its source manifest")
    return observed


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--no-build", action="store_true", help="use the existing Framework source tree")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not arguments.no_build:
        build_source()
    tree_record = check_source_manifest()
    record = build_image(
        IMAGE,
        RECORD,
        tree=TREE,
        tree_record=tree_record,
        source_manifest=SOURCE_MANIFEST,
        fail=fail,
    )
    print(
        f"Framework media build: wrote {IMAGE.relative_to(ROOT)} and "
        f"{RECORD.relative_to(ROOT)} ({record['image']['bytes']} bytes, "
        f"sha256:{str(record['image']['sha256'])[:16]}…, identity:{str(record['identity'])[:16]}…)"
    )


if __name__ == "__main__":
    main()
