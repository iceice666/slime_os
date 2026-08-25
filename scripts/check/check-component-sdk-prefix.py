#!/usr/bin/env python3

"""CP8: platform build inputs come from the SDK release, not from `slime_os/build`.

Two profiles are published, `aarch64-sel4-qemu-virt` and `aarch64-rpi5`. The gate
proves each asset is content-addressed and reproducible, that an external build
consumes only the SDK clone and its extracted prefix, that the QEMU asset yields
a booting component, that the RPi asset yields a component the QEMU profile
refuses as wrong-target, and that corrupt, truncated, swapped, and
metadata-mismatched archives are refused before Cargo or bindgen runs.

The RPi arm is host-side target qualification only. It says an external build
against the `bcm2712-rpi5` prefix produces a `bcm2712`-qualified image and that
the QEMU profile refuses it; it says nothing about a physical board, which only
this repository's board gates can claim.
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
from component_sdk import ComponentSdkError  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"
CHECK = load_script("component_sdk_prefix_generation_check", "check/check-generation.py")
SDK_REPOSITORY = "https://github.com/iceice666/slime_os-component_sdk"
QEMU_PROFILE = "aarch64-sel4-qemu-virt"
RPI_PROFILE = "aarch64-rpi5"
EXTERNAL_IMPLEMENTATION = "cp8-console"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK prefix check: {message}")


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


def export(root: Path):
    try:
        return component_sdk.export(
            root / "sdk",
            version="1.0.0",
            sdk_repository=SDK_REPOSITORY,
            profiles=(QEMU_PROFILE, RPI_PROFILE),
            source=ROOT,
        )
    except ComponentSdkError as error:
        fail(str(error))


def prove_archives_are_clean(sdk: Path, record: dict, root: Path) -> dict[str, Path]:
    """Extraction into an empty directory reproduces the recorded prefix identity.

    The extracted trees are also scanned for host paths. That is a distinct claim
    from the archive hash matching: an archive can be perfectly reproducible and
    still carry this checkout's path inside a generated header, which is exactly
    the leak the exporter canonicalizes.
    """
    cache = root / "extracted"
    extracted = component_sdk.verify_prefix_extraction(sdk, record, cache)
    for profile, prefix in extracted.items():
        component_sdk.assert_no_host_paths(prefix, ROOT)
        kernel = prefix / "bin" / "kernel.elf"
        if not kernel.is_file():
            fail(f"{profile}: extracted prefix has no kernel")
        asset = next(entry for entry in record["profiles"] if entry["profile"] == profile)
        digest = hashlib.sha256(kernel.read_bytes()).hexdigest()
        if digest != asset["prefix"]["kernelHash"]:
            fail(f"{profile}: extracted kernel does not match the recorded pin hash")
        recipe = asset["prefix"]["rebuildRecipe"]
        if "build-sel4.py" not in recipe or "check-sel4-pins.py" not in recipe:
            fail(f"{profile}: the published rebuild recipe does not name the build and pin gates")
    print(
        "component SDK prefix: both profile archives extracted to their recorded tree "
        "identities with no build-host path and a pin-matching kernel",
        flush=True,
    )
    return extracted


def consumer(root: Path, sdk: Path, name: str) -> Path:
    """A minimal external checkout, path-pinned to the SDK clone.

    A path pin rather than a git pin: CP6 and CP7 already prove git-pinned
    resolution, and what CP8 is about is the *platform asset*, so the dependency
    edge is held boring on purpose.
    """
    checkout = root / name
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
    return checkout


def build_through_sdk(
    sdk: Path, checkout: Path, profile: str, root: Path, *, label: str
) -> tuple[Path, str]:
    """Build through the SDK entry point and prove where its inputs came from.

    The witness is structural rather than a syscall trace. The consumer's
    manifest names only the SDK clone, the build writes into the temporary tree,
    and the ambient environment is *poisoned* first: `SEL4_PREFIX` points at a
    path that does not exist and `SLIME_TARGET_PROFILE` names the wrong profile.
    A build that still succeeds, and reports a prefix outside this checkout, can
    only have obtained both from the release record -- which is the claim CP8
    owes, since an entry point that merely inherited a correct ambient prefix
    would pass a naive check while leaving the consumer dependent on
    `slime_os/build`.
    """
    target_dir = root / f"target-{label}"
    environment = os.environ.copy()
    environment["SEL4_PREFIX"] = str(root / "definitely-not-a-prefix")
    environment["SLIME_TARGET_PROFILE"] = "aarch64-unknown-none"
    built = run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
            "--profile",
            profile,
            "--manifest-path",
            str(checkout / "Cargo.toml"),
            "--package",
            EXTERNAL_IMPLEMENTATION,
            "--target-dir",
            str(target_dir),
            "--cache",
            str(root / "prefix-cache"),
            "--print-environment",
        ],
        cwd=checkout,
        env=environment,
        description=f"build the external component for {profile}",
    )
    if str(ROOT / "build") in built.stdout:
        fail(f"{profile}: the SDK build referenced the source checkout's prefix")
    exported_prefix = next(
        (line for line in built.stdout.splitlines() if line.startswith("SEL4_PREFIX=")), ""
    )
    if str(ROOT) in exported_prefix or not exported_prefix:
        fail(f"{profile}: the SDK build did not export its own verified prefix")
    elf = target_dir / "aarch64-sel4-minimal" / "release" / "console.elf"
    if not elf.is_file():
        fail(f"{profile}: the SDK build produced no component ELF")
    return elf, exported_prefix


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


def build_generation(
    root: Path, elf: Path, *, profile: str, label: str, allow_failure: bool = False
) -> subprocess.CompletedProcess[str]:
    specs = root / f"specs-{label}"
    specs.mkdir()
    external_specs(specs, hashlib.sha256(elf.read_bytes()).hexdigest())
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = profile
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    return run(
        [
            sys.executable,
            str(BUILDER),
            "--component-spec-root",
            str(specs),
            "--external-component",
            f"{EXTERNAL_IMPLEMENTATION}={elf}",
            str(root / f"generation-{label}"),
        ],
        cwd=ROOT,
        env=environment,
        description=f"build the {label} generation",
        allow_failure=allow_failure,
    )


def prove_qemu_boot(root: Path, elf: Path) -> None:
    build_generation(root, elf, profile=QEMU_PROFILE, label="qemu")
    output = root / "generation-qemu"
    generation = CHECK.check_generation((output / "generation.bin").read_bytes())
    store = CHECK.check_bootstore((output / "boot-store.bin").read_bytes())
    if store["selected"]["identity"] != generation["identity"]:
        fail("the signed boot store did not select the SDK-prefix generation")
    run(
        [
            sys.executable,
            str(SEL4_BUILDER),
            "--component-graph",
            "--prebuilt-generation",
            str(output / "generation.bin"),
        ],
        cwd=ROOT,
        description="embed the SDK-prefix generation",
    )
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description="boot the SDK-prefix generation",
    )
    print(
        "component SDK prefix: the QEMU asset produced an ELF that entered a signed "
        "generation and booted the QEMU component graph",
        flush=True,
    )


def prove_rpi_qualification(root: Path, elf: Path) -> None:
    """The RPi ELF is `bcm2712`-qualified, and the QEMU profile refuses it.

    Host-side qualification only. The board's own gates are the only evidence
    that can claim a physical boot, and this arm deliberately makes no such
    claim.
    """
    admitted = build_generation(root, elf, profile=RPI_PROFILE, label="rpi")
    if f"implementation={EXTERNAL_IMPLEMENTATION} provider=external" not in admitted.stdout:
        fail("the RPi generation did not report the SDK-built component as external")
    refused = build_generation(
        root, elf, profile=QEMU_PROFILE, label="rpi-wrong-target", allow_failure=True
    )
    if refused.returncode == 0:
        fail("a bcm2712-qualified ELF was admitted into a QEMU-profile generation")
    if "target" not in refused.stdout.lower():
        fail(f"the wrong-target refusal did not name the target mismatch:\n{refused.stdout}")
    if (root / "generation-rpi-wrong-target" / "generation.bin").exists():
        fail("the wrong-target refusal left a signed generation artifact")
    print(
        "component SDK prefix: the RPi asset produced a bcm2712-qualified ELF that the "
        "QEMU profile refused as wrong-target (host-side qualification only)",
        flush=True,
    )


def prove_malformed_archives_are_refused(root: Path, sdk: Path, record: dict) -> None:
    """Corrupt, truncated, swapped, and mismatched archives, each refused."""
    qemu = next(entry for entry in record["profiles"] if entry["profile"] == QEMU_PROFILE)
    rpi = next(entry for entry in record["profiles"] if entry["profile"] == RPI_PROFILE)
    original = (sdk / qemu["prefix"]["archive"]).read_bytes()
    archive = sdk / qemu["prefix"]["archive"]
    swapped = (sdk / rpi["prefix"]["archive"]).read_bytes()

    cases = (
        ("corrupt", bytearray(original[:2048] + b"\0" * 512 + original[2560:])),
        ("truncated", bytearray(original[: len(original) // 3])),
        ("swapped-profile", bytearray(swapped)),
    )
    try:
        for label, mutated in cases:
            archive.write_bytes(bytes(mutated))
            built = run(
                [
                    sys.executable,
                    str(sdk / "tools" / "sdk-build.py"),
                    "--profile",
                    QEMU_PROFILE,
                    "--verify-only",
                    "--cache",
                    str(root / f"cache-{label}"),
                ],
                cwd=sdk,
                description=f"verify the {label} archive",
                allow_failure=True,
            )
            if built.returncode == 0:
                fail(f"the {label} prefix archive was accepted")
            if "hash" not in built.stdout and "truncat" not in built.stdout:
                fail(f"the {label} refusal did not name the mismatch:\n{built.stdout}")
    finally:
        archive.write_bytes(original)

    # A metadata mismatch: the archive is intact but the record disagrees with it.
    # Refused by the same verification, which is what makes the record load-bearing
    # rather than decorative.
    normalized = json.loads((sdk / "component-sdk-release.json").read_text(encoding="utf-8"))
    for entry in normalized["profiles"]:
        if entry["profile"] == QEMU_PROFILE:
            entry["prefix"]["archiveHash"] = "0" * 64
    mismatched = component_sdk.normalize(normalized)
    identity = hashlib.sha256(
        component_sdk.default_contract.IDENTITY_DOMAIN + mismatched
    ).hexdigest()
    (sdk / "component-sdk-release.json").write_bytes(mismatched)
    (sdk / "component-sdk-release.identity").write_text(identity + "\n", encoding="utf-8")
    refused = run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
            "--profile",
            QEMU_PROFILE,
            "--verify-only",
            "--cache",
            str(root / "cache-mismatch"),
        ],
        cwd=sdk,
        description="verify a metadata-mismatched release",
        allow_failure=True,
    )
    if refused.returncode == 0:
        fail("a release whose record disagreed with its archive was accepted")
    print(
        "component SDK prefix: corrupt, truncated, swapped-profile, and "
        "metadata-mismatched archives were each refused before Cargo ran",
        flush=True,
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-prefix-") as temporary:
        root = Path(temporary)
        exported = export(root)
        sdk = exported.root
        record = exported.record
        prove_archives_are_clean(sdk, record, root)
        checkout = consumer(root, sdk, "cp8-consumer")
        qemu_elf, qemu_prefix = build_through_sdk(sdk, checkout, QEMU_PROFILE, root, label="qemu")
        rpi_elf, rpi_prefix = build_through_sdk(sdk, checkout, RPI_PROFILE, root, label="rpi")
        if qemu_prefix == rpi_prefix:
            fail("both profiles resolved to one prefix, so the assets are not separate")
        if qemu_elf.read_bytes() == rpi_elf.read_bytes():
            fail("the two profiles produced byte-identical ELFs, so neither is qualified")
        prove_qemu_boot(root, qemu_elf)
        prove_rpi_qualification(root, rpi_elf)
        prove_malformed_archives_are_refused(root, sdk, record)

    print(
        "component SDK prefix check: one immutable SDK release supplied verified "
        f"{QEMU_PROFILE} and {RPI_PROFILE} seL4 prefixes; an external checkout built "
        "target-qualified ELFs against each with no reference to build/sel4-prefix*, "
        "the QEMU ELF booted the component graph, the RPi ELF was admitted for "
        "bcm2712 and refused by the QEMU profile, and four malformed archives were "
        "refused before Cargo ran"
    )


if __name__ == "__main__":
    main()
