#!/usr/bin/env python3
"""PCI function binding gate: the root binds one declared PCI function on qemu-arm-virt.

The judge trusts no line the guest prints about the machine. It reads the
machine facts from the device tree QEMU dumps for the pinned machine, and it
reads the configured state of every PCI function back through the QEMU monitor
after the guest finishes. It reads the derived generation manifest to see which
instance holds which hardware grant. Two boots of one image run:

* the bind arm places `qemu-xhci` at the declared function 00:03.0 and an
  undeclared `pci-testdev` at 00:05.0;
* the identity arm places `pci-testdev` at 00:03.0 and `qemu-xhci` at 00:04.0,
  so the declared function carries the wrong identity.

QEMU's default virtio-net PCI function sits at 00:01.0 in both boots. It is one
more function the generation does not declare.

The file name stays outside `check-sel4-*` and the closure name outside a
top-level `CLOSURE` constant until the `sel4-io-pci` closure exists, because
the test-run inventory and the devloop closure digest both resolve those names.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Callable, NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import devloop_observations  # noqa: E402
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import GENERATION_COMPOSITIONS, load_qemu_profile, sha256_file  # noqa: E402
from sel4_plane import run_plane  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
PCI_CLOSURE = "sel4-io-pci"
MANIFEST = GENERATION_COMPOSITIONS / "sel4-io-pci.zti"
TIMEOUT = 240
PROBE = "pci-binding-probe"
INTRUDER = "pci-intruder"

# The one function the generation declares, and the identity it declares there.
DECLARED_SLOT = 3
DECLARED_ID = "1b36:000d"
XHCI_WORD = 0x000D1B36
TESTDEV_WORD = 0x00051B36
DEFAULT_NIC_SLOT = 1
DEFAULT_NIC_WORD = 0x10001AF4
BIND_UNDECLARED_SLOT = 5
IDENTITY_XHCI_SLOT = 4

# Non-default port counts: a probe that prints the model's default capability
# registers instead of reading them disagrees with the monitor.
BIND_DEVICES: tuple[str, ...] = (
    "-device",
    "qemu-xhci,addr=0x3,p2=3,p3=2",
    "-device",
    "pci-testdev,addr=0x5",
)
IDENTITY_DEVICES: tuple[str, ...] = (
    "-device",
    "pci-testdev,addr=0x3",
    "-device",
    "qemu-xhci,addr=0x4,p2=3,p3=2",
)

PCI_COMMAND_MEMORY = 0x0002
HARDWARE_KINDS = ("device", "mmioRegion", "interruptSource", "dmaAccount")
HEX = r"[0-9a-f]"

BIND = re.compile(
    rf"^SLIME_PCI bind function=00:0{DECLARED_SLOT}\.0 id={DECLARED_ID} bar=0 type=mem64 "
    rf"size=0x(?P<size>{HEX}+) address=0x(?P<address>{HEX}+) command=0x(?P<command>{HEX}{{4}}) "
    r"irq=(?P<irq>\d+)$",
    re.M,
)
BOUND = re.compile(
    rf"^\[{PROBE}\] bound bytes=(?P<bytes>\d+) irqs=(?P<irqs>\d+) epoch=(?P<epoch>\d+)$", re.M
)
CAPABILITIES = re.compile(
    rf"^\[{PROBE}\] xhci cap=0x(?P<cap>{HEX}{{8}}) hcsparams1=0x(?P<hcs>{HEX}{{8}}) "
    r"epoch=(?P<epoch>\d+)$",
    re.M,
)
# The root enforces the MMIO bound, so the root prints the refusal.
ROOT_REFUSED = re.compile(
    r"^SLIME_IO mmio refused task=(?P<task>\d+) offset=(?P<offset>\d+) length=(?P<length>\d+) "
    r"error=(?P<error>[A-Za-z]+)$",
    re.M,
)
BEYOND = re.compile(rf"^\[{PROBE}\] mmio beyond bar refused offset=(?P<offset>\d+)$", re.M)
FAULTING = re.compile(rf"^\[{PROBE}\] faulting with live authority$", re.M)
FAULT = re.compile(r"^SLIME_GRAPH component fault task=(?P<task>\d+) ", re.M)
RECLAIM = re.compile(r"^SLIME_IO reclaim task=(?P<task>\d+) (?P<fields>.*)$", re.M)
COMPLETE = re.compile(rf"^\[{PROBE}\] pci binding plane complete$", re.M)
DENIED = re.compile(rf"^\[{INTRUDER}\] denied device=1 mmio=1 interrupt=1$", re.M)
HEALTHY = re.compile(
    r"^SLIME_GRAPH HEALTHY generation=\d+ required=\d+ live=0 completed=\d+ failed=0$", re.M
)
IDENTITY_FATAL = re.compile(
    rf"^SLIME_ROOT FATAL pci function=00:0{DECLARED_SLOT}\.0 declared={DECLARED_ID} "
    rf"found=(?P<found>{HEX}{{4}}:{HEX}{{4}})$",
    re.M,
)
ANY_FATAL = re.compile(r"SLIME_ROOT FATAL")
MONITOR_BANNER = re.compile(r"^QEMU \d+\.\d+\.\d+ monitor - type 'help' for more information$", re.M)
XP_REPLY = re.compile(r"^(?P<address>[0-9a-f]+): (?P<words>(?:0x[0-9a-f]{8} ?)+)$")
INFO_PCI_BAR = re.compile(
    r"^\s+BAR(?P<index>\d): (?P<kind>.+?) at 0x(?P<start>[0-9a-f]+) \[0x(?P<end>[0-9a-f]+)\]\.?$"
)
INFO_PCI_FUNCTION = re.compile(r"^\s+Bus\s+0, device\s+(?P<slot>\d+), function (?P<function>\d+):$")

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_IO FAIL",
    rf"\[{PROBE}\] fail: ",
    rf"\[{INTRUDER}\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)
# The identity arm expects one fatal record; every other failure still ends it.
IDENTITY_FAILURE_MARKERS: tuple[str, ...] = tuple(m for m in FAILURE_MARKERS if m != r"SLIME_ROOT FATAL")


class Refusal(Exception):
    """The judge refused an input."""


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 PCI binding plane check: {message}")


def refuse(message: str) -> NoReturn:
    raise Refusal(message)


# --------------------------------------------------------------------------- machine facts


@dataclass(frozen=True)
class MachineFacts:
    """The qemu-arm-virt PCI facts QEMU states in its own device tree."""

    ecam_base: int
    ecam_size: int
    mem32_base: int
    mem32_size: int
    gic_spi_base: int
    # (device bits & mask, pin & mask, SPI number)
    interrupt_map: tuple[tuple[int, int, int], ...]
    device_mask: int
    pin_mask: int

    def config(self, slot: int, offset: int) -> int:
        """ECAM address of one register of function 00:<slot>.0."""
        address = self.ecam_base + (slot << 15) + offset
        if not self.ecam_base <= address < self.ecam_base + self.ecam_size:
            fail(f"slot {slot} offset {offset:#x} lies outside ECAM")
        return address

    def irq(self, slot: int, pin: int) -> int:
        """seL4 IRQ number of the INTx route of function 00:<slot>.0 on `pin` (1 = INTA)."""
        key = ((slot << 11) & self.device_mask, pin & self.pin_mask)
        for device, mapped_pin, spi in self.interrupt_map:
            if (device, mapped_pin) == key:
                return self.gic_spi_base + spi
        refuse(f"the device tree maps no INTx route for slot {slot} pin {pin}")


def cells(text: str) -> list[int]:
    return [int(token, 16) for token in re.findall(r"0x[0-9a-f]+", text)]


def node_property(node: str, name: str) -> list[int]:
    match = re.search(rf"^\s*{re.escape(name)} = <(?P<cells>[^>]*)>;$", node, re.M)
    if match is None:
        fail(f"the PCIe host node has no {name} property")
    return cells(match.group("cells"))


def read_machine_facts(profile: dict[str, object]) -> MachineFacts:
    """Dump the pinned machine's device tree and read its PCIe host.

    The host node does not depend on the attached functions, so the dump
    attaches none.
    """
    qemu = shutil.which("qemu-system-aarch64")
    dtc = shutil.which("dtc")
    if qemu is None or dtc is None:
        fail("qemu-system-aarch64 and dtc must be on PATH")
    with tempfile.TemporaryDirectory(prefix="slime-pci-dtb-") as directory:
        blob = Path(directory) / "virt.dtb"
        dumped = subprocess.run(
            [
                qemu,
                "-machine",
                f"{profile['machine']},dumpdtb={blob}",
                "-cpu",
                str(profile["cpu"]),
                "-smp",
                str(profile["cpus"]),
                "-m",
                f"size={profile['memory_mib']}M",
                "-nographic",
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if dumped.returncode != 0 or not blob.is_file():
            fail(f"QEMU did not dump the device tree: {dumped.stdout.strip()}")
        source = subprocess.run(
            [dtc, "-q", "-I", "dtb", "-O", "dts", str(blob)],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if source.returncode != 0:
            fail(f"dtc cannot decode the dumped device tree: {source.stderr.strip()}")
    return parse_machine_facts(source.stdout)


def parse_machine_facts(dts: str) -> MachineFacts:
    nodes = re.findall(r"^\tpcie@[0-9a-f]+ \{\n(.*?)^\t\};$", dts, re.M | re.S)
    if len(nodes) != 1:
        fail(f"the device tree declares {len(nodes)} PCIe host nodes, expected 1")
    node = nodes[0]
    if 'compatible = "pci-host-ecam-generic";' not in node:
        fail("the PCIe host is not pci-host-ecam-generic")
    reg = node_property(node, "reg")
    if len(reg) != 4:
        fail(f"unexpected PCIe reg cells: {reg}")
    ecam_base, ecam_size = (reg[0] << 32) | reg[1], (reg[2] << 32) | reg[3]
    ranges = node_property(node, "ranges")
    if len(ranges) % 7:
        fail("PCIe ranges is not a list of 7-cell entries")
    mem32 = [
        ranges[index : index + 7]
        for index in range(0, len(ranges), 7)
        if ranges[index] & 0x0300_0000 == 0x0200_0000
    ]
    if len(mem32) != 1:
        fail(f"expected one 32-bit memory window, found {len(mem32)}")
    window = mem32[0]
    pci_address = (window[1] << 32) | window[2]
    cpu_address = (window[3] << 32) | window[4]
    if pci_address != cpu_address:
        fail("the 32-bit memory window translates addresses; the judge assumes identity")
    mask = node_property(node, "interrupt-map-mask")
    if len(mask) != 4:
        fail(f"unexpected interrupt-map-mask cells: {mask}")
    raw_map = node_property(node, "interrupt-map")
    if len(raw_map) % 10:
        fail("interrupt-map is not a list of 10-cell entries")
    entries = []
    for index in range(0, len(raw_map), 10):
        entry = raw_map[index : index + 10]
        # child address (3) + pin (1) + GIC phandle + parent address (2) + type, number, flags
        if entry[7] != 0:
            fail("interrupt-map routes INTx to a non-SPI interrupt")
        entries.append((entry[0] & mask[0], entry[3] & mask[3], entry[8]))
    return MachineFacts(
        ecam_base=ecam_base,
        ecam_size=ecam_size,
        mem32_base=cpu_address,
        mem32_size=(window[5] << 32) | window[6],
        gic_spi_base=32,
        interrupt_map=tuple(entries),
        device_mask=mask[0],
        pin_mask=mask[3],
    )


# --------------------------------------------------------------------------- monitor


@dataclass(frozen=True)
class MonitorRead:
    label: str
    address: int
    words: int

    def command(self) -> str:
        return f"xp /{self.words}wx {self.address:#x}"


def function_reads(facts: MachineFacts, label: str, slot: int) -> tuple[MonitorRead, ...]:
    return (
        MonitorRead(f"{label} id", facts.config(slot, 0x00), 1),
        MonitorRead(f"{label} command", facts.config(slot, 0x04), 1),
        # All six BAR registers: a 64-bit BAR at index 4 is outside the first two.
        MonitorRead(f"{label} bars", facts.config(slot, 0x10), 6),
    )


def bind_reads(facts: MachineFacts) -> tuple[MonitorRead, ...]:
    return (
        *function_reads(facts, "declared", DECLARED_SLOT),
        MonitorRead("declared interrupt", facts.config(DECLARED_SLOT, 0x3C), 1),
        # The bind rule places BAR 0 at the window base; see `judge_bind`.
        MonitorRead("xhci capabilities", facts.mem32_base, 2),
        *function_reads(facts, "testdev", BIND_UNDECLARED_SLOT),
        *function_reads(facts, "nic", DEFAULT_NIC_SLOT),
    )


def identity_reads(facts: MachineFacts) -> tuple[MonitorRead, ...]:
    return (
        *function_reads(facts, "declared", DECLARED_SLOT),
        *function_reads(facts, "xhci", IDENTITY_XHCI_SLOT),
        *function_reads(facts, "nic", DEFAULT_NIC_SLOT),
    )


def monitor_input(reads: tuple[MonitorRead, ...]) -> str:
    """Ctrl-A c moves the mux to the monitor; `info version` is the last reply."""
    commands = ["info pci", *(read.command() for read in reads), "info version"]
    return "\x01c" + "".join(f"{command}\n" for command in commands)


def strip_echo(line: str) -> str:
    """The monitor's line editor redraws each keystroke; keep the prompt and the final text."""
    if not line.startswith("(qemu) "):
        return re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", line).strip()
    final = re.split(r"\x1b\[D", line[len("(qemu) ") :])[-1]
    return "(qemu) " + re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", final).strip()


@dataclass(frozen=True)
class MonitorView:
    words: dict[str, tuple[int, ...]]
    bars: dict[int, dict[int, tuple[int, int]]]


def parse_monitor(transcript: str, after: int, reads: tuple[MonitorRead, ...], version: str) -> MonitorView:
    """Read the monitor's replies that follow the guest's last required line.

    The guest shares the serial mux with the monitor. Each `xp` reply counts
    only on the line directly after the echo of its own command, and a command
    echoed twice is refused.
    """
    tail = transcript[after:]
    banner = MONITOR_BANNER.search(tail)
    if banner is None:
        refuse("the monitor never answered after the guest's last required line")
    lines = tail[banner.end() :].split("\n")
    echoes = [strip_echo(line) for line in lines]
    words: dict[str, tuple[int, ...]] = {}
    for read in reads:
        expected = f"(qemu) {read.command()}"
        indices = [index for index, echo in enumerate(echoes) if echo == expected]
        if len(indices) != 1:
            refuse(f"monitor command {read.command()!r} echoed {len(indices)} times")
        # `xp` prints at most four words per line; each further line starts at the next address.
        values: list[int] = []
        line = indices[0] + 1
        while len(values) < read.words:
            if line >= len(lines):
                refuse(f"no complete reply to {read.command()!r}")
            reply = XP_REPLY.match(lines[line].strip())
            if reply is None or int(reply.group("address"), 16) != read.address + 4 * len(values):
                refuse(f"malformed reply to {read.command()!r}: {lines[line]!r}")
            values.extend(int(word, 16) for word in reply.group("words").split())
            line += 1
        if len(values) != read.words:
            refuse(f"{read.label}: expected {read.words} words, read {len(values)}")
        words[read.label] = tuple(values)
    info_start = [index for index, echo in enumerate(echoes) if echo == "(qemu) info pci"]
    if len(info_start) != 1:
        refuse("the monitor did not answer `info pci` exactly once")
    bars: dict[int, dict[int, tuple[int, int]]] = {}
    slot: int | None = None
    for line in lines[info_start[0] + 1 :]:
        if strip_echo(line).startswith("(qemu)"):
            break
        function = INFO_PCI_FUNCTION.match(line)
        if function is not None:
            slot = int(function.group("slot")) if function.group("function") == "0" else None
            if slot is not None:
                bars.setdefault(slot, {})
            continue
        bar = INFO_PCI_BAR.match(line)
        if bar is not None and slot is not None:
            bars[slot][int(bar.group("index"))] = (int(bar.group("start"), 16), int(bar.group("end"), 16))
    if not re.search(rf"^{re.escape(version)}$", tail[banner.end() :], re.M):
        refuse("the monitor did not report the pinned QEMU version last")
    return MonitorView(words=words, bars=bars)


# --------------------------------------------------------------------------- judges


class Tally:
    """Counts the assertions a judge made, so the record states what it checked."""

    def __init__(self) -> None:
        self.cases = 0

    def require(self, condition: bool, message: str) -> None:
        if not condition:
            refuse(message)
        self.cases += 1


def single(pattern: re.Pattern[str], transcript: str, label: str, start: int = 0) -> re.Match[str]:
    found = list(pattern.finditer(transcript, start))
    if len(found) != 1:
        refuse(f"{label}: expected one line after position {start}, found {len(found)}")
    return found[0]


def reject_failures(transcript: str, markers: tuple[str, ...]) -> None:
    for marker in markers:
        found = re.search(marker, transcript)
        if found is not None:
            refuse(f"failure marker in serial transcript: {found.group(0)!r}")


def reclaim_fields(line: re.Match[str]) -> dict[str, int]:
    return {key: int(value) for key, value in re.findall(r"(\w+)=(\d+)", line.group("fields"))}


def require_untouched(tally: Tally, view: MonitorView, label: str, slot: int, identity: int) -> None:
    """A function the root must not configure: QEMU placed it, and it decodes nothing."""
    words = view.words
    tally.require(words[f"{label} id"] == (identity,), f"QEMU does not place {identity:08x} at 00:0{slot}.0")
    tally.require(words[f"{label} command"][0] & 0xFFFF == 0, f"the root changed the command register of 00:0{slot}.0")
    assigned = assigned_bars(words[f"{label} bars"])
    tally.require(not assigned, f"the root assigned BAR(s) {assigned} of 00:0{slot}.0")
    tally.require(not view.bars.get(slot), f"`info pci` maps a BAR of 00:0{slot}.0")


def assigned_bars(registers: tuple[int, ...]) -> list[int]:
    """Indices of the six BAR registers that hold an address.

    An I/O BAR keeps its two low bits, a memory BAR its four. A 64-bit memory
    BAR takes the next register as its upper half, which holds no flag bits.
    """
    assigned: list[int] = []
    index = 0
    while index < len(registers):
        value = registers[index]
        if value & 0x1:
            if value & ~0x3:
                assigned.append(index)
            index += 1
            continue
        wide = (value >> 1) & 0x3 == 0x2
        upper = registers[index + 1] if wide and index + 1 < len(registers) else 0
        if value & ~0xF or upper:
            assigned.append(index)
        index += 2 if wide else 1
    return assigned


def require_fields(tally: Tally, line: re.Match[str], expected: tuple[tuple[str, int], ...], label: str) -> None:
    fields = reclaim_fields(line)
    for key, value in expected:
        tally.require(fields.get(key) == value, f"{label}: {key}={fields.get(key)}, expected {value}")


def judge_manifest(manifest: dict) -> int:
    """The derived manifest grants one device, one MMIO region and one interrupt, no DMA, and nothing to the intruder."""
    tally = Tally()
    grants = manifest.get("grants", [])
    instances = manifest.get("instances", [])
    hardware = [grant for grant in grants if grant.get("capabilityKind") in HARDWARE_KINDS]
    kinds = sorted(grant["capabilityKind"] for grant in hardware)
    tally.require(
        kinds == ["device", "interruptSource", "mmioRegion"],
        f"the composition declares hardware grants {kinds}, expected one device, MMIO region and interrupt",
    )
    intruders = [instance for instance in instances if instance.get("executable") == INTRUDER]
    tally.require(len(intruders) == 1, f"the composition runs {len(intruders)} {INTRUDER} instances")
    intruder = intruders[0]["name"]
    tally.require(intruders[0].get("bindings") == [], f"{INTRUDER} binds {intruders[0].get('bindings')}")
    tally.require(
        all(intruder not in (grant.get("source"), grant.get("target")) for grant in hardware),
        f"a hardware grant names {intruder}",
    )
    return tally.cases


def judge_bind(transcript: str, facts: MachineFacts, version: str) -> int:
    tally = Tally()
    reject_failures(transcript, FAILURE_MARKERS)
    healthy = single(HEALTHY, transcript, "graph health")
    guest = transcript[: healthy.end()]
    bind = single(BIND, guest, "root binding")
    size, address = int(bind.group("size"), 16), int(bind.group("address"), 16)
    tally.require(bind.group("command") == f"{PCI_COMMAND_MEMORY:04x}", "the root enabled more than memory decoding")
    tally.require(size >= 0x10 and size & (size - 1) == 0, f"BAR size {size:#x} is not a power of two")
    tally.require(
        facts.mem32_base % size == 0 and address == facts.mem32_base,
        f"BAR 0 at {address:#x}, expected the window base {facts.mem32_base:#x}",
    )
    tally.require(address + size <= facts.mem32_base + facts.mem32_size, "BAR 0 leaves the 32-bit window")

    view = parse_monitor(transcript, healthy.end(), bind_reads(facts), version)
    words = view.words
    tally.require(words["declared id"] == (XHCI_WORD,), "QEMU does not place qemu-xhci at the declared slot")
    tally.require(
        words["declared command"][0] & 0xFFFF == PCI_COMMAND_MEMORY,
        f"command register {words['declared command'][0] & 0xFFFF:#06x}: expected memory decoding only",
    )
    tally.require(
        words["declared bars"] == (address | 0x4, 0, 0, 0, 0, 0),
        f"BAR registers {words['declared bars']} disagree with the root's address {address:#x}",
    )
    pin = (words["declared interrupt"][0] >> 8) & 0xFF
    tally.require(pin == 1, f"declared function reports interrupt pin {pin}, expected INTA")
    tally.require(
        int(bind.group("irq")) == facts.irq(DECLARED_SLOT, pin),
        f"root irq {bind.group('irq')} differs from the device tree route {facts.irq(DECLARED_SLOT, pin)}",
    )
    tally.require(
        view.bars.get(DECLARED_SLOT, {}).get(0) == (address, address + size - 1),
        f"`info pci` maps BAR 0 at {view.bars.get(DECLARED_SLOT, {}).get(0)}, not the root's range",
    )
    require_untouched(tally, view, "testdev", BIND_UNDECLARED_SLOT, TESTDEV_WORD)
    require_untouched(tally, view, "nic", DEFAULT_NIC_SLOT, DEFAULT_NIC_WORD)

    cap_words = words["xhci capabilities"]
    incarnations = list(BOUND.finditer(guest, bind.end()))
    capabilities = list(CAPABILITIES.finditer(guest, bind.end()))
    tally.require(len(incarnations) == 2 and len(capabilities) == 2, "the probe did not bind exactly twice")
    for epoch, (bound, caps) in enumerate(zip(incarnations, capabilities, strict=True), start=1):
        tally.require(bound.start() < caps.start(), f"epoch {epoch}: capabilities read before binding")
        tally.require(
            int(bound.group("epoch")) == epoch and int(caps.group("epoch")) == epoch,
            f"incarnation {epoch} reports another epoch",
        )
        tally.require(int(bound.group("bytes")) == size, f"epoch {epoch}: MMIO bytes {bound.group('bytes')} differ from BAR size {size}")
        tally.require(int(bound.group("irqs")) == 1, f"epoch {epoch}: probe holds {bound.group('irqs')} interrupt sources")
        tally.require(
            (int(caps.group("cap"), 16), int(caps.group("hcs"), 16)) == cap_words,
            f"epoch {epoch}: probe read {caps.group('cap')}/{caps.group('hcs')}, the device holds "
            f"{cap_words[0]:08x}/{cap_words[1]:08x}",
        )
    first_caps, second_bound = capabilities[0], incarnations[1]

    # The first incarnation: a refused access past BAR 0, then a fault with live authority.
    refused = single(ROOT_REFUSED, guest, "root refusal past BAR 0", first_caps.end())
    tally.require(int(refused.group("offset")) == size, f"the root refused offset {refused.group('offset')}, not the first byte past BAR 0")
    tally.require(int(refused.group("length")) > 0, "the root refused an empty access")
    beyond = single(BEYOND, guest, "probe report of the refusal", refused.end())
    tally.require(int(beyond.group("offset")) == size, "the probe reports another refused offset")
    faulting = single(FAULTING, guest, "fault with live authority", beyond.end())
    tally.require(faulting.start() < second_bound.start(), "the fault is not in the first incarnation")
    # Exactly one component faults in the whole run: the probe, after it says so.
    faults = list(FAULT.finditer(guest))
    tally.require(len(faults) == 1, f"the root printed {len(faults)} fault records, expected the probe's alone")
    fault = faults[0]
    tally.require(fault.start() >= faulting.end(), "the fault record precedes the probe's fault")
    task = fault.group("task")
    tally.require(refused.group("task") == task, f"the refusal names task {refused.group('task')}, the fault task {task}")

    reclaims = list(RECLAIM.finditer(guest))
    tally.require(len(reclaims) == 2, f"the root printed {len(reclaims)} reclaim records, expected 2")
    faulted, exited = reclaims
    tally.require(fault.end() <= faulted.start() < second_bound.start(), "the first reclaim is not between the fault and the replacement")
    tally.require(faulted.group("task") == task, f"the first reclaim names task {faulted.group('task')}, the fault task {task}")
    require_fields(
        tally,
        faulted,
        (
            # The slice grants no DMA: the probe held MMIO and one interrupt only.
            ("pre_mmio_bytes", size),
            ("pre_irq_sources", 1),
            ("pre_dma_pages", 0),
            ("pre_dma_mappings", 0),
            ("reclaimed_mmio_bytes", size),
            ("reclaimed_mmio_mappings", 1),
            ("reclaimed_irq_sources", 1),
            ("post_mmio_bytes", 0),
            ("post_mmio_mappings", 0),
            ("post_irq_sources", 0),
            ("post_dma_pages", 0),
            ("post_dma_mappings", 0),
            ("fresh_epoch", 2),
        ),
        "fault reclaim",
    )
    complete = single(COMPLETE, guest, "probe completion", capabilities[1].end())
    tally.require(complete.end() <= exited.start(), "the second reclaim precedes the replacement's completion")
    tally.require(exited.group("task") != task, "the second reclaim names the faulted task again")
    require_fields(
        tally,
        exited,
        (
            ("pre_mmio_bytes", size),
            ("pre_irq_sources", 1),
            ("pre_dma_pages", 0),
            ("post_mmio_bytes", 0),
            ("post_mmio_mappings", 0),
            ("post_irq_sources", 0),
            ("post_dma_pages", 0),
            ("post_dma_mappings", 0),
        ),
        "exit reclaim",
    )
    single(DENIED, guest, "ungranted denial")
    return tally.cases


def judge_identity(transcript: str, facts: MachineFacts, version: str) -> int:
    tally = Tally()
    reject_failures(transcript, IDENTITY_FAILURE_MARKERS)
    fatal = single(IDENTITY_FATAL, transcript, "identity refusal")
    fatals = ANY_FATAL.findall(transcript)
    tally.require(len(fatals) == 1, f"the root printed {len(fatals)} fatal records")
    guest = transcript[: fatal.end()]
    for pattern, label in (
        (r"SLIME_PCI bind", "a binding"),
        (rf"\[{PROBE}\]", "a probe line"),
        (rf"\[{INTRUDER}\]", "an intruder line"),
        (r"SLIME_GRAPH ", "a graph record"),
    ):
        tally.require(re.search(pattern, guest) is None, f"the refused generation printed {label}")
    view = parse_monitor(transcript, fatal.end(), identity_reads(facts), version)
    found = view.words["declared id"][0]
    tally.require(
        fatal.group("found") == f"{found & 0xFFFF:04x}:{found >> 16:04x}",
        f"the refusal names {fatal.group('found')}, the function holds {found:08x}",
    )
    require_untouched(tally, view, "declared", DECLARED_SLOT, TESTDEV_WORD)
    require_untouched(tally, view, "xhci", IDENTITY_XHCI_SLOT, XHCI_WORD)
    require_untouched(tally, view, "nic", DEFAULT_NIC_SLOT, DEFAULT_NIC_WORD)
    return tally.cases


# --------------------------------------------------------------------------- controls


def edit(transcript: str, pattern: re.Pattern[str], change: Callable[[re.Match[str]], str], index: int = 0) -> str:
    found = list(pattern.finditer(transcript))
    if len(found) <= index:
        fail(f"control needs line {index} matching {pattern.pattern!r}")
    match = found[index]
    return transcript[: match.start()] + change(match) + transcript[match.end() :]


def replace_group(match: re.Match[str], group: str, value: str) -> str:
    line = match.group(0)
    start, end = match.start(group) - match.start(), match.end(group) - match.start()
    return line[:start] + value + line[end:]


def reply_for(read: MonitorRead, line: int = 0) -> re.Pattern[str]:
    """One line of an `xp` reply; line `n` starts four words after line `n - 1`."""
    return re.compile(rf"^{read.address + 16 * line:x}: (?P<words>(?:0x[0-9a-f]{{8}} ?)+)$", re.M)


def insert_before(transcript: str, pattern: re.Pattern[str], line: str) -> str:
    match = pattern.search(transcript)
    if match is None:
        fail(f"control needs a line matching {pattern.pattern!r}")
    return transcript[: match.start()] + line + "\n" + transcript[match.start() :]


def spoof_reply(transcript: str, read: MonitorRead) -> str:
    """Guest output after the switch prints its own echo and reply for one read."""
    banner = MONITOR_BANNER.search(transcript)
    if banner is None:
        fail("control needs the monitor banner")
    spoofed = f"\n(qemu) {read.command()}\n{read.address:x}: 0x00000000"
    return transcript[: banner.end()] + spoofed + transcript[banner.end() :]


def rotated_interrupts(facts: MachineFacts) -> MachineFacts:
    rotated = tuple((device, pin, (spi - 3 + 1) % 4 + 3) for device, pin, spi in facts.interrupt_map)
    return replace(facts, interrupt_map=rotated)


def run_controls(mutations: tuple[tuple[str, Callable[[], object]], ...]) -> int:
    refused = 0
    for label, judge in mutations:
        try:
            judge()
        except Refusal:
            refused += 1
            continue
        fail(f"control accepted: {label}")
    return refused


def set_word(transcript: str, read: MonitorRead, index: int, value: int) -> str:
    """Replace one word of one `xp` reply, keeping the reply's length."""

    position = index % 4

    def change(match: re.Match[str]) -> str:
        values = match.group("words").split()
        if position >= len(values):
            fail(f"control needs word {index} of {read.command()!r}")
        values[position] = f"0x{value:08x}"
        return replace_group(match, "words", " ".join(values))

    return edit(transcript, reply_for(read, index // 4), change)


def shift_info_pci_bar(transcript: str, step: int) -> str:
    """Move the first mapped BAR range that `info pci` prints."""
    pattern = re.compile(r"^(?P<lead>\s+BAR\d: .+? at 0x)(?P<start>[0-9a-f]+) \[0x(?P<end>[0-9a-f]+)\]", re.M)
    return edit(
        transcript,
        pattern,
        lambda m: f"{m.group('lead')}{int(m.group('start'), 16) + step:x} [0x{int(m.group('end'), 16) + step:x}]",
    )


def map_info_pci_bar(transcript: str, slot: int, start: int) -> str:
    """Add a mapped BAR 0 line under one function of `info pci`."""
    header = re.compile(rf"^\s+Bus\s+0, device\s+{slot}, function 0:$", re.M)
    return edit(transcript, header, lambda m: f"{m.group(0)}\n      BAR0: 32 bit memory at {start:#x} [{start + 0xFFF:#x}].")


def bind_controls(transcript: str, facts: MachineFacts, version: str) -> int:
    reads = {read.label: read for read in bind_reads(facts)}

    def words(label: str, value: str) -> str:
        return edit(transcript, reply_for(reads[label]), lambda m: replace_group(m, "words", value))

    def word(label: str, index: int, value: int) -> str:
        return set_word(transcript, reads[label], index, value)

    def field(pattern: re.Pattern[str], index: int, old: str, new: str) -> str:
        changed = edit(transcript, pattern, lambda m: m.group(0).replace(old, new), index)
        if changed == transcript:
            fail(f"control field {old!r} is absent")
        return changed

    def plus(value: str, step: int) -> str:
        return str(int(value) + step)

    texts: tuple[tuple[str, str], ...] = (
        ("binding moved off the window base", edit(transcript, BIND, lambda m: replace_group(m, "address", f"{int(m.group('address'), 16) + 0x1000:x}"))),
        ("binding enabled bus mastering", edit(transcript, BIND, lambda m: replace_group(m, "command", "0006"))),
        ("binding names another interrupt", edit(transcript, BIND, lambda m: replace_group(m, "irq", plus(m.group("irq"), 1)))),
        ("device has bus mastering on", words("declared command", "0x00100006")),
        ("device BAR differs from the binding", word("declared bars", 0, facts.mem32_base + 0x1000 | 0x4)),
        ("info pci maps BAR 0 elsewhere", shift_info_pci_bar(transcript, 0x1000)),
        ("undeclared testdev decodes memory", words("testdev command", "0x00000002")),
        ("undeclared testdev holds a BAR", word("testdev bars", 0, facts.mem32_base + 0x10000)),
        ("info pci maps a testdev BAR", map_info_pci_bar(transcript, BIND_UNDECLARED_SLOT, facts.mem32_base + 0x10000)),
        ("default NIC decodes memory", words("nic command", "0x00000002")),
        ("default NIC BAR 4 assigned with decoding off", word("nic bars", 4, facts.mem32_base + 0x40000 | 0xC)),
        ("probe read a constant", edit(transcript, CAPABILITIES, lambda m: replace_group(m, "hcs", f"{int(m.group('hcs'), 16) ^ 0x100:08x}"))),
        ("probe MMIO bound is not the BAR size", edit(transcript, BOUND, lambda m: replace_group(m, "bytes", plus(m.group("bytes"), int(m.group("bytes")))))),
        ("root refusal missing", edit(transcript, ROOT_REFUSED, lambda _m: "")),
        ("root refusal names another task", edit(transcript, ROOT_REFUSED, lambda m: replace_group(m, "task", plus(m.group("task"), 1)))),
        ("root refusal names another offset", edit(transcript, ROOT_REFUSED, lambda m: replace_group(m, "offset", plus(m.group("offset"), -4)))),
        ("fault reclaim names another task", edit(transcript, RECLAIM, lambda m: replace_group(m, "task", plus(m.group("task"), 1)))),
        ("fault reclaim left MMIO mapped", field(RECLAIM, 0, "post_mmio_bytes=0", "post_mmio_bytes=4096")),
        ("probe held a DMA account", field(RECLAIM, 0, "pre_dma_pages=0", "pre_dma_pages=1")),
        ("exit reclaim left the interrupt held", field(RECLAIM, 1, "post_irq_sources=0", "post_irq_sources=1")),
        ("exit reclaim missing", edit(transcript, RECLAIM, lambda _m: "", 1)),
        ("replacement reused the old epoch", edit(transcript, BOUND, lambda m: replace_group(m, "epoch", "1"), 1)),
        ("intruder not denied", edit(transcript, DENIED, lambda _m: "")),
        ("intruder faulted before the probe", insert_before(transcript, FAULTING, "SLIME_GRAPH component fault task=12 kind=VmFault address=Some(0)")),
        ("explicit probe failure", insert_before(transcript, HEALTHY, f"[{PROBE}] fail: injected control")),
        ("guest output repeats a monitor command", spoof_reply(transcript, reads["testdev command"])),
    )
    for label, mutated in texts:
        if mutated == transcript:
            fail(f"control {label!r} did not change the transcript")
    mutations: list[tuple[str, Callable[[], object]]] = [
        (label, lambda mutated=mutated: judge_bind(mutated, facts, version)) for label, mutated in texts
    ]
    mutations.append(("judge model with another INTx route", lambda: judge_bind(transcript, rotated_interrupts(facts), version)))
    return run_controls(tuple(mutations))


def identity_controls(transcript: str, facts: MachineFacts, version: str) -> int:
    reads = {read.label: read for read in identity_reads(facts)}

    def words(label: str, value: str) -> str:
        return edit(transcript, reply_for(reads[label]), lambda m: replace_group(m, "words", value))

    def word(label: str, index: int, value: int) -> str:
        return set_word(transcript, reads[label], index, value)

    texts: tuple[tuple[str, str], ...] = (
        ("refused function still bound", insert_before(transcript, IDENTITY_FATAL, "SLIME_PCI bind function=00:03.0")),
        ("probe launched", insert_before(transcript, IDENTITY_FATAL, f"[{PROBE}] bound bytes=16384 irqs=1 epoch=1")),
        ("refusal names the declared identity as found", edit(transcript, IDENTITY_FATAL, lambda m: replace_group(m, "found", DECLARED_ID))),
        ("declared function decodes memory", words("declared command", "0x00000002")),
        ("misplaced xHCI holds a BAR", word("xhci bars", 0, facts.mem32_base | 0x4)),
        ("info pci maps a misplaced xHCI BAR", map_info_pci_bar(transcript, IDENTITY_XHCI_SLOT, facts.mem32_base)),
        ("default NIC holds a BAR", word("nic bars", 1, facts.mem32_base + 0x20000)),
        ("second fatal record", insert_before(transcript, IDENTITY_FATAL, "SLIME_ROOT FATAL earlier")),
    )
    return run_controls(tuple((label, lambda mutated=mutated: judge_identity(mutated, facts, version)) for label, mutated in texts))


def manifest_controls(manifest: dict) -> int:
    def mutated(change: Callable[[dict], None]) -> dict:
        copy = json.loads(json.dumps(manifest))
        change(copy)
        return copy

    def intruder(copy: dict) -> dict:
        return next(instance for instance in copy["instances"] if instance.get("executable") == INTRUDER)

    def first_hardware(copy: dict) -> dict:
        return next(grant for grant in copy["grants"] if grant.get("capabilityKind") in HARDWARE_KINDS)

    texts: tuple[tuple[str, dict], ...] = (
        ("intruder binds a hardware grant", mutated(lambda c: intruder(c)["bindings"].append({"grant": first_hardware(c)["name"]}))),
        ("composition declares a DMA account", mutated(lambda c: c["grants"].append({**first_hardware(c), "name": "pci-dma", "capabilityKind": "dmaAccount"}))),
        ("a hardware grant targets the intruder", mutated(lambda c: first_hardware(c).update(target=intruder(c)["name"]))),
    )
    return run_controls(tuple((label, lambda value=value: judge_manifest(value)) for label, value in texts))


# --------------------------------------------------------------------------- driver


def read_manifest() -> dict:
    if not MANIFEST.is_file():
        fail(f"{MANIFEST.relative_to(ROOT)} does not exist; the {PCI_CLOSURE} composition has not landed")
    environment = dict(os.environ)
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    decoded = subprocess.run(
        [str(binary()), "json", str(MANIFEST)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if decoded.returncode:
        fail(f"cannot decode {MANIFEST.relative_to(ROOT)}: {decoded.stderr.strip()}")
    try:
        return json.loads(decoded.stdout)
    except json.JSONDecodeError as error:
        fail(f"cannot parse decoded {MANIFEST.relative_to(ROOT)}: {error}")


def build_image() -> Path:
    try:
        built = build_closure_image(PCI_CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    if sha256_file(built.image, fail) != built.digest():
        fail(f"{built.image} changed after it was built")
    return built.image


def boot(
    image: Path,
    devices: tuple[str, ...],
    trigger: re.Pattern[str],
    reads: tuple[MonitorRead, ...],
    version: str,
    markers: tuple[str, ...],
) -> str:
    terminal = re.compile(rf"^{re.escape(version)}$|" + "|".join(markers))
    return run_plane(
        image=image,
        timeout=TIMEOUT,
        terminal_condition=terminal,
        fail=fail,
        pins_path=PINS,
        additional_arguments=devices,
        input_steps=(((trigger,), monitor_input(reads)),),
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Boot and check the seL4 PCI function binding plane")
    parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    profile = load_qemu_profile(fail, PINS)
    version = str(profile["qemu_version"])
    facts = read_machine_facts(profile)
    manifest = read_manifest()
    image = build_image()

    bind = boot(image, BIND_DEVICES, HEALTHY, bind_reads(facts), version, FAILURE_MARKERS)
    identity = boot(image, IDENTITY_DEVICES, ANY_FATAL, identity_reads(facts), version, IDENTITY_FAILURE_MARKERS)
    try:
        cases = judge_manifest(manifest)
        cases += judge_bind(bind, facts, version)
        cases += judge_identity(identity, facts, version)
    except Refusal as error:
        fail(str(error))
    refused = manifest_controls(manifest) + bind_controls(bind, facts, version) + identity_controls(identity, facts, version)
    devloop_observations.record(casesObserved=cases, negativeControlsRefused=refused)
    print(
        f"seL4 PCI binding plane check: one declared function bound at the window base with memory "
        f"decoding only, a wrong identity refused before any write; cases={cases} controls-refused={refused}"
    )


if __name__ == "__main__":
    main()
