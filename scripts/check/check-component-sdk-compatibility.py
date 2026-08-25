#!/usr/bin/env python3

"""CP9: version policy and an evidence-backed compatibility matrix.

Two real immutable releases are published to a stand-in canonical repository and
classified against each other; every published matrix row is backed by a build
and the narrowest QEMU boot gate that pairing was actually exercised by.

The negative controls change one compatibility identity at a time and require
the expected classification, including the case CP9 exists for: equal crate
versions across a changed syscall ABI must be refused rather than accepted
because Cargo would still compile.

An untested pairing is asked for explicitly and must come back unsupported. That
is the whole of the matrix's meaning: absence is not implicit compatibility.
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

import component_sdk  # noqa: E402
import component_sdk_release_contract as contract  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"
PUBLISHER = ROOT / "scripts" / "build" / "publish-component-sdk.py"
CHECK = load_script("component_sdk_compat_generation_check", "check/check-generation.py")
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"
BRANCH = "generated"
QEMU_PROFILE = "aarch64-sel4-qemu-virt"
RPI_PROFILE = "aarch64-rpi5"
SIGNING_KEY = ROOT / "contracts" / "release" / "v1" / "test-keys" / "key1"
EXTERNAL_IMPLEMENTATION = "cp9-console"
MATRIX = ROOT / "sdk" / "compatibility-matrix.zti"
GRAPH_GATE = "just sel4_component_graph_check"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK compatibility check: {message}")


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


def publish(url: str, version: str, profiles: tuple[str, ...]) -> None:
    command = [
        sys.executable,
        str(PUBLISHER),
        "--version",
        version,
        "--sdk-url",
        url,
        "--branch",
        BRANCH,
        "--signing-key",
        str(SIGNING_KEY),
        "--push",
        "--source-commit",
        "HEAD",
    ]
    for profile in profiles:
        command += ["--profile", profile]
    # `--source-commit HEAD` is deliberate: this gate runs during development,
    # where `contracts/` and the SDK library are the very files being changed, so
    # publication is asked for the committed tree explicitly. CP7 owns the
    # defaulted-HEAD dirty refusal and proves it; repeating it here would only
    # mean this gate could not run at all.
    run(command, cwd=ROOT, description=f"publish SDK {version}", allow_failure=True)


def canonical_remote(root: Path) -> str:
    bare = root / "slime_os-component_sdk.git"
    run(
        ["git", "init", "--quiet", "--bare", "--initial-branch", BRANCH, str(bare)],
        cwd=root,
        description="create the stand-in canonical repository",
    )
    return str(bare)


def clone(url: str, destination: Path) -> Path:
    run(
        ["git", "clone", "--quiet", "--branch", BRANCH, url, str(destination)],
        cwd=destination.parent,
        description="clone the published SDK",
    )
    return destination


def head(path: Path) -> str:
    return run(["git", "rev-parse", "HEAD"], cwd=path, description="read HEAD").stdout.strip()


def prove_two_real_releases(root: Path, url: str) -> tuple[dict, dict, str, str]:
    """Publish two immutable releases and classify the second against the first.

    The second adds the RPi profile, which is a genuine `compatible-feature`
    rather than a synthetic mutation: the exported source, contracts, toolchain,
    and rust-sel4 pin are all unchanged, and one new keyed profile appears. That
    is exactly the change CP9's structural axes exist to classify.
    """
    publish(url, "1.0.0", (QEMU_PROFILE,))
    first_clone = clone(url, root / "release-1")
    first = component_sdk.load_record(first_clone)
    component_sdk.verify_tree(first_clone, first)
    first_commit = head(first_clone)

    publish(url, "1.1.0", (QEMU_PROFILE, RPI_PROFILE))
    second_clone = clone(url, root / "release-2")
    second = component_sdk.load_record(second_clone)
    component_sdk.verify_tree(second_clone, second)
    second_commit = head(second_clone)

    if first_commit == second_commit:
        fail("the two releases share one commit, so they are not two releases")
    classification = component_sdk.admit_version_change(first, second)
    if classification != contract.CLASSIFICATION_COMPATIBLE_FEATURE:
        fail(
            "adding one target profile should classify as compatible-feature, "
            f"got {classification}"
        )
    if component_sdk.classify(None, first) != contract.CLASSIFICATION_INITIAL:
        fail("the first release did not classify as initial")
    print(
        f"component SDK compatibility: published 1.0.0 ({first_commit[:12]}) and "
        f"1.1.0 ({second_commit[:12]}); the second classified as {classification}",
        flush=True,
    )
    return first, second, first_commit, second_commit


def prove_negative_controls(first: dict, second: dict) -> None:
    """One identity changed at a time, each forcing its expected classification."""
    for axis in contract.BREAKING_AXES:
        mutated = copy.deepcopy(second)
        mutated["compatibility"][axis] = "f" * 64
        mutated["version"] = "1.2.0"
        classification = component_sdk.classify(second, mutated)
        if classification != contract.CLASSIFICATION_BREAKING:
            fail(f"changing {axis} classified as {classification}, not breaking")
        try:
            component_sdk.admit_version_change(second, mutated)
        except ComponentSdkError as error:
            # The refusal must name both what was required and what moved:
            # a message that said only "refused" would leave an operator
            # guessing which identity changed.
            if contract.CLASSIFICATION_BREAKING not in str(error) or axis not in str(error):
                fail(f"a changed {axis} under a minor bump was refused wrongly: {error}")
        else:
            fail(f"a changed {axis} was admitted under a minor version bump")

    # A structural entry mutated rather than added: breaking. This is the arm a
    # single set digest could not distinguish from the compatible-feature case
    # above, so it is the one that proves the structural comparison is real.
    for axis, key in zip(contract.STRUCTURAL_AXES, contract.STRUCTURAL_KEYS, strict=True):
        mutated = copy.deepcopy(second)
        mutated[axis][0] = copy.deepcopy(mutated[axis][0])
        if axis == "crates":
            mutated[axis][0]["identity"] = "e" * 64
        else:
            mutated[axis][0]["prefix"]["archiveHash"] = "e" * 64
        mutated["version"] = "1.2.0"
        classification = component_sdk.classify(second, mutated)
        if classification != contract.CLASSIFICATION_BREAKING:
            fail(f"changing an existing {axis} entry classified as {classification}")
        removed = copy.deepcopy(second)
        removed[axis] = removed[axis][1:]
        removed["version"] = "1.2.0"
        if not removed[axis]:
            # A single-entry axis cannot express removal; skip rather than assert
            # a property the record shape does not admit.
            continue
        if component_sdk.classify(second, removed) != contract.CLASSIFICATION_BREAKING:
            fail(f"removing an {axis} entry keyed by {key} was not breaking")

    # An unchanged release is a patch, and a version that does not advance is
    # refused outright. Both halves keep `patch` from meaning "anything".
    same = copy.deepcopy(second)
    same["version"] = "1.1.1"
    if component_sdk.classify(second, same) != contract.CLASSIFICATION_PATCH:
        fail("an unchanged release did not classify as a patch")
    stale = copy.deepcopy(second)
    stale["version"] = "1.0.0"
    try:
        component_sdk.admit_version_change(second, stale)
    except ComponentSdkError as error:
        if "does not advance" not in str(error):
            fail(f"a non-advancing version was refused wrongly: {error}")
    else:
        fail("a non-advancing version was admitted")

    # The claim CP9 exists to refuse: identical crate versions across a changed
    # syscall ABI. Cargo would compile a component against either, so a
    # version-based classifier would call this compatible.
    forged = copy.deepcopy(second)
    forged["version"] = "1.1.1"
    forged["compatibility"]["syscallAbi"] = "a" * 64
    if [entry["version"] for entry in forged["crates"]] != [
        entry["version"] for entry in second["crates"]
    ]:
        fail("the forged release changed a crate version, so it tests the wrong thing")
    try:
        component_sdk.admit_version_change(second, forged)
    except ComponentSdkError as error:
        if "syscallAbi" not in str(error):
            fail(f"the forged release was refused without naming the ABI: {error}")
    else:
        fail("a release claiming a patch across a changed syscall ABI was admitted")
    print(
        f"component SDK compatibility: {len(contract.BREAKING_AXES)} scalar axes, "
        f"{len(contract.STRUCTURAL_AXES)} structural axes, a non-advancing version, and "
        "equal crate versions across a changed syscall ABI were each classified correctly",
        flush=True,
    )


def build_and_boot(root: Path, sdk: Path, label: str) -> tuple[str, str]:
    """Build an external component from one SDK clone and boot its generation.

    This is what a matrix row is allowed to cite. A row without it would be a
    promise; with it, the row names the exact artifacts the boot observed.
    """
    checkout = root / f"consumer-{label}"
    (checkout / "console" / "src").mkdir(parents=True)
    (checkout / "Cargo.toml").write_text(
        "[workspace]\n"
        'resolver = "3"\n'
        'members = ["console"]\n'
        "\n"
        "[workspace.dependencies]\n"
        f'boot-contracts = {{ path = "{sdk}/boot-contracts" }}\n'
        f'slime-proto = {{ path = "{sdk}/components/proto" }}\n'
        f'slime-components = {{ path = "{sdk}/components/lib" }}\n'
        f'slime-rt = {{ path = "{sdk}/components/runtime" }}\n'
        f'slime-build-support = {{ path = "{sdk}/components/build-support" }}\n'
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
        ROOT / "components" / "bins" / "console" / "src" / "main.rs",
        checkout / "console" / "src" / "main.rs",
    )
    target_dir = root / f"target-{label}"
    run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
            "--profile",
            QEMU_PROFILE,
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
        description=f"build the {label} external component",
    )
    elf = target_dir / "aarch64-sel4-minimal" / "release" / "console.elf"
    if not elf.is_file():
        fail(f"{label}: the SDK build produced no component ELF")
    digest = hashlib.sha256(elf.read_bytes()).hexdigest()

    specs = root / f"specs-{label}"
    specs.mkdir()
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
    output = root / f"generation-{label}"
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = QEMU_PROFILE
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    run(
        [
            sys.executable,
            str(BUILDER),
            "--component-spec-root",
            str(specs),
            "--external-component",
            f"{EXTERNAL_IMPLEMENTATION}={elf}",
            str(output),
        ],
        cwd=ROOT,
        env=environment,
        description=f"build the {label} generation",
    )
    generation = CHECK.check_generation((output / "generation.bin").read_bytes())
    store = CHECK.check_bootstore((output / "boot-store.bin").read_bytes())
    if store["selected"]["identity"] != generation["identity"]:
        fail(f"{label}: the signed boot store did not select the generation")
    run(
        [
            sys.executable,
            str(SEL4_BUILDER),
            "--component-graph",
            "--prebuilt-generation",
            str(output / "generation.bin"),
        ],
        cwd=ROOT,
        description=f"embed the {label} generation",
    )
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description=f"boot the {label} generation",
    )
    return digest, generation["identity"].hex()


def main() -> None:
    product_commit = component_sdk.source_commit(ROOT)
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-compat-") as temporary:
        root = Path(temporary)
        url = canonical_remote(root)
        first, second, first_commit, second_commit = prove_two_real_releases(root, url)
        prove_negative_controls(first, second)

        # Every row's evidence, observed now rather than asserted. The prior
        # release's row is retained only because its external fixture still
        # builds, enters a generation, and boots against this product commit.
        first_elf, first_generation = build_and_boot(root, root / "release-1", "release-1")
        second_elf, second_generation = build_and_boot(root, root / "release-2", "release-2")
        if first_elf == second_elf:
            fail("both releases produced one ELF, so the two rows are one observation")

        rows = [
            component_sdk.matrix_row(
                first,
                sdk_commit=first_commit,
                product_commit=product_commit,
                profile=QEMU_PROFILE,
                classification=contract.CLASSIFICATION_INITIAL,
                evidence=(
                    GRAPH_GATE,
                    f"component-elf:{first_elf}",
                    f"generation:{first_generation}",
                ),
            ),
            component_sdk.matrix_row(
                second,
                sdk_commit=second_commit,
                product_commit=product_commit,
                profile=QEMU_PROFILE,
                classification=contract.CLASSIFICATION_COMPATIBLE_FEATURE,
                evidence=(
                    GRAPH_GATE,
                    f"component-elf:{second_elf}",
                    f"generation:{second_generation}",
                ),
            ),
        ]
        table = component_sdk.matrix(rows)
        identity = component_sdk.write_matrix(MATRIX, table)
        published, read_identity = component_sdk.read_matrix(MATRIX)
        if read_identity != identity:
            fail("the written matrix did not read back at its own identity")
        if published != table:
            fail("the written matrix did not read back as itself")

        for row in published["rows"]:
            if row["status"] != contract.STATUS_SUPPORTED:
                fail("a published row is not marked supported")
            if GRAPH_GATE not in row["evidence"]:
                fail("a published row does not cite the boot gate that backs it")
            if not component_sdk.supported(
                published,
                sdk_commit=row["sdkCommit"],
                product_commit=row["productCommit"],
                profile=row["profile"],
            ):
                fail("a published row did not answer as supported")

        # Three untested pairings, each of which SemVer or version-range
        # inference would happily accept: the RPi profile nobody booted, the
        # first SDK against a different product commit, and a synthetic future
        # SDK commit.
        untested = (
            (first_commit, product_commit, RPI_PROFILE),
            (first_commit, "0" * 40, QEMU_PROFILE),
            ("1" * 40, product_commit, QEMU_PROFILE),
        )
        for sdk_commit, other_product, profile in untested:
            if component_sdk.supported(
                published,
                sdk_commit=sdk_commit,
                product_commit=other_product,
                profile=profile,
            ):
                fail(f"an untested pairing was reported supported: {profile}")

        matrix_json = json.loads(MATRIX.with_suffix(".json").read_text(encoding="utf-8"))
        if len(matrix_json["rows"]) != 2:
            fail("the published matrix does not hold exactly the two evidenced rows")

    print(
        "component SDK compatibility check: two immutable releases were published and "
        f"classified (initial, compatible-feature); {len(contract.BREAKING_AXES)} scalar "
        f"and {len(contract.STRUCTURAL_AXES)} structural negative controls forced their "
        "expected classification, including equal crate versions across a changed "
        "syscall ABI; both published matrix rows are backed by a build plus a QEMU "
        "component-graph boot, and three untested pairings reported unsupported"
    )


if __name__ == "__main__":
    main()
