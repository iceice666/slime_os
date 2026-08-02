#!/usr/bin/env python3
# P2.1: firmware handoff, EL1 entry, and translation tables on
# `aarch64-qemu-virt`.
#
# Boots a verified AArch64 generation under the pinned QEMU machine and
# firmware and asserts the observed serial evidence: stage-0 selected and
# verified the closure, the kernel reached EL1 with the MMU and both caches
# enabled, memory management came up over the direct map, the heap works, and
# the generation and BootState the verified loader chose are the ones the kernel
# reports. The run must end through the profile's semihosting exit rather than
# the timeout.
#
# This closes P2.1 only. No component runs, no syscall is served, and no
# interrupt is delivered — those are P2.2 through P2.4. A pass here is not
# evidence for the parent P2 exit condition.

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import re
import shutil
import subprocess
import sys
import tempfile

from boot_contracts import TARGET_PROFILES_BY_NAME
from harness import BOOT_TIMEOUT_SECONDS, ROOT

PROFILE_NAME = "aarch64-qemu-virt"
PROFILE = TARGET_PROFILES_BY_NAME[PROFILE_NAME]

# The pinned machine. A different machine, CPU, or memory size is a different
# profile until its own checks pass, so these are fixed here rather than taken
# from the environment.
QEMU_MACHINE = "virt"
QEMU_CPU = "cortex-a72"
QEMU_MEMORY = "512M"
QEMU_SMP = "1"

# QEMU's own exit status after a guest semihosting `SYS_EXIT`.
#
# This proves the guest *reached its exit path* rather than hanging until the
# timeout or dying on a signal. It does NOT distinguish the kernel's success
# code from its failure code: QEMU collapses every semihosting exit to status 1
# regardless of the value the guest passed, which was confirmed by rebuilding
# with `QemuExitCode::Failed` and observing the same status. The serial markers
# are what establish the boot actually succeeded.
EXPECTED_EXIT_STATUS = 1

# Markers the boot must emit, in this order. Each is evidence for one of P2.1's
# required checks rather than decoration.
REQUIRED_MARKERS = (
    # stage-0 selected a BootState slot and recorded its decision.
    r"\[bootstate-trace\] v1 action=boot-known-good",
    # ...and admitted the whole executable closure before mapping it.
    r"\[stage0\] generation and kernel verified",
    # The kernel is executing AArch64 instructions from its own image.
    r"\[serial\] Slime OS aarch64-qemu-virt bring-up",
    # At EL1, not EL2 or EL3 — an EL2 boot would appear to work until the
    # first EL0 transition in P2.3.
    r"\[serial\] exception level EL1",
    # The MMU and both caches are on under stage-0's tables, with the address
    # space sized as the profile requires.
    r"\[serial\] mmu=1 dcache=1 icache=1 t0sz=16 t1sz=16",
    # Physical memory is reachable through the direct map.
    r"\[serial\] direct map offset=0xffff800000000000",
    r"\[serial\] PMM: \d+ / \d+ frames free",
    # The heap is mapped into the kernel half and actually works.
    r"\[serial\] heap online",
    r"\[serial\] heap check: sum=5559680",
    # The kernel sees the generation and BootState the verified loader chose.
    r"\[serial\] generation identity=[0-9a-f]{8} bytes=\d+",
    r"\[serial\] bootstate slot=\d+ sequence=\d+ attempts=\d+ running_pending=(?:true|false)",
    r"\[bringup\] aarch64 EL1 vertical slice reached",
)

# Any of these in the transcript means the boot failed even if a later marker
# somehow appeared.
FAILURE_MARKERS = (
    r"\[stage0\] boot failed",
    r"Synchronous Exception",
    r"\[panic\]",
)


def fail(message: str) -> None:
    raise SystemExit(f"aarch64 boot check: {message}")


def firmware() -> tuple[_Path, _Path]:
    """Locate AArch64 UEFI firmware.

    The dev shell exports `AAVMF_CODE`/`AAVMF_VARS`. Outside it, fall back to
    building the firmware through Nix rather than guessing a system path, so a
    missing firmware is a clear error instead of a mysterious boot failure.
    """
    code = os.environ.get("AAVMF_CODE")
    variables = os.environ.get("AAVMF_VARS")
    if code and variables:
        return _Path(code), _Path(variables)
    if not shutil.which("nix"):
        fail(
            "AAVMF_CODE/AAVMF_VARS are unset and `nix` is unavailable. Run "
            "inside `nix develop`, which exports both."
        )
    process = subprocess.run(
        [
            "nix",
            "build",
            "--no-link",
            "--print-out-paths",
            "nixpkgs#pkgsCross.aarch64-multiplatform.OVMF.fd",
        ],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stderr)
        fail("could not obtain AArch64 UEFI firmware")
    base = _Path(process.stdout.strip().splitlines()[-1]) / "FV"
    return base / "AAVMF_CODE.fd", base / "AAVMF_VARS.fd"


def run(
    arguments: list[str],
    failure: str,
    *,
    environment: dict | None = None,
    cwd: _Path = ROOT,
) -> None:
    process = subprocess.run(
        arguments,
        cwd=cwd,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stdout)
        fail(failure)


def build_artifacts(generation_dir: _Path, image: _Path) -> None:
    environment = dict(os.environ)
    environment["SLIME_TARGET_PROFILE"] = PROFILE_NAME

    # `aarch64-unknown-none`'s precompiled sysroot is built non-PIC, so it
    # cannot link into the position-independent image stage-0 relocates. Build
    # the core crates from source for this target only.
    # Run from `kernel/`, not the repository root: the PIE and linker flags this
    # target needs live in `kernel/.cargo/config.toml`, and cargo only reads a
    # config from the invocation directory upward. Building from the root
    # silently produces an EXEC image the generation builder then rejects.
    run(
        [
            "cargo",
            "build",
            "--release",
            "-p",
            "slime_os-kernel",
            "--target",
            PROFILE.cargo_target,
            "-Z",
            "build-std=core,alloc,compiler_builtins",
        ],
        "AArch64 kernel build failed",
        environment=environment,
        cwd=ROOT / "kernel",
    )

    kernel = ROOT / "target" / PROFILE.cargo_target / "release" / "slime_os-kernel"
    if not kernel.is_file():
        fail(f"kernel binary missing: {kernel}")

    run(
        [
            str(ROOT / "scripts" / "build" / "build-generation.py"),
            str(kernel),
            str(generation_dir),
        ],
        "AArch64 generation build failed",
        environment=environment,
    )

    image_environment = dict(environment)
    image_environment["SLIME_GENERATION_DIR"] = str(generation_dir)
    run(
        [
            "bash",
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(kernel),
            str(image),
            "64",
        ],
        "AArch64 boot image build failed",
        environment=image_environment,
    )


# A boot expected to fail admission never reaches a guest exit path: firmware
# falls through to its own boot menu and waits. That is the correct outcome, so
# the negative scenario is bounded far more tightly than a successful boot and
# treats the timeout as evidence rather than failure.
REJECTION_TIMEOUT_SECONDS = 90


def boot(
    image: _Path,
    code: _Path,
    variables: _Path,
    work: _Path,
    *,
    timeout: int = BOOT_TIMEOUT_SECONDS,
    expect_exit: bool = True,
) -> tuple[str, int]:
    # Per-run writable NVRAM so firmware variables do not leak between runs.
    work.mkdir(parents=True, exist_ok=True)
    nvram = work / "AAVMF_VARS.fd"
    shutil.copy(variables, nvram)
    nvram.chmod(0o644)
    arguments = [
        PROFILE.qemu_binary,
        "-machine",
        QEMU_MACHINE,
        "-cpu",
        QEMU_CPU,
        "-smp",
        QEMU_SMP,
        "-m",
        QEMU_MEMORY,
        "-drive",
        f"if=pflash,format=raw,readonly=on,file={code}",
        "-drive",
        f"if=pflash,format=raw,file={nvram}",
        "-drive",
        f"format=raw,file={image}",
        # This profile has no `isa-debug-exit` device; the kernel reports its
        # exit status through Arm semihosting instead.
        "-semihosting",
        "-display",
        "none",
        "-serial",
        "stdio",
    ]
    try:
        process = subprocess.run(
            arguments,
            cwd=ROOT,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        output = error.output or ""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        if not expect_exit:
            # Firmware parked at its boot menu after refusing the image. The
            # transcript is what this scenario asserts on.
            return output, 0
        sys.stdout.write(output)
        fail(
            f"boot did not terminate within {timeout}s; the kernel never "
            f"reached its exit path"
        )
    return process.stdout, process.returncode


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match:
            line = next(
                (line for line in transcript.splitlines() if match.group(0) in line),
                match.group(0),
            )
            fail(f"boot reported a failure: {line.strip()}")

    position = 0
    for pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            if re.search(pattern, transcript):
                fail(f"marker out of order: {pattern}")
            fail(f"missing expected boot marker: {pattern}")
        position = match.end()


def check_wrong_target_rejected(work: _Path, code: _Path, variables: _Path) -> None:
    """An x86 generation must be refused before executable bytes are mapped.

    Builds a real `x86_64-qemu-virtio` generation, wraps it in an AArch64 boot
    image, and boots it: the AArch64 stage-0 loader must reject the closure as
    the wrong target rather than attempting it. Asserting this needs a genuinely
    wrong artifact, not a synthetic one, because the whole point is that the
    admission path — not a fixture — does the refusing.
    """
    environment = dict(os.environ)
    environment["SLIME_TARGET_PROFILE"] = "x86_64-qemu-virtio"
    x86_kernel = ROOT / "target" / "x86_64-unknown-none" / "release" / "slime_os-kernel"
    run(
        ["cargo", "build", "--release", "-p", "slime_os-kernel"],
        "x86 kernel build failed",
        environment=environment,
        cwd=ROOT / "kernel",
    )
    generation_dir = work / "x86-generation"
    run(
        [
            str(ROOT / "scripts" / "build" / "build-generation.py"),
            str(x86_kernel),
            str(generation_dir),
        ],
        "x86 generation build failed",
        environment=environment,
    )

    # The AArch64 loader and boot filename, carrying the x86 generation.
    image = work / "slime-wrong-target.img"
    image_environment = dict(os.environ)
    image_environment["SLIME_TARGET_PROFILE"] = PROFILE_NAME
    image_environment["SLIME_GENERATION_DIR"] = str(generation_dir)
    run(
        [
            "bash",
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(ROOT / "target" / PROFILE.cargo_target / "release" / "slime_os-kernel"),
            str(image),
            "64",
        ],
        "wrong-target boot image build failed",
        environment=image_environment,
    )

    transcript, _ = boot(
        image,
        code,
        variables,
        work / "wrong-target",
        timeout=REJECTION_TIMEOUT_SECONDS,
        expect_exit=False,
    )
    sys.stdout.write(transcript)
    if not re.search(r"\[stage0\] boot failed: Target\(ProfileMismatch\)", transcript):
        fail(
            "an x86 generation was not rejected by the AArch64 loader; expected "
            "a structured stage-0 failure before executable bytes were mapped"
        )
    # It must fail in admission, not by running and crashing later.
    for marker in (r"exception level EL", r"aarch64 EL1 vertical slice reached"):
        if re.search(marker, transcript):
            fail(f"wrong-target generation reached kernel execution: {marker}")
    print("aarch64 boot check: x86 generation rejected before executable mapping")


def main() -> None:
    code, variables = firmware()
    for path in (code, variables):
        if not path.is_file():
            fail(f"AArch64 UEFI firmware missing: {path}")

    with tempfile.TemporaryDirectory(prefix="slime-aarch64-boot.") as directory:
        work = _Path(directory)
        generation_dir = work / "generation"
        image = work / "slime-aarch64.img"
        build_artifacts(generation_dir, image)
        transcript, returncode = boot(image, code, variables, work / "boot")

        sys.stdout.write(transcript)
        check_transcript(transcript)

        # A clean run must terminate through the semihosting exit rather than a
        # signal or a timeout. See `EXPECTED_EXIT_STATUS` for what this does and
        # does not establish.
        if returncode < 0:
            fail(f"QEMU terminated on signal {-returncode}")
        if returncode != EXPECTED_EXIT_STATUS:
            fail(
                f"boot exited with status {returncode}, expected "
                f"{EXPECTED_EXIT_STATUS} from a semihosting exit"
            )

        check_wrong_target_rejected(work, code, variables)

    print(
        "aarch64 boot check: EL1 reached with MMU and caches enabled, direct map "
        "and heap online, verified generation observed; wrong-target generation "
        "rejected before executable mapping"
    )


if __name__ == "__main__":
    main()
