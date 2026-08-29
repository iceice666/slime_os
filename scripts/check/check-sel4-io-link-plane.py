#!/usr/bin/env python3
"""IO3 gate: a supervised userspace virtio-net driver serves LinkDevice over IO0/IO1."""
from __future__ import annotations

import argparse
import json
import re
import shutil
import socket
import subprocess
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import load_qemu_profile, profile_integer, profile_text, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-io-link.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-link.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-link.zti"
IMAGE_VARIANT = "io-link"
TIMEOUT = 20

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "authority, negotiation, and link state",
        (
            r"SLIME_ROOT generation admitted number=52 ",
            r"\[virtio-net-driver\] negotiated legacy features=0 queues rx=16 tx=16 epoch=1",
            r"\[virtio-net-driver\] mmio mechanism=mediated-bounded-read32-write32",
            r"\[io-link-probe\] link query state=up",
        ),
    ),
    (
        "transmit reaches the device and the echo returns",
        (
            r"\[io-link-probe\] rx provisioned=4",
            r"\[io-link-probe\] transmit allowed bytes=60",
            r"\[virtio-net-driver\] tx completed frames=1",
            r"\[io-link-probe\] transmit completion status=ok bytes=60",
            r"\[io-link-probe\] echo verified bytes=60 payload-intact=1",
        ),
    ),
    (
        "bounded queues and exhaustion policy",
        (
            r"\[io-link-probe\] tx backpressure accepted=[1-8] full=1 overwrite=0",
            r"\[io-link-probe\] rx exhausted policy=pause outstanding=\d+ dropped=0 overwrite=0",
        ),
    ),
    (
        "coalesced readiness drains all progress",
        (
            r"\[io-link-probe\] rx continuous frames=4 replenished=4",
            r"\[io-link-probe\] readiness completions=[1-8] wakes=[1-8] max-per-wake=[1-8] pending=0",
        ),
    ),
    (
        "bounds and malformed descriptor refusals",
        (
            r"\[virtio-net-driver\] bounds refused undersized=1 oversized=0 device-programmed=0",
            r"\[virtio-net-driver\] bounds refused undersized=1 oversized=1 device-programmed=0",
            r"\[io-link-probe\] frame bounds refused undersized=1 oversized=1",
            r"\[virtio-net-driver\] malformed descriptor refused=1 device-programmed=0",
            r"\[io-link-probe\] malformed descriptor refused=1",
        ),
    ),
    (
        "reset settles duplex work",
        (
            r"\[virtio-net-driver\] rx drained=\d+ replenished=\d+ stalled=0 tx-stalled=0 device-refused=0",
            r"\[virtio-net-driver\] coalesced pass tx=[1-8] rx=[1-4] drained=all remaining-tx=\d+",
            r"\[virtio-net-driver\] reset settled tx=1 rx=1 leases=2",
            r"\[io-link-probe\] reset completions tx=1 rx=1 status=reset",
        ),
    ),
    (
        "restart reclaims charges and rejects stale completions",
        (
            r"\[virtio-net-driver\] restart reclaimed dma=0 requests=0 leases=0 mmio=1 irq=1",
            r"\[virtio-net-driver\] fresh epoch old=1 new=2",
            r"\[io-link-probe\] stale completions refused tx=1 rx=1 fresh-epoch=2",
        ),
    ),
    (
        "ungranted component and terminal cleanup",
        (
            r"\[io-link-intruder\] denied transmit=1 receive=1 query=1 raw=1 emitted=0",
            r"\[io-link-probe\] io link plane complete",
            r"SLIME_GRAPH HEALTHY generation=52 required=4 live=0 completed=4 failed=0",
        ),
    ),
)


FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[virtio-net-driver\] fail: ",
    r"\[io-link-probe\] fail: ",
    r"\[io-link-intruder\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O link plane check: {message}")


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD_SCRIPT), "--io-link-plane"], cwd=ROOT, check=False
    )
    if process.returncode != 0:
        fail(f"image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not IMAGE.is_file() or not MANIFEST.is_file():
        fail("image or identity manifest missing")
    try:
        identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse identity manifest: {error}")
    if identity.get("variant") != IMAGE_VARIANT:
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def reserve_udp_port() -> tuple[socket.socket, int]:
    receiver = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    receiver.bind(("127.0.0.1", 0))
    receiver.settimeout(0.1)
    return receiver, int(receiver.getsockname()[1])


def backend_echo(receiver: socket.socket, qemu_port: int, stop: threading.Event) -> None:
    sender = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        while not stop.is_set():
            try:
                frame, _ = receiver.recvfrom(2048)
            except socket.timeout:
                continue
            if len(frame) >= 12:
                echoed = bytearray(frame)
                echoed[:6], echoed[6:12] = frame[6:12], frame[:6]
                sender.sendto(echoed, ("127.0.0.1", qemu_port))
    finally:
        sender.close()


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    receiver, backend_port = reserve_udp_port()
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    qemu_port = int(probe.getsockname()[1])
    probe.close()
    stop = threading.Event()
    echo = threading.Thread(target=backend_echo, args=(receiver, qemu_port, stop), daemon=True)
    echo.start()
    command = [
        qemu,
        "-machine", profile_text(profile, "machine", fail),
        "-cpu", profile_text(profile, "cpu", fail),
        "-smp", str(profile_integer(profile, "cpus", fail)),
        "-m", f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE),
        "-netdev", f"socket,id=slimelink,udp=127.0.0.1:{backend_port},localaddr=127.0.0.1:{qemu_port}",
        "-device", "virtio-net-device,netdev=slimelink,mac=52:54:00:53:4c:01",
    ]
    process = subprocess.Popen(
        command, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
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
        stop.set()
        echo.join(timeout=2)
        receiver.close()
    transcript = "\n".join(lines)
    if timed_out and re.search(CHAINS[-1][1][-1], transcript) is None:
        print(transcript)
        fail("QEMU timed out before terminal cleanup")
    return transcript

def check_transcript(transcript: str) -> None:
    try:
        match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    except SystemExit:
        print(transcript)
        raise


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in (
        "generation = 52;",
        'name = "virtio-net-driver";',
        'name = "io-link-probe";',
        'name = "io-link-intruder";',
        'name="io-link-tx-request-ready";',
        'name="io-link-rx-request-ready";',
    ):
        if declaration not in text:
            fail(f"fixture is missing {declaration!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O link proof plane")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    if not arguments.no_build:
        build_image()
    check_manifest()
    transcript = boot(load_qemu_profile(fail, PINS))
    check_transcript(transcript)
    print("seL4 I/O link plane check: duplex readiness, replenishment, reset, restart, and authority proved")


if __name__ == "__main__":
    main()
