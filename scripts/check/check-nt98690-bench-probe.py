#!/usr/bin/env python3

"""Host regressions for the NT98690 GPIO/PWM bench probe control flow."""

from __future__ import annotations

import importlib.util
import io
import sys
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from types import ModuleType
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PROBE_PATH = ROOT / "scripts" / "check" / "check-nt98690-boot.py"
PROMPT = "nvt: "


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
            value = self.registers.get(address, 0)
            return f"{address:011x}: {value:08x}\r\n{PROMPT}"
        if parts[0] != "mw.l":
            raise AssertionError(f"unexpected command: {command}")
        address = int(parts[1], 0)
        value = int(parts[2], 0) & 0xFFFF_FFFF
        count = self.write_counts.get(address, 0) + 1
        self.write_counts[address] = count
        if (address, count) not in self.drop_writes:
            if address == PROBE.PWM_BASE + PROBE.PWM_ENABLE:
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

        # The CLI cases have to reject *for the channel*. Two accidents would
        # otherwise pass them: forgetting to install `sys.argv`, and the
        # missing-`--serial` exit. So the argv is installed, `--serial` is
        # supplied, and the failure text must name the channel bound -- while
        # opening the board raises something `expect_failure` cannot accept.
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
        gpio_input_latch_untouched,
        snapshot_failure_writes_nothing,
        channel_allowlist,
        cleanup_unknown_is_explicit,
        failed_run_preserves_raw_transcript,
        pins_bind_the_count_scale,
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
