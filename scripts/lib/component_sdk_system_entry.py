#!/usr/bin/env python3
"""Build and optionally boot one system closure from this SDK release."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

TREE_DOMAIN = b"slime-component-sdk-tree-v1\0"


def fail(message: str) -> None:
    raise SystemExit(f"SDK system image: {message}")


def absorb(digest, value: bytes) -> None:
    digest.update(len(value).to_bytes(8, "little"))
    digest.update(value)


def tree_digest(root: Path) -> str:
    digest = hashlib.sha256()
    digest.update(TREE_DOMAIN)
    files = sorted(
        (path for path in root.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(root).as_posix(),
    )
    digest.update(len(files).to_bytes(8, "little"))
    for path in files:
        absorb(digest, path.relative_to(root).as_posix().encode())
        digest.update(b"\1" if os.access(path, os.X_OK) else b"\0")
        absorb(digest, hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def extract(archive: Path, destination: Path) -> None:
    destination.mkdir()
    root = destination.resolve()
    with tarfile.open(archive, mode="r:") as opened:
        members = opened.getmembers()
        if not members:
            fail("system corpus is empty")
        for member in members:
            if not member.isfile():
                fail(f"system corpus member {member.name!r} is not a regular file")
            target = (destination / member.name).resolve()
            if not target.is_relative_to(root):
                fail(f"system corpus member {member.name!r} escapes the destination")
            handle = opened.extractfile(member)
            data = b"" if handle is None else handle.read()
            if len(data) != member.size:
                fail(f"system corpus member {member.name!r} is truncated")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            target.chmod(0o755 if member.mode & 0o111 else 0o644)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", default="sel4-channel")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--boot", action="store_true")
    arguments = parser.parse_args()
    sdk = Path(__file__).resolve().parents[1]
    record = json.loads((sdk / "component-sdk-release.json").read_text(encoding="utf-8"))
    systems = record.get("systems")
    if not systems:
        fail("this release declares no systems")
    matches = [entry for entry in systems if entry["name"] == arguments.name]
    if len(matches) != 1:
        fail(f"release declares {len(matches)} systems named {arguments.name!r}")
    asset = matches[0]
    archive = sdk / asset["archive"]
    if not archive.is_file():
        fail(f"missing system corpus {asset['archive']}")
    if hashlib.sha256(archive.read_bytes()).hexdigest() != asset["archiveHash"]:
        fail(f"system corpus {asset['archive']} does not match its recorded hash")
    root = sdk / ".system-source" / arguments.name
    if root.exists():
        shutil.rmtree(root)
    root.parent.mkdir(exist_ok=True)
    extract(archive, root)
    observed_tree = tree_digest(root)
    if observed_tree != asset["treeHash"]:
        fail(f"system corpus tree hash mismatch: {observed_tree} != {asset['treeHash']}")
    sys.path.insert(0, str(root / "scripts/lib"))
    from system_image_closure import compile_closure, compile_test_run

    closure = compile_closure(root / asset["closure"])
    test_run = compile_test_run(root / asset["testRun"])
    if closure.identity.hex() != asset["closureIdentity"]:
        fail("closure identity mismatch")
    if test_run.identity.hex() != asset["testRunIdentity"]:
        fail("test-run identity mismatch")
    output = arguments.output.resolve()
    subprocess.run(
        [
            sys.executable,
            str(root / "scripts/build/build-system-image.py"),
            str(root / asset["closure"]),
            str(output),
        ],
        cwd=root,
        check=True,
    )
    result = json.loads((output / "build-result.json").read_text(encoding="utf-8"))
    if result["closureIdentity"] != asset["closureIdentity"]:
        fail("build result names a different closure")
    image = output / result["image"]["path"]
    if hashlib.sha256(image.read_bytes()).hexdigest() != result["image"]["sha256"]:
        fail("built image hash disagrees with its build result")
    if arguments.boot:
        environment = dict(os.environ)
        environment["PYTHONPATH"] = str(root / "scripts/lib")
        subprocess.run(
            [
                sys.executable,
                str(root / "scripts/check/check-sel4-channel-plane.py"),
                "--no-build",
                "--image",
                str(image),
            ],
            cwd=root,
            env=environment,
            check=True,
        )
    print(result["image"]["sha256"])


if __name__ == "__main__":
    main()
