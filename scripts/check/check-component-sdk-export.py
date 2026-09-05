#!/usr/bin/env python3

"""CP6: the SDK export is deterministic, self-describing, and boundary-clean.

Six properties, each with a deliberate negative control where the property can
be weakened:

1. Two isolated exports from one source tree are byte-identical and report one
   exported-tree identity.
2. Changing an allowlisted public source or a pin changes that identity; a
   product-only file does not.
3. The emitted record decodes through the generated Zutai binding and every
   digest it states matches the emitted bytes.
4. Cargo metadata for a minimal external repository resolves every SDK crate
   inside the exported tree, with nothing escaping to the source checkout.
5. A component built from a local git commit of the export enters CP4's
   external-artifact path and boots on the QEMU component graph.
6. No exported byte names a build-host path, and the export refuses a
   destination it would have to merge into.
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
import component_sdk_system  # noqa: E402
from component_sdk import ComponentSdkError  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"
CHECK = load_script("component_sdk_export_generation_check", "check/check-generation.py")
RELEASE_CHECKER = ROOT / "contracts" / "component-sdk-release" / "v1" / "check.zt"
PROFILE = "aarch64-sel4-qemu-virt"
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"
EXTERNAL_IMPLEMENTATION = "cp6-console"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK export check: {message}")


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    description: str,
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
    if process.returncode != 0:
        fail(f"{description} failed:\n{process.stdout}")
    return process


def export(destination: Path, *, source: Path = ROOT, version: str = "1.0.0"):
    """Export a mirror, recording this checkout's commit and repository.

    A mirror is not a git repository, so the identity fields it cannot discover
    are supplied. That is also what keeps the perturbation controls honest: the
    recorded commit is held fixed across them, so an identity change can only
    come from the exported bytes.
    """
    try:
        return component_sdk.export(
            destination,
            version=version,
            sdk_repository=SDK_REPOSITORY,
            profiles=(PROFILE,),
            source=source,
            prefix_source=ROOT,
            commit=component_sdk.source_commit(ROOT),
            repository=component_sdk.source_repository(ROOT),
        )
    except ComponentSdkError as error:
        fail(str(error))


# Everything the exporter reads out of a source tree, plus two files now
# published as system-corpus content and one genuinely unpublished file the
# negative control perturbs. Every export input is *derived* from
# `component_sdk`'s own declarations rather than restated, so a new exported
# path cannot leave the mirror silently incomplete -- which it did once, when
# the linker scripts became an export input and this list did not know.
PUBLISHED_PROBES = (
    "slime-root/src/main.rs",
    "contracts/generation-manifest/v1/compositions/sel4-demo.zti",
)
UNPUBLISHED_PROBE = "devlog/README.md"
MIRROR_PATHS = (
    component_sdk_system.COPY_ROOTS
    + ("Cargo.toml", "sel4/pins.toml", "contracts", "scripts/lib/component_sdk_system_entry.py")
    + PUBLISHED_PROBES
    + (UNPUBLISHED_PROBE,)
    + tuple(path for path, _ in component_sdk.EXPORT_CRATES)
    + component_sdk.VENDORED
    + component_sdk.LINKER_SCRIPTS
    + component_sdk.target_spec_source_paths()
)


def mirror(root: Path, name: str) -> Path:
    """A copy of the working tree's export inputs, so a probe cannot touch it.

    Deliberately not a `git worktree`: the exporter must be able to export the
    tree it is pointed at, and a worktree of `HEAD` omits both uncommitted
    contract sources and the unpopulated `deps/rust-sel4` submodule.
    """
    destination = root / name
    destination.mkdir(parents=True)
    for relative in MIRROR_PATHS:
        source = ROOT / relative
        target = destination / relative
        if target.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, target, ignore=component_sdk.COPY_IGNORE)
        else:
            shutil.copyfile(source, target)
    return destination


def prove_determinism(root: Path) -> tuple[str, str, Path]:
    """Two exports of one source tree, byte for byte.

    Both run from the same mirror rather than from the live checkout, so the
    identity this returns is directly comparable with the perturbed exports
    below: a mirror is what the sensitivity controls can modify.
    """
    source = mirror(root, "source")
    first = export(root / "export-a", source=source)
    second = export(root / "export-b", source=source)
    if first.tree_identity != second.tree_identity:
        fail("two isolated exports reported different exported-tree identities")
    if first.identity != second.identity:
        fail("two isolated exports reported different release-record identities")
    difference = subprocess.run(
        ["diff", "-r", str(first.root), str(second.root)],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if difference.returncode != 0:
        fail(f"two isolated exports differ:\n{difference.stdout}")
    component_sdk.verify_tree(first.root, first.record)
    component_sdk.verify_digests(first.root, first.record, source)
    shutil.rmtree(second.root)
    print("component SDK export: two isolated exports are byte-identical", flush=True)
    return first.tree_identity, first.identity.hex(), source


def prove_identity_sensitivity(root: Path, tree_baseline: str, release_baseline: str) -> None:
    """An allowlisted source or pin change moves the release identity; a
    product-only change does not.

    Both halves are needed. Without the first, the identity could be a constant;
    without the second, it could be a digest of the whole repository, which would
    make every unrelated product commit look like an SDK change.

    The comparison covers both identities the export reports. `treeIdentity` is
    a digest over the exported bytes; the release identity additionally covers
    everything the record declares about them. Watching both is what makes each
    probe's expectation a statement rather than an accident: an exported source
    change must move both, and a product-only change must move neither.
    """

    def append(addition: bytes):
        return lambda text: text + addition

    def retoolchain(text: bytes) -> bytes:
        return text.replace(
            b'toolchain = "nightly-2026-04-04"', b'toolchain = "nightly-2026-04-05"'
        )

    perturbations = (
        (
            "components/runtime/src/lib.rs",
            append(b"\n// CP6 export sensitivity probe.\n"),
            True,
            True,
        ),
        ("sel4/pins.toml", retoolchain, True, True),
        (
            "slime-root/src/main.rs",
            append(b"\n// CP6 product-only probe.\n"),
            True,
            True,
        ),
        (
            "slime-root/src/main.rs",
            append(b"\n// CP6 published product-source probe.\n"),
            True,
            True,
        ),
        (
            "contracts/generation-manifest/v1/compositions/sel4-demo.zti",
            append(b"\n-- CP6 published product-source probe.\n"),
            True,
            True,
        ),
        (
            UNPUBLISHED_PROBE,
            append(b"\n<!-- CP6 unpublished-file probe. -->\n"),
            False,
            False,
        ),
    )
    for index, (relative, mutate, release_moves, tree_moves) in enumerate(perturbations):
        tree = mirror(root, f"perturbed-{index}")
        target = tree / relative
        if not target.is_file():
            fail(f"perturbation target does not exist: {relative}")
        original = target.read_bytes()
        mutated = mutate(original)
        if mutated == original:
            fail(f"{relative}: the perturbation changed nothing, so it proves nothing")
        target.write_bytes(mutated)
        if component_sdk.allowlisted(relative) is not release_moves:
            fail(f"{relative}: the exporter's allowlist disagrees with this control")
        perturbed = export(root / f"export-perturbed-{index}", source=tree)
        if (perturbed.identity.hex() != release_baseline) is not release_moves:
            verb = "did not change" if release_moves else "changed"
            fail(f"perturbing {relative} {verb} the release identity")
        if (perturbed.tree_identity != tree_baseline) is not tree_moves:
            verb = "did not change" if tree_moves else "changed"
            fail(f"perturbing {relative} {verb} the exported-tree identity")
        shutil.rmtree(perturbed.root)
        shutil.rmtree(tree)
    print(
        "component SDK export: the release identity moves for an allowlisted source, a "
        "pin, and published product source, and holds for a file the export never reads",
        flush=True,
    )


def prove_record_decodes(sdk: Path, source: Path) -> dict:
    record = component_sdk.load_record(sdk)
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_COMPONENT_SDK_RELEASE_PATH"] = str(
        sdk / "component-sdk-release.zti"
    )
    decoded = subprocess.run(
        [str(binary()), "run", str(RELEASE_CHECKER)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if decoded.returncode != 0 or not decoded.stdout.startswith("#valid"):
        fail(f"emitted release record did not decode as #valid:\n{decoded.stdout}")
    # A record that decodes but disagrees with its own bytes would satisfy the
    # schema and still be wrong, so the digests are checked separately.
    component_sdk.verify_digests(sdk, record, source)
    truncated = sdk.parent / "truncated-record"
    truncated.mkdir()
    shutil.copyfile(sdk / "component-sdk-release.json", truncated / "component-sdk-release.json")
    shutil.copyfile(sdk / "component-sdk-release.zti", truncated / "component-sdk-release.zti")
    (truncated / "component-sdk-release.identity").write_text("0" * 64 + "\n", encoding="utf-8")
    try:
        component_sdk.load_record(truncated)
    except ComponentSdkError:
        pass
    else:
        fail("a record whose identity file disagreed with its bytes was accepted")
    print(
        f"component SDK export: the record decodes and states {len(record['crates'])} crate "
        f"and {len(record['contracts'])} contract identities matching its bytes",
        flush=True,
    )
    return record


def external_workspace(root: Path, sdk: Path, revision: str) -> Path:
    """A minimal consumer repository pinned to a local git commit of the export."""
    checkout = root / "cp6-consumer"
    (checkout / "console" / "src").mkdir(parents=True)
    url = sdk.resolve().as_uri()
    dependency = f'{{ git = "{url}", rev = "{revision}" }}'
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
    return checkout


def commit_sdk(sdk: Path) -> str:
    for arguments, description in (
        (["init", "-q"], "initialize export repository"),
        (["config", "user.email", "cp6@example.invalid"], "configure export git email"),
        (["config", "user.name", "CP6 export"], "configure export git name"),
        (["add", "."], "stage the export"),
        (["commit", "-qm", "sdk: generated export"], "commit the export"),
    ):
        run(["git", *arguments], cwd=sdk, description=description)
    revision = run(
        ["git", "rev-parse", "HEAD"], cwd=sdk, description="read export commit"
    ).stdout.strip()
    if len(revision) != 40:
        fail(f"export commit is not a full SHA-1 identity: {revision!r}")
    return revision


def assert_metadata_boundary(checkout: Path, sdk: Path, revision: str) -> None:
    metadata = run(
        ["cargo", "metadata", "--format-version", "1", "--quiet"],
        cwd=checkout,
        description="resolve external Cargo metadata",
    )
    data = json.loads(metadata.stdout)
    expected = {name for _, name in component_sdk.EXPORT_CRATES}
    seen: set[str] = set()
    for package in data["packages"]:
        manifest = Path(package["manifest_path"]).resolve()
        if manifest.is_relative_to(ROOT.resolve()):
            fail(f"external dependency escaped to the source checkout: {manifest}")
        if package["name"] in expected:
            source = package.get("source") or ""
            if not source.startswith("git+") or f"#{revision}" not in source:
                fail(f"{package['name']} did not resolve through the pinned export commit")
            seen.add(package["name"])
    missing = sorted(expected - seen)
    if missing:
        fail(f"external metadata did not resolve every SDK crate: {missing}")
    for path in checkout.rglob("*"):
        if not path.is_file() or ".git" in path.parts:
            continue
        if str(ROOT) in path.read_text(encoding="utf-8", errors="ignore"):
            fail(f"external checkout names the source checkout: {path}")
    print(
        f"component SDK export: {len(seen)} SDK crates resolved through the pinned "
        "export commit and none escaped to the source checkout",
        flush=True,
    )


def build_external_component(root: Path, sdk: Path, checkout: Path) -> Path:
    target_dir = root / "external-target"
    verified = run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
            "--profile",
            PROFILE,
            "--verify-only",
            "--cache",
            str(root / "prefix-cache"),
        ],
        cwd=checkout,
        description="verify the release record before building",
    )
    # The entry point must verify the record and the prefix before it builds
    # anything. Asserted rather than assumed: a build that silently skipped
    # verification would still produce an ELF and pass every check below.
    if "verified" not in verified.stdout:
        fail(f"the SDK build entry point did not report a verified release:\n{verified.stdout}")
    run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
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
        description="build the external component through the SDK entry point",
    )
    elf = target_dir / "aarch64-sel4-minimal" / "release" / "console.elf"
    if not elf.is_file():
        fail("the SDK build entry point produced no console ELF")
    workspace = (
        ROOT
        / "target"
        / "components"
        / PROFILE
        / "generation-1"
        / "aarch64-sel4-minimal"
        / "release"
        / "console.elf"
    )
    if workspace.is_file() and workspace.read_bytes() == elf.read_bytes():
        fail("the external ELF is byte-identical to the workspace artifact")
    return elf


def external_specs(destination: Path, digest: str) -> None:
    for entry in admit_specs():
        spec = copy.deepcopy(entry.spec)
        if entry.name == "console":
            spec["implementation"] = {
                "provider": "external",
                "binary": EXTERNAL_IMPLEMENTATION,
                "contentHash": digest,
            }
        (destination / f"{entry.name}.zti").write_text(
            component_sdk.zti(spec) + "\n", encoding="utf-8"
        )


def prove_boot(root: Path, elf: Path, slisp: Path) -> None:
    specs = root / "specs"
    specs.mkdir()
    external_specs(specs, hashlib.sha256(elf.read_bytes()).hexdigest())
    output = root / "mixed-generation"
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
        description="build the mixed generation",
    )
    marker = f"implementation={EXTERNAL_IMPLEMENTATION} provider=external"
    if marker not in built.stdout:
        fail("the generation builder did not report the SDK-built component as external")
    generation = CHECK.check_generation((output / "generation.bin").read_bytes())
    store = CHECK.check_bootstore((output / "boot-store.bin").read_bytes())
    if store["selected"]["identity"] != generation["identity"]:
        fail("the signed boot store did not select the mixed generation")
    run(
        [
            sys.executable,
            str(SEL4_BUILDER),
            "--component-graph",
            "--prebuilt-generation",
            str(output / "generation.bin"),
        ],
        cwd=ROOT,
        description="embed the mixed generation",
    )
    manifest = json.loads(
        (ROOT / "build" / "slime-sel4-graph.identity.json").read_text(encoding="utf-8")
    )
    embedded = manifest.get("generation")
    if not isinstance(embedded, dict) or embedded.get("identity") != generation["identity"].hex():
        fail("the seL4 image did not embed the exact signed mixed generation")
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description="boot the mixed generation on the QEMU component graph",
    )
    print(
        "component SDK export: an SDK-built external ELF entered a signed generation "
        "and booted the QEMU component graph",
        flush=True,
    )


def prove_refusals(root: Path) -> None:
    populated = root / "populated"
    populated.mkdir()
    (populated / "stray").write_text("stray\n", encoding="utf-8")
    for description, keywords in (
        ("an export into a populated destination", ("already exists",)),
        ("an export at an invalid version", ("MAJOR.MINOR.PATCH",)),
        ("an export naming an unknown profile", ("unknown SDK target profile",)),
    ):
        try:
            if "populated" in description:
                component_sdk.export(
                    populated,
                    version="1.0.0",
                    sdk_repository=SDK_REPOSITORY,
                    profiles=(PROFILE,),
                )
            elif "version" in description:
                component_sdk.export(
                    root / "bad-version",
                    version="1.0",
                    sdk_repository=SDK_REPOSITORY,
                    profiles=(PROFILE,),
                )
            else:
                component_sdk.export(
                    root / "bad-profile",
                    version="1.0.0",
                    sdk_repository=SDK_REPOSITORY,
                    profiles=("aarch64-unknown-none",),
                )
        except ComponentSdkError as error:
            if not any(keyword in str(error) for keyword in keywords):
                fail(f"{description} was refused for the wrong reason: {error}")
        else:
            fail(f"{description} was not refused")
    print("component SDK export: three malformed export requests were refused", flush=True)


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-export-") as temporary:
        root = Path(temporary)
        tree_identity, release_identity, source = prove_determinism(root)
        prove_identity_sensitivity(root, tree_identity, release_identity)
        sdk = root / "export-a"
        record = prove_record_decodes(sdk, source)
        revision = commit_sdk(sdk)
        checkout = external_workspace(root, sdk, revision)
        assert_metadata_boundary(checkout, sdk, revision)
        elf = build_external_component(root, sdk, checkout)
        slisp = build_product_slisp(root / "slisp.elf")
        prove_boot(root, elf, slisp)
        prove_refusals(root)

    print(
        f"component SDK export check: one exporter produced SDK {record['version']} "
        f"({tree_identity[:16]}) twice byte-identically, described it in a decoding "
        "component-sdk-release/v1 record whose every digest matched the emitted bytes, "
        "resolved five SDK crates through a pinned commit with no path into this "
        "checkout, and booted an SDK-built external component on the QEMU component graph"
    )


if __name__ == "__main__":
    main()
