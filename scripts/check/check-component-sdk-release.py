#!/usr/bin/env python3

"""CP7: permanent SDK publication, idempotence, and reverse drift.

The permanent repository is `iceice666/slime_os-component_sdk`. Its real
publication path needs a credential this gate does not hold and must not, so the
gate publishes to a local bare clone of that repository's `generated` branch
through the same `scripts/build/publish-component-sdk.py` the release identity
runs. What is proven is the publisher's behavior, not GitHub's: idempotence,
version reuse refusal, dirty-source refusal, immutable-tag refusal, non-allowlisted
file refusal, byte-exact regeneration from the recorded source commit, and a
component built from the published commit entering a signed generation and
booting.

The one clause this gate cannot observe is the hosted repository's branch
protection and credential boundary, which is a GitHub setting rather than a
repository artifact. `roadmap/10-component-platform.md` records it as configured
rather than gate-proven, on the same terms this repository already refuses to
call QEMU evidence board evidence.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from component_paths import build_product_slisp, source_path  # noqa: E402
import component_sdk  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402

PUBLISHER = ROOT / "scripts" / "build" / "publish-component-sdk.py"
BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"
CHECK = load_script("component_sdk_release_generation_check", "check/check-generation.py")
CANONICAL_SDK = "https://github.com/iceice666/slime_os-component_sdk"
BRANCH = "generated"
PROFILE = "aarch64-sel4-qemu-virt"
EXTERNAL_IMPLEMENTATION = "cp7-console"
VERSION = "1.0.0"
# The repository's existing release trust root. Reused rather than a new key:
# `contracts/release/v1/test-keys` is what already signs a generation, and a
# second key format or key set for tags would mean two answers to "who signs a
# release".
SIGNING_KEY = ROOT / "contracts" / "release" / "v1" / "test-keys" / "key1"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK release check: {message}")


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    description: str,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    process = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0 and not allow_failure:
        fail(f"{description} failed:\n{process.stdout}")
    return process


def publish(
    url: str,
    *,
    version: str = VERSION,
    extra: tuple[str, ...] = (),
    pinned: bool = True,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Publish through the release path.

    `pinned` names the committed source explicitly, which is what a real
    publication does. It is dropped only by the dirty-source arm below, where the
    point is precisely that a defaulted `HEAD` must refuse an uncommitted
    exported set.
    """
    command = [
        sys.executable,
        str(PUBLISHER),
        "--version",
        version,
        "--sdk-url",
        url,
        "--branch",
        BRANCH,
        "--profile",
        PROFILE,
        "--signing-key",
        str(SIGNING_KEY),
    ]
    if pinned:
        command += ["--source-commit", "HEAD"]
    return run(
        [*command, *extra],
        cwd=ROOT,
        description=f"publish SDK {version}",
        allow_failure=allow_failure,
    )


def canonical_remote(root: Path) -> str:
    """A local bare clone standing in for the canonical repository.

    The canonical URL is recorded in every published record, so what the gate
    substitutes is the transport, not the identity: the published record still
    names `iceice666/slime_os-component_sdk` as its `sdkRepository`.
    """
    bare = root / "slime_os-component_sdk.git"
    run(
        ["git", "init", "--quiet", "--bare", "--initial-branch", BRANCH, str(bare)],
        cwd=root,
        description="create the stand-in canonical repository",
    )
    return str(bare)


def prove_atomic_publication(root: Path) -> None:
    """A refused tag must leave neither half of the release published.

    The hook rejects only release tags. Two separate pushes would therefore
    publish the branch first and fail second; one atomic push refuses both refs.
    """
    bare = root / "atomic-refusal.git"
    run(
        ["git", "init", "--quiet", "--bare", "--initial-branch", BRANCH, str(bare)],
        cwd=root,
        description="create the atomic-publication remote",
    )
    hook = bare / "hooks" / "pre-receive"
    hook.write_text(
        "#!/bin/sh\n"
        "while read old new ref; do\n"
        '    case "$ref" in\n'
        "        refs/tags/sdk-v*) exit 1 ;;\n"
        "    esac\n"
        "done\n"
        "exit 0\n",
        encoding="utf-8",
    )
    hook.chmod(0o755)

    refused = publish(str(bare), version="9.9.9", extra=("--push",), allow_failure=True)
    if refused.returncode == 0:
        fail("publication succeeded despite the remote refusing its release tag")
    refs = run(
        ["git", "for-each-ref", "--format=%(refname)"],
        cwd=bare,
        description="inspect the refused atomic publication",
    ).stdout.split()
    if refs:
        fail(f"a refused release tag left partial remote refs: {refs}")
    print(
        "component SDK release: a refused release tag left no generated branch commit",
        flush=True,
    )


def clone(url: str, destination: Path) -> Path:
    run(
        ["git", "clone", "--quiet", "--branch", BRANCH, url, str(destination)],
        cwd=destination.parent,
        description="clone the published SDK",
    )
    return destination


def prove_first_publication(root: Path, url: str) -> tuple[str, dict]:
    published = publish(url, extra=("--push",))
    if "not pushed" in published.stdout:
        fail("publication reported a local-only result despite --push")
    clone_root = clone(url, root / "published")
    record = component_sdk.load_record(clone_root)
    if record["sdkRepository"] != CANONICAL_SDK:
        fail(
            "the published record does not name the canonical SDK repository: "
            f"{record['sdkRepository']}"
        )
    component_sdk.verify_tree(clone_root, record)
    commit = run(
        ["git", "rev-parse", "HEAD"], cwd=clone_root, description="read published commit"
    ).stdout.strip()
    tags = run(
        ["git", "tag", "--list"], cwd=clone_root, description="read published tags"
    ).stdout.split()
    if tags != [f"sdk-v{VERSION}"]:
        fail(f"expected exactly one immutable release tag, saw {tags}")
    signature = run(
        ["git", "cat-file", "tag", f"sdk-v{VERSION}"],
        cwd=clone_root,
        description="read the release tag object",
    ).stdout
    # The tag object must carry a signature. Verification needs an allowed-signers
    # file, which is a deployment fact rather than a repository artifact, so what
    # is asserted here is that the release identity signed the tag at all -- an
    # unsigned tag would carry no `SSHSIG` armor.
    if "-----BEGIN SSH SIGNATURE-----" not in signature:
        fail("the release tag is not signed")
    body = run(
        ["git", "log", "-1", "--format=%B"], cwd=clone_root, description="read commit message"
    ).stdout
    if record["sourceCommit"] not in body:
        fail("the generated commit does not name its originating slime_os commit")
    if record["treeIdentity"] not in body:
        fail("the generated commit does not name its exported-tree identity")
    print(
        f"component SDK release: published {VERSION} as {commit[:12]} with tag "
        f"sdk-v{VERSION} naming source {record['sourceCommit'][:12]}",
        flush=True,
    )
    return commit, record


def prove_idempotence(root: Path, url: str, commit: str) -> None:
    repeated = publish(url, extra=("--push",))
    if "already published" not in repeated.stdout:
        fail(f"a republication of an unchanged tree was not recognized:\n{repeated.stdout}")
    clone_root = clone(url, root / "published-again")
    again = run(
        ["git", "rev-parse", "HEAD"], cwd=clone_root, description="read published commit"
    ).stdout.strip()
    if again != commit:
        fail("republishing an unchanged tree created a new commit")
    count = run(
        ["git", "rev-list", "--count", "HEAD"], cwd=clone_root, description="count commits"
    ).stdout.strip()
    if count != "1":
        fail(f"the generated branch holds {count} commits after one publication")
    shutil.rmtree(clone_root)
    print("component SDK release: republishing an unchanged tree wrote nothing", flush=True)


def prove_refusals(root: Path, url: str) -> None:
    """Every refusal the publisher owes, each with its distinguishing message."""
    reused = publish(url, version=VERSION, allow_failure=True)
    if "already published" not in reused.stdout:
        fail("reusing a published version with an unchanged tree was not reported")

    # A dirty exported set with the source commit *defaulted*. The refusal exists
    # because a defaulted publication means "publish what I have", and what the
    # operator has is not what the commit contains.
    dirty = ROOT / "components" / "runtime" / "src" / "cp7-dirty-probe.rs"
    dirty.write_text("// CP7 dirty-source probe.\n", encoding="utf-8")
    try:
        refused = publish(url, version="1.0.1", pinned=False, allow_failure=True)
        if refused.returncode == 0 or "uncommitted changes" not in refused.stdout:
            fail(f"publication from a dirty source tree was not refused:\n{refused.stdout}")
    finally:
        dirty.unlink()

    # An unverified source commit: a well-formed SHA-1 this repository does not
    # contain. Refused before anything is exported.
    unknown = publish(
        url, version="1.0.1", extra=("--source-commit", "0" * 40), pinned=False, allow_failure=True
    )
    if unknown.returncode == 0:
        fail("publication from a source commit this repository lacks succeeded")

    # A changed tree reusing a published version. Built by publishing a mutated
    # export under the same version through a second source mirror, which is the
    # only way to reach the "same version, different tree" arm.
    #
    # Every export input is derived from `component_sdk`'s own declarations
    # rather than restated, so a newly exported path cannot leave this mirror
    # silently incomplete — which it did once, when the linker scripts became an
    # export input and a hand-written list did not know.
    mirror = root / "mutated-source"
    mirror.mkdir()
    for relative in (
        ("Cargo.toml", "sel4/pins.toml", "contracts")
        + tuple(path for path, _ in component_sdk.EXPORT_CRATES)
        + component_sdk.VENDORED
        + component_sdk.LINKER_SCRIPTS
        + component_sdk.target_spec_source_paths()
    ):
        target = mirror / relative
        if target.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if (ROOT / relative).is_dir():
            shutil.copytree(ROOT / relative, target, ignore=component_sdk.COPY_IGNORE)
        else:
            shutil.copyfile(ROOT / relative, target)
    probe = mirror / "components" / "runtime" / "src" / "lib.rs"
    probe.write_text(probe.read_text(encoding="utf-8") + "\n// CP7 probe.\n", encoding="utf-8")
    mutated = component_sdk.export(
        root / "mutated-export",
        version=VERSION,
        sdk_repository=CANONICAL_SDK,
        profiles=(PROFILE,),
        source=mirror,
        prefix_source=ROOT,
        commit=component_sdk.source_commit(ROOT),
        repository=component_sdk.source_repository(ROOT),
    )
    published = component_sdk.load_record(clone(url, root / "published-state"))
    if mutated.tree_identity == published["treeIdentity"]:
        fail("the mutated export did not differ from the published tree")
    try:
        component_sdk.admit_version_change(published, mutated.record)
    except ComponentSdkError as error:
        if "does not advance past" not in str(error):
            fail(f"a reused version with a changed tree was refused for the wrong reason: {error}")
    else:
        fail("a reused version with a changed tree was admitted")

    # A non-allowlisted file inside an otherwise valid export.
    stray = root / "stray-export"
    shutil.copytree(mutated.root, stray)
    (stray / "secret.pem").write_text("not an SDK file\n", encoding="utf-8")
    publisher = load_script("component_sdk_publisher", "build/publish-component-sdk.py")
    try:
        publisher.assert_allowlisted_tree(stray, mutated.record)
    except SystemExit as error:
        if "undeclared" not in str(error):
            fail(f"an SDK tree with a stray file was refused for the wrong reason: {error}")
    else:
        fail("an SDK tree carrying a non-allowlisted file was accepted")

    shutil.rmtree(root / "published-state")
    print(
        "component SDK release: reused versions, a dirty source tree, a changed tree "
        "reusing a version, and a non-allowlisted file were each refused",
        flush=True,
    )


def prove_reverse_drift(root: Path, url: str) -> None:
    """Regenerate the published commit from the source commit it records.

    The published tree is compared byte for byte against a fresh export of its
    own recorded `sourceCommit`, taken from a detached worktree rather than the
    working tree. That is the property CP7 owes: a hosted commit is accepted only
    where it reproduces from the source it names, so a hand edit in the mirror is
    a refusal rather than a difference nobody reads.
    """
    clone_root = clone(url, root / "drift-clone")
    record = component_sdk.load_record(clone_root)
    worktree = root / "recorded-source"
    run(
        [
            "git",
            "worktree",
            "add",
            "--detach",
            "--quiet",
            str(worktree),
            record["sourceCommit"],
        ],
        cwd=ROOT,
        description="check out the recorded source commit",
    )
    try:
        # `git worktree add` leaves submodules unpopulated, and the export needs
        # `deps/rust-sel4` whole. The recorded commit pins its gitlink, so the
        # bytes are checked against that pin rather than assumed.
        pinned = run(
            ["git", "rev-parse", f"{record['sourceCommit']}:deps/rust-sel4"],
            cwd=ROOT,
            description="read the recorded rust-sel4 pin",
        ).stdout.strip()
        current = run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT / "deps" / "rust-sel4",
            description="read the checked-out rust-sel4 commit",
        ).stdout.strip()
        if pinned != current:
            fail(
                "the recorded source commit pins rust-sel4 at "
                f"{pinned[:12]} but this checkout has {current[:12]}"
            )
        shutil.copytree(
            ROOT / "deps" / "rust-sel4",
            worktree / "deps" / "rust-sel4",
            ignore=component_sdk.COPY_IGNORE,
            dirs_exist_ok=True,
        )
        regenerated = component_sdk.export(
            root / "regenerated",
            version=record["version"],
            sdk_repository=record["sdkRepository"],
            profiles=tuple(entry["profile"] for entry in record["profiles"]),
            source=worktree,
            prefix_source=ROOT,
            commit=record["sourceCommit"],
            repository=record["sourceRepository"],
        )
    finally:
        run(
            ["git", "worktree", "remove", "--force", str(worktree)],
            cwd=ROOT,
            description="remove the recorded-source worktree",
        )
    if regenerated.tree_identity != record["treeIdentity"]:
        fail(
            "the published commit does not regenerate from its recorded source: "
            f"{record['treeIdentity'][:16]} published, {regenerated.tree_identity[:16]} rebuilt"
        )
    difference = subprocess.run(
        ["diff", "-r", "--exclude=.git", str(clone_root), str(regenerated.root)],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if difference.returncode != 0:
        fail(f"the published tree differs from its regeneration:\n{difference.stdout}")

    # The negative control: a hand edit in the mirror must be refused. This is
    # the arm that makes the comparison above load-bearing rather than a
    # tautology over two copies of one export.
    patched = clone_root / "README.md"
    patched.write_text(patched.read_text(encoding="utf-8") + "\nHand edit.\n", encoding="utf-8")
    try:
        component_sdk.verify_tree(clone_root, record)
    except ComponentSdkError as error:
        if "identity mismatch" not in str(error):
            fail(f"a hand-edited mirror was refused for the wrong reason: {error}")
    else:
        fail("a hand-edited mirror passed the drift check")
    shutil.rmtree(clone_root)
    shutil.rmtree(regenerated.root)
    print(
        "component SDK release: the published commit regenerated byte-identically from "
        "its recorded source commit, and a hand edit was refused",
        flush=True,
    )


def prove_hosted_build_and_boot(root: Path, url: str, slisp: Path) -> dict:
    """A fresh clone of the published commit builds and boots a component."""
    clone_root = clone(url, root / "consumer-sdk")
    record = component_sdk.load_record(clone_root)
    revision = run(
        ["git", "rev-parse", "HEAD"], cwd=clone_root, description="read published commit"
    ).stdout.strip()
    checkout = root / "cp7-consumer"
    (checkout / "console" / "src").mkdir(parents=True)
    dependency = f'{{ git = "{clone_root.resolve().as_uri()}", rev = "{revision}" }}'
    (checkout / "Cargo.toml").write_text(
        "[workspace]\n"
        'resolver = "3"\n'
        'members = ["console"]\n'
        "\n"
        "[workspace.dependencies]\n"
        f"boot-contracts = {dependency}\n"
        f"slime-proto = {dependency}\n"
        f"slime-components = {dependency}\n"
        f"slime-rt = {dependency}\n"
        f"slime-build-support = {dependency}\n"
        "\n"
        "[profile.release]\n"
        'panic = "abort"\n'
        'opt-level = "s"\n'
        "codegen-units = 1\n"
        "debug = false\n",
        encoding="utf-8",
    )
    (checkout / "console" / "Cargo.toml").write_text(
        "[package]\n"
        f'name = "{EXTERNAL_IMPLEMENTATION}"\n'
        'version = "0.1.0"\n'
        'edition = "2024"\n'
        "publish = false\n"
        'rust-version = "1.96"\n'
        'build = "build.rs"\n'
        "\n"
        "[[bin]]\n"
        'name = "console"\n'
        'path = "src/main.rs"\n'
        "test = false\n"
        "\n"
        "[dependencies]\n"
        "boot-contracts.workspace = true\n"
        "slime-components.workspace = true\n"
        "slime-proto.workspace = true\n"
        "slime-rt.workspace = true\n"
        "\n"
        "[build-dependencies]\n"
        "slime-build-support.workspace = true\n",
        encoding="utf-8",
    )
    (checkout / "console" / "build.rs").write_text(
        "fn main() {\n    slime_build_support::configure();\n}\n", encoding="utf-8"
    )
    shutil.copyfile(
        source_path("console"),
        checkout / "console" / "src" / "main.rs",
    )
    metadata = run(
        ["cargo", "metadata", "--format-version", "1", "--quiet"],
        cwd=checkout,
        description="resolve the consumer's metadata",
    )
    expected = {name for _, name in component_sdk.EXPORT_CRATES}
    seen = set()
    for package in json.loads(metadata.stdout)["packages"]:
        manifest = Path(package["manifest_path"]).resolve()
        if manifest.is_relative_to(ROOT.resolve()):
            fail(f"a consumer dependency resolved into this checkout: {manifest}")
        if package["name"] in expected:
            source = package.get("source") or ""
            if not source.startswith("git+") or f"#{revision}" not in source:
                fail(f"{package['name']} did not resolve through the published commit")
            seen.add(package["name"])
    if seen != expected:
        fail(f"the consumer did not resolve every SDK crate: {sorted(expected - seen)}")

    target_dir = root / "consumer-target"
    run(
        [
            sys.executable,
            str(clone_root / "tools" / "sdk-build.py"),
            "--profile",
            PROFILE,
            "--manifest-path",
            str(checkout / "Cargo.toml"),
            "--package",
            EXTERNAL_IMPLEMENTATION,
            "--target-dir",
            str(target_dir),
            "--cache",
            str(root / "prefix-cache"),
        ],
        cwd=checkout,
        description="build a component from the published SDK",
    )
    elf = target_dir / "aarch64-sel4-minimal" / "release" / "console.elf"
    if not elf.is_file():
        fail("the published SDK produced no component ELF")

    specs = root / "specs"
    specs.mkdir()
    digest = hashlib.sha256(elf.read_bytes()).hexdigest()
    for entry in admit_specs():
        spec = copy.deepcopy(entry.spec)
        if entry.name == "console":
            spec["implementation"] = {
                "provider": "external",
                "binary": EXTERNAL_IMPLEMENTATION,
                "contentHash": digest,
            }
        (specs / f"{entry.name}.zti").write_text(
            component_sdk.zti(spec) + "\n", encoding="utf-8"
        )
    output = root / "hosted-generation"
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = PROFILE
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    built = run(
        [
            sys.executable,
            str(BUILDER),
            "--component-spec-root",
            str(specs),
            "--external-component",
            f"{EXTERNAL_IMPLEMENTATION}={elf}",
            "--external-component",
            f"slisp-external={slisp}",
            str(output),
        ],
        cwd=ROOT,
        env=environment,
        description="build a generation from the published SDK's component",
    )
    if f"implementation={EXTERNAL_IMPLEMENTATION} provider=external" not in built.stdout:
        fail("the builder did not report the published SDK's component as external")
    generation = CHECK.check_generation((output / "generation.bin").read_bytes())
    store = CHECK.check_bootstore((output / "boot-store.bin").read_bytes())
    if store["selected"]["identity"] != generation["identity"]:
        fail("the signed boot store did not select the hosted-SDK generation")
    run(
        [
            sys.executable,
            str(SEL4_BUILDER),
            "--component-graph",
            "--prebuilt-generation",
            str(output / "generation.bin"),
        ],
        cwd=ROOT,
        description="embed the hosted-SDK generation",
    )
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description="boot the hosted-SDK generation",
    )
    print(
        f"component SDK release: a fresh clone of {revision[:12]} built a component that "
        "entered a signed generation and booted the QEMU component graph",
        flush=True,
    )
    return record


def prove_deletion_is_harmless(root: Path) -> None:
    """Removing every SDK clone leaves the in-tree build path untouched."""
    for name in ("consumer-sdk", "published", "published-again"):
        path = root / name
        if path.exists():
            shutil.rmtree(path)
    run(
        [sys.executable, str(SEL4_BUILDER), "--component-graph"],
        cwd=ROOT,
        description="rebuild the in-tree component graph",
    )
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description="boot the in-tree component graph",
    )
    print(
        "component SDK release: with every SDK clone deleted, the ordinary in-tree "
        "component graph still built and booted",
        flush=True,
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-release-") as temporary:
        root = Path(temporary)
        url = canonical_remote(root)
        slisp = build_product_slisp(root / "slisp.elf")
        prove_atomic_publication(root)
        commit, record = prove_first_publication(root, url)
        prove_idempotence(root, url, commit)
        prove_refusals(root, url)
        prove_reverse_drift(root, url)
        prove_hosted_build_and_boot(root, url, slisp)
        prove_deletion_is_harmless(root)

    print(
        f"component SDK release check: SDK {record['version']} was published once as an "
        f"immutable commit and signed tag naming source {record['sourceCommit'][:12]}, "
        "republished nothing, refused four malformed publications, regenerated "
        "byte-identically from its recorded source commit while refusing a hand-edited "
        "mirror, and supplied an external component that booted the QEMU component graph"
    )


if __name__ == "__main__":
    main()
