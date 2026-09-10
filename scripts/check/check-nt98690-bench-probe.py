#!/usr/bin/env python3

"""Host regressions for the NT98690 GPIO, PWM, and UART bench probe control flow."""

from __future__ import annotations

import importlib.util
import io
import os
import sys
import tempfile
import time
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from types import ModuleType
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PROBE_PATH = ROOT / "scripts" / "check" / "check-nt98690-boot.py"
PROMPT = "nvt: "

sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from mavlink import (  # noqa: E402
    PINNED_HEARTBEATS,
    FrameDecoder,
    encode_heartbeat,
    x25_crc,
)


class BoardOpened(Exception):
    """Raised in place of opening a serial port, so no case can mistake it for a rejection."""


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 bench probe regression: {message}")


def load_probe() -> ModuleType:
    spec = importlib.util.spec_from_file_location("nt98690_bench_probe", PROBE_PATH)
    if spec is None or spec.loader is None:
        fail(f"cannot load {PROBE_PATH.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PROBE = load_probe()


class ModelConsole:
    """Minimal U-Boot MMIO model used by the production probe functions."""

    def __init__(
        self,
        *,
        drop_writes: set[tuple[int, int]] | None = None,
        timeout_after_write: set[int] | None = None,
        interrupt_on_sleep: bool = False,
        busy_reads_after_interrupt: int = 0,
        banner: bool = True,
    ) -> None:
        self.registers = initial_registers()
        self.uart = Uart16550Model()
        self.drop_writes = drop_writes or set()
        self.timeout_after_write = timeout_after_write or set()
        self.interrupt_on_sleep = interrupt_on_sleep
        self.busy_reads_after_interrupt = busy_reads_after_interrupt
        self.banner = banner
        self.write_counts: dict[int, int] = {}
        self.commands: list[str] = []
        self.attempted_commands: list[str] = []
        self.received_bytes = bytearray()
        self.endpoint = "model"
        self.framing_errors = 0
        self._pending = ""
        self._interrupted = False

    def write(self, data: bytes, timeout: float = 10.0) -> None:
        del timeout
        command = data.decode("ascii").strip()
        if not command:
            if not (self._interrupted and self.busy_reads_after_interrupt > 0):
                self._pending = PROMPT
            return
        self.attempted_commands.append(command)
        if self._interrupted and self.busy_reads_after_interrupt > 0:
            self.busy_reads_after_interrupt -= 1
            if self.busy_reads_after_interrupt > 0:
                raise SystemExit(
                    f"model command timed out while remote command remained busy: {command}"
                )
        self.commands.append(command)
        if command == "reset":
            self._pending = "U-Boot 2021.10\n" if self.banner else ""
            return
        output = self._execute(command)
        self._pending = f"{command}\r\n{PROMPT}" if output is None else output

    def read_for(self, seconds: float) -> str:
        del seconds
        if self._interrupted and self.busy_reads_after_interrupt > 0:
            self.busy_reads_after_interrupt -= 1
            return ""
        pending, self._pending = self._pending, ""
        if pending:
            self.received_bytes.extend(pending.encode())
        return pending

    def flush_input(self) -> None:
        self.read_for(0)

    def close(self) -> None:
        pass

    def _execute(self, command: str) -> str | None:
        parts = command.split()
        if parts[0] == "sleep":
            if self.interrupt_on_sleep:
                self.interrupt_on_sleep = False
                self._interrupted = True
                raise KeyboardInterrupt
            return None
        if parts[0] == "md.l":
            address = int(parts[1], 0)
            if self._is_uart(address):
                value = self.uart.read(self._uart_index(address), self._uart_clock_running())
            else:
                value = self.registers.get(address, 0)
            return f"{address:011x}: {value:08x}\r\n{PROMPT}"
        if parts[0] != "mw.l":
            raise AssertionError(f"unexpected command: {command}")
        address = int(parts[1], 0)
        value = int(parts[2], 0) & 0xFFFF_FFFF
        count = self.write_counts.get(address, 0) + 1
        self.write_counts[address] = count
        if (address, count) not in self.drop_writes:
            if self._is_uart(address):
                self.uart.write(
                    self._uart_index(address), value, self._uart_clock_running()
                )
            elif address == PROBE.PWM_BASE + PROBE.PWM_ENABLE:
                self.registers[address] |= value
            elif address == PROBE.PWM_BASE + PROBE.PWM_DISABLE:
                self.registers[PROBE.PWM_BASE + PROBE.PWM_ENABLE] &= ~value
            elif address == PROBE.PWM_BASE + PROBE.PWM_LOAD:
                pass
            elif address == PROBE.GPIO_BASE + PROBE.GPIO_P_SET:
                self.registers[PROBE.GPIO_BASE + PROBE.GPIO_P_DATA] |= value
            elif address == PROBE.GPIO_BASE + PROBE.GPIO_P_CLR:
                self.registers[PROBE.GPIO_BASE + PROBE.GPIO_P_DATA] &= ~value
            else:
                self.registers[address] = value
        if address in self.timeout_after_write:
            self.timeout_after_write.remove(address)
            raise SystemExit(f"model lost acknowledgement for {address:#x}")
        return None

    @staticmethod
    def _is_uart(address: int) -> bool:
        return PROBE.UART8_BASE <= address < PROBE.UART8_BASE + 0x1000

    @staticmethod
    def _uart_index(address: int) -> int:
        return (address - PROBE.UART8_BASE) >> PROBE.UART8_REG_SHIFT

    def _uart_clock_running(self) -> bool:
        gate = self.registers.get(PROBE.CG_BASE + PROBE.CG_UART8_CLK_EN, 0)
        reset = self.registers.get(PROBE.CG_BASE + PROBE.CG_UART8_RESET, 0)
        return bool(gate & (1 << PROBE.CG_UART8_CLK_EN_BIT)) and bool(
            reset & (1 << PROBE.CG_UART8_RESET_BIT)
        )


#: RADIO_STATUS's identity and checksum seed, from `common.xml`. Only the
#: injector below needs them; the decoder under test never verifies this
#: message, which is the point of the scenario that feeds it one.
RADIO_STATUS_MSGID = 109
RADIO_STATUS_CRC_EXTRA = 185


class Uart16550Model:
    """A 16550 far enough to hold the probe honest about its own sequence.

    Three behaviours are what the scenarios turn on: FCR does not read back and
    IIR reports the FIFO state instead, a read of index 0 pops the receive FIFO
    rather than returning what was transmitted, and the block answers nothing at
    all while its clock is gated or its reset is asserted. Each of those is a way
    a plausible-looking probe would be wrong on hardware.
    """

    def __init__(self) -> None:
        self.dll = 0
        self.dlm = 0
        self.lcr = 0
        self.ier = 0
        self.mcr = 0
        self.fifo_enabled = False
        self.configured = False
        self.tx = bytearray()
        self.temt_stuck = False

    @property
    def dlab(self) -> bool:
        return bool(self.lcr & 0x80)

    def _require_clock(self, running: bool) -> None:
        if not running:
            raise AssertionError(
                "the probe touched UART8 while its clock was gated or its reset held; "
                "this block does not answer there"
            )

    def read(self, index: int, clock_running: bool) -> int:
        self._require_clock(clock_running)
        if index == PROBE.UART_THR and not self.dlab:
            raise AssertionError(
                "the probe read the receive buffer, which pops the FIFO; only the "
                "divisor latch may be read at index 0"
            )
        if index == PROBE.UART_THR:
            return self.dll
        if index == PROBE.UART_IER:
            return self.dlm if self.dlab else self.ier
        if index == PROBE.UART_IIR:
            return PROBE.UART_IIR_FIFO_ENABLED if self.fifo_enabled else 0x01
        if index == PROBE.UART_LCR:
            return self.lcr
        if index == PROBE.UART_MCR:
            return self.mcr
        if index == PROBE.UART_LSR:
            # A transmitter that is draining normally, plus the receive-side
            # bits an unrouted RX pad floats: a probe comparing the whole
            # register rather than the two transmitter bits fails here.
            floating = 0x11
            if self.temt_stuck and self.configured:
                return floating | PROBE.UART_LSR_THRE
            return floating | PROBE.UART_LSR_TX_MASK
        raise AssertionError(f"unexpected UART8 register index {index}")

    def write(self, index: int, value: int, clock_running: bool) -> None:
        self._require_clock(clock_running)
        if index == PROBE.UART_THR:
            if self.dlab:
                self.dll = value & 0xFF
            else:
                self.tx.append(value & 0xFF)
            return
        if index == PROBE.UART_IER:
            if self.dlab:
                self.dlm = value & 0xFF
            else:
                self.ier = value & 0xFF
            return
        if index == PROBE.UART_IIR:
            # FCR: write-only, so nothing here reads back.
            self.fifo_enabled = bool(value & 0x01)
            return
        if index == PROBE.UART_LCR:
            was_dlab = self.dlab
            self.lcr = value & 0xFF
            if was_dlab and not self.dlab:
                self.configured = True
            return
        if index == PROBE.UART_MCR:
            self.mcr = value & 0x1F
            return
        raise AssertionError(f"unexpected UART8 register index {index}")


class ModelReceiver:
    """The ground radio: whatever the model transmitted, optionally spoiled.

    Frames are taken whole because that is the granularity the mutations need,
    and a radio really does deliver them that way -- it packetises on MAVLink
    frame boundaries, which is also why `radio_status_before_each` is a faithful
    thing to inject rather than an invented hazard.
    """

    FRAME_LEN = 21

    def __init__(
        self,
        uart: Uart16550Model,
        *,
        radio_status_before_each: bool = False,
        corrupt_seq: int | None = None,
        corrupt_len_seq: int | None = None,
        drop_seq: int | None = None,
    ) -> None:
        self.uart = uart
        self.radio_status_before_each = radio_status_before_each
        self.corrupt_seq = corrupt_seq
        self.corrupt_len_seq = corrupt_len_seq
        self.drop_seq = drop_seq
        self.received_bytes = bytearray()
        self.framing_errors = 0
        self.endpoint = "model-radio"

    def describe(self) -> str:
        return f"{self.endpoint} at 8N1"

    def read_bytes_for(self, seconds: float) -> bytes:
        del seconds
        out = bytearray()
        while len(self.uart.tx) >= self.FRAME_LEN:
            frame = bytearray(self.uart.tx[: self.FRAME_LEN])
            del self.uart.tx[: self.FRAME_LEN]
            seq = frame[4]
            if self.radio_status_before_each:
                out += b"\x01\x02" + radio_status_frame(200, 190)
            if seq == self.drop_seq:
                continue
            if seq == self.corrupt_seq:
                frame[14] ^= 0xFF
            if seq == self.corrupt_len_seq:
                # Longer than a heartbeat but short enough to complete once the
                # next frame arrives, so the claimed end lands past the start
                # byte that follows: exactly the case a decoder must survive.
                frame[1] = 12
            out += frame
        self.received_bytes.extend(out)
        return bytes(out)

    def close(self) -> None:
        pass


def radio_status_frame(rssi: int, remrssi: int) -> bytes:
    """A RADIO_STATUS the radio injects into the ground-side stream.

    Built here rather than imported: `scripts/lib/mavlink.py` decodes this
    message but has no reason to produce one, and a test that used the
    library's own encoder for both sides would prove less.
    """
    payload = bytes([0, 0, 0, 0, rssi, remrssi, 0, 0, 0])
    header = bytes([len(payload), 0, 0, 9, 51, 68, RADIO_STATUS_MSGID, 0, 0])
    crc = x25_crc(bytes([RADIO_STATUS_CRC_EXTRA]), x25_crc(header + payload))
    return bytes([0xFD]) + header + payload + bytes([crc & 0xFF, crc >> 8])


def initial_registers() -> dict[int, int]:
    return {
        PROBE.TOP_BASE + PROBE.TOP_PWM_MUX: 0,
        PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC: 0xFFFF_FFFF,
        PROBE.TOP_BASE + PROBE.TOP_PWM12_MUX: 0x0001_0000,
        PROBE.CG_BASE + PROBE.CG_PWM_CLK_DIV0: 0x01DF_01DF,
        PROBE.CG_BASE + PROBE.CG_PWM_CLK_EN: 0x0600_0000,
        PROBE.CG_BASE + PROBE.CG_PWM12_CLK_EN: 1 << 22,
        PROBE.CG_BASE + PROBE.CG_PWM_RESET: 1,
        PROBE.PWM_BASE + PROBE.PWM_ENABLE: 1 << 12,
        PROBE.GPIO_BASE + PROBE.GPIO_P_DATA: 0,
        PROBE.GPIO_BASE + PROBE.GPIO_P_DIR: 0,
        PROBE.PAD_BASE + PROBE.PAD_P1_STATUS: 0x24,
        # UART8 at the prompt: clock gated, reset held, no pad routed, and a
        # divider field left at something other than the 48 MHz encoding -- the
        # probe must write its own and put this back.
        PROBE.CG_BASE + PROBE.CG_UART8_CLK_DIV: 0x0013_0000,
        PROBE.CG_BASE + PROBE.CG_UART8_CLK_EN: 0x0000_0000,
        PROBE.CG_BASE + PROBE.CG_UART8_RESET: 0x0000_0010,
        PROBE.TOP_BASE + PROBE.TOP_UART8_MUX: 0x0000_0000,
        PROBE.TOP_BASE + PROBE.TOP_UART8_RTSCTS_MUX: 0x0000_0000,
        PROBE.PAD_BASE + PROBE.PAD_PGPIO4_PULL: 0x0000_0100,
    }


def run_pwm(
    console: ModelConsole, pulses: tuple[int, ...] = (1000, 1500, 2000)
) -> BaseException | None:
    original_sleep = PROBE.time.sleep

    def model_sleep(seconds: float) -> None:
        if console.interrupt_on_sleep:
            console.interrupt_on_sleep = False
            console._interrupted = True
            console._pending = ""
            raise KeyboardInterrupt

    PROBE.time.sleep = model_sleep
    try:
        try:
            PROBE.pwm_probe(console, PROMPT, 0, 20000, pulses, 0, 0, 30)
        except BaseException as error:
            return error
        return None
    finally:
        PROBE.time.sleep = original_sleep



def run_uart(
    console: ModelConsole,
    receiver: ModelReceiver | None,
    frames: int = 10,
    divisor: int = 52,
) -> BaseException | None:
    original_sleep = PROBE.time.sleep

    def model_sleep(seconds: float) -> None:
        del seconds
        if console.interrupt_on_sleep:
            console.interrupt_on_sleep = False
            console._interrupted = True
            console._pending = ""
            raise KeyboardInterrupt

    PROBE.time.sleep = model_sleep
    try:
        try:
            PROBE.uart_probe(console, PROMPT, frames, 0, receiver, 0.1, divisor, 30)
        except BaseException as error:
            return error
        return None
    finally:
        PROBE.time.sleep = original_sleep


UART_SHARED_WORDS = (
    ("TOP pad function", PROBE.TOP_BASE, "TOP_PGPIO_FUNC"),
    ("TOP UART8 mux", PROBE.TOP_BASE, "TOP_UART8_MUX"),
    ("UART8 clock gate", PROBE.CG_BASE, "CG_UART8_CLK_EN"),
    ("UART8 clock divider", PROBE.CG_BASE, "CG_UART8_CLK_DIV"),
    ("UART8 reset", PROBE.CG_BASE, "CG_UART8_RESET"),
)


def assert_uart_restored(console: ModelConsole) -> None:
    expected = initial_registers()
    for name, base, attribute in UART_SHARED_WORDS:
        address = base + getattr(PROBE, attribute)
        if console.registers[address] != expected[address]:
            fail(
                f"{name} ({address:#x}) was not restored: "
                f"{console.registers[address]:#010x} != {expected[address]:#010x}"
            )


def uart_writes(console: ModelConsole, address: int) -> list[int]:
    prefix = f"mw.l {address:#x} "
    return [
        int(command.split()[2], 0) for command in console.commands if command.startswith(prefix)
    ]


def touches_uart8(command: str) -> bool:
    parts = command.split()
    if len(parts) < 2 or parts[0] not in ("md.l", "mw.l"):
        return False
    return PROBE.UART8_BASE <= int(parts[1], 0) < PROBE.UART8_BASE + 0x1000


def command_index(console: ModelConsole, predicate) -> int | None:
    for index, command in enumerate(console.commands):
        if predicate(command):
            return index
    return None


def assert_restored(console: ModelConsole) -> None:
    expected = initial_registers()
    for address in (
        PROBE.TOP_BASE + PROBE.TOP_PWM_MUX,
        PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC,
        PROBE.CG_BASE + PROBE.CG_PWM_CLK_DIV0,
        PROBE.CG_BASE + PROBE.CG_PWM_CLK_EN,
    ):
        if console.registers[address] != expected[address]:
            fail(f"register {address:#x} was not restored")
    if console.registers[PROBE.PWM_BASE + PROBE.PWM_ENABLE] & 1:
        fail("PWM0 remained enabled")


def assert_core_rail_untouched(console: ModelConsole) -> None:
    forbidden = {
        PROBE.CG_BASE + PROBE.CG_PWM_RESET,
        PROBE.CG_BASE + PROBE.CG_PWM12_CLK_EN,
        PROBE.CG_BASE + 0x324,
        PROBE.TOP_BASE + PROBE.TOP_PWM12_MUX,
        PROBE.PWM_BASE + PROBE.pwm_control_offset(12),
        PROBE.PWM_BASE + PROBE.pwm_period_offset(12),
    }
    writes = {
        int(command.split()[1], 0) for command in console.commands if command.startswith("mw.l ")
    }
    touched = forbidden & writes
    if touched:
        fail(f"probe wrote core-rail register(s): {sorted(hex(address) for address in touched)}")
    for command in console.commands:
        if command.startswith(f"mw.l {PROBE.PWM_BASE + PROBE.PWM_DISABLE:#x} "):
            if int(command.split()[2], 0) & (1 << 12):
                fail("probe disabled the core-rail PWM channel")


def expect_failure(error: BaseException | None, phrase: str) -> None:
    if error is None:
        fail(f"scenario unexpectedly passed; expected {phrase!r}")
    if phrase not in PROBE.exception_text(error):
        fail(f"failure did not mention {phrase!r}: {PROBE.exception_text(error)}")


def normal_pwm() -> None:
    console = ModelConsole()
    error = run_pwm(console)
    if error is not None:
        fail(f"normal PWM run failed: {PROBE.exception_text(error)}")
    assert_restored(console)
    assert_core_rail_untouched(console)
    if "reset" not in console.commands:
        fail("normal PWM run did not reset after verified cleanup")


def cancellation_cleanup() -> None:
    console = ModelConsole(interrupt_on_sleep=True)
    error = run_pwm(console)
    if not isinstance(error, KeyboardInterrupt):
        fail(f"cancellation was not preserved: {PROBE.exception_text(error) if error else 'PASS'}")
    assert_restored(console)


def busy_cancellation_cleanup() -> None:
    console = ModelConsole(interrupt_on_sleep=True, busy_reads_after_interrupt=2)
    error = run_pwm(console)
    if not isinstance(error, KeyboardInterrupt):
        fail(
            f"busy cancellation was not preserved: {PROBE.exception_text(error) if error else 'PASS'}"
        )
    assert_restored(console)


def dropped_pulse_detected() -> None:
    period = PROBE.PWM_BASE + PROBE.pwm_period_offset(0)
    console = ModelConsole(drop_writes={(period, 3)})
    error = run_pwm(console)
    expect_failure(error, "register verification failed")
    assert_restored(console)
    later_ext = PROBE.pwm_period_words(0, 2000, 20000)[1]
    if console.registers[PROBE.PWM_BASE + PROBE.pwm_ext_period_offset(0)] == later_ext:
        fail("probe continued normal pulses after a dropped write")


def dropped_divider_detected() -> None:
    divider = PROBE.CG_BASE + PROBE.CG_PWM_CLK_DIV0
    console = ModelConsole(drop_writes={(divider, 1)})
    error = run_pwm(console)
    expect_failure(error, "register verification failed")
    if PROBE.PWM_BASE + PROBE.PWM_ENABLE in {
        int(command.split()[1], 0) for command in console.commands if command.startswith("mw.l ")
    }:
        fail("probe enabled PWM after divider verification failed")
    assert_restored(console)


def dropped_restore_detected() -> None:
    divider = PROBE.CG_BASE + PROBE.CG_PWM_CLK_DIV0
    console = ModelConsole(drop_writes={(divider, 2), (divider, 3)})
    error = run_pwm(console)
    expect_failure(error, "restore PWM divider")
    if "reset" not in console.commands:
        fail("probe did not attempt bounded board recovery after a synchronized restore failure")
    clock = PROBE.CG_BASE + PROBE.CG_PWM_CLK_EN
    if console.registers[clock] != initial_registers()[clock]:
        fail("later safe cleanup was abandoned after divider restoration failed")


def gpio_ack_loss_restored() -> None:
    direction = PROBE.GPIO_BASE + PROBE.GPIO_P_DIR
    console = ModelConsole(timeout_after_write={direction})
    try:
        PROBE.gpio_probe(console, PROMPT, 0, 1, 0, 30)
    except BaseException as error:
        if "lost acknowledgement" not in PROBE.exception_text(error):
            fail(f"GPIO primary failure was not preserved: {PROBE.exception_text(error)}")
    else:
        fail("GPIO acknowledgement loss unexpectedly passed")
    expected = initial_registers()
    if console.registers[direction] != expected[direction]:
        fail("GPIO direction was not restored after acknowledgement loss")
    function = PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC
    if console.registers[function] != expected[function]:
        fail("GPIO function was not restored after acknowledgement loss")


def gpio_output_latch_restored() -> None:
    """A pad that arrives as a GPIO output driven high must leave driven high.

    Every probe cycle ends on CLR, so restoring direction and function alone
    would hand the pad back driven low -- a different electrical state than the
    one surveyed, under a function that documents verified restoration.
    """
    data = PROBE.GPIO_BASE + PROBE.GPIO_P_DATA
    direction = PROBE.GPIO_BASE + PROBE.GPIO_P_DIR
    console = ModelConsole()
    console.registers[direction] = 1
    console.registers[data] = 1
    PROBE.gpio_probe(console, PROMPT, 0, 2, 0, 30)
    if not console.registers[data] & 1:
        fail("GPIO probe left a pad driven low that it found driven high")
    if console.registers[direction] != 1:
        fail("GPIO probe did not restore the output direction it found")
    # The restoring write must be the last one to reach the latch, and it must
    # arrive while the pin is still an output: SET/CLR do not reach the latch
    # once direction is back to input. Two cycles issue two SETs, so the third
    # is the restoration.
    set_command = f"mw.l {PROBE.GPIO_BASE + PROBE.GPIO_P_SET:#x} 0x1"
    clr_command = f"mw.l {PROBE.GPIO_BASE + PROBE.GPIO_P_CLR:#x} 0x1"
    latch = [
        index
        for index, command in enumerate(console.commands)
        if command in (set_command, clr_command)
    ]
    if [console.commands[index] for index in latch].count(set_command) != 3:
        fail(f"the probe did not rewrite the output latch once after two cycles: {latch}")
    if console.commands[latch[-1]] != set_command:
        fail("the probe's last latch write was not the restoring SET")
    directions = [
        index
        for index, command in enumerate(console.commands)
        if command.startswith(f"mw.l {direction:#x} ")
    ]
    if directions and directions[-1] < latch[-1]:
        fail("GPIO probe restored direction before the output latch")


def gpio_dead_pad_detected() -> None:
    """A pad that does not follow SET must fail, not report a toggle.

    SET and CLR have no readback of their own, so without a `GPIO_P_DATA` check
    the prompt returning is the whole evidence -- and a reported toggle that
    never happened would have the operator rule out a reachable header pin.
    """
    set_register = PROBE.GPIO_BASE + PROBE.GPIO_P_SET
    console = ModelConsole(drop_writes={(set_register, 1)})
    error = None
    try:
        PROBE.gpio_probe(console, PROMPT, 0, 3, 0, 30)
    except BaseException as caught:
        error = caught
    expect_failure(error, "register verification failed")
    clr_register = PROBE.GPIO_BASE + PROBE.GPIO_P_CLR
    if console.write_counts.get(clr_register, 0):
        fail("GPIO probe drove further levels after a pad failed to follow SET")
    expected = initial_registers()
    for address in (
        PROBE.GPIO_BASE + PROBE.GPIO_P_DIR,
        PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC,
    ):
        if console.registers[address] != expected[address]:
            fail(f"register {address:#x} was not restored after the dead-pad failure")


def gpio_input_latch_untouched() -> None:
    """A pad found as an input is never driven: there is no latch to put back."""
    console = ModelConsole()
    PROBE.gpio_probe(console, PROMPT, 0, 1, 0, 30)
    for offset in (PROBE.GPIO_P_SET, PROBE.GPIO_P_CLR):
        target = f"mw.l {PROBE.GPIO_BASE + offset:#x} "
        toggles = [command for command in console.commands if command.startswith(target)]
        if len(toggles) != 1:
            fail(f"expected exactly one probe cycle write to {target.strip()}: {toggles}")
    expected = initial_registers()
    for address in (
        PROBE.GPIO_BASE + PROBE.GPIO_P_DIR,
        PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC,
    ):
        if console.registers[address] != expected[address]:
            fail(f"register {address:#x} was not restored after the GPIO probe")


def snapshot_failure_writes_nothing() -> None:
    console = ModelConsole()
    original = PROBE.read_one
    calls = 0

    def fail_second_read(model: ModelConsole, prompt: str, address: int) -> int:
        nonlocal calls
        calls += 1
        if calls == len(PROBE.CORE_RAIL_INVARIANTS) + 2:
            raise SystemExit("snapshot read failed")
        return original(model, prompt, address)

    PROBE.read_one = fail_second_read
    try:
        try:
            PROBE.gpio_probe(console, PROMPT, 0, 1, 0, 30)
        except SystemExit:
            pass
        else:
            fail("snapshot failure unexpectedly passed")
    finally:
        PROBE.read_one = original
    if any(command.startswith("mw.l ") for command in console.commands):
        fail("GPIO probe wrote before completing its snapshots")


def channel_allowlist() -> None:
    for channel in (-1, 6, 12, 31, 32, 42):
        console = ModelConsole()
        try:
            PROBE.gpio_probe(console, PROMPT, channel, 1, 0, 30)
        except SystemExit:
            pass
        else:
            fail(f"GPIO channel {channel} unexpectedly passed")
        if console.commands:
            fail(f"GPIO channel {channel} touched the board before rejection")
        console = ModelConsole()
        error = None
        try:
            PROBE.pwm_probe(console, PROMPT, channel, 20000, (1000,), 0, 0, 30)
        except BaseException as caught:
            error = caught
        expect_failure(error, "outside 0..5")
        if console.commands:
            fail(f"PWM channel {channel} touched the board before rejection")

        # The rejection must name the channel bound and must happen before
        # `Console` is constructed, so `--serial` is supplied (an absent
        # endpoint exits for its own reason) and opening a port raises
        # `BoardOpened`, which is not a rejection.
        for arguments in (
            ("--dry-run", "--pwm-channel", str(channel)),
            ("--gpio-probe", str(channel)),
        ):
            original_argv = sys.argv
            original_console = PROBE.Console

            def forbidden_console(
                *args: object, rejected_channel: int = channel, **kwargs: object
            ) -> None:
                del args, kwargs
                raise BoardOpened(
                    f"CLI channel {rejected_channel} opened the board before rejection"
                )

            PROBE.Console = forbidden_console
            sys.argv = [str(PROBE_PATH), "--serial", "model", *arguments]
            error = None
            try:
                try:
                    PROBE.main()
                except BaseException as caught:
                    error = caught
            finally:
                PROBE.Console = original_console
                sys.argv = original_argv
            expect_failure(error, "outside 0..5")
    for channel in (0, 5):
        PROBE.validate_probe_channel(channel)


def cleanup_unknown_is_explicit() -> None:
    console = ModelConsole(interrupt_on_sleep=True, busy_reads_after_interrupt=1000)
    stderr = io.StringIO()
    with redirect_stderr(stderr):
        error = run_pwm(console)
    expect_failure(error, "cleanup failures")
    if "Unable to confirm that PWM output is disabled" not in stderr.getvalue():
        fail("unknown disable state did not print the mandatory operator warning")
    if console.attempted_commands.count("reset") != 1:
        fail("probe attempted recovery reset after prompt state became unknown")


def failed_run_preserves_raw_transcript() -> None:
    divider = PROBE.CG_BASE + PROBE.CG_PWM_CLK_DIV0
    console = ModelConsole(drop_writes={(divider, 1)})
    original_argv = sys.argv
    original_console = PROBE.Console
    with tempfile.TemporaryDirectory() as directory:
        transcript = Path(directory) / "failed-uart.log"
        sys.argv = [
            str(PROBE_PATH),
            "--pwm-probe",
            "--serial",
            "model",
            "--pwm-hold-seconds",
            "0",
            "--transcript",
            str(transcript),
        ]
        PROBE.Console = lambda *args, **kwargs: console
        try:
            try:
                PROBE.main()
            except SystemExit:
                pass
            else:
                fail("failed PWM CLI run unexpectedly passed")
        finally:
            PROBE.Console = original_console
            sys.argv = original_argv
        if not transcript.is_file() or not transcript.read_bytes():
            fail("failed PWM CLI run did not preserve its raw transcript")


def pins_bind_the_count_scale() -> None:
    """`--pwm-period-us` is a counter value only at the pinned divider.

    `check_pwm_pins` is what refuses a pins edit that keeps the ratio but moves
    the scale, so a later consumer cannot inherit a halved pulse silently.
    """
    profile = PROBE.load_profile()
    PROBE.check_pwm_pins(profile)
    for key, value in (
        ("pwm_clock_divider", 239),
        ("pwm_clock_hz", 500_000),
        ("pwm_clock_source_hz", 240_000_000),
    ):
        edited = dict(profile)
        edited[key] = value
        try:
            PROBE.check_pwm_pins(edited)
        except SystemExit:
            continue
        fail(f"a pins edit of {key} to {value} did not stop the probe")



def pinned_vectors_reproduced() -> None:
    """The bytes this lane puts on the air are fixed, and the decoder reads them.

    A ground station identifies a system by what these frames say, so an
    encoder change is a change to the claim rather than an implementation
    detail. The published CRC check value is here too: a checksum routine that
    is subtly wrong still produces stable-looking frames nothing can verify.
    """
    if x25_crc(b"123456789") != 0x6F91:
        fail("the X.25 checksum does not reproduce its published check value")
    for seq, expected in PINNED_HEARTBEATS.items():
        encoded = encode_heartbeat(seq)
        if encoded != expected:
            fail(f"heartbeat seq={seq} encoded {encoded.hex()} rather than {expected.hex()}")
        if len(encoded) != 21:
            fail(f"heartbeat seq={seq} is {len(encoded)} bytes, not the 21 a v2 frame carries")
    frames = FrameDecoder().feed(b"".join(PINNED_HEARTBEATS.values()))
    if len(frames) != len(PINNED_HEARTBEATS):
        fail(f"the decoder found {len(frames)} frames in {len(PINNED_HEARTBEATS)} concatenated")
    for frame, seq in zip(frames, PINNED_HEARTBEATS, strict=True):
        if frame.msgid != 0 or not frame.crc_ok or frame.seq != seq:
            fail(f"the decoder misread the pinned frame seq={seq}: {frame}")


def parmrk_markers_stripped_for_bytes() -> None:
    """A framed reader must see the wire's bytes, not a decoded rendering.

    `read_for` replaces every byte above 0x7f, which is most of a checksum, so a
    decoder reads bytes instead. That path carries the same obligation the text
    one has: this tty is opened with `PARMRK`, which doubles a literal 0xff and
    marks a framing error with a three-byte sequence. Seq 255 puts a literal
    0xff in the header, so a byte read that skipped either would hand the
    decoder a frame one byte too long.

    Driven through a pipe rather than by calling the stripper, because the
    question is what the read path returns.
    """
    sys.path.insert(0, str(ROOT / "scripts" / "lib"))
    from uboot_console import Console as RealConsole

    read_fd, write_fd = os.pipe()
    console = RealConsole.__new__(RealConsole)
    console.fd = read_fd
    console._socket = None
    console._fail = fail
    console.framing_errors = 0
    console._parmrk_pending = b""
    console.received_bytes = bytearray()
    console.endpoint = "model-pipe"
    try:
        vector = PINNED_HEARTBEATS[255]
        os.write(write_fd, vector.replace(b"\xff", b"\xff\xff") + b"\xff\x00A")
        collected = b""
        deadline = time.monotonic() + 2.0
        while len(collected) < len(vector) and time.monotonic() < deadline:
            collected += console.read_bytes_for(0.1)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    if collected != vector:
        fail(f"the byte read returned {collected.hex()} rather than the frame {vector.hex()}")
    if console.framing_errors != 1:
        fail(f"the byte read counted {console.framing_errors} framing errors, not the one sent")
    frames = FrameDecoder().feed(collected)
    if len(frames) != 1 or not frames[0].crc_ok or frames[0].seq != 255:
        fail(f"the frame did not survive the read path intact: {frames}")


def normal_uart() -> None:
    console = ModelConsole()
    receiver = ModelReceiver(console.uart)
    error = run_uart(console, receiver)
    if error is not None:
        fail(f"normal UART run failed: {PROBE.exception_text(error)}")
    assert_uart_restored(console)
    assert_core_rail_untouched(console)
    if "reset" not in console.commands:
        fail("normal UART run did not reset after verified cleanup")
    # The RX pad and the flow-control mux belong to nothing this lane drives.
    if uart_writes(console, PROBE.TOP_BASE + PROBE.TOP_UART8_RTSCTS_MUX):
        fail("the probe wrote the RTS/CTS mux, which this lane never routes")
    function = console.registers[PROBE.TOP_BASE + PROBE.TOP_PGPIO_FUNC]
    if not function & (1 << (PROBE.UART8_PAD_BIT + 1)):
        fail("the probe took the receive pad out of GPIO mode")
    if uart_writes(console, PROBE.PAD_BASE + PROBE.PAD_PGPIO4_PULL):
        fail("the probe wrote the pad's pull configuration")


def line_configuration_follows_uboot() -> None:
    """The order the vendor's own driver uses, for the reason it uses it.

    A divisor loaded while the shifter is busy corrupts the byte in flight, and
    interrupts or modem control left set on a port nothing services are a fault
    waiting for the first frame. FCR is write-only, so IIR's FIFO encoding is
    the only confirmation the FIFOs came on.
    """
    console = ModelConsole()
    run_uart(console, ModelReceiver(console.uart), frames=1)
    def step_index(index: int, value: int) -> int | None:
        prefix = f"mw.l {PROBE.uart8_register(index):#x} {value:#x}"
        return command_index(console, lambda command: command.startswith(prefix))

    order = [
        step_index(index, value)
        for index, value in (
            (PROBE.UART_IER, 0),
            (PROBE.UART_MCR, 0),
            (PROBE.UART_IIR, PROBE.UART_FCR_ENABLE_RESET),
            (PROBE.UART_LCR, PROBE.UART_LCR_DLAB_8N1),
        )
    ]
    if any(step is None for step in order) or order != sorted(order):
        fail(f"the line was not configured in U-Boot's order: {order}")
    if console.uart.dll != 52 or console.uart.dlm != 0:
        fail(f"the divisor latch holds {console.uart.dlm:#x}{console.uart.dll:#x}, not 52")
    if console.uart.lcr != PROBE.UART_LCR_8N1:
        fail("the probe left DLAB set, so the transmit register is still the divisor latch")
    if not console.uart.fifo_enabled:
        fail("the probe did not enable the FIFOs")


def divider_restored_to_surveyed_value() -> None:
    """The divider is written, not read: the prompt's value is evidence only.

    Nothing on the development host records what the vendor loader leaves in
    this field, so a probe that derived its baud rate from what it found would
    be trusting an unobserved number. It programs 48 MHz and puts back whatever
    was there.
    """
    console = ModelConsole()
    address = PROBE.CG_BASE + PROBE.CG_UART8_CLK_DIV
    surveyed = initial_registers()[address]
    run_uart(console, ModelReceiver(console.uart), frames=1)
    values = uart_writes(console, address)
    field = PROBE.CG_UART8_CLK_DIVIDER << PROBE.CG_UART8_CLK_DIV_SHIFT
    if not values or (values[0] & PROBE.CG_UART8_CLK_DIV_MASK) != field:
        fail(f"the probe did not program the 48 MHz divider field: {[hex(v) for v in values]}")
    if console.registers[address] != surveyed:
        fail("the probe did not restore the divider field it found")


def reset_set_once_never_pulsed() -> None:
    """This reset is active-low: releasing it is a set, and a pulse is a reset.

    The vendor clock driver only ever sets this bit for UART8, and a bench probe
    that helpfully toggled it would be resetting a block mid-run for no reason.
    """
    console = ModelConsole()
    run_uart(console, ModelReceiver(console.uart), frames=1)
    address = PROBE.CG_BASE + PROBE.CG_UART8_RESET
    bit = 1 << PROBE.CG_UART8_RESET_BIT
    values = uart_writes(console, address)
    if len(values) != 2:
        fail(f"expected one release and one restore of the reset bit, got {len(values)}")
    if not values[0] & bit:
        fail("the probe's first reset write did not release the block")
    if values[1] & bit:
        fail("the probe did not restore the held reset it found")


def reset_already_released_untouched() -> None:
    console = ModelConsole()
    address = PROBE.CG_BASE + PROBE.CG_UART8_RESET
    console.registers[address] |= 1 << PROBE.CG_UART8_RESET_BIT
    run_uart(console, ModelReceiver(console.uart), frames=1)
    if uart_writes(console, address):
        fail("the probe wrote a reset bit that was already released")


def gated_port_not_read_before_ungate() -> None:
    """A read of this block while its clock is gated aborts U-Boot outright."""
    console = ModelConsole()
    error = run_uart(console, ModelReceiver(console.uart), frames=1)
    if error is not None:
        fail(f"the ordered run failed: {PROBE.exception_text(error)}")
    gate = command_index(
        console,
        lambda c: c.startswith(f"mw.l {PROBE.CG_BASE + PROBE.CG_UART8_CLK_EN:#x} "),
    )
    port = command_index(console, touches_uart8)
    if gate is None or port is None or port < gate:
        fail(f"the probe touched UART8 (command {port}) before ungating it (command {gate})")


def reset_release_dropped_refuses() -> None:
    """A release that did not take must stop the run before it drives a pad."""
    address = PROBE.CG_BASE + PROBE.CG_UART8_RESET
    console = ModelConsole(drop_writes={(address, 1)})
    error = run_uart(console, ModelReceiver(console.uart), frames=1)
    expect_failure(error, "register verification failed")
    for later in (PROBE.CG_BASE + PROBE.CG_UART8_CLK_DIV, PROBE.CG_BASE + PROBE.CG_UART8_CLK_EN):
        if uart_writes(console, later):
            fail(f"the probe continued to {later:#x} after the reset release failed")
    if console.uart.tx:
        fail("the probe transmitted after the reset release failed")


def uart_dropped_restore_detected() -> None:
    """Every shared word is put back, and a restore that did not take is named.

    This is the finding that reopened the PWM lane's bench milestone: writes
    were issued without reading them back, so what the board did with them was
    never observed.
    """
    for name, base, attribute in UART_SHARED_WORDS:
        address = base + getattr(PROBE, attribute)
        # Drop the restoring write and its one retry; earlier writes stand.
        counts = len(uart_writes(_completed_uart_run(), address))
        console = ModelConsole(drop_writes={(address, counts), (address, counts + 1)})
        error = run_uart(console, ModelReceiver(console.uart), frames=1)
        expect_failure(error, f"restore {name}")
        if "reset" not in console.commands:
            fail(f"the probe did not attempt recovery after {name} failed to restore")


def _completed_uart_run() -> ModelConsole:
    console = ModelConsole()
    run_uart(console, ModelReceiver(console.uart), frames=1)
    return console


def no_receiver_reports_unverified() -> None:
    """Transmitting is not evidence of transmission."""
    console = ModelConsole()
    stdout = io.StringIO()
    with redirect_stdout(stdout):
        error = run_uart(console, None, frames=2)
    if error is not None:
        fail(f"a receiverless run failed: {PROBE.exception_text(error)}")
    if "frames_decoded=unobserved" not in stdout.getvalue():
        fail("a run with no receiver claimed something about what reached the air")


def decoder_resyncs_after_radio_status() -> None:
    """The radio's own frames share this stream and must not break the count."""
    console = ModelConsole()
    receiver = ModelReceiver(console.uart, radio_status_before_each=True)
    stdout = io.StringIO()
    with redirect_stdout(stdout):
        error = run_uart(console, receiver, frames=4)
    if error is not None:
        fail(f"a run with radio status frames failed: {PROBE.exception_text(error)}")
    if "frames_decoded=4" not in stdout.getvalue():
        fail(f"not every heartbeat was decoded around the radio's own frames: {stdout.getvalue()}")
    if "other_msgids=4" not in stdout.getvalue():
        fail("the radio's own frames were not counted separately")
    if "rssi=200/190" not in stdout.getvalue():
        fail("the link's reported signal strength was not read")


def corrupted_crc_not_counted() -> None:
    console = ModelConsole()
    receiver = ModelReceiver(console.uart, corrupt_seq=2)
    stdout = io.StringIO()
    with redirect_stdout(stdout):
        error = run_uart(console, receiver, frames=4)
    expect_failure(error, "decoded 3 of 4")
    if "crc_failures=1" not in stdout.getvalue():
        fail("a heartbeat that arrived corrupt was not reported as such")
    assert_uart_restored(console)
    if "reset" not in console.commands:
        fail("the probe did not return the board to its firmware after a decode failure")


def dropped_frame_detected() -> None:
    console = ModelConsole()
    receiver = ModelReceiver(console.uart, drop_seq=1)
    error = run_uart(console, receiver, frames=3)
    expect_failure(error, "decoded 2 of 3")
    assert_uart_restored(console)



def decoder_resyncs_past_a_bad_length() -> None:
    """A corrupt length must not swallow the frame behind it.

    The length byte is what sizes a frame, and it is only trustworthy once the
    checksum computed over that size verifies. A decoder that stepped past a
    frame it could not verify would consume whatever followed, so a single
    corrupted byte on the air would cost two heartbeats instead of one -- and on
    a link that is dropping bytes, it would never resynchronise at all.
    """
    console = ModelConsole()
    receiver = ModelReceiver(console.uart, corrupt_len_seq=0)
    stdout = io.StringIO()
    with redirect_stdout(stdout):
        error = run_uart(console, receiver, frames=3)
    expect_failure(error, "decoded 2 of 3")
    printed = stdout.getvalue()
    for seq in (1, 2):
        if f"frame seq={seq} sent decoded=yes" not in printed:
            fail(f"a frame behind the corrupt length was lost with it: {printed}")


def temt_timeout_detected() -> None:
    """A transmitter that stops draining must fail, not spin or claim success."""
    console = ModelConsole()
    console.uart.temt_stuck = True
    error = run_uart(console, ModelReceiver(console.uart), frames=3)
    expect_failure(error, "did not report")
    if len(console.uart.tx) > 21:
        fail("the probe sent a second frame after the first never drained")
    assert_uart_restored(console)


def uart_cleanup_on_interrupt() -> None:
    console = ModelConsole(interrupt_on_sleep=True)
    error = run_uart(console, ModelReceiver(console.uart), frames=3)
    if not isinstance(error, KeyboardInterrupt):
        fail(f"cancellation was not preserved: {PROBE.exception_text(error) if error else 'PASS'}")
    assert_uart_restored(console)


def uart_pins_bind_the_divisor() -> None:
    """The clock, the baud, and the divisor are one fact in three keys.

    A self-consistent but different set transmits at a rate the ground radio
    does not listen at, which looks exactly like a dead pad.
    """
    profile = PROBE.load_profile()
    PROBE.check_uart_pins(profile)
    for key, value in (
        ("uart8_divisor", 26),
        ("uart8_clock_hz", 24_000_000),
        ("uart8_clock_divider", 19),
        ("uart8_baud", 115_200),
        ("uart8_clock_source_hz", 240_000_000),
        ("uart8_reg_shift", 0),
    ):
        edited = dict(profile)
        edited[key] = value
        try:
            PROBE.check_uart_pins(edited)
        except SystemExit:
            continue
        fail(f"a pins edit of {key} to {value} did not stop the probe")


def diagnostic_divisor_never_passes() -> None:
    """A rate experiment is a measurement, and must not read as a passing gate."""
    console = ModelConsole()
    original_argv = sys.argv
    original_console = PROBE.Console
    stdout = io.StringIO()
    sys.argv = [
        str(PROBE_PATH),
        "--uart-probe",
        "--serial",
        "model",
        "--uart-frames",
        "1",
        "--uart-interval-seconds",
        "0",
        "--uart-divisor",
        "104",
    ]
    PROBE.Console = lambda *args, **kwargs: console
    try:
        with redirect_stdout(stdout), redirect_stderr(io.StringIO()):
            PROBE.main()
    except SystemExit as error:
        fail(f"the diagnostic run exited: {error}")
    finally:
        PROBE.Console = original_console
        sys.argv = original_argv
    if "DIAGNOSTIC" not in stdout.getvalue():
        fail("an overridden divisor produced a verdict that reads as a pass")
    if console.uart.dll != 104:
        fail("the overridden divisor did not reach the port")


def uart_cli_rejects_before_opening() -> None:
    """Argument checks happen before anything opens a port."""
    for arguments in (
        ("--uart-frames", "0"),
        ("--uart-frames", "257"),
        ("--uart-divisor", "0"),
        ("--uart-divisor", "65536"),
    ):
        original_argv = sys.argv
        original_console = PROBE.Console

        def forbidden_console(*args: object, **kwargs: object) -> None:
            del args, kwargs
            raise BoardOpened("the CLI opened a port before validating its arguments")

        PROBE.Console = forbidden_console
        sys.argv = [str(PROBE_PATH), "--serial", "model", "--uart-probe", *arguments]
        error = None
        try:
            try:
                PROBE.main()
            except BaseException as caught:
                error = caught
        finally:
            PROBE.Console = original_console
            sys.argv = original_argv
        if isinstance(error, BoardOpened):
            fail(f"{arguments} opened the board before rejection")
        if not isinstance(error, SystemExit):
            fail(f"{arguments} was not rejected: {error}")


def listen_reports_without_board() -> None:
    """Pairing is checked with the radio alone, before a board is powered."""
    uart = Uart16550Model()
    uart.tx.extend(encode_heartbeat(0))
    receiver = ModelReceiver(uart, radio_status_before_each=True)
    stdout = io.StringIO()
    with redirect_stdout(stdout):
        PROBE.uart_listen(receiver, 0.01)
    printed = stdout.getvalue()
    if "heartbeats=1" not in printed or f"msgid={RADIO_STATUS_MSGID}" not in printed:
        fail(f"listening did not report what the radio delivered: {printed}")


def main() -> None:
    cases = (
        normal_pwm,
        cancellation_cleanup,
        busy_cancellation_cleanup,
        dropped_pulse_detected,
        dropped_divider_detected,
        dropped_restore_detected,
        gpio_ack_loss_restored,
        gpio_output_latch_restored,
        gpio_dead_pad_detected,
        gpio_input_latch_untouched,
        snapshot_failure_writes_nothing,
        channel_allowlist,
        cleanup_unknown_is_explicit,
        failed_run_preserves_raw_transcript,
        pins_bind_the_count_scale,
        pinned_vectors_reproduced,
        parmrk_markers_stripped_for_bytes,
        normal_uart,
        line_configuration_follows_uboot,
        divider_restored_to_surveyed_value,
        reset_set_once_never_pulsed,
        reset_already_released_untouched,
        gated_port_not_read_before_ungate,
        reset_release_dropped_refuses,
        uart_dropped_restore_detected,
        no_receiver_reports_unverified,
        decoder_resyncs_after_radio_status,
        corrupted_crc_not_counted,
        dropped_frame_detected,
        decoder_resyncs_past_a_bad_length,
        temt_timeout_detected,
        uart_cleanup_on_interrupt,
        uart_pins_bind_the_divisor,
        diagnostic_divisor_never_passes,
        uart_cli_rejects_before_opening,
        listen_reports_without_board,
    )
    failures = 0
    for case in cases:
        try:
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                case()
        except BaseException as error:
            print(f"[ ] {case.__name__}: {PROBE.exception_text(error)}")
            failures += 1
        else:
            print(f"[X] {case.__name__}")
    if failures:
        fail(f"{failures}/{len(cases)} scenarios failed")
    print(f"nt98690 bench probe regression: PASS ({len(cases)} scenarios)")


if __name__ == "__main__":
    main()
