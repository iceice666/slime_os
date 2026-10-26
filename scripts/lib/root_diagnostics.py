"""Judge for the root's per-operation diagnostic lines.

The root may print each per-operation diagnostic once per key and count the
repeats. This module holds a serial transcript to that rule:

* `SLIME_CLOCK served task=T label=L ...` at most once per (T, L);
* `SLIME_CLOCK refused task=T label=L class=C ...` at most once per (T, L, C);
* `SLIME_IO payload dma ... direction=D` at most once per D, each followed by
  exactly its `frames` count of `SLIME_IO payload frame` lines;
* exactly one `SLIME_ROOT diagnostic repeats clock_served=N clock_refused=N
  io_device_read=N io_device_write=N` line, before `SLIME_GRAPH served`.

The counters are not trusted on their own. A device-read DMA carries every
frame the guest transmits and a device-write DMA backs every receive buffer the
driver posts, so the printed lines plus the repeats must equal what the
scripted peer saw on the wire: device reads exactly the frames the peer
received, device writes the frames the peer sent plus at most the posted
receive slots. Standard library only.
"""

from __future__ import annotations

import dataclasses
import re
from collections import Counter

SERVED = re.compile(r"^SLIME_CLOCK served task=(\d+) label=(\d+) ")
REFUSED = re.compile(r"^SLIME_CLOCK refused task=(\d+) label=(\d+) class=(\S+) ")
DMA = re.compile(r"^SLIME_IO payload dma pages=(\d+) frames=(\d+) writable=(\w+) direction=(DeviceRead|DeviceWrite)$")
FRAME = re.compile(r"^SLIME_IO payload frame index=\d+ ")
SUMMARY = re.compile(
    r"^SLIME_ROOT diagnostic repeats clock_served=(\d+) clock_refused=(\d+) "
    r"io_device_read=(\d+) io_device_write=(\d+)$"
)
GRAPH_SERVED = re.compile(r"^SLIME_GRAPH served live=")
# The receive slots the network service posts to the virtio-net driver
# (`RX_FRAMES` in components/services/network-service/src/link.rs).
RX_SLOTS = 4
ANSI = re.compile(r"\x1b\[[0-9;]*m")


@dataclasses.dataclass(frozen=True)
class Verdict:
    served_keys: int
    refused_keys: int
    device_read: int
    device_write: int
    repeats: tuple[int, int, int, int]

    def line(self) -> str:
        served, refused, read, write = self.repeats
        return (
            f"[root-diagnostics] served_keys={self.served_keys} refused_keys={self.refused_keys} "
            f"repeats clock_served={served} clock_refused={refused} "
            f"device_read={self.device_read} device_write={self.device_write}"
        )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def judge(transcript: str, guest_tx_frames: int, guest_rx_frames: int, rx_slots: int = RX_SLOTS) -> Verdict:
    """Raise ValueError unless `transcript` keeps the once-per-key rule and its
    counters agree with the frames the peer saw. Returns what was counted."""
    lines = [ANSI.sub("", line).strip() for line in transcript.splitlines()]
    served: Counter[tuple[str, str]] = Counter()
    refused: Counter[tuple[str, str, str]] = Counter()
    dma: Counter[str] = Counter()
    summaries: list[tuple[int, tuple[int, int, int, int]]] = []
    graph_served: int | None = None
    pending_frames = 0
    for position, line in enumerate(lines):
        if pending_frames:
            if FRAME.match(line):
                pending_frames -= 1
                continue
            # A frame line may be preceded by an unrelated interleaved line.
            if line.startswith("SLIME_IO payload"):
                raise ValueError("a payload DMA line was not followed by its frame lines")
        if (match := SERVED.match(line)) is not None:
            served[(match[1], match[2])] += 1
        elif (match := REFUSED.match(line)) is not None:
            refused[(match[1], match[2], match[3])] += 1
        elif (match := DMA.match(line)) is not None:
            require(pending_frames == 0, "a payload DMA line was not followed by its frame lines")
            dma[match[4]] += 1
            pending_frames = int(match[2])
            require(pending_frames >= 1, "a payload DMA line named no frame")
        elif FRAME.match(line):
            raise ValueError("a payload frame line follows no printed payload DMA line")
        elif (match := SUMMARY.match(line)) is not None:
            summaries.append((position, (int(match[1]), int(match[2]), int(match[3]), int(match[4]))))
        elif GRAPH_SERVED.match(line) and graph_served is None:
            graph_served = position
    require(pending_frames == 0, "a payload DMA line was not followed by its frame lines")
    repeated = [key for key, count in served.items() if count > 1]
    require(not repeated, f"the root printed SLIME_CLOCK served more than once for task/label {repeated[:3]}")
    repeated_refusals = [key for key, count in refused.items() if count > 1]
    require(not repeated_refusals, f"the root printed SLIME_CLOCK refused more than once for task/label/class {repeated_refusals[:3]}")
    repeated_dma = [key for key, count in dma.items() if count > 1]
    require(not repeated_dma, f"the root printed SLIME_IO payload dma more than once for direction {repeated_dma}")
    require(len(summaries) == 1, f"the root printed {len(summaries)} diagnostic repeat summaries, expected exactly one")
    position, repeats = summaries[0]
    require(graph_served is not None and position < graph_served, "the diagnostic repeat summary does not precede SLIME_GRAPH served")
    served_repeats, refused_repeats, read_repeats, write_repeats = repeats
    require(served and served_repeats > 0, "the root counted no repeated clock service")
    device_read = dma["DeviceRead"] + read_repeats
    device_write = dma["DeviceWrite"] + write_repeats
    require(
        device_read == guest_tx_frames,
        f"the root accounted {device_read} device-read DMA operations, but the guest put {guest_tx_frames} frames on the wire",
    )
    require(
        guest_rx_frames <= device_write <= guest_rx_frames + rx_slots,
        f"the root accounted {device_write} device-write DMA operations, outside the {guest_rx_frames} frames the guest received plus {rx_slots} posted slots",
    )
    return Verdict(len(served), len(refused), device_read, device_write, repeats)


def honest_transcript(tx: int = 40, rx: int = 38) -> str:
    """A synthetic transcript that keeps the rule, for the controls."""
    lines = [
        "SLIME_ROOT generation admitted number=170 executables=4 instances=5 grants=10 health=5 bootstrap=1",
        "SLIME_CLOCK served task=0 label=44 result=100 live=0",
        "SLIME_CLOCK served task=0 label=45 result=1 live=1",
        "SLIME_CLOCK served task=3 label=44 result=200 live=1",
        "SLIME_CLOCK refused task=3 label=46 class=stale detail=NoTimer",
        "SLIME_IO payload dma pages=1 frames=1 writable=true direction=DeviceWrite",
        "SLIME_IO payload frame index=0 slot=1249 paddr=Some(1614823424)",
        "[network-service] tcp open holder=buffers-large socket=0 local_port=49152 rx_bytes=65536 tx_bytes=65536",
        "SLIME_IO payload dma pages=1 frames=1 writable=false direction=DeviceRead",
        "SLIME_IO payload frame index=0 slot=1235 paddr=Some(1613774848)",
        "SLIME_GRAPH HEALTHY generation=170 required=5 live=0 completed=5 failed=0",
        f"SLIME_ROOT diagnostic repeats clock_served=900 clock_refused=12 io_device_read={tx - 1} io_device_write={rx + RX_SLOTS - 1}",
        "SLIME_GRAPH served live=0 unsupported=0 buffers=32 windows=3 tasks=3",
    ]
    return "\n".join(lines) + "\n"


def controls() -> int:
    """Run every mutation through `judge`; return how many were refused for the stated reason."""
    tx, rx = 40, 38
    honest = honest_transcript(tx, rx)
    verdict = judge(honest, tx, rx)
    require(verdict.device_read == tx and verdict.device_write == rx + RX_SLOTS, "the honest transcript did not reconcile")
    # The lower edge of the device-write window and an interleaved line before a frame line are accepted.
    judge(honest.replace(f"io_device_write={rx + RX_SLOTS - 1}", f"io_device_write={rx - 1}"), tx, rx)
    judge(honest.replace("SLIME_IO payload frame index=0 slot=1235", "[io-tcp-buffers-probe] role=large attached=1\nSLIME_IO payload frame index=0 slot=1235"), tx, rx)
    refused = 0

    def refuse(fragment: str, text: str, tx_frames: int = tx, rx_frames: int = rx) -> None:
        nonlocal refused
        try:
            judge(text, tx_frames, rx_frames)
        except ValueError as error:
            require(fragment in str(error), f"a root diagnostics control was refused for another reason: expected {fragment!r}, got {error}")
            refused += 1
            return
        raise ValueError(f"the root diagnostics judge accepted corrupted evidence: {fragment}")

    served = "SLIME_CLOCK served task=0 label=44 result=100 live=0\n"
    refused_line = "SLIME_CLOCK refused task=3 label=46 class=stale detail=NoTimer\n"
    read_dma = "SLIME_IO payload dma pages=1 frames=1 writable=false direction=DeviceRead\nSLIME_IO payload frame index=0 slot=1235 paddr=Some(1613774848)\n"
    summary = next(line for line in honest.splitlines() if line.startswith("SLIME_ROOT diagnostic repeats"))
    refuse("SLIME_CLOCK served more than once", honest.replace(served, served + served.replace("result=100", "result=101")))
    refuse("SLIME_CLOCK refused more than once", honest.replace(refused_line, refused_line * 2))
    refuse("SLIME_IO payload dma more than once", honest.replace(read_dma, read_dma * 2))
    refuse("not followed by its frame lines", honest.replace("SLIME_IO payload frame index=0 slot=1235 paddr=Some(1613774848)\n", ""))
    refuse("follows no printed payload DMA line", honest.replace(read_dma, read_dma + "SLIME_IO payload frame index=0 slot=7 paddr=None\n"))
    refuse("expected exactly one", honest.replace(summary + "\n", ""))
    refuse("expected exactly one", honest.replace(summary + "\n", summary + "\n" + summary + "\n"))
    refuse("does not precede SLIME_GRAPH served", honest.replace(summary + "\n", "").replace("SLIME_GRAPH served live=0 unsupported=0 buffers=32 windows=3 tasks=3\n", "SLIME_GRAPH served live=0 unsupported=0 buffers=32 windows=3 tasks=3\n" + summary + "\n"))
    refuse("counted no repeated clock service", honest.replace("clock_served=900", "clock_served=0"))
    refuse("device-read DMA operations", honest, tx_frames=tx + 1)
    refuse("device-read DMA operations", honest, tx_frames=tx - 1)
    refuse("device-write DMA operations", honest, rx_frames=rx + RX_SLOTS + 1)
    refuse("device-write DMA operations", honest.replace(f"io_device_write={rx + RX_SLOTS - 1}", f"io_device_write={rx + RX_SLOTS}"))
    refuse("device-write DMA operations", honest.replace(f"io_device_write={rx + RX_SLOTS - 1}", f"io_device_write={rx - 2}"))
    return refused
