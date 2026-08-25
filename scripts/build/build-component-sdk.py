#!/usr/bin/env python3

"""Export one deterministic component SDK tree and its release record (CP6).

The repository-owned exporter. `scripts/lib/component_sdk.py` holds the file
set, the identity rules, and the release record; this script is its command
line, so a candidate export and a published release cannot be constructed by
different code.
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402

DEFAULT_SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="directory to write the SDK tree into")
    parser.add_argument("--version", required=True, help="SDK release version, MAJOR.MINOR.PATCH")
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        choices=sorted(component_sdk.PROFILE_PLATFORMS),
        help="target profile to publish a platform asset for (repeatable)",
    )
    parser.add_argument(
        "--sdk-repository",
        default=DEFAULT_SDK_REPOSITORY,
        help="canonical generated SDK repository the release is published to",
    )
    parser.add_argument(
        "--source-commit",
        help="originating slime_os commit; defaults to this checkout's HEAD",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="replace an existing destination directory",
    )
    arguments = parser.parse_args()

    if arguments.force and arguments.destination.exists():
        shutil.rmtree(arguments.destination)
    profiles = tuple(arguments.profile) or ("aarch64-sel4-qemu-virt", "aarch64-rpi5")
    try:
        exported = component_sdk.export(
            arguments.destination,
            version=arguments.version,
            profiles=profiles,
            sdk_repository=arguments.sdk_repository,
            commit=arguments.source_commit,
        )
    except ComponentSdkError as error:
        raise SystemExit(f"component SDK export: {error}") from error

    print(
        f"component SDK export: {exported.version} from "
        f"{exported.record['sourceCommit'][:12]} -> {arguments.destination}"
    )
    print(f"  tree identity   {exported.tree_identity}")
    print(f"  record identity {exported.identity.hex()}")
    for profile in exported.record["profiles"]:
        print(
            f"  profile {profile['profile']} "
            f"prefix={profile['prefix']['archiveHash'][:16]} "
            f"target={profile['targetSpecHash'][:16]}"
        )


if __name__ == "__main__":
    main()
