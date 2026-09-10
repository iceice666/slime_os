# A MAVLink heartbeat from the H1V1: opening the lane and its bench probe

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Decision |
| Status | Proposed |
| Scope | `scripts/check/check-nt98690-boot.py`, `scripts/check/check-nt98690-bench-probe.py`, `scripts/check/check-sel4-pins.py`, `scripts/lib/mavlink.py`, `scripts/lib/uboot_console.py`, `sel4/pins.toml`, `.tasks/items/` |
| Work items | 01a08ac5-1496-7988-a11a-bb5a3744456c, 01a08ac5-14a7-757b-a456-c91868d651c8, 01a08ac5-14ba-7d23-a751-b06678692de9, 01a08ac5-14d4-74b7-88dd-af653b077edd |
| Gates | `just nt98690_bench_probe_check`, `just sel4_pin_check`, `just sel4_gate_control_check` |
| Trigger | The H1V1 drives an actuator from the vendor prompt (P6.PWM.A), and the next thing asked of it is to say something outward |
| Baseline | Nothing in the tree transmits on a serial port: UART0 is the kernel's debug console and the shell's input, the root's UART adapter only reads, and no component can pace itself in real time |

## Summary

This opens a lane to transmit a MAVLink v2 HEARTBEAT once per second from the resident Slime
graph on the named Novatek NT98690 H1V1, through the SoC's UART8 into an RFD900x telemetry
radio, and lands the first session's half of it: the bench mode that answers, from the vendor
U-Boot prompt, the hardware questions Slime's own bring-up would otherwise assume. No Slime code
transmits yet, and no board run has happened, so this entry is `Proposed`; the probe's own
result will be its own `Audit` entry.

"Radio on GPIO" is the ask, and the answer is a hardware UART whose transmit pad happens to be
on the GPIO bank the PWM lane already found on the 40-pin header. Nothing is bit-banged.

The lane is cut into three sessions on the P6.PWM pattern: a board-specific bench probe, a
board-neutral mechanism and protocol observed only under QEMU, then the board-specific bring-up
and gate. The middle session names no SoC.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/lib/mavlink.py` | HEARTBEAT framing, the X.25 checksum, and a resynchronising v2 frame decoder, in the standard library | The instrument that says a board transmitted is in the repository, and adds no dependency to any gate that reads it |
| `scripts/lib/uboot_console.py` | `Console.read_bytes_for`, factored out of `read_for`, which now decodes its result | A framed binary protocol is read as bytes; `read_for`'s `errors="replace"` rewrites most of a checksum |
| `scripts/check/check-nt98690-boot.py` | `--uart-probe` transmits pinned heartbeats out of UART8 and decodes them on a paired radio; `--uart-listen-seconds` reads that radio alone; `--dry-run --uart-probe` renders the write sequence with no board | A hardware fact a later gate depends on is observed before it is depended on, and an unlinked radio is separated from a silent board before one is powered |
| `scripts/check/check-nt98690-boot.py` | Every clock-generator and pinmux write is read-modify-write on one field, every restoration is verified by readback, and the core-rail invariants are asserted before the first write and after the last | The CPU's core-voltage regulator keeps running, and what the board did with a restoring write is observed rather than assumed |
| `sel4/pins.toml`, `scripts/check/check-sel4-pins.py` | UART8's base, register layout, clock source, divider, rate, baud, and divisor pinned and checked; `observed_uart_routes` empty, so every observed key must be absent until a board run fills it | A pins edit that keeps the arithmetic self-consistent but moves the rate fails a check rather than transmitting where nothing listens |
| `scripts/check/check-nt98690-bench-probe.py` | A 16550 model, a ground-radio model with three ways to spoil a frame, and 21 further scenarios | The probe's control flow, its register ordering, and its cleanup are regressions rather than claims |
| `.tasks/items/` | The lane epic, the bench milestone, an IO-track milestone for the mechanism, and P6.E for the board evidence, with the dependency edges between them | — |

## Decisions

**The port is UART8, and only its transmit pad is routed.** UART0 is the kernel's debug console
and the resident shell's input; a radio sharing it would interleave with the shell. UART8's
`_1` function reaches P_GPIO[4] and [5] on the same 3.3 V bank the ESC probe found on the
header, and nothing in the boot path claims it: U-Boot compiles no driver past its console port
and never calls its own `serial_preinit`, and Linux's device tree routes no UART6-9 pad. Since
this lane never receives, `P_GPIO[5]` stays in GPIO mode and the flow-control mux is never
written. A consequence worth stating: with the receive input unrouted, the line-status
register's receive bits float, so every comparison here masks to the two transmitter bits.

**The divider is written, not read.** The device tree declares UART8 at 48 MHz, which is what
Linux programs; no source on this host says what the vendor loader leaves in that field at the
prompt. The probe surveys it, reports the rate it encodes, programs its own, and restores what
it found. Deriving a baud rate from an unobserved field would make the one number the radio
depends on an assumption.

**The reset bit is set, never pulsed.** `CG+0x9C` bit 23 is an active-low RSTN: 1 is released.
The vendor clock driver only ever sets it for this block, and a bench probe that helpfully
toggled it would be resetting a UART mid-run for no reason.

**Restoration is verified by readback from the first run.** `P6.PWM.A` was closed on a
transcript that carried its restoration *writes* and no readbacks, and was reopened for exactly
that gap. Every shared word this probe changes is restored through the same read-modify-write
path that verifies it, and a restoration that did not take is named in the failure.

**The claim rests on a decoded frame, not a register.** Readback proves a register took a value.
What proves a pad transmitted is a frame arriving on the other radio with a valid checksum and
the sequence number that was sent, which is why the ground radio is part of the probe rather
than an operator's terminal. A run with no receiver transmits and reports `frames_decoded=unobserved`;
it claims nothing about the air. This is the one place this lane differs from the ESC lane,
where the actuator's response is necessarily an operator observation: here the instrument is
code, so the later board gate's exit condition contains no operator reading at all.

**A rate experiment is visible in the transcript, not in a code edit.** If frames are sent and
nothing decodes, the suspect is the 480 MHz source the clock tree names. `--uart-divisor` exists
to test that in one run, and any value but the pinned one makes the verdict `DIAGNOSTIC`, which
closes nothing.

**The bench mode lives in the existing checker; the later gate will not.** Same execution
environment, same `md.l`/`mw.l` protocol, same prompt, no marker contract: the verification
discipline puts this beside `--pwm-probe` and `--reset-probe`, and the gate count is unchanged
at 48. The board gate that ends the lane has its own image, its own session, and its own ordered
marker contract, so it will be its own checker.

## Open risks and follow-ups

- **Whether P_GPIO[4] reaches a connector is unresolved and is this lane's one go/no-go.** The
  ESC run found P_GPIO[0] on the 40-pin header but its position was never written down, and no
  schematic, pinout table, or header note exists anywhere on this host. `--gpio-probe 4` answers
  it. If it is not broken out, the fallback is UART8's second route on C_GPIO[17..18], and only
  if those reach a connector.
- **The 480 MHz source is a device-tree declaration.** Every other number here was cross-checked
  against two vendor sources; this one has one. A first run that transmits nothing decodable
  tests it directly.
- **The radio pair is assumed configured.** Both radios must share their network id, air rate,
  and frequency band. `--uart-listen-seconds` checks the link before the board is touched, and
  reading the radio's own parameters is an operator step: scripting command mode risks leaving a
  radio in it on a failure path.
- **Two later sessions are planned, not built.** The board-neutral mechanism needs a syscall that
  reports the timer's rate, because a component today reads raw ticks and cannot express one
  second; and it needs the declared-device authority the ESC lane's second session introduces.
  Neither exists yet.
- The probe has not been run. Nothing here is observed evidence about this board.

## Artifacts and provenance

- [`plan.md`](plan.md) — the lane's plan of record: the durable board, protocol, and repository
  facts with a source for each, the fixed names, the reuse map, the session split, and the
  risks. Kept beside this entry so a later session re-verifies one line instead of re-exploring
  the vendor BSP.
- Vendor sources for every register fact are cited inline in `plan.md`, section A2. The SoC
  pinmux table is at `/srv/novatek/sdk/docs/NT98690_PinMux_table_20231101-MainPinMux.csv` on the
  development host, and the board's own device tree is
  `/srv/novatek/sdk/worktrees/lamb-h1v1/output/nvt-evb.bin`.
- The vendor's own never-compiled recipe for this exact route is
  `BSP/u-boot/arch/arm/mach-novatek/nvt_ns02201_a64/soc.c:286-308`; the probe performs the same
  three field writes plus the reset and divider it never had to touch.
- The bench mode's write sequence is reproducible with no board:
  `python3 scripts/check/check-nt98690-boot.py --dry-run --uart-probe`.
- Every scenario added to `just nt98690_bench_probe_check` was confirmed to fail against a
  reverted fix; two were rewritten when that check showed they did not cover what they claimed.
