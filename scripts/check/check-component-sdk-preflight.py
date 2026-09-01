#!/usr/bin/env python3

"""Report the SDK version a publication from this source commit must claim.

CP9 makes the change classification computable from the release record, but
nothing computed it *before* a publication was attempted: `--version` is a
free-text workflow input, and `admit_version_change` only refuses an
understated version once the publisher is already running on the release
machine. The operator therefore learned a wrong version after the prefix build,
from the one job holding the release credentials.

This reads the hosted canonical release, exports the current source commit, and
prints the classification and the lowest version that publication would admit.
It writes nothing and needs no credential, so it runs on an ordinary runner
before the release job is reached.

It is also the only thing in this repository that reads the hosted release at
all. `devlog/2026-08-26-cp7-hosted-publication-hardening/index.md` records that
absence: the hosted assertions were observed once, by hand, so hosted drift
would otherwise surface only at the next publication.

The profile axis is compared over the intersection of the hosted and exported
profile sets. Exporting a subset is a local choice about which prefixes were
built, not a release removing a platform, and reporting it as `profiles:removed`
would manufacture a breaking change out of how the gate was invoked. Profiles
that could not be compared are named in the output rather than passed over.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402

CANONICAL_SDK = "https://github.com/iceice666/slime_os-component_sdk"
BRANCH = "generated"
# The export validates its version against the contract's MAJOR.MINOR.PATCH
# bound, so the probe must be a real semver. It is never compared against the
# hosted version and never published: the whole point is to compute what the
# version should be, and `--require-version` checks the operator's value
# against that computation rather than against this one.
PROBE_VERSION = "0.0.0"
REQUIRED_BUMP = {
    "breaking": "major",
    "compatible-feature": "minor",
    "patch": "patch",
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK preflight: {message}")


def run(
    command: list[str], *, cwd: Path, description: str, allow_failure: bool = False
) -> subprocess.CompletedProcess[str]:
    process = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0 and not allow_failure:
        fail(f"{description} failed:\n{process.stdout}")
    return process


def hosted_record(url: str, branch: str, destination: Path) -> dict | None:
    """The hosted release record, or None when nothing is published yet."""
    cloned = run(
        ["git", "clone", "--quiet", "--depth", "1", "--branch", branch, url, str(destination)],
        cwd=destination.parent,
        description=f"clone {url}",
        allow_failure=True,
    )
    if cloned.returncode != 0:
        return None
    try:
        # Self-checks all three record files against one identity, so a
        # hand-edited mirror is refused here rather than compared against.
        return component_sdk.load_record(destination)
    except ComponentSdkError as error:
        fail(f"the hosted release record is not admissible: {error}")


def next_version(previous: str, classification: str) -> str:
    major, minor, patch = (int(part) for part in previous.split("."))
    bump = REQUIRED_BUMP[classification]
    if bump == "major":
        return f"{major + 1}.0.0"
    if bump == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def aligned(previous: dict, current: dict) -> tuple[dict, dict, tuple[str, ...]]:
    """Restrict the profile axis to the profiles both records declare."""
    old = {entry["profile"] for entry in previous["profiles"]}
    new = {entry["profile"] for entry in current["profiles"]}
    shared = old & new
    if not shared:
        fail(
            "the exported and hosted releases share no target profile, so no "
            "comparison is possible; export at least one hosted profile"
        )
    before = dict(previous)
    after = dict(current)
    before["profiles"] = [e for e in previous["profiles"] if e["profile"] in shared]
    after["profiles"] = [e for e in current["profiles"] if e["profile"] in shared]
    return before, after, tuple(sorted((old | new) - shared))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        choices=sorted(component_sdk.PROFILE_PLATFORMS),
        help="profile to export; defaults to every profile the hosted release declares",
    )
    parser.add_argument("--sdk-url", default=CANONICAL_SDK, help="hosted SDK transport")
    parser.add_argument("--branch", default=BRANCH)
    parser.add_argument(
        "--source-commit", help="source commit to export; defaults to this checkout's HEAD"
    )
    parser.add_argument(
        "--require-version",
        help="fail unless this version is the lowest publication would admit",
    )
    parser.add_argument(
        "--github-output",
        action="store_true",
        help="also emit key=value lines for a workflow step output",
    )
    arguments = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="slime-sdk-preflight-") as temporary:
        root = Path(temporary)
        previous = hosted_record(arguments.sdk_url, arguments.branch, root / "hosted")

        profiles = tuple(arguments.profile)
        if not profiles:
            profiles = (
                tuple(entry["profile"] for entry in previous["profiles"])
                if previous is not None
                else component_sdk.DEFAULT_PROFILES
            )

        commit = arguments.source_commit or component_sdk.source_commit(ROOT)
        try:
            exported = component_sdk.export(
                root / "export",
                version=PROBE_VERSION,
                sdk_repository=arguments.sdk_url,
                profiles=profiles,
                commit=commit,
            )
        except ComponentSdkError as error:
            fail(f"the current source commit does not export: {error}")

        if previous is None:
            print("component SDK preflight: nothing is published; this would be the initial release")
            print("  classification  initial")
            print("  version         1.0.0")
            if arguments.require_version and arguments.require_version != "1.0.0":
                fail(f"an initial release must be 1.0.0, not {arguments.require_version}")
            return

        before, after, uncompared = aligned(previous, exported.record)
        classification = component_sdk.classify(before, after)
        breaking, feature = component_sdk.changed_axes(before, after)
        required = next_version(previous["version"], classification)

        print(f"component SDK preflight: hosted {previous['version']} -> source {commit[:12]}")
        print(f"  hosted commit   {previous['sourceCommit'][:12]}")
        print(f"  classification  {classification}")
        print(f"  version         {required} (lowest publication admits)")
        for axis in breaking:
            print(f"  breaking        {axis}")
        for axis in feature:
            print(f"  feature         {axis}")
        if not breaking and not feature:
            print("  no compatibility axis moved")
        if uncompared:
            print(
                "  NOT COMPARED    profiles "
                + ",".join(uncompared)
                + " (absent from one side; the profile axis is only partly checked)"
            )

        if arguments.github_output:
            summary = {
                "classification": classification,
                "required_version": required,
                "hosted_version": previous["version"],
            }
            for key, value in summary.items():
                print(f"::notice::{key}={value}")
            print(json.dumps(summary))

        if arguments.require_version is not None:
            if arguments.require_version == required:
                print(f"component SDK preflight: {required} matches the required change class")
                return
            # Overstating is admitted by `admit_version_change` and refused
            # here: a workflow input that does not match the computed version is
            # more likely a typo than a deliberate conservative bump, and the
            # operator can still publish an overstated version by passing it to
            # the publisher directly.
            fail(
                f"requested version {arguments.require_version} is not the required "
                f"{required} for a {classification} change"
            )


if __name__ == "__main__":
    main()
