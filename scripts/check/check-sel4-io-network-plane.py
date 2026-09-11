#!/usr/bin/env python3
"""IO4 and IO11 gate: destination-scoped network authority under seL4, and
the data plane behind it.

Two arms over one checker. The authority arm boots `sel4-io-network`, whose
service is bound to a loopback that performs no link operation, and proves the
exact-destination boundary alone. The tcp arm boots `sel4-io-tcp`, whose
service is bound to the IO3 virtio-net driver behind a frame-level peer on
QEMU's UDP socket backend (`scripts/lib/link_peer.py`), so every frame that
leaves the guest is observed by the gate and not merely absent.
"""

from __future__ import annotations

import argparse
import re
import socket
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import sha256_file  # noqa: E402

import link_peer  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from sel4_plane import run_plane  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
# The closure identity names the build's inputs and is re-resolved from repository
# state before building, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-io-network"
TCP_CLOSURE = "sel4-io-tcp"
IMAGE: Path | None = None
COMPOSITIONS = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions"
FIXTURE = COMPOSITIONS / "sel4-io-network.zti"
TCP_FIXTURE = COMPOSITIONS / "sel4-io-tcp.zti"
TIMEOUT = 240
# The MAC the composition declares for the service, which is also the one the
# QEMU device carries: the gate reads it from the fixture so the two cannot drift.
MAC_DECLARATION = re.compile(r'mac\s*=\s*"([0-9a-f:]{17})"')

AUTHORITY_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "admission",
        (
            r"SLIME_ROOT generation admitted number=53 executables=5 instances=5 grants=3 ",
            r"\[network-service\] authority destinations=5 rights=connect,send,recv",
            r"\[network-service\] declared socket_limit=7 listener_limit=0 dns_record_limit=2",
        ),
    ),
    (
        "loopback honesty",
        (r"\[io-link-loopback\] declared endpoint bindings=1 protocol operations=0",),
    ),
    (
        "granted path",
        (
            r"\[io-network-probe\] tcp capabilities=1 rights=connect,send,recv",
            r"\[io-network-probe\] successful capability operations=2",
            r"\[io-network-probe\] exact destination refusals=1",
            r"\[io-network-probe\] dns records=1 budget_refusals=1",
            r"\[io-network-probe\] socket charges=2 budget_refusals=1",
            r"\[io-network-probe\] closed capabilities=4 shutdown=1",
        ),
    ),
    (
        "denials",
        (
            r"\[io-network-intruder\] exact authority refusals=8",
            r"\[io-network-intruder\] cross-holder capability refusals=4",
            r"\[io-network-intruder\] rights-mask refusals=2",
            r"\[io-network-intruder\] structured denials=14 shutdown=1",
        ),
    ),
    (
        "service close",
        (
            r"\[network-service\] observed requests=33 packets=7 socket_refusals=1 listener_refusals=0 dns_refusals=1 cross_holder_refusals=4",
            r"SLIME_GRAPH HEALTHY generation=53 required=5 live=0 completed=5 failed=0",
        ),
    ),
)
# The data-plane arm. Chains are one component each, in program order, because
# the matcher orders markers only within a chain and the components' startup
# interleaving is the scheduler's: a service line before or after a driver
# line is not a claim the plane makes. Counts that depend on the peer's timing
# (frames, replenishments) are shapes; the rest are exact observed values.
TCP_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "tcp admission",
        (r"SLIME_ROOT generation admitted number=54 executables=5 instances=5 grants=8 ",),
    ),
    (
        "tcp service",
        (
            r"\[network-service\] authority destinations=5 rights=connect,send,recv",
            r"\[network-service\] declared socket_limit=5 listener_limit=0 dns_record_limit=0",
            r"\[network-service\] clock rate=[0-9]+",
            r"\[network-service\] interface addr=10\.0\.0\.1/24 gateway=none mac=52:54:00:53:4c:01",
            r"\[network-service\] link query state=up rx provisioned=4",
            r"\[network-service\] link frames total=[0-9]+ tx=[0-9]+ rx=[0-9]+ arp=[0-9]+ icmp=[0-9]+ tcp=0 other=0",
            r"\[network-service\] link statistics tx=[0-9]+ rx=[0-9]+",
            r"\[network-service\] link released",
            r"\[network-service\] observed requests=22 packets=3 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=1",
        ),
    ),
    (
        "tcp driver",
        (
            r"\[virtio-net-driver\] negotiated legacy features=0 queues rx=16 tx=16 epoch=1",
            r"\[virtio-net-driver\] rx drained=[0-9]+ replenished=[0-9]+ stalled=0 tx-stalled=0 device-refused=0",
            r"\[virtio-net-driver\] reset settled tx=[0-9]+ rx=[0-9]+ leases=[0-9]+",
            r"\[virtio-net-driver\] fresh epoch old=1 new=2",
        ),
    ),
    (
        "tcp client",
        (
            r"\[io-tcp-probe\] clock rate=[0-9]+",
            r"\[io-tcp-probe\] tcp capabilities=1 rights=connect,send,recv",
            r"\[io-tcp-probe\] held ms=3000",
            r"\[io-tcp-probe\] closed capabilities=1 shutdown=1",
        ),
    ),
    (
        "tcp denials",
        (
            r"\[io-network-intruder\] exact authority refusals=8",
            r"\[io-network-intruder\] cross-holder capability refusals=4",
            r"\[io-network-intruder\] rights-mask refusals=2",
            r"\[io-network-intruder\] structured denials=14 shutdown=1",
        ),
    ),
    (
        "tcp health",
        (r"SLIME_GRAPH HEALTHY generation=54 required=5 live=0 completed=5 failed=0",),
    ),
)
# One table for the gate control: it mutates every marker of both arms.
CHAINS = AUTHORITY_CHAINS + TCP_CHAINS
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[network-service\] fail: ",
    r"\[network-service\] link fail: ",
    r"\[io-network-probe\] fail: ",
    r"\[io-network-intruder\] fail: ",
    r"\[io-tcp-probe\] fail: ",
    r"\[virtio-net-driver\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O network plane check: {message}")


def build_image(closure: str) -> Path:
    try:
        built = build_closure_image(closure)
    except ClosureImageError as error:
        fail(str(error))
    image = built.image
    actual = sha256_file(image, fail)
    if actual != built.digest():
        fail(f"{image} SHA-256 is {actual}, but the build result records {built.digest()}; the image changed after it was built")
    return image


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for pattern in (
        r"generation\s*=\s*53;",
        r"networkDestinations\s*=\s*\[",
        r'name\s*=\s*"network-service"',
        r'name\s*=\s*"io-network-probe"',
        r'name\s*=\s*"io-network-intruder"',
        r'name\s*=\s*"io-link-loopback"',
    ):
        if re.search(pattern, text) is None:
            fail(f"fixture is missing {pattern!r}")


def check_tcp_fixture() -> str:
    text = TCP_FIXTURE.read_text(encoding="utf-8")
    for pattern in (
        r"generation\s*=\s*54;",
        r"networkDestinations\s*=\s*\[",
        r"networkInterfaces\s*=\s*\[",
        r'address\s*=\s*"10\.0\.0\.2"',
        r'name\s*=\s*"network-service"',
        r'name\s*=\s*"io-tcp-probe"',
        r'name\s*=\s*"io-network-intruder"',
        r'name\s*=\s*"virtio-net-driver"',
        r'name\s*=\s*"io-link-tx-request-ready"',
    ):
        if re.search(pattern, text) is None:
            fail(f"tcp fixture is missing {pattern!r}")
    macs = MAC_DECLARATION.findall(text)
    if len(macs) != 1:
        fail(f"tcp fixture declares {len(macs)} interface MACs, expected exactly one")
    return macs[0]


def reserve_udp_port() -> tuple[socket.socket, int]:
    receiver = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    receiver.bind(("127.0.0.1", 0))
    receiver.settimeout(0.05)
    return receiver, int(receiver.getsockname()[1])


def run_authority_arm(image: Path) -> None:
    terminal = re.compile(AUTHORITY_CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    transcript = run_plane(
        image=image,
        timeout=TIMEOUT,
        terminal_condition=terminal,
        fail=fail,
        pins_path=PINS,
    )
    match_marker_contract(transcript, AUTHORITY_CHAINS, FAILURE_MARKERS, fail)


def run_tcp_arm(image: Path, mac: str) -> None:
    receiver, backend_port = reserve_udp_port()
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    qemu_port = int(probe.getsockname()[1])
    probe.close()
    peer = link_peer.Peer()
    stop = threading.Event()
    thread = threading.Thread(target=link_peer.serve, args=(receiver, qemu_port, stop, peer), daemon=True)
    thread.start()
    terminal = re.compile(TCP_CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    try:
        transcript = run_plane(
            image=image,
            timeout=TIMEOUT,
            terminal_condition=terminal,
            fail=fail,
            pins_path=PINS,
            additional_arguments=(
                "-netdev",
                f"socket,id=slimelink,udp=127.0.0.1:{backend_port},localaddr=127.0.0.1:{qemu_port}",
                "-device",
                f"virtio-net-device,netdev=slimelink,mac={mac}",
            ),
        )
    finally:
        stop.set()
        thread.join(timeout=2)
        receiver.close()
    try:
        match_marker_contract(transcript, TCP_CHAINS, FAILURE_MARKERS, fail)
    except SystemExit:
        print(transcript)
        raise
    ledger = peer.ledger
    print(f"[peer] {ledger.summary()}")
    guest_mac = bytes.fromhex(mac.replace(":", ""))
    foreign = ledger.foreign_sources(guest_mac)
    if foreign:
        fail(f"frames arrived from a MAC the composition does not declare: {[f.hex(':') for f in foreign]}")
    if ledger.guest_mac != guest_mac:
        fail("the guest never answered the peer's ARP request for its declared address")
    if ledger.count_received("arp-reply") < 1:
        fail("no ARP reply from the guest")
    replies = ledger.echo_replies_matching(link_peer.ECHO_IDENTIFIER)
    if replies != list(range(1, link_peer.ECHO_COUNT + 1)):
        fail(f"echo replies from the guest were {replies}, expected every sequence 1..{link_peer.ECHO_COUNT}")
    undeclared = {destination for destination in ledger.ip_destinations() if destination != link_peer.PEER_IP}
    if undeclared:
        fail(f"the guest addressed IPv4 hosts the composition does not declare: {sorted(undeclared)}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O network proof planes")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--arm", choices=("authority", "tcp", "all"), default="all")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if arguments.arm in ("authority", "all"):
        check_fixture()
        image = build_image(CLOSURE) if not arguments.no_build else IMAGE
        if image is None:
            fail("--no-build needs a previously built image")
        run_authority_arm(image)
        print(
            "seL4 I/O network plane check: exact authority, per-destination budgets, "
            "structured denials, and honest backend absence proved"
        )
    if arguments.arm in ("tcp", "all"):
        mac = check_tcp_fixture()
        image = build_image(TCP_CLOSURE)
        run_tcp_arm(image, mac)
        print(
            "seL4 I/O tcp plane check: the network service attached to virtio-net, "
            "answered the peer's ARP and ICMP echo on its declared interface, "
            "held its client's exact destination, and released the link cleanly"
        )


if __name__ == "__main__":
    main()
