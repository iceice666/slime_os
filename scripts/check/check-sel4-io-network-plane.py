#!/usr/bin/env python3
"""IO4 and IO11 gate: destination-scoped network authority under seL4, and
the data plane behind it.

Six arms share this checker: authority, external TCP, local TCP, client lifetime,
service fault, and driver reset. The authority arm boots `sel4-io-network`, whose
service is bound to a loopback that performs no link operation, and proves the
exact-destination boundary alone. The tcp arm boots `sel4-io-tcp`, whose
service is bound to the IO3 virtio-net driver behind a frame-level peer on
QEMU's UDP socket backend (`scripts/lib/link_peer.py`). Its packet ledger
independently proves the exact 4096-byte TCP echo, handshake and graceful
close, refused-port reset, absence of forbidden egress, and ARP/ICMP traffic.
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
            r"\[io-network-intruder\] guessed capability refusals=4",
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
            r"\[network-service\] authority destinations=5 rights=connect,send,recv",
            r"\[network-service\] declared socket_limit=5 listener_limit=0 dns_record_limit=0",
            r"\[network-service\] clock rate=[0-9]+",
            r"\[network-service\] interface addr=10\.0\.0\.1/24 gateway=none mac=52:54:00:53:4c:01",
            r"\[network-service\] link query state=up rx provisioned=4",
            r"\[network-service\] application bytes sent=4096 received=4096",
            r"\[network-service\] application buffers released",
            r"\[network-service\] application connection handles live=0",
            r"\[network-service\] link frames total=[0-9]+ tx=[0-9]+ rx=[0-9]+ arp=[0-9]+ icmp=[0-9]+ tcp=[1-9]\d* other=0",
            r"\[network-service\] link statistics tx=[0-9]+ rx=[0-9]+",
            r"\[network-service\] link released",
            r"\[network-service\] observed requests=20 packets=2 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=0",
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
            r"\[io-tcp-probe\] tcp established=1 rights=connect,send,recv",
            r"\[io-tcp-probe\] stream sent=4096 received=4096 identical=1 partial_writes=[1-9]\d* would_block=[0-9]+",
            r"\[io-tcp-probe\] tcp close completed=1",
            r"\[io-tcp-probe\] peer refused=1 undeclared address=1 port=1",
            r"\[io-tcp-probe\] held ms=3000",
            r"\[io-tcp-probe\] closed capabilities=1 shutdown=1",
        ),
    ),
    (
        "tcp denials",
        (
            r"\[io-network-intruder\] exact authority refusals=8",
            r"\[io-network-intruder\] guessed capability refusals=4",
            r"\[io-network-intruder\] rights-mask refusals=2",
            r"\[io-network-intruder\] structured denials=14 shutdown=1",
        ),
    ),
    (
        "tcp health",
        (r"SLIME_GRAPH HEALTHY generation=54 required=5 live=0 completed=5 failed=0",),
    ),
)
LOCAL_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("local admission", (r"SLIME_ROOT generation admitted number=155 executables=3 instances=4 grants=5 ",)),
    ("local service", (
        r"\[network-service\] loopback interface=127\.0\.0\.1 external_nic=none",
        r"\[network-service\] wait wakes=[1-9]\d* coalesced=[0-9]+",
        r"\[network-service\] loopback frames=[1-9]\d* rejected=0 handles=0 external_frames=0 resets=0 syns=2 fins=2",
    )),
    ("local publisher", (
        r"\[io-local-network-probe\] role=publisher attached=1",
        r"\[io-local-network-probe\] role=publisher authority_refusals=4",
        r"\[io-local-network-probe\] role=publisher connected=1",
        r"\[io-local-network-probe\] role=publisher timeout_retries=[01]",
        r"\[io-local-network-probe\] role=publisher sent=4096 received=2048 identical=1",
        r"\[io-local-network-probe\] role=publisher connections closed=1",
        r"\[io-local-network-probe\] role=publisher notification_wakes=[1-9]\d*",
        r"\[io-local-network-probe\] role=publisher loans returned=2 shutdown=1",
    )),
    ("local subscriber", (
        r"\[io-local-network-probe\] role=subscriber attached=1",
        r"\[io-local-network-probe\] role=subscriber authority_refusals=3",
        r"\[io-local-network-probe\] role=subscriber listening=1",
        r"\[io-local-network-probe\] role=subscriber accepted=1",
        r"\[io-local-network-probe\] role=subscriber timeout=1 resumed=1",
        r"\[io-local-network-probe\] role=subscriber sent=2048 received=4096 identical=1",
        r"\[io-local-network-probe\] role=subscriber eof=1 connections closed=1 listeners closed=1",
        r"\[io-local-network-probe\] role=subscriber notification_wakes=[1-9]\d*",
        r"\[io-local-network-probe\] role=subscriber loans returned=2 shutdown=1",
    )),
    ("local health", (r"SLIME_GRAPH HEALTHY generation=155 required=4 live=0 completed=4 failed=0",)),
)

LIFETIME_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("lifetime supervisor", (
        r"\[io-network-lifetime-probe\] round=1 victim_fault=1 survivor_exit=1 service_exit=1",
        r"\[io-network-lifetime-probe\] round=2 victim_exit=1 survivor_exit=1 service_exit=1",
        r"\[io-network-lifetime-probe\] rounds=2 reclaimed=1 restarted=1",
    )),
    ("lifetime service", (
        r"\[network-service\] incarnation=1",
        r"\[network-service\] client death handles=1 sockets=1 bytes=4096 sessions_released=1",
        r"\[network-service\] loopback frames=[1-9]\d* rejected=0 handles=0 external_frames=0 resets=[1-9]\d*",
        r"\[network-service\] incarnation=2",
    )),
    ("lifetime stale handles", (
        r"\[io-network-lifetime-probe\] role=victim old_handle_refused=1 fresh_identity=1",
    )),
    ("lifetime accepted stale handle", (
        r"\[io-network-lifetime-probe\] role=survivor old_handle_refused=1 fresh_identity=1",
    )),
    ("lifetime health", (r"SLIME_GRAPH HEALTHY generation=156 required=2 live=0 completed=2 failed=0",)),
    ("lifetime clients", (
        r"\[io-network-lifetime-probe\] role=victim sent=1024 acknowledged=16 faulting=1",
        r"\[io-network-lifetime-probe\] role=survivor received=1024 identical=1 reset=1",
        r"\[io-network-lifetime-probe\] role=survivor received=1024 identical=1 eof=1 fresh=1",
    )),
)

SERVICE_FAULT_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("service fault stale client", (r"\[io-network-lifetime-probe\] role=victim old_handle_refused=1 fresh_identity=1",)),
    ("service fault stale accept", (r"\[io-network-lifetime-probe\] role=survivor old_handle_refused=1 fresh_identity=1",)),
    ("service fault", (
        r"\[network-service\] incarnation=1",
        r"\[network-service\] injected in-flight fault incarnation=1",
        r"SLIME_GRAPH holder reclaimed task=4 charges=12 actions=16",
        r"\[io-network-lifetime-probe\] round=1 service_fault=1 clients_invalidated=2",
        r"\[network-service\] incarnation=2",
        r"\[io-network-lifetime-probe\] service_fault=1 clients_invalidated=2 restarted=1",
        r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*",
        r"SLIME_GRAPH HEALTHY generation=157 required=2 live=0 completed=2 failed=0",
    )),
)

DRIVER_RESET_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("driver reset service", (
        r"\[network-service\] incarnation=1",
        r"\[network-service\] driver reset requests=1 handles=1 sockets=1 bytes=4096",
        r"\[network-service\] incarnation=2",
    )),
    ("driver reset client", (
        r"\[io-tcp-probe\] driver reset sent=1024 terminal=reset transferred=0",
        r"\[io-tcp-probe\] driver reset loans returned=2 shutdown=1",
        r"\[io-tcp-probe\] old_handle_refused=1 fresh_identity=1",
        r"\[io-tcp-probe\] stream sent=4096 received=4096 identical=1 partial_writes=[1-9]\d* would_block=[0-9]+",
        r"\[io-tcp-probe\] tcp close completed=1",
    )),
    ("driver reset epoch", (
        r"\[virtio-net-driver\] reset queued tx=[0-9]+ rx=[0-9]+",
        r"\[virtio-net-driver\] reset settled tx=[0-9]+ rx=[0-9]+ leases=[0-9]+",
        r"\[virtio-net-driver\] fresh epoch old=1 new=2",
        r"\[virtio-net-driver\] negotiated legacy features=0 queues rx=16 tx=16 epoch=2",
        r"\[virtio-net-driver\] fresh epoch old=2 new=3",
    )),
    ("driver reset supervisor", (
        r"\[io-network-lifetime-probe\] driver_reset=1 restarted=1 rounds=2",
        r"SLIME_GRAPH HEALTHY generation=158 required=2 live=0 completed=2 failed=0",
    )),
)

# Every arm participates in the shared missing/reordered/failure controls.
CHAINS = AUTHORITY_CHAINS + TCP_CHAINS + LOCAL_CHAINS + LIFETIME_CHAINS + SERVICE_FAULT_CHAINS + DRIVER_RESET_CHAINS
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[network-service\] fail: ",
    r"\[network-service\] link fail: ",
    r"\[io-network-probe\] fail: ",
    r"\[io-network-intruder\] fail: ",
    r"\[io-tcp-probe\] fail: ",
    r"\[io-local-network-probe\] fail: ",
    r"\[io-network-lifetime-probe\] fail: ",
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


def run_tcp_arm(image: Path, mac: str, transcript_path: Path | None, *, driver_reset: bool = False) -> None:
    receiver, backend_port = reserve_udp_port()
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    qemu_port = int(probe.getsockname()[1])
    probe.close()
    peer = link_peer.Peer(hold_first_tcp=driver_reset)
    stop = threading.Event()
    peer_errors: list[BaseException] = []

    def observe() -> None:
        try:
            link_peer.serve(receiver, qemu_port, stop, peer)
        except BaseException as error:
            peer_errors.append(error)

    thread = threading.Thread(target=observe, daemon=True)
    thread.start()
    chains = DRIVER_RESET_CHAINS if driver_reset else TCP_CHAINS
    terminal = re.compile(chains[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
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
    if thread.is_alive():
        fail("peer observer did not stop; wire evidence is incomplete")
    if peer_errors:
        fail(f"peer observer failed; wire evidence is incomplete: {peer_errors[0]!r}")
    if transcript_path is not None:
        packets = "".join(
            f"[peer {direction} {index}] {frame!r}\n"
            for direction, frames in (("received", peer.ledger.received), ("sent", peer.ledger.sent))
            for index, frame in enumerate(frames)
        )
        transcript_path.write_text(transcript + f"\n[peer] {peer.ledger.summary()}\n" + packets, encoding="utf-8")
    try:
        match_marker_contract(transcript, chains, FAILURE_MARKERS, fail)
    except SystemExit:
        print(transcript)
        raise
    ledger = peer.ledger
    print(f"[peer] {ledger.summary()}")
    if driver_reset:
        try:
            link_peer.qualify_driver_reset_ledger(ledger, bytes.fromhex(mac.replace(":", "")))
        except ValueError as error:
            fail(f"driver reset peer wire evidence: {error}")
    else:
        check_tcp_ledger(ledger, bytes.fromhex(mac.replace(":", "")))


def check_tcp_ledger(ledger: link_peer.Ledger, guest_mac: bytes) -> None:
    try:
        link_peer.qualify_tcp_ledger(ledger, guest_mac)
    except ValueError as error:
        fail(f"peer wire evidence: {error}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 I/O network proof planes")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--arm", choices=("authority", "tcp", "local", "lifetime", "service-fault", "driver-reset", "all"), default="all")
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
    if arguments.arm in ("driver-reset", "all"):
        image = build_image("sel4-io-driver-reset-in-flight")
        run_tcp_arm(image, "52:54:00:53:4c:01", arguments.transcript, driver_reset=True)
        print("seL4 I/O driver reset check: actual in-flight virtio-net reset, typed settlement, supervised restart and fresh external bytes proved")
    if arguments.arm in ("service-fault", "all"):
        image = build_image("sel4-io-service-fault-in-flight")
        terminal = re.compile(r"SLIME_GRAPH HEALTHY generation=157 required=2 live=0 completed=2 failed=0|" + "|".join(FAILURE_MARKERS))
        transcript = run_plane(image=image, timeout=TIMEOUT, terminal_condition=terminal, fail=fail, pins_path=PINS)
        if arguments.transcript is not None:
            arguments.transcript.write_text(transcript, encoding="utf-8")
        try:
            match_marker_contract(transcript, SERVICE_FAULT_CHAINS, FAILURE_MARKERS, fail)
        except SystemExit:
            print(transcript)
            raise
        print("seL4 I/O service fault check: in-flight service fault, client invalidation and fresh-incarnation recovery proved")
    if arguments.arm in ("lifetime", "all"):
        image = build_image("sel4-io-lifetime")
        terminal = re.compile(r"SLIME_GRAPH HEALTHY generation=156 required=2 live=0 completed=2 failed=0|" + "|".join(FAILURE_MARKERS))
        transcript = run_plane(image=image, timeout=TIMEOUT, terminal_condition=terminal, fail=fail, pins_path=PINS)
        if arguments.transcript is not None:
            arguments.transcript.write_text(transcript, encoding="utf-8")
        try:
            match_marker_contract(transcript, LIFETIME_CHAINS, FAILURE_MARKERS, fail)
        except SystemExit:
            print(transcript)
            raise
        print("seL4 I/O lifetime plane check: actual client fault, interrupted receive reset, buffer reclamation and fresh-incarnation traffic proved")
    if arguments.arm in ("local", "all"):
        image = build_image("sel4-io-local")
        terminal = re.compile(LOCAL_CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
        transcript = run_plane(image=image, timeout=TIMEOUT, terminal_condition=terminal, fail=fail, pins_path=PINS)
        if arguments.transcript is not None:
            arguments.transcript.write_text(transcript, encoding="utf-8")
        try:
            match_marker_contract(transcript, LOCAL_CHAINS, FAILURE_MARKERS, fail)
        except SystemExit:
            print(transcript)
            raise
        print("seL4 I/O local plane check: real TCP listen/connect/accept, independent bidirectional bytes, EOF and normal teardown without a NIC proved")
    if arguments.arm in ("tcp", "all"):
        mac = check_tcp_fixture()
        image = build_image(TCP_CLOSURE)
        run_tcp_arm(image, mac, arguments.transcript)
        print(
            "seL4 I/O tcp plane check: exact 4096-byte TCP echo and graceful close, "
            "refused-port reset, no forbidden egress, ARP/ICMP replies, "
            "and clean virtio-net link release proved"
        )


if __name__ == "__main__":
    main()
