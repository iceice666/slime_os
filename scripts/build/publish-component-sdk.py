#!/usr/bin/env python3

"""Publish one SDK export to the permanent generated repository (CP7).

Publication is one-way: this exports the current `slime_os` commit, compares the
result with what the SDK repository already carries, and writes at most one
generated commit plus one immutable `sdk-v<version>` tag. It never merges, never
force-pushes, and never edits a published commit.

Refusals, all before anything is written:

* a dirty source tree, because a published commit must name a source commit that
  reproduces it;
* a source commit the repository does not contain, for the same reason;
* a reused version whose published tree differs, and a changed tree that reuses
  a version -- an immutable tag that moved is worse than a missing one;
* a release record that does not describe its own tree;
* an SDK tree carrying a file the exporter's allowlist does not name.

Idempotence is decided by the exported-tree identity, not by a diff: two exports
of one source commit are byte-identical by CP6, so an unchanged identity means
there is nothing to publish.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402

DEFAULT_SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"
DEFAULT_BRANCH = "generated"
IDENTITY_NAME = "Slime release bot"
IDENTITY_EMAIL = "release-bot@slime-os.invalid"


def fail(message: str) -> None:
    raise SystemExit(f"component SDK publication: {message}")


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


def assert_clean_source(source: Path) -> None:
    """Refuse a dirty working tree when the source commit was not named.

    A publication exports a *commit*, never the working tree, so a dirty tree is
    not itself unsafe -- it is a sign the operator meant something the commit
    does not contain. Naming `--source-commit` explicitly says the opposite and
    lifts this refusal.

    `deps/zutai` is exempt: it is the compiler submodule, and a published SDK
    carries neither its bytes nor its identity.
    """
    status = run(
        ["git", "status", "--porcelain=v1"], cwd=source, description="read source status"
    ).stdout.splitlines()
    dirty = []
    for line in status:
        relative = line[3:].strip().split(" -> ")[-1]
        if relative.startswith("deps/zutai"):
            continue
        if component_sdk.allowlisted(relative) or relative.startswith("contracts/"):
            dirty.append(relative)
    if dirty:
        fail(
            "the source tree has uncommitted changes in the exported set: "
            + ", ".join(sorted(dirty))
            + "; commit them, or name --source-commit to publish an earlier commit"
        )


def assert_commit_present(source: Path, commit: str) -> None:
    result = run(
        ["git", "cat-file", "-e", f"{commit}^{{commit}}"],
        cwd=source,
        description="verify source commit",
        allow_failure=True,
    )
    if result.returncode != 0:
        fail(f"source commit {commit} is not present in this repository")


def source_worktree(source: Path, commit: str, destination: Path) -> Path:
    """Check out the exact source commit, so publication never exports a worktree.

    This is what makes CP7's reverse-drift check possible: a published commit
    reproduces from the commit it records because that is literally what was
    exported. Exporting the working tree would make the recorded commit a label
    rather than a claim.

    `git worktree add` leaves submodules unpopulated, and the export needs
    `deps/rust-sel4` whole, so its bytes are copied from this checkout after
    confirming the checkout's commit equals the gitlink the recorded commit pins.
    """
    run(
        ["git", "worktree", "add", "--detach", "--quiet", str(destination), commit],
        cwd=source,
        description="check out the source commit",
    )
    pinned = run(
        ["git", "rev-parse", f"{commit}:deps/rust-sel4"],
        cwd=source,
        description="read the recorded rust-sel4 pin",
    ).stdout.strip()
    current = run(
        ["git", "rev-parse", "HEAD"],
        cwd=source / "deps" / "rust-sel4",
        description="read the checked-out rust-sel4 commit",
    ).stdout.strip()
    if pinned != current:
        fail(
            f"source commit {commit[:12]} pins rust-sel4 at {pinned[:12]} but this "
            f"checkout has {current[:12]}; run git submodule update"
        )
    shutil.copytree(
        source / "deps" / "rust-sel4",
        destination / "deps" / "rust-sel4",
        ignore=component_sdk.COPY_IGNORE,
        dirs_exist_ok=True,
    )
    return destination


def remove_worktree(source: Path, path: Path) -> None:
    run(
        ["git", "worktree", "remove", "--force", str(path)],
        cwd=source,
        description="remove the source worktree",
    )


def clone(url: str, branch: str, destination: Path) -> tuple[Path, bool]:
    """Clone the SDK repository, tolerating a repository with no commits yet."""
    result = run(
        ["git", "clone", "--quiet", "--branch", branch, url, str(destination)],
        cwd=destination.parent,
        description="clone the SDK repository",
        allow_failure=True,
    )
    if result.returncode == 0:
        return destination, True
    if destination.exists():
        shutil.rmtree(destination)
    empty = run(
        ["git", "clone", "--quiet", url, str(destination)],
        cwd=destination.parent,
        description="clone the SDK repository",
        allow_failure=True,
    )
    if empty.returncode != 0:
        fail(f"cannot clone {url}:\n{empty.stdout}")
    run(
        ["git", "checkout", "--quiet", "-B", branch],
        cwd=destination,
        description="create the generated branch",
    )
    return destination, False


def published_state(clone_root: Path, populated: bool) -> tuple[dict | None, set[str]]:
    if not populated:
        return None, set()
    try:
        record = component_sdk.load_record(clone_root)
    except ComponentSdkError:
        return None, set()
    tags = set(
        run(["git", "tag", "--list"], cwd=clone_root, description="list published tags")
        .stdout.split()
    )
    return record, tags


def assert_allowlisted_tree(sdk: Path, record: dict) -> None:
    """Every top-level entry the export wrote is one the record names."""
    declared = set(record["files"])
    for path in sorted(sdk.iterdir()):
        if path.name == ".git":
            continue
        relative = path.name
        if relative in declared:
            continue
        # A declared entry may be a nested path (`components/lib`), so a
        # top-level directory is admissible when the record names something
        # inside it and nothing inside it is undeclared.
        if path.is_dir():
            unknown = [
                entry.relative_to(sdk).as_posix()
                for entry in path.rglob("*")
                if entry.is_file()
                and not any(
                    entry.relative_to(sdk).as_posix().startswith(f"{name}/")
                    or entry.relative_to(sdk).as_posix() == name
                    for name in declared
                )
            ]
            if not unknown:
                continue
            fail(f"the exported tree carries undeclared file(s): {unknown[:4]}")
        fail(f"the exported tree carries an undeclared entry: {relative}")


def replace_tree(clone_root: Path, sdk: Path) -> None:
    """Make the clone's worktree exactly the exported tree.

    Delete-then-copy rather than overlay: an overlay would silently retain a file
    a later export stopped emitting, and the published commit must be the export,
    not the export plus history.
    """
    for entry in sorted(clone_root.iterdir()):
        if entry.name == ".git":
            continue
        if entry.is_dir():
            shutil.rmtree(entry)
        else:
            entry.unlink()
    for entry in sorted(sdk.iterdir()):
        if entry.name == ".git":
            continue
        destination = clone_root / entry.name
        if entry.is_dir():
            shutil.copytree(entry, destination)
        else:
            shutil.copyfile(entry, destination)
            destination.chmod(entry.stat().st_mode & 0o7777)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="SDK release version")
    parser.add_argument(
        "--sdk-url",
        default=DEFAULT_SDK_REPOSITORY,
        help="transport the generated commit is cloned from and pushed to",
    )
    parser.add_argument(
        "--sdk-repository",
        default=DEFAULT_SDK_REPOSITORY,
        help=(
            "canonical SDK repository recorded in the release and pinned by the "
            "consumer template; distinct from --sdk-url, which is only the "
            "transport a mirror or a local clone may differ on"
        ),
    )
    parser.add_argument("--branch", default=DEFAULT_BRANCH)
    parser.add_argument(
        "--profile",
        action="append",
        default=[],
        choices=sorted(component_sdk.PROFILE_PLATFORMS),
    )
    parser.add_argument(
        "--push",
        action="store_true",
        help="push the generated commit and tag; omitted, publication is local only",
    )
    parser.add_argument(
        "--source-commit",
        help=(
            "the exact protected slime_os commit to publish; defaults to HEAD and "
            "requires a clean exported set when defaulted"
        ),
    )
    parser.add_argument(
        "--signing-key",
        type=Path,
        help=(
            "SSH private key the release tag is signed with; omitted, the tag is "
            "annotated but unsigned"
        ),
    )
    arguments = parser.parse_args()

    if arguments.source_commit is None:
        assert_clean_source(ROOT)
        commit = component_sdk.source_commit(ROOT)
    else:
        commit = run(
            ["git", "rev-parse", f"{arguments.source_commit}^{{commit}}"],
            cwd=ROOT,
            description="resolve the named source commit",
        ).stdout.strip()
    assert_commit_present(ROOT, commit)
    profiles = tuple(arguments.profile) or component_sdk.DEFAULT_PROFILES

    with tempfile.TemporaryDirectory(prefix="slime-sdk-publish-") as temporary:
        root = Path(temporary)
        # The export always runs against a detached checkout of the recorded
        # commit, never the working tree. That is what makes the recorded commit
        # a reproducible claim rather than a label, and it is the property CP7's
        # reverse-drift check verifies.
        worktree = source_worktree(ROOT, commit, root / "source")
        try:
            exported = component_sdk.export(
                root / "export",
                version=arguments.version,
                sdk_repository=arguments.sdk_repository,
                profiles=profiles,
                source=worktree,
                prefix_source=ROOT,
                commit=commit,
                repository=component_sdk.source_repository(ROOT),
            )
        except ComponentSdkError as error:
            fail(str(error))
        finally:
            remove_worktree(ROOT, worktree)
        component_sdk.verify_tree(exported.root, exported.record)
        assert_allowlisted_tree(exported.root, exported.record)

        clone_root, populated = clone(arguments.sdk_url, arguments.branch, root / "sdk")
        previous, tags = published_state(clone_root, populated)
        tag = f"sdk-v{arguments.version}"

        if previous is not None:
            if previous["treeIdentity"] == exported.tree_identity:
                print(
                    f"component SDK publication: {previous['version']} "
                    f"({exported.tree_identity[:16]}) is already published; nothing to do"
                )
                return
            if previous["version"] == arguments.version:
                fail(
                    f"version {arguments.version} is already published with a different "
                    f"tree ({previous['treeIdentity'][:16]} vs "
                    f"{exported.tree_identity[:16]}); publish a new version"
                )
            component_sdk.admit_version_change(previous, exported.record)
        if tag in tags:
            fail(f"tag {tag} already exists; an immutable tag is never moved")

        replace_tree(clone_root, exported.root)
        for arguments_list, description in (
            (["config", "user.name", IDENTITY_NAME], "configure release identity"),
            (["config", "user.email", IDENTITY_EMAIL], "configure release identity"),
            (["add", "--all"], "stage the generated tree"),
        ):
            run(["git", *arguments_list], cwd=clone_root, description=description)
        message = (
            f"sdk: {arguments.version} from {commit}\n"
            "\n"
            f"Generated by scripts/build/build-component-sdk.py from slime_os {commit}.\n"
            f"Exported-tree identity: {exported.tree_identity}\n"
            f"Release-record identity: {exported.identity.hex()}\n"
        )
        run(
            ["git", "commit", "--quiet", "-m", message],
            cwd=clone_root,
            description="write the generated commit",
        )
        sdk_commit = run(
            ["git", "rev-parse", "HEAD"], cwd=clone_root, description="read generated commit"
        ).stdout.strip()
        tag_command = ["git", "tag"]
        if arguments.signing_key is not None:
            key = arguments.signing_key.expanduser().resolve()
            if not key.is_file():
                fail(f"signing key not found: {key}")
            # SSH signing rather than GPG: this repository's release trust root
            # is already an Ed25519 SSH key set
            # (`contracts/release/v1/test-keys`, consumed by
            # `scripts/lib/release_trust.py`), and introducing a second key
            # format for tags would mean two answers to "who signs a release".
            for pair, description in (
                (["gpg.format", "ssh"], "select SSH tag signing"),
                (["user.signingkey", str(key)], "select the release signing key"),
            ):
                run(
                    ["git", "config", *pair],
                    cwd=clone_root,
                    description=description,
                )
            tag_command.append("-s")
        else:
            tag_command.append("-a")
        run(
            [*tag_command, tag, "-m", f"Slime component SDK {arguments.version}"],
            cwd=clone_root,
            description="create the immutable release tag",
        )
        if arguments.push:
            run(
                ["git", "push", "origin", f"HEAD:{arguments.branch}"],
                cwd=clone_root,
                description="push the generated commit",
            )
            run(
                ["git", "push", "origin", tag],
                cwd=clone_root,
                description="push the release tag",
            )
        print(
            f"component SDK publication: {arguments.version} -> {sdk_commit} ({tag})"
            + ("" if arguments.push else "; not pushed")
        )
        print(f"  source commit   {commit}")
        print(f"  tree identity   {exported.tree_identity}")
        print(f"  record identity {exported.identity.hex()}")


if __name__ == "__main__":
    main()
