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
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from boot_contracts import (  # noqa: E402
    COMPONENT_IMAGE_ELF_MAGIC,
    COMPONENT_IMAGE_HEADER_TARGET_PROFILE_OFFSET,
    COMPONENT_IMAGE_MAGIC,
)
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
    sdk: Path, checkout: Path, profile: str, root: Path, record: dict, *, label: str
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
    # Cargo names the output directory by the JSON specification's file stem or
    # by the triple itself, and a `[[bin]]` for a triple target carries no `.elf`
    # suffix. Both come from the record's own `cargoTarget` rather than being
    # assumed, which is the same resolution `tools/sdk-update.py` performs.
    asset = next(entry for entry in record["profiles"] if entry["profile"] == profile)
    release = target_dir / Path(asset["cargoTarget"]).stem / "release"
    elf = next(
        (path for path in (release / "console.elf", release / "console") if path.is_file()), None
    )
    if elf is None:
        fail(f"{profile}: the SDK build produced no component artifact in {release}")
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
    root: Path,
    elf: Path,
    *,
    profile: str,
    label: str,
    prefix: str,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Build a generation whose workspace components come from the SDK's prefix.

    `SEL4_PREFIX` is the SDK-verified, extracted prefix rather than
    `build/sel4-prefix*`. `build-generation.py` compiles the workspace component
    wrappers itself, and `sel4-config-data` resolves libsel4 through that
    variable, so pointing it at the asset is what makes this arm test the asset:
    the generation's own components are built against the published prefix, not
    only the external ELF.
    """
    specs = root / f"specs-{label}"
    specs.mkdir()
    external_specs(specs, hashlib.sha256(elf.read_bytes()).hexdigest())
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = profile
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    environment["SEL4_PREFIX"] = prefix
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


def prove_qemu_boot(root: Path, elf: Path, prefix: str) -> None:
    build_generation(root, elf, profile=QEMU_PROFILE, label="qemu", prefix=prefix)
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


def declared_profile_ids(root: Path) -> tuple[int, int]:
    """The target-profile id each generation wrapped the external ELF under.

    Read out of the generation bytes rather than inferred: the wrapper's
    `targetProfile` field is exactly what `boot_contracts::target_profile::admit`
    compares before a component's bytes are mapped, so reading it is what makes
    "the profiles are not interchangeable" an observation.

    Both wrapper magics are accepted because both are in use and carry the same
    header: the seL4 profile wraps a whole ELF (`SLIMECME`) while the bare-metal
    triple re-bases segments onto the profile's component base (`SLIMECM2`). That
    difference is the point of the comparison, not an obstacle to it.
    """
    found = []
    for label in ("rpi", "rpi-into-qemu"):
        data = (root / f"generation-{label}" / "generation.bin").read_bytes()
        index = min(
            (
                position
                for position in (
                    data.find(COMPONENT_IMAGE_ELF_MAGIC),
                    data.find(COMPONENT_IMAGE_MAGIC),
                )
                if position >= 0
            ),
            default=-1,
        )
        if index < 0:
            fail(f"{label}: the generation carries no wrapped component image")
        (profile_id,) = struct.unpack_from(
            "<I", data, index + COMPONENT_IMAGE_HEADER_TARGET_PROFILE_OFFSET
        )
        found.append(profile_id)
    return found[0], found[1]


def prove_rpi_qualification(
    root: Path, rpi_elf: Path, qemu_elf: Path, rpi_prefix: str, qemu_prefix: str
) -> None:
    """The RPi ELF is `bcm2712`-qualified, and the profiles are not interchangeable.

    Two directions, because the two profiles fail differently and both matter:

    * A QEMU-target ELF entering an RPi generation is refused at build time. The
      `aarch64-unknown-none` wrapper requires the fixed component load base its
      target profile declares, and a seL4 JSON-target ELF links at its own
      addresses, so the host refuses it outright.
    * An RPi ELF entering a QEMU generation is *admitted* at build time and
      refused by the root before any of its bytes are mapped. That asymmetry is
      real rather than a gap: the seL4 wrapper carries the profile's id, ABI, and
      feature mask, and `boot_contracts::target_profile::admit` compares them by
      exact equality when the image is loaded. The refusal therefore belongs to
      the boot path, which is exactly where `just sel4_demo_check`'s wrong-target
      arm observes it, so this gate asserts the wrapper's declared identity
      rather than restating a boot assertion that already exists.

    Host-side qualification only. The board's own gates are the only evidence
    that can claim a physical boot, and this arm deliberately makes no such
    claim.
    """
    admitted = build_generation(
        root, rpi_elf, profile=RPI_PROFILE, label="rpi", prefix=rpi_prefix
    )
    if f"implementation={EXTERNAL_IMPLEMENTATION} provider=external" not in admitted.stdout:
        fail("the RPi generation did not report the SDK-built component as external")

    refused = build_generation(
        root,
        qemu_elf,
        profile=RPI_PROFILE,
        label="qemu-into-rpi",
        prefix=rpi_prefix,
        allow_failure=True,
    )
    if refused.returncode == 0:
        fail("a QEMU-target ELF was admitted into an RPi-profile generation")
    if "load layout" not in refused.stdout:
        fail(f"the cross-profile refusal did not name the layout mismatch:\n{refused.stdout}")
    if (root / "generation-qemu-into-rpi" / "generation.bin").exists():
        fail("the cross-profile refusal left a signed generation artifact")

    # The other direction: admitted at build time, and the wrapper it produced
    # declares the RPi profile rather than the QEMU one, which is what the root
    # compares before mapping.
    crossed = build_generation(
        root,
        rpi_elf,
        profile=QEMU_PROFILE,
        label="rpi-into-qemu",
        prefix=qemu_prefix,
    )
    if f"implementation={EXTERNAL_IMPLEMENTATION} provider=external" not in crossed.stdout:
        fail("the cross-profile generation did not report the component as external")
    rpi_id, qemu_id = declared_profile_ids(root)
    if rpi_id == qemu_id:
        fail("both generations wrapped the component under one profile identity")
    print(
        f"component SDK prefix: the RPi asset produced a bcm2712-qualified ELF (profile "
        f"id {rpi_id}) the RPi profile admits, the QEMU-target ELF was refused by the "
        f"RPi build, and a QEMU generation wrapping it declares profile id {qemu_id} for "
        "the root to refuse before mapping (host-side qualification only)",
        flush=True,
    )


def prove_malformed_archives_are_refused(root: Path, sdk: Path, record: dict) -> None:
    """Corrupt, truncated, swapped, and mismatched archives, each refused."""
    qemu = next(entry for entry in record["profiles"] if entry["profile"] == QEMU_PROFILE)
    rpi = next(entry for entry in record["profiles"] if entry["profile"] == RPI_PROFILE)
    original = (sdk / qemu["prefix"]["archive"]).read_bytes()
    archive = sdk / qemu["prefix"]["archive"]
    swapped = (sdk / rpi["prefix"]["archive"]).read_bytes()

    # The corruption flips one bit at the archive's midpoint rather than zeroing a
    # range: a tar carries long runs of zero padding, and an earlier version of
    # this control overwrote 512 padding bytes with zeros, changed nothing, and so
    # proved nothing. The mutation is asserted to differ below.
    corrupt = bytearray(original)
    corrupt[len(corrupt) // 2] ^= 0xFF
    cases = (
        ("corrupt", bytes(corrupt)),
        ("truncated", original[: len(original) // 3]),
        ("swapped-profile", swapped),
    )
    try:
        for label, mutated in cases:
            if mutated == original:
                fail(f"the {label} mutation did not change the archive, so it proves nothing")
            archive.write_bytes(mutated)
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
            if "identity" not in built.stdout and "hash" not in built.stdout:
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
        qemu_elf, qemu_export = build_through_sdk(
            sdk, checkout, QEMU_PROFILE, root, record, label="qemu"
        )
        rpi_elf, rpi_export = build_through_sdk(
            sdk, checkout, RPI_PROFILE, root, record, label="rpi"
        )
        if qemu_export == rpi_export:
            fail("both profiles resolved to one prefix, so the assets are not separate")
        if qemu_elf.read_bytes() == rpi_elf.read_bytes():
            fail("the two profiles produced byte-identical ELFs, so neither is qualified")
        qemu_prefix = qemu_export.split("=", 1)[1]
        rpi_prefix = rpi_export.split("=", 1)[1]
        prove_qemu_boot(root, qemu_elf, qemu_prefix)
        prove_rpi_qualification(root, rpi_elf, qemu_elf, rpi_prefix, qemu_prefix)
        prove_malformed_archives_are_refused(root, sdk, record)

    print(
        "component SDK prefix check: one immutable SDK release supplied verified "
        f"{QEMU_PROFILE} and {RPI_PROFILE} seL4 prefixes; an external checkout built "
        "target-qualified ELFs against each with SEL4_PREFIX poisoned and no reference "
        "to build/sel4-prefix*, the QEMU ELF booted the component graph, the RPi ELF "
        "was admitted only for bcm2712 while the QEMU-target ELF was refused by the "
        "RPi build, and four malformed archives were refused before Cargo ran"
    )


if __name__ == "__main__":
    main()
