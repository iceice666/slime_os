"""One platform-aware boot path for every seL4 QEMU plane gate.

Each plane gate used to carry its own copy of the same QEMU invocation, and
adding a third architecture would have meant a third copy in each of them. What
actually varies between platforms is the boot route — a packaged loader ELF
handed to `-kernel`, or seL4 pc99's native Multiboot2 file tree booted through
pinned q35/OVMF — and that belongs in one place so a plane cannot accidentally
support one architecture and not another.

Plane gates keep owning their marker chains, fixtures, and semantic assertions.
This module owns only: which platforms exist, which target profile and QEMU
binary each implies, where its artifacts land, whether the artifacts match the
identity manifest, and how to run one bounded boot.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import threading
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import NoReturn

from harness import (
    load_qemu_profile,
    profile_integer,
    profile_text,
    qemu_kernel_arguments,
    sha256_file,
)
from pc99_media import boot_media, qemu_command as pc99_qemu_command

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"

# Every emulated seL4 platform a plane gate may target, with the pins table and
# emulator binary each one implies. Physical platforms are absent by design:
# they are observed, not booted by a gate.
PLATFORMS: dict[str, tuple[str, str]] = {
    "qemu-arm-virt": ("qemu_arm_virt", "qemu-system-aarch64"),
    "qemu-riscv-virt": ("qemu_riscv_virt", "qemu-system-riscv64"),
    "qemu-pc99": ("qemu_pc99", "qemu-system-x86_64"),
}

TARGET_PROFILES: dict[str, str] = {
    "qemu-arm-virt": "aarch64-sel4-qemu-virt",
    "qemu-riscv-virt": "riscv64-sel4-qemu-virt",
    "qemu-pc99": "x86_64-sel4-qemu-pc99",
}

# AArch64 was the only platform when these artifact names were established, so
# its files carry no suffix and every existing gate reads them by that name.
DEFAULT_PLATFORM = "qemu-arm-virt"


def artifact_paths(stem: str, platform: str) -> tuple[Path, Path]:
    """The image and identity manifest for one variant on one platform."""
    suffix = "" if platform == DEFAULT_PLATFORM else f"-{platform}"
    return (
        ROOT / "build" / f"{stem}{suffix}.elf",
        ROOT / "build" / f"{stem}{suffix}.identity.json",
    )


def profile_for(platform: str, fail: Callable[[str], NoReturn]) -> dict[str, object]:
    return load_qemu_profile(fail, PINS, PLATFORMS[platform][0])


def verify_identity(
    manifest_path: Path,
    *,
    platform: str,
    variant: str | None,
    image_path: Path,
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    """Refuse to boot bytes the identity manifest does not describe.

    Both boot routes are verified against what is actually on disk. The loader
    route has one packaged ELF to digest; the Multiboot2 route has an EFI tree,
    whose per-file digests and path-sensitive tree digest are recomputed here.
    A gate that skipped this would assert markers against whichever build ran
    last rather than the one it asked for.
    """
    if not manifest_path.is_file():
        fail(f"missing identity manifest {manifest_path.relative_to(ROOT)}")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {manifest_path.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{manifest_path.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    if manifest.get("platform") != platform:
        fail(f"identity platform is {manifest.get('platform')!r}, not {platform!r}")
    expected_profile = TARGET_PROFILES[platform]
    if manifest.get("target_profile") != expected_profile:
        fail(
            f"identity target profile is {manifest.get('target_profile')!r}, "
            f"not {expected_profile!r}"
        )
    if variant is not None and manifest.get("variant") != variant:
        fail(f"wrong image variant {manifest.get('variant')!r}, expected {variant!r}")

    if manifest.get("boot_route") == "multiboot2":
        media = manifest.get("media")
        if not isinstance(media, dict) or not isinstance(media.get("tree_sha256"), str):
            fail("identity manifest records no boot media tree digest")
        if manifest.get("image") is not None:
            fail("a multiboot2 identity manifest must not claim a packaged image")
        if manifest.get("elf", {}).get("loader") is not None:
            fail("a multiboot2 identity manifest must not claim a rust-sel4 loader")
        tree = ROOT / str(media["tree"])
        if not tree.is_dir():
            fail(f"missing boot media tree {media['tree']}")
        observed = boot_media(tree, profile=profile_for(platform, fail), fail=fail)
        if observed["tree_sha256"] != media["tree_sha256"]:
            fail(
                f"{media['tree']} tree digest is {observed['tree_sha256']}, but the identity "
                f"manifest records {media['tree_sha256']}; rebuild before booting"
            )
        for relative, record in media["files"].items():
            if observed["files"].get(relative) != record:
                fail(f"boot media file {relative} does not match the identity manifest")
        return manifest

    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not image_path.is_file():
        fail(f"missing packaged image {image_path.relative_to(ROOT)}")
    actual = sha256_file(image_path, fail)
    if actual != image["sha256"]:
        fail(
            f"{image_path.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )
    return manifest


def boot_command(
    manifest: dict[str, object],
    *,
    platform: str,
    image_path: Path,
    fail: Callable[[str], NoReturn],
    extra: Sequence[str] = (),
) -> list[str]:
    """The pinned QEMU command for one platform's boot route."""
    profile = profile_for(platform, fail)
    section, qemu_binary = PLATFORMS[platform]
    if manifest.get("boot_route") == "multiboot2":
        return pc99_qemu_command(
            tree=ROOT / str(manifest["media"]["tree"]),
            profile=profile,
            fail=fail,
            # Per boot and per platform: OVMF writes to its variable store, so
            # the pinned template is never the file handed to the emulator, and
            # two platforms' boots must not share one store.
            vars_copy=ROOT / "build" / "media" / f".{platform}.vars.fd",
            extra=extra,
        )
    qemu = shutil.which(qemu_binary)
    if qemu is None:
        fail(f"{qemu_binary} is not on PATH")
    return [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail, section),
        "-cpu",
        profile_text(profile, "cpu", fail, section),
        "-smp",
        str(profile_integer(profile, "cpus", fail, section)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail, section)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        *qemu_kernel_arguments(qemu_binary, image_path, fail),
        *extra,
    ]


def run(
    command: Sequence[str],
    *,
    terminal: re.Pattern[str],
    timeout: int,
    fail: Callable[[str], NoReturn],
    feed: Callable[[subprocess.Popen[str], list[str]], None] | None = None,
) -> str:
    """Boot bounded and return the transcript.

    The guest is never waited on for exit: a root task that finishes its work
    suspends itself, and a resident product graph runs forever, so both would
    always hit the deadline. Output is read line by line and the guest killed
    once the terminal pattern appears; a watchdog covers the wedged case, where
    nothing is emitted and the read loop would block forever.

    `feed` takes over the read loop for a gate that must supply bounded input
    once the guest asks for it, and appends the lines it consumes.
    """
    print(f"[boot] {' '.join(command)}", flush=True)
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            list(command),
            cwd=ROOT,
            stdin=subprocess.PIPE if feed is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    watchdog = threading.Timer(timeout, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        if feed is not None:
            feed(process, lines)
        else:
            for line in process.stdout:
                lines.append(line.rstrip("\n"))
                if terminal.search(line):
                    break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if timed_out and terminal.search(transcript) is None:
        report_transcript(transcript)
        fail(f"boot exceeded {timeout}s without reaching the terminal condition")
    return transcript


def report_transcript(transcript: str, lines: int = 40) -> None:
    tail = transcript.splitlines()[-lines:]
    if tail:
        print("--- serial transcript (tail) ---")
        print("\n".join(tail))
        print("--- end transcript ---", flush=True)
