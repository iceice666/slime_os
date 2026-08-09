#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4.elf"
MANIFEST = ROOT / "build" / "slime-sel4.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"

# The boot is bounded: a wedged guest must fail loudly instead of hanging the
# gate. seL4 plus the root task reaches its final marker in a few seconds.
BOOT_TIMEOUT_SECONDS = 120

# Ordered evidence for the standalone seL4 vertical slice, matched in this
# order against the serial transcript. Together these establish the whole
# chain: the root task admitted the generation and its authority manifest,
# activated no legacy component image, took ownership of untyped memory,
# acquired the real EL1 physical timer IRQ and observed one delivered and
# acknowledged interrupt with the counter advancing across the wait, staged
# both child tasks from the native ELF fixture, activated each one, served
# its badged request, then observed a clean exit from one and a real VM
# fault from the other, reclaimed both, and reached its ready state with
# nothing live.
#
# Numeric fields (badges, slot ranges, counts, fault status words) vary per
# build and are matched loosely. What is pinned is the ordering, the task and
# role identities, the operation number, the exit status, the fault kind, and
# the terminal `live=0`.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    ("generation admitted", r"SLIME_ROOT generation admitted number=\d+"),
    # C8.2/C8.4 on the retained generation. `check-sel4-stream-plane.py` pins
    # the shape of the graph the *seL4* fixtures declare; this is the x86
    # generation P5.1 retained, and its graph is a different one — three
    # schemas, four routes, and the only interposition hop any plane boots.
    #
    # Worth pinning separately: this graph was authored for the retired kernel
    # and is admitted here by `slime-root`'s own ceilings, so it is the one
    # case where admission judges a graph it did not co-evolve with. Until
    # this, the marker was asserted on exactly one plane and so could not
    # distinguish "checked" from "not emitted at all".
    (
        "the retained generation's fabric graph is admitted with its declared shape",
        r"SLIME_ROOT fabric graph=admitted schemas=3 routes=4 "
        r"participants=7 interpositions=1",
    ),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    (
        "legacy component images not activated",
        # `slimecm` must be non-zero: with no legacy image present the claim
        # would be vacuously true. `elf=1` and the `tasks=2` marker below pin
        # that every task the root activated came from the native fixture, so
        # the count is a behavioral assertion rather than a self-report.
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=\d+ slimecm=[1-9]\d* elf=\d+ unrecognized=0",
    ),
    (
        "allocator admitted nonzero kernel resources",
        r"SLIME_ROOT allocator slots=[1-9]\d* untypeds=[1-9]\d* bytes=[1-9]\d*",
    ),
    (
        "timer source acquired",
        r"SLIME_TIMER acquired irq=30 freq_hz=\d+",
    ),
    (
        "timer interrupt delivered",
        r"SLIME_TIMER delivered badge=0x1 polls=\d+",
    ),
    (
        "timer expiry serviced and acknowledged",
        r"SLIME_TIMER serviced events=1 programming=\S",
    ),
    (
        # Corroborating only: CNTPCT_EL0 free-runs, so a non-zero delta proves
        # the counter is readable and monotonic across the wait, NOT that the
        # interrupt fired. `SLIME_TIMER delivered` above is the load-bearing
        # IRQ evidence; this one would still hold if delivery had degraded.
        "timer counter readable and advancing",
        r"SLIME_TIMER advanced start=\d+ end=\d+ delta=[1-9]\d*",
    ),
    ("timer phase complete", r"SLIME_TIMER OK"),
    (
        "two typed frame allocations were independent and exactly accounted",
        r"SLIME_FOUNDATION frames independent objects_delta=2 slots_delta=2 "
        r"bytes_delta=8192 caps_deleted=2",
    ),
    # P5.4.2a: the root can reach a device. Placed after the timer phase
    # because that is where the device probe runs — the ordering in this list
    # is the boot's, not a grouping by subject.
    #
    # `untypeds=[1-9]` is BootInfo naming device regions at all: before this
    # slice the allocator discarded them, so the count was structurally zero.
    # `granules=4 slots=32` is thirty-two register banks mapped non-cacheably
    # into the root's own VSpace and read back; a mapping that faulted, or a
    # watermark mistake landing past its target, produces no line at all.
    #
    # `found=0` is asserted, not tolerated. This gate boots with no `-drive`,
    # so every transport must report device id 0 — a probe that "found"
    # something here would be reading a constant rather than a register.
    (
        "BootInfo named device untyped memory",
        r"SLIME_ROOT devices untypeds=[1-9]\d*",
    ),
    (
        "every declared virtio transport was mapped and probed, and none is attached",
        r"SLIME_ROOT virtio probed granules=4 slots=32 found=0",
    ),
    (
        "clean-exit task staged",
        r"SLIME_ROOT native fixture staged task=0 role=clean-exit \S+ badge=\S+",
    ),
    (
        "deliberate-fault task staged",
        r"SLIME_ROOT native fixture staged task=1 role=deliberate-fault \S+ badge=\S+",
    ),
    ("allocation complete", r"SLIME_ROOT allocations complete tasks=2 "),
    # The shared-buffer phase maps real frames into the clean-exit fixture's
    # VSpace before any task is activated, so its mapping markers precede
    # activation; the child's observations then interleave with that fixture's
    # request record, and the root's adjudication follows both fixtures.
    (
        "shared read-write region mapped at the exact requested range",
        r"SLIME_BUF mapped buffer=\d+ vaddr=0x40000000\.\.0x40001000 pages=1 "
        r"rights=read-write holder=0 frames=[1-9]\d* tables=\d+",
    ),
    (
        "shared read-only region mapped at the exact requested range",
        r"SLIME_BUF mapped buffer=\d+ vaddr=0x40010000\.\.0x40011000 pages=1 "
        r"rights=read-only holder=0 frames=[1-9]\d* tables=\d+",
    ),
    (
        "shared-buffer accounting charged before the child runs",
        r"SLIME_BUF accounting live=2 pages=2 mappings=2 holder_pages=2 orphans=0",
    ),
    ("clean-exit task activated", r"SLIME_ROOT task activated task=0 role=clean-exit"),
    ("clean-exit child issued a request", r"SLIME_CHILD request op=5 tag=0x534c494d45524551"),
    (
        "root served the clean-exit request",
        r"SLIME_ROOT request badge=\S+ task=0 operation=5 directive=0",
    ),
    (
        "root replied to the clean-exit child",
        r"SLIME_ROOT child request served task=0 role=clean-exit operation=5 result=0",
    ),
    # The exact bytes, pinned literally: this is the assertion that a real
    # frame — not a separately zeroed page — is shared with the child.
    (
        "child read back the exact bytes the root wrote",
        r"SLIME_CHILD shared read vaddr=0x40000040 "
        r"observed=0x534255465f525721 expected=0x534255465f525721",
    ),
    (
        "read-only mapping refused the child write by mechanism",
        r"SLIME_BUF probe refused task=0 kind=ro-write access=Write address=0x40010040",
    ),
    (
        "read-only region still holds the root's bytes after the refused write",
        r"SLIME_CHILD ro write result observed=0x534255465f525721 "
        r"intrusion=0xdeadbeefdeadbeef",
    ),
    (
        "execute-never mapping refused execution from a data page",
        r"SLIME_BUF probe refused task=0 kind=wx-execute access=Execute address=0x40000000",
    ),
    (
        "child reported the shared-buffer phase",
        r"SLIME_BUF child reported task=0 observed=0x534255465f525721 flags=0x3",
    ),
    ("child exited cleanly", r"SLIME_CHILD clean exit status=0"),
    (
        "root observed the clean exit",
        r"SLIME_ROOT child exit observed task=0 role=clean-exit status=0",
    ),
    (
        "deliberate-fault task activated",
        r"SLIME_ROOT task activated task=1 role=deliberate-fault",
    ),
    (
        "root served the fault-directive request",
        r"SLIME_ROOT request badge=\S+ task=1 operation=5 directive=1",
    ),
    ("child requested a fault", r"SLIME_CHILD fault requested addr=0x0"),
    (
        "root observed the child fault",
        r"SLIME_ROOT child fault observed task=1 role=deliberate-fault "
        r"kind=VirtualMemory \{ access: Write",
    ),
    # Adjudication runs after both fixtures have finished, because the root
    # decides the phase from its own fault records rather than from the child's
    # self-report. Teardown then proves the accounting really returns to zero.
    (
        "root confirmed the exact bytes crossed the shared frame both ways",
        r"SLIME_BUF readback vaddr=0x40000040 root_wrote=0x534255465f525721 "
        r"child_read=0x534255465f525721 child_wrote=0x4348494c445f4f4b match=1",
    ),
    (
        "both mapping protections were enforced and supervised",
        r"SLIME_BUF rights enforced ro_write=refused wx_execute=refused probes=2 supervised=1",
    ),
    (
        "teardown reclaimed every frame and mapping with quotas back at zero",
        r"SLIME_BUF teardown unmapped=[1-9]\d* revoked=[1-9]\d* released=[1-9]\d* "
        r"live=0 pages=0 mappings=0 holder_pages=0 orphans=0",
    ),
    # B9 on seL4 (P5.4.10): `kernel/tests/task_reclamation.rs` measures frame
    # conservation across spawn/release cycles — a task that goes away returns
    # every frame it consumed. The seL4 shape of the same property is CSlot
    # conservation, and the counts were wildcards here, so a task reclaiming
    # half its slots passed unnoticed.
    #
    # Each task's range is pinned exactly: contiguous, equal-width, and
    # adjoining its neighbour, which is the conservation B9 asserts. Root
    # CSlots are deliberately *not* returned to the allocator
    # (`task.rs::CleanupRecord::revoke`), so the property here is "every slot a
    # task took is accounted for", not "the free count returns to its start" —
    # the drift B9 measures cannot exist on a monotonic allocator.
    # The absolute base is *not* the property: it is wherever the allocator's
    # cursor stands once the root's static tables are placed and its boot-time
    # allocations are made, so it moves whenever either changes. P5.4.9's larger
    # tables moved it 832 → 839, P5.4.2a's device probe — which retypes ten
    # granules to reach four scattered MMIO pages — moved it 839 → 849, and
    # P5.4.2b's IRQ binding took four more for 853, and P5.4.3's namespace, device, and
    # scope tables moved it to 860. What is
    # pinned is each range's width (50) and that the second adjoins the first
    # exactly. A repin here is expected when the root's boot-time allocation
    # changes and suspicious otherwise.
    #
    # Interleaved with the settle markers rather than grouped after them: each
    # task is reclaimed as it settles, and this list is order-sensitive, so
    # grouping would assert a sequence the root does not produce.
    ("clean-exit task settled", r"SLIME_ROOT task settled task=0 role=clean-exit termination=Exit\(0\)"),
    (
        "the clean-exit task's slots are all accounted for",
        r"SLIME_ROOT task reclaimed task=0 source=fabric-service slots=865\.\.915",
    ),
    (
        "deliberate-fault task settled",
        r"SLIME_ROOT task settled task=1 role=deliberate-fault termination=Fault\(",
    ),
    (
        "the faulted task's slots adjoin them with no gap and no overlap",
        r"SLIME_ROOT task reclaimed task=1 source=generation-manager slots=915\.\.965",
    ),
    ("both tasks reclaimed", r"SLIME_ROOT cleanup tasks=2 slots=100 live=0"),
    (
        "root reached ready",
        r"SLIME_ROOT READY tasks=2 grants=\d+ declared_grants=\d+ reclaimed_slots=100",
    ),
)

# Anything here means the slice failed even if a later marker still appeared.
# `SLIME_ROOT FATAL` is the root task's own abort path: it suspends rather than
# exiting, so without matching it here a rejected child image would burn the
# whole boot timeout instead of failing immediately with the reason.
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_ROOT \w+ rejected",
    r"SLIME_ROOT closure rejected",
    # A service loop that never reaches its terminal message: a livelock, not
    # a slow child.
    r"SLIME_ROOT service budget exhausted",
    # The child's own panic handler, and the one outcome that would mean
    # isolation genuinely broke: the deliberate store to an unmapped page
    # returned instead of faulting.
    r"SLIME_CHILD panic ",
    r"SLIME_CHILD fault escaped",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    # A Rust panic or abort in the root task. `sel4-panicking` prints
    # `PanicInfo` verbatim (`panicked at <loc>:\n<msg>`) and `AbortInfo` as
    # `aborted at <loc>`. The root task suspends rather than exiting, so
    # without these the gate would burn the full timeout on a panic instead of
    # reporting it.
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
    # The bounded timer-proof wait loop (`platform_timer.rs` wired in
    # `main.rs`) hits this instead of hanging when the scheduled IRQ never
    # arrives — e.g. a broken IRQ/notification bind.
    r"SLIME_TIMER FAIL timeout",
    # The shared-buffer phase fails closed: a protection that stopped being
    # enforced, a byte pattern that did not survive the shared frame, an
    # unattributable fault from the probing fixture, or a teardown that left a
    # frame or charge behind all abort here rather than silently omitting a
    # marker. `SLIME_ROOT FATAL` above already catches these, but naming the
    # family explicitly keeps the failure legible in the transcript.
    r"SLIME_BUF FAIL .*",
    r"SLIME_FOUNDATION FAIL .*",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 root boot check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


def build_image() -> None:
    print(f"[build] {sys.executable} {BUILD_SCRIPT.relative_to(ROOT)}", flush=True)
    try:
        process = subprocess.run(
            [sys.executable, str(BUILD_SCRIPT)],
            cwd=ROOT,
            check=False,
        )
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> dict[str, object]:
    if not MANIFEST.is_file():
        fail(
            f"missing identity manifest {MANIFEST.relative_to(ROOT)}; "
            "run `just sel4_qemu_image_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}; run `just sel4_qemu_image_check`")
    actual = sha256_file(IMAGE)
    if actual != image["sha256"]:
        fail(
            f"{IMAGE.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )
    return manifest


def boot(profile: dict[str, object]) -> str:
    """Boot the image and return the serial transcript.

    The root task ends by suspending itself, so QEMU stays alive forever after
    the slice completes: waiting for an exit would always hit the timeout.
    Serial output is read line by line instead, and the guest is terminated as
    soon as the terminal marker — or any failure marker — appears. Reaching the
    deadline without the terminal marker is the failure, and the transcript
    collected so far is what gets reported.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    # A wedged guest emits nothing at all, so the deadline cannot live inside
    # the read loop: it would never be evaluated. A watchdog kills QEMU instead,
    # which closes the pipe and ends the loop with whatever was captured.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the pinned standalone Slime seL4 image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 root boot check: ordered generation, timer, task, IPC, fault, and "
        "ready markers observed on the pinned qemu-arm-virt profile"
    )


if __name__ == "__main__":
    main()
