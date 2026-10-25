"""Judge for network-service waking on link completions.

network-service parks on its one wait-set Notification. On main the virtio-net
driver signals TX and RX completion on separate Notifications the service never
waits on, so a frame that arrives while no client request is pending is
noticed only at the service's next idle timer, up to 10 ms later. The fix
declares a `device` wait-set source: the driver signals the service's wait
Notification on every completion.

Three independent checks:

* the derived composition: the driver holds exactly one signal binding on the
  Notification the service's wait set uses, and the service's wait set has
  exactly one `device` row on that badge, with no drain slot;
* the wire: in the rx-window scenario the probe reads nothing, so no client
  request wakes the service, and the median time from a peer data segment to
  the guest frame that acknowledges it is below the bound for each holder;
* the transcript: the service reports `[network-service] wait device wakes=D`
  once, with 1 <= D <= its `wait wakes=` count.

Standard library only.
"""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence

import link_peer as lp
import tcp_buffers_peer as tbp

# Compositions whose network-service has a wait set and a virtio-net driver.
COMPOSITIONS: tuple[str, ...] = ("sel4-io-tcp-buffers", "sel4-io-tcp-listener", "sel4-io-tcp-options", "sel4-zenoh-external")
DRIVER = "virtio-net-driver"
SERVICE = "network-service"
# Half the service's 10 ms idle timer: a timer-paced acknowledgment cannot have
# a median below it, a completion-woken one has no reason to approach it.
LATENCY_BOUND_MS = 4.0
DEVICE_WAKES = re.compile(r"^\[network-service\] wait device wakes=(\d+)$", re.M)
WAKES = re.compile(r"^\[network-service\] wait wakes=(\d+) coalesced=\d+$", re.M)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _rows(text: str, section: str) -> list[dict[str, str]]:
    """One manifest section's rows as flat field maps."""
    block = re.search(rf"\n  {section} = \[(.*?)\n  \];", text, re.S)
    if block is None:
        return []
    rows = []
    for body in re.findall(r"\{(.*?)\n    \};", block.group(1), re.S):
        rows.append({key: value.strip('"') for key, value in re.findall(r'^\s*(\w+)\s*=\s*("[^"]*"|-?[0-9]+|true|false);', body, re.M)})
    return rows


def judge_composition(name: str, text: str) -> int:
    """Raise ValueError unless the driver signals the service's wait Notification
    as one declared `device` source; return the facts checked."""
    waits = [row for row in _rows(text, "waitSet") if row.get("waiter") == SERVICE]
    timers = [row for row in waits if row.get("kind") == "timer"]
    require(len(timers) == 1, f"{name}: network-service has {len(timers)} timer wait-set rows, expected one")
    notification = timers[0]["notification"]
    require(all(row.get("notification") == notification for row in waits), f"{name}: network-service waits on more than one Notification")
    signals = [row for row in _rows(text, "notificationBindings") if row.get("holder") == DRIVER and row.get("role") == "signal" and row.get("grant") == notification]
    require(len(signals) == 1, f"{name}: {DRIVER} holds {len(signals)} signal bindings on {notification}, expected one")
    badge = str(int(signals[0]["slot"]) % 63)
    devices = [row for row in waits if row.get("kind") == "device"]
    require(len(devices) == 1, f"{name}: network-service has {len(devices)} device wait-set rows, expected one")
    device = devices[0]
    require(device.get("badgeBit") == badge, f"{name}: the device row's badge bit {device.get('badgeBit')} is not the driver's binding slot {badge}")
    require("drainSlot" not in device, f"{name}: the device row names a drain slot")
    require(sum(1 for row in waits if row.get("badgeBit") == badge) == 1, f"{name}: another wait-set row shares the device badge bit {badge}")
    return 6


def ack_latencies(flow: tbp.Flow) -> list[float]:
    """Milliseconds from each peer data segment to the first later guest frame
    that acknowledges all of it. `flow` has passed `tbp.handshake`."""
    latencies: list[float] = []
    for segment in flow.peer:
        frame = segment.frame
        if not frame.tcp_payload or frame.tcp_flags & lp.TCP_SYN:
            continue
        end = flow.peer_offset(frame) + len(frame.tcp_payload)
        answer = next(
            (entry for entry in flow.guest if entry.order > segment.order and entry.frame.tcp_flags & lp.TCP_ACK and flow.guest_acked(entry.frame) >= end),
            None,
        )
        if answer is not None:
            latencies.append((answer.time - segment.time) * 1000)
    return latencies


def median(values: Sequence[float]) -> float:
    ordered = sorted(values)
    return ordered[len(ordered) // 2]


def judge_latencies(latencies: Mapping[str, Sequence[float]]) -> dict[str, float]:
    """Raise ValueError unless every holder's median acknowledgment latency is below the bound."""
    require(set(latencies) == set(tbp.HOLDERS), f"latencies name holders {sorted(latencies)}, expected {sorted(tbp.HOLDERS)}")
    medians: dict[str, float] = {}
    for holder, values in latencies.items():
        require(len(values) >= 3, f"the {holder} rx-window scenario acknowledged {len(values)} peer segments, too few to judge")
        medians[holder] = median(values)
        require(medians[holder] < LATENCY_BOUND_MS, f"the {holder} rx-window median acknowledgment latency is {medians[holder]:.2f} ms, not below {LATENCY_BOUND_MS} ms")
    return medians


def judge_wakes(transcript: str) -> int:
    """Raise ValueError unless the service reports a plausible positive device wake count."""
    devices = DEVICE_WAKES.findall(transcript)
    require(len(devices) == 1, f"network-service reported {len(devices)} device wake lines, expected one")
    wakes = WAKES.findall(transcript)
    require(len(wakes) == 1, f"network-service reported {len(wakes)} wait wake lines, expected one")
    device, total = int(devices[0]), int(wakes[0])
    require(1 <= device <= total, f"network-service reported {device} device wakes of {total}")
    return device


def honest_manifest() -> str:
    return """{
  notificationBindings = [
    {
      grant = "x-network-request";
      holder = "client";
      role = "signal";
      slot = 0;
    };
    {
      grant = "x-network-request";
      holder = "virtio-net-driver";
      role = "signal";
      slot = 5;
    };
    {
      grant = "x-network-request";
      holder = "network-service";
      role = "wait";
      slot = 5;
    };
  ];
  waitSet = [
    {
      badgeBit = 0;
      drainSlot = 2;
      kind = "stream";
      notification = "x-network-request";
      waiter = "network-service";
    };
    {
      badgeBit = 5;
      kind = "device";
      notification = "x-network-request";
      waiter = "network-service";
    };
    {
      badgeBit = 7;
      kind = "timer";
      notification = "x-network-request";
      waiter = "network-service";
    };
  ];
}
"""


def _flow(delays_ms: Sequence[float]) -> tbp.Flow:
    """A synthetic receive flow: the peer sends 1024-byte segments, the guest acknowledges each after `delay`."""
    guest_isn, peer_isn = 1000, 5000
    guest: list[tbp.Observation] = []
    peer: list[tbp.Observation] = []
    order = 0
    now = 0.0

    def frame(source_port: int, destination_port: int, sequence: int, acknowledgment: int, flags: int, payload: bytes, source_ip: bytes, destination_ip: bytes) -> lp.Frame:
        segment = lp.tcp(source_ip, destination_ip, source_port, destination_port, sequence, acknowledgment, flags, payload, 1024)
        decoded = lp.decode(lp.ethernet(lp.PEER_MAC, lp.PEER_MAC, lp.ETHERTYPE_IPV4, lp.ipv4(source_ip, destination_ip, lp.IP_PROTOCOL_TCP, segment)))
        assert decoded is not None
        return decoded

    for index, delay in enumerate(delays_ms):
        order += 1
        peer.append(tbp.Observation(order, now, frame(4301, 49153, peer_isn + 1 + 1024 * index, guest_isn + 1, lp.TCP_ACK | lp.TCP_PSH, bytes(1024), lp.PEER_IP, lp.GUEST_IP)))
        now += delay / 1000
        order += 1
        guest.append(tbp.Observation(order, now, frame(49153, 4301, guest_isn + 1, peer_isn + 1 + 1024 * (index + 1), lp.TCP_ACK, b"", lp.GUEST_IP, lp.PEER_IP)))
        now += 0.0005
    return tbp.Flow(4301, 49153, guest, peer, guest_isn, peer_isn)


def controls() -> int:
    """Run every mutation through the judges; return how many were refused for the stated reason."""
    require(judge_composition("honest", honest_manifest()) == 6, "the honest manifest did not yield every fact")
    fast = ack_latencies(_flow([0.5, 1.0, 0.8, 30.0, 0.6]))
    require([round(value, 3) for value in fast] == [0.5, 1.0, 0.8, 30.0, 0.6], f"ack_latencies mismeasured a synthetic flow: {fast}")
    judge_latencies({"large": fast, "baseline": fast})
    judge_wakes("[network-service] wait wakes=40 coalesced=2\n[network-service] wait device wakes=40\n")
    refused = 0

    def refuse(fragment: str, call) -> None:
        nonlocal refused
        try:
            call()
        except ValueError as error:
            require(fragment in str(error), f"a link wake control was refused for another reason: expected {fragment!r}, got {error}")
            refused += 1
            return
        raise ValueError(f"the link wake judge accepted corrupted evidence: {fragment}")

    honest = honest_manifest()
    device_row = """    {
      badgeBit = 5;
      kind = "device";
      notification = "x-network-request";
      waiter = "network-service";
    };
"""
    driver_binding = """    {
      grant = "x-network-request";
      holder = "virtio-net-driver";
      role = "signal";
      slot = 5;
    };
"""
    refuse("device wait-set rows, expected one", lambda: judge_composition("m", honest.replace(device_row, "")))
    refuse("device wait-set rows, expected one", lambda: judge_composition("m", honest.replace(device_row, device_row * 2)))
    refuse("signal bindings on x-network-request, expected one", lambda: judge_composition("m", honest.replace(driver_binding, "")))
    refuse("signal bindings on x-network-request, expected one", lambda: judge_composition("m", honest.replace(driver_binding, driver_binding * 2)))
    refuse("is not the driver's binding slot", lambda: judge_composition("m", honest.replace("badgeBit = 5;\n      kind = \"device\"", "badgeBit = 6;\n      kind = \"device\"")))
    refuse("names a drain slot", lambda: judge_composition("m", honest.replace("badgeBit = 5;\n      kind = \"device\"", "badgeBit = 5;\n      drainSlot = 4;\n      kind = \"device\"")))
    refuse("shares the device badge bit", lambda: judge_composition("m", honest.replace("badgeBit = 0;\n      drainSlot = 2;", "badgeBit = 5;\n      drainSlot = 2;")))
    refuse("waits on more than one Notification", lambda: judge_composition("m", honest.replace("badgeBit = 5;\n      kind = \"device\";\n      notification = \"x-network-request\"", "badgeBit = 5;\n      kind = \"device\";\n      notification = \"io-link-rx-completion-ready\"")))
    refuse("timer wait-set rows, expected one", lambda: judge_composition("m", honest.replace("kind = \"timer\"", "kind = \"stream\"")))
    slow = ack_latencies(_flow([12.0, 11.0, 0.5, 13.0, 9.0]))
    refuse("not below 4.0 ms", lambda: judge_latencies({"large": fast, "baseline": slow}))
    refuse("not below 4.0 ms", lambda: judge_latencies({"large": [4.0, 4.0, 4.0], "baseline": fast}))
    refuse("too few to judge", lambda: judge_latencies({"large": fast[:2], "baseline": fast}))
    refuse("expected ['baseline', 'large']", lambda: judge_latencies({"large": fast}))
    refuse("device wake lines, expected one", lambda: judge_wakes("[network-service] wait wakes=40 coalesced=2\n"))
    refuse("device wake lines, expected one", lambda: judge_wakes("[network-service] wait wakes=40 coalesced=2\n[network-service] wait device wakes=1\n[network-service] wait device wakes=1\n"))
    refuse("device wakes of", lambda: judge_wakes("[network-service] wait wakes=40 coalesced=2\n[network-service] wait device wakes=0\n"))
    refuse("device wakes of", lambda: judge_wakes("[network-service] wait wakes=40 coalesced=2\n[network-service] wait device wakes=41\n"))
    refuse("wait wake lines, expected one", lambda: judge_wakes("[network-service] wait device wakes=1\n"))
    return refused
