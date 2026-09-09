#!/usr/bin/env python3

"""P6.A: boot the probe on a named Novatek NT98690 H1V1 and read its evidence.

A new execution environment, which is why this is its own checker rather than a
case inside an existing one: an unmodified vendor U-Boot driven over a serial
console, a payload delivered on removable media, an AArch64 handoff at EL2, and
a console that may be a TCP bridge to another host. None of the seL4 plane
gates can reach any of that; what they share -- ordered marker matching and its
tamper control -- is imported rather than re-implemented.

What this gate proves, and the order it proves it in, matters more than the
individual assertions. Before spending a boot it establishes that the board is
at its prompt, that the SD slot answers, that the device tree U-Boot will pass
is really a device tree, and that the bytes on the card are the bytes that were
built. Only then does it launch, because a payload that prints nothing is
otherwise indistinguishable from a card that was never written.

Nothing here writes eMMC, and nothing here writes a block device at all: the
card is staged by the operator with the command the payload builder prints. The
board returns to its own firmware through the probe's PSCI reset, so a run
costs no physical intervention and a failed run costs a power cycle.

The board is physical, so its absence is a failure and never a skip. Without
`--serial` this exits non-zero, because a gate that passes with no board
attached is a gate that says nothing.

Beside the scored run are bench modes that assert no marker contract and have
no `just` recipe, because each answers a hardware question a later gate would
otherwise assume: `--survey` reads the boot environment, `--reset-probe`
performs TF-A's watchdog sequence from the non-secure world, and `--gpio-probe`
and `--pwm-probe` drive one pad -- the first as a plain output slow enough to
find with a meter, the second as servo PWM held long enough to watch a servo or
ESC respond. The PWM modes never write a whole clock-generator or pinmux word:
both blocks are shared with PWM channel 12, which drives this SoC's CPU core
voltage, so every such write is read-modify-write and four core-rail readings
are asserted before the first one and after the last.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import time
import tomllib
from pathlib import Path
from typing import Callable, NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from arm64_image import parse_header  # noqa: E402
from sel4_gate_markers import chains_from_gate, match_marker_contract  # noqa: E402
from uboot_console import (  # noqa: E402
    Console,
    reach_uboot,
    report_transcript,
    send_command,
)

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
PINS_SECTION = "ns02201_h1v1"
BUILDER = ROOT / "scripts" / "build" / "build-nt98690-payload.py"
OUT_DIR = ROOT / "build" / "nt98690-payload"
IDENTITY = OUT_DIR / "identity.json"

PROMPT_WINDOW_SECONDS = 150.0
PAYLOAD_SECONDS = 30.0
RECOVERY_SECONDS = 90.0
CLEANUP_SYNC_SECONDS = 10.0
REGISTER_COMMAND_SECONDS = 10.0

#: The vendor U-Boot banner, which reappearing is how a completed PSCI reset is
#: observed. Pinned from `[ns02201_h1v1].uboot_version`.
BANNER_PATTERN = r"U-Boot 2021\.10"

#: Read-only questions for `--survey`, asked at the prompt before any scored
#: run. Each one settles something this gate would otherwise have to assume:
#: which slot the card is in (the eMMC answers as `mmc2`, so the SD is not
#: necessarily device 0), what `${fdtcontroladdr}` actually holds on a board
#: whose loader stages one tree at 0x100000 and whose U-Boot uses another, and
#: whether the prompt string in the pins is the real one. Nothing here writes:
#: no `saveenv`, no `mmc write`, no eMMC access at all.
SURVEY_COMMANDS: tuple[str, ...] = (
    "printenv fdtcontroladdr",
    "printenv bootcmd",
    "printenv bootdelay",
    "md.l ${fdtcontroladdr} 1",
    "mmc list",
    "mmc dev 0",
    "mmc dev 1",
    "fatls mmc 0:1",
    "bdinfo",
)

#: Ordered evidence for one probe run, from selecting the card through the
#: board's return to its own firmware.
#:
#: Every pattern here is instantiable by `check-sel4-gate-controls.py`'s
#: `literal_for`, which is what lets the shared tamper control prove this chain
#: rejects deleted, transposed, and failure-marked transcripts. That constrains
#: the regex vocabulary to literals, `\d+`, and counted character classes -- so
#: claims a regex cannot make ("this value is non-zero", "this one grew") are
#: made by the probe itself as literal `check ... = ok` lines, decided where the
#: values are. The hex readings below are measurements, reported for P6.B to
#: build on; the `check` lines are the contract.
REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "U-Boot selected a card slot",
        r"is current device",
    ),
    (
        "the address U-Boot will pass as the device tree holds one",
        r"edfe0dd0",
    ),
    (
        "fatload read the probe off the card",
        r"\d+ bytes read in \d+ ms",
    ),
    (
        "U-Boot accepted and relocated the device-tree argument",
        r"Loading Device Tree to ",
    ),
    (
        "control left U-Boot for the payload",
        r"Starting kernel \.\.\.",
    ),
    (
        "the probe reached its entry point",
        r"=== SLIME_NT98690 probe: entry reached ===",
    ),
    (
        "the firmware handed over at EL2",
        r"SLIME_NT98690 el         = 0x0{15}2",
    ),
    (
        "booti placed the image at the pinned load address",
        r"SLIME_NT98690 base       = 0x0{8}10{7}",
    ),
    (
        "firmware passed a device-tree pointer in x0",
        r"SLIME_NT98690 x0         = 0x[0-9a-f]{16}",
    ),
    (
        "that pointer addresses a flattened device tree",
        r"SLIME_NT98690 fdt_magic  = 0x0{8}d00dfeed",
    ),
    (
        "the boot core is a Cortex-A73",
        r"SLIME_NT98690 midr_part  = 0x0{13}d09",
    ),
    (
        "the implemented physical-address range is the pinned 40-bit encoding",
        r"SLIME_NT98690 parange    = 0x0{15}2",
    ),
    (
        "the primary core's CNTFRQ_EL0 holds the pinned 12 MHz",
        r"SLIME_NT98690 cntfrq     = 0x0{10}b71b00",
    ),
    (
        "the counter's rate was estimated against the line rate",
        r"SLIME_NT98690 cnt_hz_est = 0x[0-9a-f]{16}",
    ),
    (
        "the interrupt controller answered above 4 GiB with the pinned identity",
        r"SLIME_NT98690 gicd_typer = 0x0{12}fc6a",
    ),
    (
        "its interrupt line count decoded to the pinned 352",
        r"SLIME_NT98690 gic_irqs   = 0x0{13}160",
    ),
    (
        "the image ran where it was linked",
        r"SLIME_NT98690 check placement   = ok",
    ),
    (
        "the exception level is the one the seL4 loader requires",
        r"SLIME_NT98690 check el2         = ok",
    ),
    (
        "the device-tree pointer verified",
        r"SLIME_NT98690 check fdt_magic   = ok",
    ),
    (
        "the payload was entered with the MMU off",
        r"SLIME_NT98690 check mmu_off     = ok",
    ),
    (
        "the counter advanced during the run",
        r"SLIME_NT98690 check cnt_advance = ok",
    ),
    (
        "the interrupt controller reported a plausible identity",
        r"SLIME_NT98690 check gicd        = ok",
    ),
    (
        "every check passed",
        r"SLIME_NT98690 PAYLOAD_OK",
    ),
    (
        "the probe asked firmware to reset the board",
        r"SLIME_NT98690 reset request kind=psci",
    ),
    (
        "the board returned to its own firmware unattended",
        BANNER_PATTERN,
    ),
)

#: Any of these in the transcript fails the run before ordered matching, so a
#: board that reached `PAYLOAD_OK` through a degraded path cannot pass.
#:
#: `Moving Image from` is the sharpest of them: this board's U-Boot prints it
#: only when it relocated the image away from where it was loaded, which is
#: exactly the placement contract P6.B's seL4 image inherits. Tolerating it here
#: would mean shipping that image to an address nothing verified.
FAILURE_MARKERS: tuple[str, ...] = (
    r"Moving Image from",
    r"Bad Linux ARM64 Image magic!",
    r"Wrong Image Format for booti command",
    r"Could not find a valid device tree",
    r"FDT and ATAGS support not compiled in",
    r"ERROR: can't get kernel image",
    r"MMC Device \d+ not found",
    r"Unable to read file",
    r'"Synchronous Abort" handler',
    r"Kernel panic",
    r"SLIME_NT98690 FAULT",
    r"SLIME_NT98690 PAYLOAD_FAIL",
    r"SLIME_NT98690 reset failed",
    r"= FAIL",
)


#: The watchdog sequence TF-A's `nova_system_reset` performs on this SoC
#: (`plat/novatek/nvt_ns02201/pm.c`; `RTC_PWBC_RESET 0` selects this branch),
#: with offsets and bits from its `novatek_def.h`: release the watchdog's clock
#: reset, enable its clock, unlock it, fire a manual reset. `--reset-probe`
#: issues the same five writes from the non-secure world, which is the one
#: question P6.B's root-driven reset depends on and BL31 cannot answer for it.
#: Nothing here touches eMMC or firmware: BL31 does exactly this on every PSCI
#: SYSTEM_RESET, and the P6.A probe's reset already went through it.
RESET_CG_BASE = 0x2_F002_0000
RESET_CG_CLOCK_RESET = 0x9C  # ATF_CG_RESET_OFS; bit ATF_WDT_RST
RESET_CG_CLOCK_RESET_BIT = 4
RESET_CG_CLOCK_ENABLE = 0x400  # ATF_CG_ENABLE_OFS; bit ATF_WDT_POS
RESET_CG_CLOCK_ENABLE_BIT = 24
RESET_WDT_BASE = 0x2_F006_0000
RESET_WDT_MANUAL = 0x0C  # MAN_RST_OFS
RESET_WDT_UNLOCK = (0x5A96_0112, 0x5A96_0113)
RESET_PROBE_SECONDS = 30.0


#: The blocks `--pwm-probe` and `--gpio-probe` drive, with offsets from the
#: vendor BSP: the PWM block (`drivers/pwm/pwm-nvtivot.c`, corroborated
#: independently by U-Boot's own `drivers/pwm/nvt_pwm.c`), the pinmux/TOP block
#: (`plat-ns02201_a64/top_reg.h`), the clock generator
#: (`include/dt-bindings/clock/nvt-ns02201.h` with the per-channel wiring in the
#: board's `nvt-clock.dtsi`), the GPIO block (`plat-ns02201_a64/nvt-gpio.h`), and
#: the pad block (`plat-ns02201_a64/pad.h`). Bases are asserted against
#: `sel4/pins.toml` at load, so an edit to either alone fails rather than drives
#: the wrong address.
PWM_BASE = 0x2_F012_0000
PWM_ENABLE = 0x100  # write-1-to-set; also reads back the live enable state
PWM_DISABLE = 0x104  # write-1-to-clear
PWM_LOAD = 0x108  # latch a new period while the channel free-runs
PWM_MAX_PROBE_CHANNEL = 5  # 0-7 are 16-bit; 0-5 are the pads nothing else claims
CG_BASE = RESET_CG_BASE
CG_PWM_CLK_DIV0 = 0x30  # bits[13:0] channels 0-3, bits[29:16] channels 4-7
CG_PWM_CLK_EN = 0x84  # bit per channel, 1 = clock on
CG_PWM_RESET = 0xA4  # bit 0, shared by every PWM channel
CG_PWM12_CLK_EN = 0x8C  # bit 22 is channel 12's gate
TOP_BASE = 0x2_F001_0000
TOP_PWM_MUX = 0x18  # 4-bit field per channel 0-7; 1 selects the `_1` pad
TOP_PWM12_MUX = 0x1C  # channel 12's field is bits[19:16]
TOP_PGPIO_FUNC = 0xA8  # bit per P_GPIO; 0 = FUNCTION, 1 = GPIO
GPIO_BASE = 0x2_F004_0000
GPIO_P_DATA = 0x04  # P_GPIO bank: pad level
GPIO_P_DIR = 0x34  # 1 = output
GPIO_P_SET = 0x64  # write-1-to-set
GPIO_P_CLR = 0x94  # write-1-to-clear
PAD_BASE = 0x2_F003_0000
PAD_P1_STATUS = 0x210  # bit 4: P1 rail is 1.8 V rather than 3.3 V

#: Channel 12 drives the CPU core-voltage regulator through P_GPIO[42]: U-Boot
#: programs it during `otp_init` (`nvt-otp.c` `prepare_power_trim`), and the
#: board runs on what it outputs. Its clock gate, the *shared* PWM reset, its
#: divider, its pad mux, and its enable bit are therefore never written by
#: anything here -- every clock-generator and pinmux write goes through
#: `read_modify_write`, and these four readings are asserted before any of them
#: and again after the probe restores what it changed.
CORE_RAIL_INVARIANTS: tuple[tuple[str, int, int, int, int], ...] = (
    ("cg_pwm_reset", CG_BASE, CG_PWM_RESET, 0x1, 0x1),
    ("cg_pwm12_clock", CG_BASE, CG_PWM12_CLK_EN, 1 << 22, 1 << 22),
    ("top_pwm12_mux", TOP_BASE, TOP_PWM12_MUX, 0xF << 16, 0x1 << 16),
    ("pwm12_enabled", PWM_BASE, PWM_ENABLE, 1 << 12, 1 << 12),
)

#: Read-only readings `--pwm-probe` reports before it writes anything, and
#: compares against afterwards. The pinmux words matter most: no firmware in
#: this board's boot path applies the vendor device tree's pinmux (U-Boot has no
#: pinctrl driver compiled in and Linux never runs here), so what routes a PWM
#: channel to a pad is whatever reset left plus whatever U-Boot did for the core
#: rail -- a fact this gate reads rather than assumes.
PWM_SURVEY_REGISTERS: tuple[tuple[str, int, int], ...] = (
    ("top_pwm_mux", TOP_BASE, TOP_PWM_MUX),
    ("top_pgpio_func", TOP_BASE, TOP_PGPIO_FUNC),
    ("top_pwm12_mux", TOP_BASE, TOP_PWM12_MUX),
    ("cg_pwm_divider", CG_BASE, CG_PWM_CLK_DIV0),
    ("cg_pwm_clock_enable", CG_BASE, CG_PWM_CLK_EN),
    ("cg_pwm12_clock", CG_BASE, CG_PWM12_CLK_EN),
    ("cg_pwm_reset", CG_BASE, CG_PWM_RESET),
    ("pwm_enable", PWM_BASE, PWM_ENABLE),
    ("gpio_p_data", GPIO_BASE, GPIO_P_DATA),
    ("gpio_p_dir", GPIO_BASE, GPIO_P_DIR),
    ("pad_p1_status", PAD_BASE, PAD_P1_STATUS),
)

PWM_PROBE_PULSES = (1000, 1500, 2000)
PWM_PROBE_PERIOD_US = 20000
PWM_PROBE_HOLD_SECONDS = 8
#: GPIO data samples per pulse width. Zero, because the board answered: on
#: 2026-09-08 every sample read the same word while PWM0 ran on its pad, so the
#: GPIO block does not report a pad held by another function -- and each sample
#: is a serial round trip, which made the run long enough to be abandoned.
#: Kept as an option for a different pad group, never as a default.
PWM_PROBE_SAMPLES = 0
GPIO_PROBE_CYCLES = 6
GPIO_PROBE_HOLD_SECONDS = 2


def pwm_control_offset(channel: int) -> int:
    """The channel's cycle-count register: 0 means free run."""
    return channel * 8


def pwm_period_offset(channel: int) -> int:
    return channel * 8 + 0x04


def pwm_ext_period_offset(channel: int) -> int:
    """The upper byte of each period field. Channels 0-7 only."""
    return 0x230 + channel * 4


def pwm_period_words(rise: int, fall: int, base_period: int) -> tuple[int, int]:
    """Split a servo pulse into the two registers the block concatenates.

    The block compares a counter against `rise` and `fall` over `base_period`
    counts, driving the pad high between them, and splits each field's low byte
    into the period register and its high byte into the extended one. The
    extended register is always concatenated for channels 0-7, so it is written
    even when it is zero -- leaving it holds a previous run's high bytes and
    silently scales the pulse.
    """
    if not 0 <= rise <= fall <= base_period:
        raise ValueError(f"rise {rise} <= fall {fall} <= base_period {base_period} is violated")
    if base_period > 0xFFFF:
        raise ValueError(f"base_period {base_period} exceeds the 16-bit counter")
    low = (rise & 0xFF) | ((fall & 0xFF) << 8) | ((base_period & 0xFF) << 16)
    high = ((rise >> 8) & 0xFF) | (((fall >> 8) & 0xFF) << 8) | (((base_period >> 8) & 0xFF) << 16)
    return low, high


def cg_divider_word(current: int, channel: int, field: int) -> int:
    """`current` with this channel's 14-bit divider field replaced.

    Channels 0-3 share the low field and 4-7 share the high one, so programming
    one channel's rate moves its whole group -- and touching neither group is
    what keeps channel 12's divider, which lives in another register entirely,
    out of reach.
    """
    if not 0 <= channel <= 7:
        raise ValueError(f"channel {channel} has no divider field in this register")
    if not 3 <= field <= 0x3FFF:
        raise ValueError(f"divider field {field} is outside the encodable 3..16383 range")
    shift = 0 if channel < 4 else 16
    return (current & ~(0x3FFF << shift)) | (field << shift)


def pinmux_words(top_mux: int, top_func: int, channel: int) -> tuple[int, int]:
    """The two TOP words that route PWM `channel` to P_GPIO[`channel`].

    The mux field selects the `_1` pad function and the P_GPIO word must say
    FUNCTION rather than GPIO for that pin; either alone leaves the pad silent.
    """
    if not 0 <= channel <= 7:
        raise ValueError(f"channel {channel} is not routed by this register")
    shift = channel * 4
    return ((top_mux & ~(0xF << shift)) | (0x1 << shift), top_func & ~(1 << channel))


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 boot check: {message}")


def validate_probe_channel(channel: int) -> None:
    if not 0 <= channel <= PWM_MAX_PROBE_CHANNEL:
        fail(
            f"channel {channel} is outside 0..{PWM_MAX_PROBE_CHANNEL}: only those "
            "channels are authorized for this bench probe"
        )


def parse_pwm_pulses(value: str) -> tuple[int, ...]:
    try:
        pulses = tuple(int(part) for part in value.split(","))
    except ValueError:
        fail(f"--pwm-pulse-us must be comma-separated integers: {value!r}")
    if not pulses:
        fail("--pwm-pulse-us must contain at least one pulse width")
    return pulses


def validate_pwm_probe_inputs(
    channel: int, period_us: int, pulses: tuple[int, ...], hold: float, samples: int
) -> None:
    validate_probe_channel(channel)
    if not 2000 <= period_us <= 0xFFFF:
        fail(f"period {period_us} us is outside the 2000..65535 count range at 1 MHz")
    for pulse in pulses:
        if not 0 < pulse < period_us:
            fail(f"pulse {pulse} us must be inside the {period_us} us frame")
    if hold < 0:
        fail("--pwm-hold-seconds must be non-negative")
    if samples < 0:
        fail("--pwm-samples must be non-negative")


def validate_gpio_probe_inputs(channel: int, cycles: int, hold: float) -> None:
    validate_probe_channel(channel)
    if cycles <= 0:
        fail("--gpio-cycles must be positive")
    if hold < 0:
        fail("--gpio-hold-seconds must be non-negative")


def exception_text(error: BaseException) -> str:
    if isinstance(error, SystemExit):
        return str(error.code)
    return f"{type(error).__name__}: {error}"


def synchronize_prompt(console: Console, prompt: str, seconds: float = CLEANUP_SYNC_SECONDS) -> str:
    """Confirm the current boot session's prompt without resetting the board."""
    collected = ""
    attempts = max(1, int(seconds))
    for _ in range(attempts):
        console.write(b"\r", timeout=min(0.25, seconds))
        chunk = console.read_for(min(0.75, seconds))
        collected += chunk
        if re.search(BANNER_PATTERN, chunk):
            raise RuntimeError("the board reset; the original U-Boot session no longer exists")
        if prompt in chunk:
            return collected
    raise RuntimeError(
        f"the current boot session did not answer at the {prompt!r} prompt within {seconds:g}s"
    )


def run_cleanup_step(
    name: str,
    action: Callable[[], None],
    console: Console,
    prompt: str,
    errors: list[str],
) -> bool:
    """Run one cleanup action, with one bounded retry after prompt recovery."""
    first_error: BaseException | None = None
    try:
        action()
        print(f"[cleanup] {name}: verified")
        return True
    except BaseException as error:
        first_error = error
        print(
            f"[cleanup] {name}: first attempt failed: {exception_text(error)}",
            file=sys.stderr,
        )
    try:
        synchronize_prompt(console, prompt)
        print(f"[cleanup] {name}: prompt resynchronized; retrying once")
    except BaseException as sync_error:
        assert first_error is not None
        errors.append(
            f"{name}: {exception_text(first_error)}; prompt resynchronization: "
            f"{exception_text(sync_error)}"
        )
        raise RuntimeError("cleanup cannot safely issue further board commands") from sync_error
    try:
        action()
        print(f"[cleanup] {name}: verified after retry")
        return True
    except BaseException as retry_error:
        assert first_error is not None
        errors.append(
            f"{name}: first attempt {exception_text(first_error)}; "
            f"retry {exception_text(retry_error)}"
        )
        print(
            f"[cleanup] {name}: retry failed: {exception_text(retry_error)}",
            file=sys.stderr,
        )
    try:
        synchronize_prompt(console, prompt)
        print(f"[cleanup] {name}: prompt remains synchronized; continuing safe cleanup")
        return False
    except BaseException as sync_error:
        errors.append(f"prompt resynchronization after {name}: {exception_text(sync_error)}")
        raise RuntimeError("cleanup cannot safely issue further board commands") from sync_error


def load_profile() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pins: {PINS_PATH.relative_to(ROOT)}")
    pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    profile = pins.get(PINS_SECTION)
    if not isinstance(profile, dict):
        fail(f"sel4/pins.toml has no [{PINS_SECTION}] table")
    for key in (
        "board",
        "serial_baud",
        "payload_load_address",
        "boot_partition",
        "boot_files",
        "uboot_prompt",
        "uboot_select_device",
        "uboot_launch",
        "sw18_boot_position",
    ):
        if key not in profile:
            fail(f"sel4/pins.toml [{PINS_SECTION}] must pin {key}")
    return profile


def build() -> None:
    completed = subprocess.run(
        [sys.executable, str(BUILDER)], cwd=ROOT, capture_output=True, text=True
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        fail(f"the payload build failed, so there is nothing to boot:\n{detail}")


def check_identity(profile: dict[str, object]) -> tuple[Path, bytes, dict[str, object]]:
    """The artifact on disk must be this build's, and agree with the pins."""
    if not IDENTITY.is_file():
        fail(f"missing {IDENTITY.relative_to(ROOT)}; run `just nt98690_payload_check`")
    try:
        identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {IDENTITY.relative_to(ROOT)}: {error}")
    required = {"schema", "boot_file", "payload_sha256", "load_address"}
    missing = sorted(required - identity.keys())
    if missing:
        fail(f"the payload identity is missing required keys: {', '.join(missing)}")
    if identity.get("schema") != 1:
        fail("the payload identity has the wrong schema")
    expected_boot_file = str(profile["boot_files"][0])
    if identity.get("boot_file") != expected_boot_file:
        fail("the payload identity names the wrong boot file")
    binary = OUT_DIR / expected_boot_file
    if not binary.is_file():
        fail(f"missing payload {binary.relative_to(ROOT)}")
    image = binary.read_bytes()
    digest = hashlib.sha256(image).hexdigest()
    if digest != identity["payload_sha256"]:
        fail(
            f"{binary.relative_to(ROOT)} does not match its identity manifest; "
            "rebuild rather than booting an artifact of unknown provenance"
        )
    load = int(str(profile["payload_load_address"]), 16)
    if int(str(identity["load_address"]), 16) != load:
        fail("the built payload's load address disagrees with the pinned one")
    if parse_header(image).text_offset != load:
        fail("the payload's Image header text_offset disagrees with the pinned load address")
    return binary, image, identity


def read_words(output: str, count: int) -> list[int]:
    """The 32-bit words in a `md.l` dump, in address order.

    U-Boot prints `<address>: <four words>  <ascii>`, so each line is split at
    the colon and only whole 8-digit tokens from the word column are taken --
    the trailing ASCII rendering can contain anything, including something that
    looks like a hex word.
    """
    words: list[int] = []
    for line in output.splitlines():
        head, separator, tail = line.strip().partition(":")
        if not separator or not re.fullmatch(r"[0-9a-f]+", head):
            continue
        for token in tail.split()[:4]:
            if re.fullmatch(r"[0-9a-f]{8}", token):
                words.append(int(token, 16))
    return words[:count]


def check_deployed_bytes(console: Console, prompt: str, load: int, image: bytes) -> None:
    """Compare what the board loaded against what was built.

    This U-Boot has no `crc32`, so a digest of the loaded span is not available
    and `md.l` at both ends of the image is what remains. It is a sampling
    rather than a proof, and it is here for one specific and common failure: a
    card carrying a *previous* build, which produces a transcript that looks
    almost right and wastes a bench session. The `fatload` byte count above
    already rules out a truncated read.
    """
    if len(image) < 64:
        fail("the payload is too short for head-and-tail deployment sampling")
    for label, offset in (("head", 0), ("tail", ((len(image) - 64) // 64) * 64)):
        output = send_command(console, f"md.l {load + offset:#x} 0x10", prompt, 10.0, fail)
        words = read_words(output, 16)
        expected = [
            int.from_bytes(image[offset + n * 4 : offset + n * 4 + 4], "little") for n in range(16)
        ]
        if len(words) < 16:
            fail(
                f"could not read 16 words back from {load + offset:#x}; `md.l` printed:\n"
                f"{output[-400:]}"
            )
        if words != expected:
            fail(
                f"the {label} of the image in board memory does not match the built "
                "payload; the card is probably carrying an older build.\n"
                f"  expected {[f'{w:08x}' for w in expected[:4]]} ...\n"
                f"  board    {[f'{w:08x}' for w in words[:4]]} ..."
            )


def report_facts(transcript: str) -> None:
    """Print what the board measured. These are readings, not assertions.

    P6.A's job is to produce them; P6.B's is to build a kernel configuration
    from them. The one that decides something here is `cntfrq`, and it decides
    it for the next milestone rather than for this gate: a zero or implausible
    value means the seL4 platform's timer frequency has to come from a pin and
    an override rather than from the register, which is a design consequence and
    not a boot failure.
    """
    facts = dict(re.findall(r"^SLIME_NT98690 (\w+)\s+= (0x[0-9a-f]{16})", transcript, re.MULTILINE))
    if not facts:
        return
    print("--- board facts observed (inputs to P6.B) ---")
    for name, value in facts.items():
        print(f"  {name:<11} = {value}")

    cntfrq = int(facts.get("cntfrq", "0x0"), 16)
    estimate = int(facts.get("cnt_hz_est", "0x0"), 16)
    if cntfrq == 0:
        print(
            "  note: CNTFRQ_EL0 reads zero on the primary core, as this board's "
            "TF-A programs it on secondaries only. P6.B must pin the timer "
            f"frequency (the measured estimate is ~{estimate} Hz) rather than "
            "read it."
        )
    elif estimate and not 0.8 <= estimate / cntfrq <= 1.25:
        print(
            f"  note: CNTFRQ_EL0 says {cntfrq} Hz but the line-rate estimate is "
            f"~{estimate} Hz. One of them is wrong; P6.B must resolve this before "
            "pinning a timer frequency."
        )
    print("--- end board facts ---")


def survey(console: Console, prompt: str, timeout: float) -> str:
    """Ask the board the read-only questions, asserting nothing.

    The scored run needs a slot number, a device-tree address, and a prompt
    string that the vendor's autoboot transcript cannot supply, because it
    never stops at the prompt. Getting them wrong costs a power cycle each;
    asking for them costs one.
    """
    reach_uboot(console, prompt, min(timeout, PROMPT_WINDOW_SECONDS), fail)
    collected = ""
    for command in SURVEY_COMMANDS:
        print(f"[survey] {command}")
        output = send_command(console, command, prompt, 15.0, fail)
        collected += output
        # Drop the command's own echo by matching it, not by position: whether
        # a line comes back at all depends on the console's echo behaviour, and
        # skipping index 0 blindly eats the first line of the answer when it
        # does not.
        for line in output.replace("\r", "").splitlines():
            stripped = line.strip()
            if not stripped or stripped == prompt.strip():
                continue
            if stripped == command or stripped == f"{prompt}{command}".strip():
                continue
            print(f"    {line}")
    return collected


def wait_for_banner(console: Console, seconds: float) -> str:
    """Read silently until the vendor U-Boot banner, and keep only its line.

    Say nothing: the board is resetting into its own firmware, and a keystroke
    would interrupt the autoboot this step exists to observe -- leaving the
    board at a prompt and the recovery unproven. What follows the banner is the
    vendor's own next boot, which reaches its kernel handoff about 700
    characters later and prints `Moving Image from` -- a failure marker about
    where *this* gate's payload was placed, not the vendor's. Retained, a
    correct autonomous recovery would reject the run it proves.
    """
    recovery = ""
    remaining = seconds
    while remaining > 0:
        recovery += console.read_for(1.0)
        remaining -= 1.0
        if re.search(BANNER_PATTERN, recovery):
            recovery += console.read_for(1.0)
            end = re.search(BANNER_PATTERN, recovery).end()
            line_end = recovery.find("\n", end)
            return recovery[: line_end if line_end != -1 else len(recovery)]
    return recovery


def read_register(output: str, address: int) -> int | None:
    """The value `md` printed for `address`, at either access width."""
    match = re.search(rf"(?m)^0*{address:x}:\s+([0-9a-f]{{8,16}})\b", output)
    return None if match is None else int(match.group(1), 16)


def reset_probe(console: Console, prompt: str, timeout: float) -> tuple[str, int | None]:
    """Perform TF-A's 32-bit watchdog reset sequence from U-Boot."""
    reach_uboot(console, prompt, min(timeout, PROMPT_WINDOW_SECONDS), fail)
    transcript = ""
    suffix = "l"
    print("[reset]  watchdog sequence with 32-bit writes")
    for offset, bit in (
        (RESET_CG_CLOCK_RESET, RESET_CG_CLOCK_RESET_BIT),
        (RESET_CG_CLOCK_ENABLE, RESET_CG_CLOCK_ENABLE_BIT),
    ):
        address = RESET_CG_BASE + offset
        output = send_command(console, f"md.{suffix} {address:#x} 1", prompt, 10.0, fail)
        transcript += output
        value = read_register(output, address)
        if value is None:
            fail(f"could not read the clock-gate register at {address:#x}:\n{output[-300:]}")
        print(f"[reset]    {address:#x} = {value:#x} -> set bit {bit}")
        transcript += send_command(
            console, f"mw.{suffix} {address:#x} {value | (1 << bit):#x}", prompt, 10.0, fail
        )
    for key in RESET_WDT_UNLOCK:
        transcript += send_command(
            console, f"mw.{suffix} {RESET_WDT_BASE:#x} {key:#x}", prompt, 10.0, fail
        )
    console.flush_input()
    console.write(f"mw.{suffix} {RESET_WDT_BASE + RESET_WDT_MANUAL:#x} 1\r".encode())
    print(f"[reset]    fired; waiting up to {RESET_PROBE_SECONDS:.0f}s for the firmware banner")
    recovery = wait_for_banner(console, RESET_PROBE_SECONDS)
    transcript += recovery
    if re.search(BANNER_PATTERN, recovery):
        print("[reset]  the board reset and its firmware returned: 32-bit writes work")
        return transcript, 32
    print(f"[reset]  no banner in {RESET_PROBE_SECONDS:.0f}s: the board did not reset")
    return transcript, None


def pwm_probe_plan(
    channel: int, period_us: int, pulses: tuple[int, ...], samples: int = PWM_PROBE_SAMPLES
) -> list[str]:
    """The normal write sequence and conditional recovery `--pwm-probe` performs."""
    validate_pwm_probe_inputs(channel, period_us, pulses, PWM_PROBE_HOLD_SECONDS, samples)
    bit = 1 << channel
    shift = 0 if channel < 4 else 16
    period_word, ext_word = pwm_period_words(0, pulses[0], period_us)
    lines = [
        f"# channel {channel} -> P_GPIO{channel}, {period_us} us frame, "
        f"pulses {', '.join(str(pulse) for pulse in pulses)} us at 1 MHz",
        f"read-only survey of {len(PWM_SURVEY_REGISTERS)} registers, then the core-rail invariants",
        f"[RMW+verify] {CG_BASE + CG_PWM_CLK_EN:#x} set {bit:#x}            # channel clock on",
        f"[RMW+verify] {CG_BASE + CG_PWM_CLK_DIV0:#x} field[{shift + 13}:{shift}] = 119   # 1 MHz count clock",
        f"      mw.l {PWM_BASE + PWM_DISABLE:#x} {bit:#x}; read ENABLE and require channel off",
        f"      mw.l {PWM_BASE + pwm_control_offset(channel):#x} 0; read back and compare",
        f"      mw.l {PWM_BASE + pwm_period_offset(channel):#x} {period_word:#010x}; read back and compare",
        f"      mw.l {PWM_BASE + pwm_ext_period_offset(channel):#x} {ext_word:#010x}; read back and compare",
        f"[RMW+verify] {TOP_BASE + TOP_PWM_MUX:#x} field[{channel * 4 + 3}:{channel * 4}] = 1",
        f"[RMW+verify] {TOP_BASE + TOP_PGPIO_FUNC:#x} clear {bit:#x}          # FUNCTION, not GPIO",
        f"      mw.l {PWM_BASE + PWM_ENABLE:#x} {bit:#x}; read ENABLE and require channel on",
    ]
    for pulse in pulses:
        low, high = pwm_period_words(0, pulse, period_us)
        lines += [
            f"      mw.l {PWM_BASE + pwm_period_offset(channel):#x} {low:#010x}; read back and compare",
            f"      mw.l {PWM_BASE + pwm_ext_period_offset(channel):#x} {high:#010x}; read back and compare",
            f"      mw.l {PWM_BASE + PWM_LOAD:#x} {bit:#x}        # self-clearing latch request; no fabricated readback",
            "      sleep"
            + (
                f", then sample {GPIO_BASE + GPIO_P_DATA:#x} bit {channel} {samples}x"
                if samples
                else ""
            ),
        ]
    lines += [
        "# Conditional cleanup after any write may have been sent, including timeout or Ctrl-C:",
        f"      mw.l {PWM_BASE + PWM_DISABLE:#x} {bit:#x}; read ENABLE and require channel off",
        "[RMW+verify] restore TOP pad function, TOP pinmux, CG divider, CG clock gate independently",
        "      re-check core-rail invariants",
        "      only after verified cleanup: `reset`, then require the vendor banner",
        "      if prompt state cannot be recovered, stop issuing commands and report cleanup unknown",
        "",
        "Readback proves register state only; LOAD acceptance, waveform, and actuator stop remain unobserved.",
        "PWM channel 12 drives this SoC's CPU core voltage and remains outside every write:",
        f"  never written: {CG_BASE + CG_PWM_RESET:#x}, {CG_BASE + CG_PWM12_CLK_EN:#x}, "
        f"{CG_BASE + 0x324:#x}, {TOP_BASE + TOP_PWM12_MUX:#x}, PWM channel 12 registers",
        f"  W1S/W1C commands contain only target bit {bit:#x}: "
        f"{PWM_BASE + PWM_ENABLE:#x}, {PWM_BASE + PWM_DISABLE:#x}, {PWM_BASE + PWM_LOAD:#x}",
    ]
    return lines


def check_pwm_pins(profile: dict[str, object]) -> None:
    """The addresses this file drives must be the ones the pins declare."""
    for key, constant in (
        ("pwm_base", PWM_BASE),
        ("pinmux_top_base", TOP_BASE),
        ("gpio_base", GPIO_BASE),
        ("pad_base", PAD_BASE),
        ("reset_cg_base", CG_BASE),
    ):
        pinned = profile.get(key)
        if pinned is None:
            fail(f"sel4/pins.toml [{PINS_SECTION}] must pin {key} before a PWM probe")
        if int(str(pinned), 16) != constant:
            fail(f"this probe drives {constant:#x} but the pins declare {key} = {pinned}")


def read_one(console: Console, prompt: str, address: int) -> int:
    output = send_command(console, f"md.l {address:#x} 1", prompt, REGISTER_COMMAND_SECONDS, fail)
    value = read_register(output, address)
    if value is None:
        fail(f"could not read the register at {address:#x}:\n{output[-300:]}")
    return value


def verify_register(
    console: Console, prompt: str, address: int, expected: int, mask: int = 0xFFFF_FFFF
) -> int:
    actual = read_one(console, prompt, address)
    if actual & mask != expected & mask:
        fail(
            f"register verification failed at {address:#x}: expected {expected:#010x}, "
            f"actual {actual:#010x}, mask {mask:#010x}"
        )
    return actual


def write_and_verify(
    console: Console, prompt: str, address: int, value: int, mask: int = 0xFFFF_FFFF
) -> int:
    send_command(console, f"mw.l {address:#x} {value:#x}", prompt, REGISTER_COMMAND_SECONDS, fail)
    return verify_register(console, prompt, address, value, mask)


def read_modify_write(
    console: Console, prompt: str, address: int, clear_mask: int, set_mask: int
) -> tuple[int, int]:
    """Read, update allowed fields, then verify the stable R/W register."""
    before = read_one(console, prompt, address)
    after = (before & ~clear_mask) | set_mask
    if after != before:
        write_and_verify(console, prompt, address, after, clear_mask | set_mask)
    else:
        verify_register(console, prompt, address, after, clear_mask | set_mask)
    return before, after


def check_core_rail(console: Console, prompt: str, when: str) -> None:
    """Refuse to continue unless channel 12 still holds the core rail."""
    for name, base, offset, mask, expected in CORE_RAIL_INVARIANTS:
        value = read_one(console, prompt, base + offset)
        if value & mask != expected:
            fail(
                f"core-rail invariant {name} does not hold {when}: "
                f"{base + offset:#x} = {value:#x}, expected {expected:#x} in {mask:#x}. "
                "PWM channel 12 drives the CPU core voltage on this board, so this "
                "probe writes nothing further"
            )
    print(f"[pwm]    core-rail invariants hold {when}")


def pwm_survey(console: Console, prompt: str) -> dict[str, int]:
    """Report the PWM, clock, pinmux, and pad state, asserting nothing."""
    readings: dict[str, int] = {}
    for name, base, offset in PWM_SURVEY_REGISTERS:
        value = read_one(console, prompt, base + offset)
        readings[name] = value
        print(f"[pwm]    {name} ({base + offset:#x}) = {value:#010x}")
    rail = "1.8V" if readings["pad_p1_status"] & (1 << 4) else "3.3V"
    print(f"[pwm]    P_GPIO[0..19] pad rail reads {rail}")
    return readings


def gpio_probe(
    console: Console, prompt: str, channel: int, cycles: int, hold: float, timeout: float
) -> str:
    """Toggle one authorized P_GPIO output and restore verified snapshots."""
    validate_gpio_probe_inputs(channel, cycles, hold)
    reach_uboot(console, prompt, min(timeout, PROMPT_WINDOW_SECONDS), fail)
    transcript = ""
    bit = 1 << channel
    check_core_rail(console, prompt, "before the GPIO probe")

    # Complete every snapshot before the first write. Cleanup never invents a
    # value when a read failed.
    func_before = read_one(console, prompt, TOP_BASE + TOP_PGPIO_FUNC)
    dir_before = read_one(console, prompt, GPIO_BASE + GPIO_P_DIR)
    modified = False
    primary_error: BaseException | None = None
    cleanup_errors: list[str] = []
    cleanup_safe = True
    print(f"[gpio]   driving P_GPIO{channel} as an output for {cycles} cycles")
    try:
        modified = True
        read_modify_write(console, prompt, TOP_BASE + TOP_PGPIO_FUNC, 0, bit)
        read_modify_write(console, prompt, GPIO_BASE + GPIO_P_DIR, 0, bit)
        for cycle in range(cycles):
            for level, offset in (("high", GPIO_P_SET), ("low", GPIO_P_CLR)):
                transcript += send_command(
                    console,
                    f"mw.l {GPIO_BASE + offset:#x} {bit:#x}",
                    prompt,
                    REGISTER_COMMAND_SECONDS,
                    fail,
                )
                print(f"[gpio]   cycle {cycle + 1}/{cycles}: P_GPIO{channel} {level}")
                time.sleep(hold)
    except BaseException as error:
        primary_error = error
    finally:
        if modified:
            for name, action in (
                (
                    "restore GPIO direction",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        GPIO_BASE + GPIO_P_DIR,
                        bit,
                        dir_before & bit,
                    ),
                ),
                (
                    "restore GPIO function",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        TOP_BASE + TOP_PGPIO_FUNC,
                        bit,
                        func_before & bit,
                    ),
                ),
            ):
                if not cleanup_safe:
                    cleanup_errors.append(f"{name}: not attempted because prompt state is unknown")
                    continue
                try:
                    run_cleanup_step(name, action, console, prompt, cleanup_errors)
                except RuntimeError:
                    cleanup_safe = False
            if cleanup_safe:
                try:
                    run_cleanup_step(
                        "core-rail invariant check",
                        lambda: check_core_rail(console, prompt, "after the GPIO probe"),
                        console,
                        prompt,
                        cleanup_errors,
                    )
                except RuntimeError:
                    cleanup_safe = False
    if cleanup_errors:
        print("[gpio]   cleanup errors: " + "; ".join(cleanup_errors), file=sys.stderr)
    if primary_error is not None:
        if cleanup_errors:
            fail(
                f"original GPIO probe failure: {exception_text(primary_error)}; "
                f"cleanup failures: {'; '.join(cleanup_errors)}"
            )
        raise primary_error
    if cleanup_errors:
        fail("GPIO probe cleanup was not fully verified: " + "; ".join(cleanup_errors))
    return transcript


def pwm_probe(
    console: Console,
    prompt: str,
    channel: int,
    period_us: int,
    pulses: tuple[int, ...],
    hold: float,
    samples: int,
    timeout: float,
) -> str:
    """Drive one PWM channel; verify registers, cleanup, and firmware return separately."""
    validate_pwm_probe_inputs(channel, period_us, pulses, hold, samples)
    reach_uboot(console, prompt, min(timeout, PROMPT_WINDOW_SECONDS), fail)
    transcript = ""
    bit = 1 << channel
    divider_mask = 0x3FFF << (0 if channel < 4 else 16)
    mux_mask = 0xF << (channel * 4)
    print("[pwm]    surveying before writing anything")
    before = pwm_survey(console, prompt)
    check_core_rail(console, prompt, "before the PWM probe")

    divider = 119
    period_address = PWM_BASE + pwm_period_offset(channel)
    ext_address = PWM_BASE + pwm_ext_period_offset(channel)
    control_address = PWM_BASE + pwm_control_offset(channel)
    mux_word, func_word = pinmux_words(before["top_pwm_mux"], before["top_pgpio_func"], channel)
    modified = False
    primary_error: BaseException | None = None
    cleanup_errors: list[str] = []
    cleanup_safe = True
    disabled_verified = False
    restored_verified = False
    banner_verified = False

    try:
        # Mark possible modification before the first command is sent: a lost
        # acknowledgement does not prove that the target rejected the write.
        modified = True
        read_modify_write(console, prompt, CG_BASE + CG_PWM_CLK_EN, 0, bit)
        read_modify_write(
            console,
            prompt,
            CG_BASE + CG_PWM_CLK_DIV0,
            divider_mask,
            cg_divider_word(0, channel, divider) & divider_mask,
        )

        transcript += send_command(
            console,
            f"mw.l {PWM_BASE + PWM_DISABLE:#x} {bit:#x}",
            prompt,
            REGISTER_COMMAND_SECONDS,
            fail,
        )
        verify_register(console, prompt, PWM_BASE + PWM_ENABLE, 0, bit)
        write_and_verify(console, prompt, control_address, 0)

        first_period, first_ext = pwm_period_words(0, pulses[0], period_us)
        write_and_verify(console, prompt, period_address, first_period)
        write_and_verify(console, prompt, ext_address, first_ext)
        read_modify_write(console, prompt, TOP_BASE + TOP_PWM_MUX, mux_mask, mux_word & mux_mask)
        read_modify_write(console, prompt, TOP_BASE + TOP_PGPIO_FUNC, bit, func_word & bit)
        transcript += send_command(
            console,
            f"mw.l {PWM_BASE + PWM_ENABLE:#x} {bit:#x}",
            prompt,
            REGISTER_COMMAND_SECONDS,
            fail,
        )
        verify_register(console, prompt, PWM_BASE + PWM_ENABLE, bit, bit)

        for pulse in pulses:
            period_word, ext_word = pwm_period_words(0, pulse, period_us)
            write_and_verify(console, prompt, period_address, period_word)
            write_and_verify(console, prompt, ext_address, ext_word)
            transcript += send_command(
                console,
                f"mw.l {PWM_BASE + PWM_LOAD:#x} {bit:#x}",
                prompt,
                REGISTER_COMMAND_SECONDS,
                fail,
            )
            # LOAD is self-clearing. Stable PERIOD/EXT readback proves the
            # requested configuration, not latch acceptance or waveform.
            verify_register(console, prompt, period_address, period_word)
            verify_register(console, prompt, ext_address, ext_word)
            duty = 100.0 * pulse / period_us
            print(
                f"[pwm]    pulse_us={pulse} period_us={period_us} duty={duty:.1f}%: "
                f"registers verified; observe the servo or ESC for {hold:g}s"
            )
            time.sleep(hold)
            if samples:
                high = sum(
                    bool(read_one(console, prompt, GPIO_BASE + GPIO_P_DATA) & bit)
                    for _ in range(samples)
                )
                print(
                    f"[pwm]    gpio_data samples={samples} high={high} "
                    f"(expected ~{duty:.0f}% only if this pad reads back)"
                )
    except BaseException as error:
        primary_error = error
    finally:
        if modified:

            def disable_target() -> None:
                nonlocal disabled_verified
                send_command(
                    console,
                    f"mw.l {PWM_BASE + PWM_DISABLE:#x} {bit:#x}",
                    prompt,
                    REGISTER_COMMAND_SECONDS,
                    fail,
                )
                verify_register(console, prompt, PWM_BASE + PWM_ENABLE, 0, bit)
                disabled_verified = True

            cleanup_steps: tuple[tuple[str, Callable[[], None]], ...] = (
                ("disable target PWM channel", disable_target),
                (
                    "restore TOP pad function",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        TOP_BASE + TOP_PGPIO_FUNC,
                        bit,
                        before["top_pgpio_func"] & bit,
                    ),
                ),
                (
                    "restore TOP PWM mux",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        TOP_BASE + TOP_PWM_MUX,
                        mux_mask,
                        before["top_pwm_mux"] & mux_mask,
                    ),
                ),
                (
                    "restore PWM divider",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        CG_BASE + CG_PWM_CLK_DIV0,
                        divider_mask,
                        before["cg_pwm_divider"] & divider_mask,
                    ),
                ),
                (
                    "restore PWM clock gate",
                    lambda: read_modify_write(
                        console,
                        prompt,
                        CG_BASE + CG_PWM_CLK_EN,
                        bit,
                        before["cg_pwm_clock_enable"] & bit,
                    ),
                ),
            )
            for index, (name, action) in enumerate(cleanup_steps):
                try:
                    run_cleanup_step(name, action, console, prompt, cleanup_errors)
                except RuntimeError:
                    cleanup_safe = False
                    for remaining_name, _ in cleanup_steps[index + 1 :]:
                        cleanup_errors.append(
                            f"{remaining_name}: not attempted because prompt state is unknown"
                        )
                    break

            if cleanup_safe:
                try:
                    run_cleanup_step(
                        "core-rail invariant check",
                        lambda: check_core_rail(console, prompt, "after PWM cleanup"),
                        console,
                        prompt,
                        cleanup_errors,
                    )
                except RuntimeError:
                    cleanup_safe = False
            restored_verified = cleanup_safe and not cleanup_errors and disabled_verified

            if cleanup_safe:
                try:
                    console.flush_input()
                    console.write(b"reset\r")
                    recovery = wait_for_banner(console, RECOVERY_SECONDS)
                    transcript += recovery
                    if not re.search(BANNER_PATTERN, recovery):
                        raise RuntimeError(
                            f"no firmware banner within {RECOVERY_SECONDS:.0f}s of `reset`"
                        )
                    banner_verified = True
                    print("[pwm]    the vendor firmware returned")
                except BaseException as error:
                    cleanup_errors.append(f"firmware return: {exception_text(error)}")

    print(
        f"[pwm]    cleanup result: channel_disabled={disabled_verified} "
        f"settings_restored={restored_verified} firmware_banner={banner_verified}"
    )
    if not disabled_verified:
        print(
            "[pwm]    Unable to confirm that PWM output is disabled; output may still be enabled.\n"
            "[pwm]    Manually remove actuator power and service or power-cycle the board.",
            file=sys.stderr,
        )
    if cleanup_errors:
        print("[pwm]    cleanup errors: " + "; ".join(cleanup_errors), file=sys.stderr)
    if primary_error is not None:
        if cleanup_errors:
            fail(
                f"original PWM probe failure: {exception_text(primary_error)}; "
                f"cleanup failures: {'; '.join(cleanup_errors)}"
            )
        raise primary_error
    if cleanup_errors or not (disabled_verified and restored_verified and banner_verified):
        fail("PWM probe cleanup or recovery was not fully verified: " + "; ".join(cleanup_errors))
    return transcript


def monitor(console: Console, timeout: float) -> None:
    """Print whatever the board says, asserting nothing.

    Separating a link fault from an image fault is the first thing a bench
    session needs, and a gate that asserts cannot do it.
    """
    print(f"[monitor] reading {console.describe()} for up to {timeout:.0f}s; Ctrl-C to stop")
    seen = 0
    idle = 0.0
    while idle < 10.0 and seen < timeout:
        chunk = console.read_for(1.0)
        seen += 1
        if chunk:
            idle = 0.0
            sys.stdout.write(chunk)
            sys.stdout.flush()
        else:
            idle += 1.0
    print()
    if console.framing_errors is None:
        print("[monitor] framing errors unobservable over a TCP bridge")
    else:
        print(f"[monitor] framing errors: {console.framing_errors}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--serial",
        help=(
            "the board's console: a tty path such as /dev/ttyUSB0, or "
            "tcp:HOST:PORT for a socat/ser2net bridge when the board is on "
            "another machine"
        ),
    )
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--monitor", action="store_true", help="print the console, assert nothing")
    parser.add_argument(
        "--survey",
        action="store_true",
        help="ask the board the read-only questions a scored run depends on",
    )
    parser.add_argument(
        "--reset-probe",
        action="store_true",
        help=(
            "perform TF-A's watchdog reset sequence from the U-Boot prompt and "
            "report whether the non-secure world may reset this board"
        ),
    )
    parser.add_argument(
        "--pwm-probe",
        action="store_true",
        help=(
            "drive servo PWM on one channel from the U-Boot prompt, holding each "
            "pulse width long enough to watch a servo or ESC respond"
        ),
    )
    parser.add_argument(
        "--gpio-probe",
        type=int,
        metavar="CHANNEL",
        help="toggle P_GPIO[CHANNEL] as a plain output so a meter can find the pad",
    )
    parser.add_argument("--pwm-channel", type=int, default=0)
    parser.add_argument("--pwm-period-us", type=int, default=PWM_PROBE_PERIOD_US)
    parser.add_argument(
        "--pwm-pulse-us",
        default=",".join(str(pulse) for pulse in PWM_PROBE_PULSES),
        help="comma-separated pulse widths to hold in turn",
    )
    parser.add_argument("--pwm-hold-seconds", type=float, default=PWM_PROBE_HOLD_SECONDS)
    parser.add_argument("--pwm-samples", type=int, default=PWM_PROBE_SAMPLES)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the PWM probe's write sequence and exit; needs no board",
    )
    parser.add_argument("--gpio-cycles", type=int, default=GPIO_PROBE_CYCLES)
    parser.add_argument("--gpio-hold-seconds", type=float, default=GPIO_PROBE_HOLD_SECONDS)
    parser.add_argument("--transcript", type=Path, help="write the captured transcript here")
    parser.add_argument("--no-build", action="store_true")
    arguments = parser.parse_args()

    profile = load_profile()
    baud = int(str(profile["serial_baud"]))
    pulses = parse_pwm_pulses(str(arguments.pwm_pulse_us))
    if arguments.dry_run or arguments.pwm_probe:
        validate_pwm_probe_inputs(
            arguments.pwm_channel,
            arguments.pwm_period_us,
            pulses,
            arguments.pwm_hold_seconds,
            arguments.pwm_samples,
        )
    if arguments.gpio_probe is not None:
        validate_gpio_probe_inputs(
            arguments.gpio_probe, arguments.gpio_cycles, arguments.gpio_hold_seconds
        )

    if arguments.dry_run:
        check_pwm_pins(profile)
        for line in pwm_probe_plan(
            arguments.pwm_channel, arguments.pwm_period_us, pulses, arguments.pwm_samples
        ):
            print(line)
        return

    if arguments.serial is None:
        fail(
            "no serial endpoint given, so no board evidence can be observed; "
            "P6.A requires an observed boot on the named Novatek NT98690 H1V1. "
            "Pass --serial /dev/ttyUSB0, or --serial tcp:HOST:PORT for a bridge"
        )

    console = Console(arguments.serial, baud, fail)
    # Accumulated outside the try so a failed run still writes what it saw. A
    # transcript is most valuable exactly when the gate rejects: that is the run
    # somebody has to diagnose, and on a physical board it cost a power cycle.
    transcript = ""
    try:
        if arguments.monitor:
            monitor(console, arguments.timeout)
            return

        if arguments.survey:
            transcript += survey(console, str(profile["uboot_prompt"]), arguments.timeout)
            return

        if arguments.reset_probe:
            probed, width = reset_probe(console, str(profile["uboot_prompt"]), arguments.timeout)
            transcript += probed
            if width is None:
                fail(
                    "32-bit watchdog writes did not reset the board from the non-secure world; "
                    "P6.B's root cannot reset it and must use the manual power-cycle path"
                )
            print(
                f"nt98690 reset probe: PASS, {width}-bit writes reset the named {profile['board']}"
            )
            return

        if arguments.gpio_probe is not None:
            check_pwm_pins(profile)
            transcript += gpio_probe(
                console,
                str(profile["uboot_prompt"]),
                arguments.gpio_probe,
                arguments.gpio_cycles,
                arguments.gpio_hold_seconds,
                arguments.timeout,
            )
            print(
                f"nt98690 gpio probe: P_GPIO{arguments.gpio_probe} toggled on the named "
                f"{profile['board']}; which header pin followed it is the operator's reading"
            )
            return

        if arguments.pwm_probe:
            check_pwm_pins(profile)
            transcript += pwm_probe(
                console,
                str(profile["uboot_prompt"]),
                arguments.pwm_channel,
                arguments.pwm_period_us,
                pulses,
                arguments.pwm_hold_seconds,
                arguments.pwm_samples,
                arguments.timeout,
            )
            print(
                f"nt98690 pwm probe: PASS, channel {arguments.pwm_channel} register "
                f"settings matched after every write, the channel was confirmed disabled, "
                f"shared settings were confirmed restored, and firmware returned on the "
                f"named {profile['board']}. LOAD acceptance, waveform, actuator response, "
                "and physical stop remain operator observations"
            )
            return

        if not arguments.no_build:
            build()
        binary, image, identity = check_identity(profile)
        load = int(str(profile["payload_load_address"]), 16)
        prompt = str(profile["uboot_prompt"])

        print(
            f"[gate]   payload {binary.name}, {len(image)} bytes, sha {identity['payload_sha256'][:16]}…"
        )
        print(f"[gate]   console {console.describe()}")
        print(
            f"[gate]   the card must carry {binary.name} at its root, and SW18 must be "
            f"{profile['sw18_boot_position']} (never the loader's rescue position)"
        )

        reach_uboot(console, prompt, min(arguments.timeout, PROMPT_WINDOW_SECONDS), fail)

        # Probe the slot before loading from it: `fatload` against an
        # un-probed card can hang this U-Boot outright, where `mmc dev` fails
        # fast and says so.
        transcript += send_command(console, str(profile["uboot_select_device"]), prompt, 15.0, fail)

        # Confirm the device tree exists before spending a boot on it. This
        # U-Boot panics rather than warns when `booti` is given no tree, and
        # `${fdtcontroladdr}` is supplied by the vendor loader rather than by us.
        tree = send_command(console, "md.l ${fdtcontroladdr} 1", prompt, 10.0, fail)
        transcript += tree
        # The word is the FDT's big-endian d00dfeed magic read back as a
        # little-endian long, so this is the tree's own header and not an
        # address that merely reads.
        if "edfe0dd0" not in tree:
            fail(
                "${fdtcontroladdr} does not point at a device tree — `md.l` read "
                f"no d00dfeed magic there. `booti` panics rather than warns on a "
                f"missing tree, so this run stops before spending a boot on it:\n{tree[-400:]}"
            )

        partition = str(profile["boot_partition"])
        loaded = send_command(
            console, f"fatload {partition} {load:#x} {binary.name}", prompt, 60.0, fail
        )
        transcript += loaded
        match = re.search(r"(\d+) bytes read", loaded)
        if match is None:
            fail(
                f"`fatload` did not report a byte count; is {binary.name} at the root "
                f"of the card in {partition}?\n{loaded[-400:]}"
            )
        if int(match.group(1)) != len(image):
            fail(
                f"the board read {match.group(1)} bytes but the payload is "
                f"{len(image)}; the card is carrying a different build"
            )

        check_deployed_bytes(console, prompt, load, image)

        launch = str(profile["uboot_launch"]).replace("{load}", f"{load:#x}")
        print(f"[gate]   launching: {launch}")
        console.flush_input()
        console.write(launch.encode() + b"\r")

        payload = ""
        deadline = PAYLOAD_SECONDS
        while deadline > 0:
            chunk = console.read_for(0.5)
            deadline -= 0.5
            payload += chunk
            if re.search(r"SLIME_NT98690 (reset request kind=psci|PAYLOAD_FAIL|FAULT)", payload):
                break
        if not payload:
            fail(
                f"the board printed nothing for {PAYLOAD_SECONDS:.0f}s after `booti`; the payload hung or UART output stopped"
            )
        transcript += payload

        print(f"[gate]   waiting up to {RECOVERY_SECONDS:.0f}s for the vendor firmware to return")
        transcript += wait_for_banner(console, RECOVERY_SECONDS)
    finally:
        raw = bytes(console.received_bytes)
        console.close()
        if arguments.transcript and raw:
            arguments.transcript.parent.mkdir(parents=True, exist_ok=True)
            arguments.transcript.write_bytes(raw)
            print(f"[gate]   transcript written to {arguments.transcript}")

    report_facts(transcript)
    match_marker_contract(
        transcript,
        chains_from_gate(sys.modules[__name__]),
        FAILURE_MARKERS,
        fail,
        before_reject=lambda: report_transcript(transcript),
    )

    if console.framing_errors is None:
        print("[gate]   framing errors unobservable over a TCP bridge")
    elif console.framing_errors:
        fail(
            f"{console.framing_errors} framing errors on the wire; the transcript "
            "is not trustworthy evidence about this board"
        )
    else:
        print("[gate]   framing errors: 0")

    print(f"nt98690 boot check: PASS on the named {profile['board']}")


if __name__ == "__main__":
    main()
