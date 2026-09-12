#!/usr/bin/env python3
"""CP15: immutable SDK releases build, boot, and roll back one closure."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/lib"))

import component_sdk  # noqa: E402
import component_sdk_system  # noqa: E402

PROFILE = "aarch64-sel4-qemu-virt"
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"


def fail(message: str) -> None:
    raise SystemExit(f"component SDK system image: {message}")


def run(command: list[str], *, cwd: Path, description: str) -> str:
    process = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"{description} failed:\n{process.stdout}")
    return process.stdout


def export_release(root: Path, version: str) -> Path:
    destination = root / f"sdk-{version}"
    component_sdk.export(
        destination,
        version=version,
        sdk_repository=SDK_REPOSITORY,
        profiles=(PROFILE,),
        source=ROOT,
    )
    run(["git", "init", "-q"], cwd=destination, description="initialize immutable SDK")
    run(["git", "add", "."], cwd=destination, description="stage immutable SDK")
    run(
        [
            "git",
            "-c",
            "user.name=Slime release bot",
            "-c",
            "user.email=release-bot@slime-os.invalid",
            "commit",
            "-q",
            "-m",
            f"release SDK {version}",
        ],
        cwd=destination,
        description="commit immutable SDK",
    )
    return destination


def build(sdk: Path, output: Path, *, boot: bool) -> dict:
    command = [
        sys.executable,
        str(sdk / "tools/sdk-system-image.py"),
        "--output",
        str(output),
    ]
    if boot:
        command.append("--boot")
    run(command, cwd=sdk.parent, description=f"build system from {sdk.name}")
    return json.loads((output / "build-result.json").read_text(encoding="utf-8"))


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-system-") as temporary:
        root = Path(temporary)
        previous = export_release(root, "2.0.0")
        current = export_release(root, "2.0.1")

        previous_output = root / "previous-image"
        previous_result = build(previous, previous_output, boot=False)
        current_result = build(current, root / "current-image", boot=True)
        if previous_result["closureIdentity"] != current_result["closureIdentity"]:
            fail("unchanged system corpus moved closure identity across a patch release")

        # Recreate the retained release at the identical output path: Cargo's
        # target directory is part of root/loader debug paths, so byte identity
        # is meaningful only when that consumer-selected path is held fixed.
        shutil.rmtree(previous_output)
        rollback_result = build(previous, previous_output, boot=True)
        for field in ("closureIdentity", "systemIdentity"):
            if rollback_result[field] != previous_result[field]:
                fail(f"rollback changed {field}")
        for artifact in ("generation", "root", "loader", "image", "identityManifest"):
            if rollback_result[artifact]["sha256"] != previous_result[artifact]["sha256"]:
                fail(f"rollback changed {artifact} identity")

        current_record = component_sdk.load_record(current)
        component_sdk.verify_tree(current, current_record)
        component_sdk.verify_digests(current, current_record)
        system = current_record["systems"][0]
        if current_result["closureIdentity"] != system["closureIdentity"]:
            fail("built result does not name the release's declared closure")

        profile_records = current_record["profiles"]
        mismatched_records = [
            (
                {
                    **entry,
                    "prefix": {**entry["prefix"], "treeHash": "0" * 64},
                }
                if entry["profile"] == PROFILE
                else entry
            )
            for entry in profile_records
        ]
        refusal_cases = (
            (
                "missing selected profile",
                [entry for entry in profile_records if entry["profile"] != PROFILE],
            ),
            ("mismatched selected prefix", mismatched_records),
        )
        for label, records in refusal_cases:
            try:
                component_sdk_system.export_asset(
                    root / f"refusal-{label.replace(' ', '-')}",
                    ROOT,
                    sdk_module=component_sdk,
                    profile_records=records,
                )
            except component_sdk.ComponentSdkError as error:
                if "selected exported profile" not in str(error):
                    fail(f"{label} failed for the wrong reason: {error}")
            else:
                fail(f"system corpus export accepted a {label}")

        # The corpus binds one closure, and that closure names one target
        # profile, so a release exporting a different profile must publish no
        # corpus rather than fail. The requirement is read from the closure
        # rather than restated here: a corpus retargeted at another platform
        # must move this gate's own expectation with it.
        corpus_profile = component_sdk_system.required_profile(ROOT)
        if corpus_profile != PROFILE:
            fail(
                f"the published corpus requires {corpus_profile!r}, but this gate "
                f"exports {PROFILE!r}"
            )
        if [entry["name"] for entry in current_record["systems"]] != [
            component_sdk_system.SYSTEM_NAME
        ]:
            fail("a release exporting the corpus profile did not publish the corpus")

    print(
        "component SDK system image: two immutable SDK releases built the declared "
        "sel4-channel closure without a slime_os checkout, the current release booted "
        "through its declared QEMU test run, rollback reproduced every previous "
        "build-result artifact identity before booting it again, corpus export "
        "refused missing or mismatched outer profile provenance, and the published "
        "corpus resolved its required target profile from its own closure"
    )


if __name__ == "__main__":
    main()
