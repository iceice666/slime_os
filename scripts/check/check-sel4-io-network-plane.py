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
# The probe's streams: the echo port carries one page (the destination's
# whole byte budget) and then 64 more bytes after the idle hold; the closing
# port carries 256 bytes and then the peer's FIN.
STREAM_BYTES = 4096
IDLE_BYTES = 64
CLOSING_BYTES = 256


def stream_pattern(length: int) -> bytes:
    """The bytes the probe sends: its `expected(index)` function, restated here
    so the peer's copy of the stream is compared against the same source."""
    return bytes((index * 7 + (index >> 8)) & 0xFF for index in range(length))
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
        (r"SLIME_ROOT generation admitted number=54 executables=5 instances=5 grants=9 ",),
    ),
    (
        "tcp service",
        (
            r"\[network-service\] authority destinations=6 rights=connect,send,recv",
            r"\[network-service\] declared socket_limit=6 listener_limit=0 dns_record_limit=0",
            r"\[network-service\] clock rate=[0-9]+",
            r"\[network-service\] interface addr=10\.0\.0\.1/24 gateway=none mac=52:54:00:53:4c:01",
            r"\[network-service\] link query state=up rx provisioned=4",
            r"\[network-service\] link quiesced sockets=1 aborted=0",
            r"\[network-service\] link frames total=[0-9]+ tx=[0-9]+ rx=[0-9]+ arp=[0-9]+ icmp=[0-9]+ tcp=[0-9]+ other=0",
            r"\[network-service\] link statistics tx=[0-9]+ rx=[0-9]+",
            r"\[network-service\] tcp sockets opened=3 established=2 reset=1 bytes-tx=4416 bytes-rx=4416",
            r"\[network-service\] link released",
            r"\[network-service\] observed requests=26 packets=5 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=1",
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
            r"\[io-tcp-probe\] connect dst=10\.0\.0\.2:4242 status=ok",
            r"\[io-tcp-probe\] tcp capabilities=1 rights=connect,send,recv",
            r"\[io-tcp-probe\] sent bytes=4096",
            r"\[io-tcp-probe\] received bytes=4096 completions=[0-9]+",
            r"\[io-tcp-probe\] stream verified bytes=4096 mismatches=0",
            r"\[io-tcp-probe\] connect dst=10\.0\.0\.2:4243 status=refused",
            r"\[io-tcp-probe\] undeclared destination refusals=1",
            r"\[io-tcp-probe\] connect dst=10\.0\.0\.2:4244 status=ok",
            r"\[io-tcp-probe\] closing stream verified bytes=256 mismatches=0",
            r"\[io-tcp-probe\] end of stream dst=10\.0\.0\.2:4244 echoed bytes=256",
            r"\[io-tcp-probe\] close dst=10\.0\.0\.2:4244 status=ok",
            r"\[io-tcp-probe\] held ms=6000 open sockets=1",
            r"\[io-tcp-probe\] idle stream verified bytes=64 mismatches=0 after ms=6000",
            r"\[io-tcp-probe\] close dst=10\.0\.0\.2:4242 status=ok",
            r"\[io-tcp-probe\] closed capabilities=2 shutdown=1",
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


def run_tcp_arm(image: Path, mac: str, transcript_path: Path | None) -> None:
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
    if transcript_path is not None:
        # The serial capture and the peer's summary, kept as evidence: a devlog
        # entry cites what was observed, not the gate's one-line verdict.
        transcript_path.write_text(transcript + f"\n[peer] {peer.ledger.summary()}\n", encoding="utf-8")
    try:
        match_marker_contract(transcript, TCP_CHAINS, FAILURE_MARKERS, fail)
    except SystemExit:
        print(transcript)
        raise
    ledger = peer.ledger
    print(f"[peer] {ledger.summary()}")
    if ledger.failure is not None:
        fail(f"the peer stopped serving: {ledger.failure}")
    guest_mac = bytes.fromhex(mac.replace(":", ""))
    foreign = ledger.foreign_sources(guest_mac)
    if foreign:
        fail(f"frames arrived from a MAC the composition does not declare: {[f.hex(':') for f in foreign]}")
    if ledger.guest_mac != guest_mac:
        fail("the guest never answered the peer's ARP request for its declared address")
    if ledger.count_received("arp-reply") < 1:
        fail("no ARP reply from the guest")
    replies = ledger.echo_replies_matching(link_peer.ECHO_IDENTIFIER, link_peer.ECHO_PAYLOAD, link_peer.GUEST_IP)
    if replies != list(range(1, link_peer.ECHO_COUNT + 1)):
        fail(
            f"echo replies from the guest carrying the request's bytes were {replies}, "
            f"expected every sequence 1..{link_peer.ECHO_COUNT}"
        )
    # An undeclared host on the link would be asked for by ARP before any IPv4
    # packet could name it, so both tables are checked.
    undeclared = {destination for destination in ledger.ip_destinations() if destination != link_peer.PEER_IP}
    undeclared |= {target for target in ledger.arp_targets() if target != link_peer.PEER_IP}
    if undeclared:
        fail(f"the guest addressed hosts the composition does not declare: {sorted(undeclared)}")
    # The service's own count of frames it transmitted, the driver's count of
    # frames it completed, and what the peer received are three independent
    # ledgers of one wire.
    statistics = re.search(r"\[network-service\] link statistics tx=(\d+) rx=(\d+)", transcript)
    if statistics is None:
        fail("the service printed no link statistics")
    frames = re.search(r"\[network-service\] link frames total=\d+ tx=(\d+) rx=(\d+)", transcript)
    if frames is None:
        fail("the service printed no frame counts")
    if int(statistics.group(1)) != len(ledger.received):
        fail(f"the driver completed {statistics.group(1)} transmits but the peer received {len(ledger.received)}")
    if frames.group(1) != statistics.group(1) or frames.group(2) != statistics.group(2):
        fail(f"the service counted tx={frames.group(1)} rx={frames.group(2)} frames but the driver tx={statistics.group(1)} rx={statistics.group(2)}")
    # The byte streams, as the peer saw them: one connection to the echo port
    # carrying the probe's 4096 bytes each way and 64 more after the idle hold,
    # closed by the guest; one to the closing port carrying 256 bytes each way
    # and closed by the peer first; one refused attempt on the closed port; and
    # nothing else.
    flows = {flow.server_port: flow for flow in peer.tcp.flows.values()}
    if len(peer.tcp.flows) != 2 or set(flows) != {link_peer.ECHO_PORT, link_peer.CLOSING_PORT}:
        fail(f"expected one connection to the echo port and one to the closing port, the peer saw {peer.tcp.summary()}")
    expected_flows = (
        (link_peer.ECHO_PORT, stream_pattern(STREAM_BYTES) + stream_pattern(IDLE_BYTES), "guest"),
        (link_peer.CLOSING_PORT, stream_pattern(CLOSING_BYTES), "peer"),
    )
    for port, payload, closed_by in expected_flows:
        flow = flows[port]
        if flow.received != len(payload) or flow.echoed != len(payload):
            fail(f"the flow to port {port} carried rx={flow.received} echo={flow.echoed}, expected {len(payload)} each way")
        if bytes(flow.payload) != payload:
            fail(f"the bytes the peer received on port {port} are not the probe's seeded stream")
        if flow.state != "closed":
            fail(f"the flow to port {port} ended in state {flow.state!r}, expected both FINs acknowledged")
        if flow.closed_by != closed_by:
            fail(f"the flow to port {port} was closed first by {flow.closed_by!r}, expected {closed_by!r}")
    if len(peer.tcp.refused) != 1:
        fail(f"expected exactly one refused connection attempt, the peer saw {len(peer.tcp.refused)}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O network proof planes")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--arm", choices=("authority", "tcp", "all"), default="all")
    parser.add_argument(
        "--transcript",
        type=Path,
        help="write the tcp arm's serial transcript and peer summary to this file",
    )
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
        run_tcp_arm(image, mac, arguments.transcript)
        print(
            "seL4 I/O tcp plane check: the network service attached to virtio-net, "
            "answered the peer's ARP and ICMP echo on its declared interface, carried "
            "its client's byte stream to its exact destination and back unchanged, "
            "refused the closed port and the undeclared host, ended the stream on the peer's "
            "close, kept an idle connection past the silence bound, and released the link cleanly"
        )


if __name__ == "__main__":
    main()
