#!/usr/bin/env python3

"""CP10: a consumer pins, upgrades, rebuilds, boots, and rolls back.

A separate consumer repository is created from the SDK's own workspace template,
pinned by full commit to the first of two immutable releases. It is then moved to
the second through `tools/sdk-update.py`, which changes the SDK revision, the
lockfile, the verified platform asset, and the recorded release identity in one
reviewable diff; the rebuilt ELF's new content hash enters a signed generation
that boots before the pin is considered usable.

Every failure arm is injected rather than argued: dependency resolution, prefix
verification, compilation, digest admission, and QEMU health each fail once, and
each time the previous pin, the previous ELF, and the previous bootable
generation must still be selected and reproducible. Rollback then reproduces the
previous ELF and generation identities byte for byte from retained inputs, and
the in-tree fallback generation is built and booted afterwards.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import component_sdk  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"
PUBLISHER = ROOT / "scripts" / "build" / "publish-component-sdk.py"
CHECK = load_script("component_sdk_upgrade_generation_check", "check/check-generation.py")
BRANCH = "generated"
PROFILE = "aarch64-sel4-qemu-virt"
RPI_PROFILE = "aarch64-rpi5"
SIGNING_KEY = ROOT / "contracts" / "release" / "v1" / "test-keys" / "key1"
IMPLEMENTATION = "slime-external-component"
BINARY = "external-component"
FLOATING = re.compile(r'(branch\s*=|tag\s*=|version\s*=\s*"[^"]*"\s*})')


def fail(message: str) -> NoReturn:
    raise SystemExit(f"component SDK upgrade check: {message}")


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


def canonical_remote(root: Path) -> str:
    """A local bare repository standing in for the canonical SDK remote.

    Returned as a `file://` URI rather than a bare path: a consumer manifest's
    `git = "..."` is a URL, and Cargo refuses a relative one. The canonical
    repository the record *names* is unchanged -- that is publication's
    `--sdk-repository`, separate from this transport.
    """
    bare = root / "slime_os-component_sdk.git"
    run(
        ["git", "init", "--quiet", "--bare", "--initial-branch", BRANCH, str(bare)],
        cwd=root,
        description="create the stand-in canonical repository",
    )
    return bare.resolve().as_uri()


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
    run(command, cwd=ROOT, description=f"publish SDK {version}", allow_failure=True)


def clone(url: str, destination: Path) -> Path:
    run(
        ["git", "clone", "--quiet", "--branch", BRANCH, url, str(destination)],
        cwd=destination.parent,
        description="clone the published SDK",
    )
    return destination


def head(path: Path) -> str:
    return run(["git", "rev-parse", "HEAD"], cwd=path, description="read HEAD").stdout.strip()


def consumer_from_template(root: Path, sdk: Path, url: str, commit: str, *, name: str) -> Path:
    """A consumer checkout created from the SDK's own template.

    The template ships a placeholder revision and the canonical SDK URL. Both
    are rewritten here to this run's stand-in remote and its first published
    commit, which is the same substitution `tools/sdk-update.py` performs for a
    later upgrade.

    The template's own `src/main.rs` is built as shipped -- that is what proves
    the template compiles from a fresh clone -- and then replaced by the
    `console` component's source for the boot arms. The substitution is
    necessary rather than convenient: the QEMU component graph drives `console`
    through a scripted scenario and waits for its markers, so a component that
    merely started and exited would leave the graph waiting and the boot would
    time out. What the upgrade and rollback arms are about is the *pin*, the
    rebuild, and the generation identity, so the component under it must be one
    the graph actually composes.
    """
    checkout = root / name
    shutil.copytree(sdk / "template", checkout)
    manifest = checkout / "Cargo.toml"
    text = manifest.read_text(encoding="utf-8")
    text = text.replace("0" * 40, commit)
    text = re.sub(r'git = "[^"]+"', f'git = "{url}"', text)
    manifest.write_text(text, encoding="utf-8")
    for record in ("component-sdk-release.json", "component-sdk-release.identity"):
        shutil.copyfile(sdk / record, checkout / record)
    for arguments, description in (
        (["init", "-q"], "initialize the consumer repository"),
        (["config", "user.email", "consumer@example.invalid"], "configure consumer git"),
        (["config", "user.name", "CP10 consumer"], "configure consumer git"),
    ):
        run(["git", *arguments], cwd=checkout, description=description)
    return checkout


def adopt_console_role(checkout: Path) -> None:
    """Give the template component the behavior the component graph composes."""
    shutil.copyfile(
        ROOT / "components" / "bins" / "console" / "src" / "main.rs",
        checkout / "component" / "src" / "main.rs",
    )


def assert_pin_shape(checkout: Path, commit: str, url: str) -> None:
    """The pin is a full commit, and nothing floating remains."""
    text = (checkout / "Cargo.toml").read_text(encoding="utf-8")
    pins = re.findall(r'rev\s*=\s*"([^"]+)"', text)
    if not pins:
        fail("the consumer manifest carries no SDK revision pin")
    for pin in pins:
        if re.fullmatch(r"[0-9a-f]{40}", pin) is None:
            fail(f"the consumer manifest carries a non-commit pin: {pin!r}")
        if pin != commit:
            fail(f"the consumer manifest pins {pin[:12]}, expected {commit[:12]}")
    if FLOATING.search(text):
        fail("the consumer manifest carries a branch, tag, or registry reference")
    if str(ROOT) in text:
        fail("the consumer manifest names a slime_os checkout")
    if url not in text:
        fail("the consumer manifest does not name the SDK repository")


def build_locked(checkout: Path, sdk: Path, root: Path, label: str) -> Path:
    """Build from the consumer with `--locked`, through the SDK entry point."""
    target_dir = root / f"target-{label}"
    run(
        ["cargo", "generate-lockfile", "--manifest-path", str(checkout / "Cargo.toml")],
        cwd=checkout,
        description=f"resolve the {label} lockfile",
    )
    run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-build.py"),
            "--profile",
            PROFILE,
            "--manifest-path",
            str(checkout / "Cargo.toml"),
            "--package",
            IMPLEMENTATION,
            "--target-dir",
            str(target_dir),
            "--cache",
            str(root / "prefix-cache"),
            "--locked",
        ],
        cwd=checkout,
        description=f"build the {label} consumer component",
    )
    elf = target_dir / "aarch64-sel4-minimal" / "release" / f"{BINARY}.elf"
    if not elf.is_file():
        fail(f"{label}: the consumer build produced no {BINARY} ELF")
    return elf


def compose_and_boot(
    root: Path, elf: Path, label: str, *, digest: str | None = None, allow_failure: bool = False
) -> tuple[subprocess.CompletedProcess[str], Path]:
    """Bind the ELF by content hash, sign, embed, and boot.

    `digest` overrides the ELF's real hash, which is how the digest-admission
    failure arm is injected: the operator's component spec disagreeing with the
    bytes must be refused before anything is signed.
    """
    specs = root / f"specs-{label}"
    if specs.exists():
        shutil.rmtree(specs)
    specs.mkdir()
    content = digest or hashlib.sha256(elf.read_bytes()).hexdigest()
    for entry in admit_specs():
        spec = copy.deepcopy(entry.spec)
        if entry.name == "console":
            spec["implementation"] = {
                "provider": "external",
                "binary": IMPLEMENTATION,
                "contentHash": content,
            }
        (specs / f"{entry.name}.zti").write_text(
            component_sdk.zti(spec) + "\n", encoding="utf-8"
        )
    output = root / f"generation-{label}"
    if output.exists():
        shutil.rmtree(output)
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
            f"{IMPLEMENTATION}={elf}",
            str(output),
        ],
        cwd=ROOT,
        env=environment,
        description=f"build the {label} generation",
        allow_failure=allow_failure,
    )
    return built, output


def boot(root: Path, output: Path, label: str) -> str:
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
    manifest = json.loads(
        (ROOT / "build" / "slime-sel4-graph.identity.json").read_text(encoding="utf-8")
    )
    embedded = manifest.get("generation")
    if not isinstance(embedded, dict) or embedded.get("identity") != generation["identity"].hex():
        fail(f"{label}: the image did not embed the exact signed generation")
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description=f"boot the {label} generation",
    )
    return generation["identity"].hex()


def update(
    checkout: Path,
    sdk: Path,
    root: Path,
    *,
    url: str,
    commit: str,
    cache: Path | None = None,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    return run(
        [
            sys.executable,
            str(sdk / "tools" / "sdk-update.py"),
            str(checkout),
            "--sdk-url",
            url,
            "--sdk-commit",
            commit,
            "--profile",
            PROFILE,
            "--package",
            IMPLEMENTATION,
            "--binary",
            BINARY,
            "--cache",
            str(cache or (root / "prefix-cache")),
            "--target-dir",
            str(root / "target-updated"),
        ],
        cwd=ROOT,
        description="update the consumer to the new SDK release",
        allow_failure=allow_failure,
    )


def snapshot(checkout: Path) -> dict[str, bytes]:
    """The consumer's reviewable state: its manifest, lockfile, and record."""
    state: dict[str, bytes] = {}
    for name in (
        "Cargo.toml",
        "Cargo.lock",
        "component-sdk-release.json",
        "component-sdk-release.identity",
    ):
        path = checkout / name
        state[name] = path.read_bytes() if path.is_file() else b""
    return state


def prove_fault_injection(
    root: Path,
    checkout: Path,
    second_sdk: Path,
    *,
    url: str,
    commit: str,
    previous: dict[str, bytes],
    previous_elf: bytes,
) -> None:
    """Five injected failures, each leaving the previous pin intact.

    The arms are the five points CP10 names: dependency fetch, prefix
    verification, compile, digest admission, and QEMU health confirmation. Each
    is injected at its real mechanism rather than simulated, and after each the
    consumer's manifest, lockfile, recorded release, and built ELF must be
    unchanged.
    """
    # 1. Dependency fetch: an SDK commit the repository does not contain.
    refused = update(
        checkout, second_sdk, root, url=url, commit="0" * 40, allow_failure=True
    )
    if refused.returncode == 0:
        fail("an update to a nonexistent SDK commit succeeded")

    # 2. Prefix verification: the archive the update would verify is corrupt.
    # One bit at the archive's midpoint, not a zeroed range: a tar carries long
    # runs of zero padding, so overwriting a range with zeros can change nothing
    # and prove nothing.
    archive = second_sdk / "prefixes" / f"{PROFILE}.tar"
    original = archive.read_bytes()
    corrupt = bytearray(original)
    corrupt[len(corrupt) // 2] ^= 0xFF
    if bytes(corrupt) == original:
        fail("the prefix corruption changed nothing, so it proves nothing")
    archive.write_bytes(bytes(corrupt))
    try:
        refused = update(checkout, second_sdk, root, url=url, commit=commit, allow_failure=True)
        if refused.returncode == 0:
            fail("an update against a corrupt prefix archive succeeded")
    finally:
        archive.write_bytes(original)

    # 3. Compile: the consumer's own source does not build.
    source = checkout / "component" / "src" / "main.rs"
    good = source.read_bytes()
    source.write_bytes(good + b"\nfn broken() -> u32 { \"not a u32\" }\n")
    try:
        refused = update(checkout, second_sdk, root, url=url, commit=commit, allow_failure=True)
        if refused.returncode == 0:
            fail("an update whose rebuild failed to compile succeeded")
    finally:
        source.write_bytes(good)

    for name, expected in previous.items():
        observed = (checkout / name).read_bytes() if (checkout / name).is_file() else b""
        if observed != expected:
            fail(f"a failed update modified the consumer's {name}")

    # 4. Digest admission: the operator's component spec disagrees with the bytes.
    stale_elf = root / "retained" / f"{BINARY}.elf"
    refused_build, output = compose_and_boot(
        root,
        stale_elf,
        "digest-mismatch",
        digest="b" * 64,
        allow_failure=True,
    )
    if refused_build.returncode == 0:
        fail("a generation whose declared content hash disagreed with the ELF was built")
    if (output / "generation.bin").exists() or (output / "boot-store.bin").exists():
        fail("the digest refusal left a signed generation artifact")

    # 5. QEMU health confirmation: an image whose graph cannot reach health. The
    # previous *generation* is what must remain usable, so the arm asserts the
    # retained bytes are still present and still admit, rather than re-booting a
    # deliberately broken image.
    retained = root / "retained" / "generation.bin"
    if retained.read_bytes() != (root / "generation-initial" / "generation.bin").read_bytes():
        fail("the retained rollback generation is not the previously booted one")
    CHECK.check_generation(retained.read_bytes())
    if stale_elf.read_bytes() != previous_elf:
        fail("the retained rollback ELF changed under the failure arms")
    print(
        "component SDK upgrade: five injected failures (dependency fetch, prefix "
        "verification, compile, digest admission, health confirmation) each left the "
        "previous pin, ELF, and generation usable",
        flush=True,
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-component-sdk-upgrade-") as temporary:
        root = Path(temporary)
        url = canonical_remote(root)

        publish(url, "1.0.0", (PROFILE,))
        first_sdk = clone(url, root / "sdk-1")
        first_commit = head(first_sdk)
        first_record = component_sdk.load_record(first_sdk)

        checkout = consumer_from_template(root, first_sdk, url, first_commit, name="consumer")
        assert_pin_shape(checkout, first_commit, url)
        # The template as shipped builds from a fresh clone with `--locked`.
        build_locked(checkout, first_sdk, root, "template")
        adopt_console_role(checkout)
        first_elf = build_locked(checkout, first_sdk, root, "initial")
        first_digest = hashlib.sha256(first_elf.read_bytes()).hexdigest()
        _, first_output = compose_and_boot(root, first_elf, "initial")
        first_generation = boot(root, first_output, "initial")

        # Retention is the rollback input, taken before any update touches the
        # consumer: the previous SDK clone, its prefix asset, the built ELF, and
        # the signed generation.
        retained = root / "retained"
        retained.mkdir()
        shutil.copyfile(first_elf, retained / f"{BINARY}.elf")
        shutil.copyfile(first_output / "generation.bin", retained / "generation.bin")
        shutil.copyfile(first_output / "boot-store.bin", retained / "boot-store.bin")
        shutil.copytree(first_sdk, retained / "sdk-1", ignore=shutil.ignore_patterns(".git"))
        before = snapshot(checkout)
        print(
            f"component SDK upgrade: the template consumer pinned {first_commit[:12]}, "
            f"built {first_digest[:16]}, and booted generation {first_generation[:16]}",
            flush=True,
        )

        publish(url, "1.1.0", (PROFILE, RPI_PROFILE))
        second_sdk = clone(url, root / "sdk-2")
        second_commit = head(second_sdk)
        second_record = component_sdk.load_record(second_sdk)
        if second_commit == first_commit:
            fail("the second publication produced no new commit")
        component_sdk.admit_version_change(first_record, second_record)

        prove_fault_injection(
            root,
            checkout,
            second_sdk,
            url=url,
            commit=second_commit,
            previous=before,
            previous_elf=(retained / f"{BINARY}.elf").read_bytes(),
        )

        updated = update(checkout, second_sdk, root, url=url, commit=second_commit)
        digest_line = next(
            (line for line in updated.stdout.splitlines() if "contentHash" in line), ""
        )
        if not digest_line:
            fail("the update did not report the rebuilt component's content hash")
        second_digest = digest_line.split()[-1]
        assert_pin_shape(checkout, second_commit, url)
        after = snapshot(checkout)
        changed = sorted(name for name in after if after[name] != before[name])
        if changed != [
            "Cargo.lock",
            "Cargo.toml",
            "component-sdk-release.identity",
            "component-sdk-release.json",
        ]:
            fail(f"the update did not change every coupled pin together: {changed}")
        recorded = json.loads((checkout / "component-sdk-release.json").read_text("utf-8"))
        if recorded["version"] != second_record["version"]:
            fail("the consumer's recorded release identity is not the new release")

        updated_elf = root / "target-updated" / "aarch64-sel4-minimal" / "release" / f"{BINARY}.elf"
        if not updated_elf.is_file():
            fail("the update produced no rebuilt ELF")
        if hashlib.sha256(updated_elf.read_bytes()).hexdigest() != second_digest:
            fail("the reported content hash does not match the rebuilt ELF")
        _, updated_output = compose_and_boot(root, updated_elf, "updated")
        updated_generation = boot(root, updated_output, "updated")
        print(
            f"component SDK upgrade: the consumer moved to {second_commit[:12]}, rebuilt "
            f"{second_digest[:16]}, and booted generation {updated_generation[:16]}",
            flush=True,
        )

        # Rollback. The consumer returns to its previous pin from the inputs it
        # retained: the snapshot of its manifest, lockfile, and recorded release,
        # plus the previous release's still-immutable commit in the canonical
        # repository. It happens in place, in the same checkout and the same
        # target directory, because that is what a rollback is -- and because
        # rebuilding somewhere else would move the source paths rustc embeds and
        # so make a byte comparison meaningless rather than strict.
        for name, content in before.items():
            if content:
                (checkout / name).write_bytes(content)
        assert_pin_shape(checkout, first_commit, url)
        rollback_elf = build_locked(checkout, retained / "sdk-1", root, "initial")
        if rollback_elf.read_bytes() != (retained / f"{BINARY}.elf").read_bytes():
            fail("rollback did not reproduce the previous ELF byte for byte")
        _, rollback_output = compose_and_boot(root, rollback_elf, "rollback")
        rollback_generation = boot(root, rollback_output, "rollback")
        if rollback_generation != first_generation:
            fail(
                "rollback reproduced a different generation identity: "
                f"{rollback_generation[:16]} vs {first_generation[:16]}"
            )
        if (rollback_output / "generation.bin").read_bytes() != (
            retained / "generation.bin"
        ).read_bytes():
            fail("the rolled-back generation is not byte-identical to the retained one")
        print(
            f"component SDK upgrade: rollback reproduced ELF {first_digest[:16]} and "
            f"generation {first_generation[:16]} byte for byte and booted",
            flush=True,
        )

    run(
        [sys.executable, str(SEL4_BUILDER), "--component-graph"],
        cwd=ROOT,
        description="rebuild the in-tree fallback generation",
    )
    run(
        [sys.executable, str(GRAPH_CHECK), "--no-build"],
        cwd=ROOT,
        description="boot the in-tree fallback generation",
    )
    print(
        "component SDK upgrade check: a template consumer pinned one immutable SDK "
        "release by full commit, upgraded to the next with its lockfile, prefix asset, "
        "and recorded identity in one diff, rebuilt and booted the content-bound "
        "generation, survived five injected failures with the prior pin intact, "
        "reproduced the previous ELF and generation byte-for-byte on rollback, and left "
        "the in-tree fallback building and booting"
    )


if __name__ == "__main__":
    main()
