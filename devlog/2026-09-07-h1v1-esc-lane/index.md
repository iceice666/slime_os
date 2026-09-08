# Driving a motor ESC from the H1V1: opening the lane and its bench probe

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Kind | Decision |
| Status | Proposed |
| Scope | `scripts/check/check-nt98690-boot.py`, `scripts/check/check-sel4-pins.py`, `sel4/pins.toml`, `.tasks/items/` |
| Work items | 01a07bac-83ad-7cfe-be0e-b589e74af74f, 01a07bac-a899-7eac-9575-62cb1ff23285, 01a07bac-acf5-7574-b0f5-d80c0fde6cd9, 01a07bac-cbe6-7051-9da4-d2a1d223dfee |
| Gates | `just sel4_pin_check`, `just sel4_gate_control_check` |
| Trigger | The H1V1 answers typed Slisp input over UART0 (P6.C), and the next thing asked of it is to move something physical |
| Baseline | Nothing in the repository drives a pad: no PWM, no GPIO, and an actuator component that is a simulated call-plane server |

## Summary

This opens a lane to drive a hobby drone ESC with servo PWM from the resident Slisp prompt on
the named Novatek NT98690 H1V1, and lands the first session's half of it: the bench modes that
answer, from the vendor U-Boot prompt, the hardware questions Slime's own bring-up would
otherwise have to assume. No Slime code drives a pad yet, and no board run has happened, so this
entry is `Proposed`; the probe's own result will be its own `Audit` entry.

The lane is cut into three sessions, and the cut is the main decision here: a board-specific
bench probe, then a board-neutral mechanism and protocol observed only under QEMU, then the
board-specific bring-up and gate. The middle session names no SoC, so the general half is
reviewable and upstreamable without the H1V1 riding along in it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check/check-nt98690-boot.py` | Two bench modes beside `--reset-probe`: `--gpio-probe` toggles one pad as a plain output slowly enough to find with a meter, and `--pwm-probe` drives servo PWM on one channel, holding each pulse width long enough to watch a servo or ESC respond. Neither asserts a marker contract, and neither gets a `just` recipe. | A hardware fact a later gate depends on is observed before it is depended on |
| `scripts/check/check-nt98690-boot.py` | Pure encoders — `pwm_period_words`, `cg_divider_word`, `pinmux_words`, the offset helpers — and `--dry-run`, which renders the probe's whole write sequence from those same helpers with no board attached | A sequence that writes registers shared with the CPU's power supply is reviewable before it is run |
| `scripts/check/check-nt98690-boot.py` | `CORE_RAIL_INVARIANTS` and `check_core_rail`, asserted before the first write and after the last; every clock-generator and pinmux write goes through `read_modify_write` | PWM channel 12 keeps driving the CPU core voltage |
| `sel4/pins.toml`, `scripts/check/check-sel4-pins.py` | The PWM, TOP, GPIO, and pad block bases, and the clock arithmetic that makes one PWM count one microsecond, pinned and checked | A divider edit that silently rescales every pulse width fails a check rather than a servo |
| `.tasks/items/` | The lane epic, the bench milestone, an IO-track milestone for the mechanism, and P6.D for the board evidence, with the dependency edges between them | — |

## Decisions

**The lane is cut general-versus-board, not just by session.** The mechanism a userspace PWM
driver needs — a device the generation *declares* rather than one the root discovers by probing
the virtio window — is missing from the IO substrate for every platform, not just this one. It
lands in its own session with QEMU evidence and no `ns02201` anywhere in it, so the H1V1's
carve order, pinmux, and clock divider stay in the session that observes them.

**The bench modes live in the existing checker, and a later gate will not.** Same execution
environment, same `md.l`/`mw.l` protocol, same prompt, and no marker contract: the verification
discipline puts them beside `--reset-probe`, which is the precedent they are modelled on. The
board gate that ends the lane is a different question — its own image, its own session, its own
ordered marker contract — and the shared tamper control pins one contract per module, so it will
be its own checker.

**The core rail is held by construction, not by care.** This SoC has no separate regulator
controller: PWM channel 12 drives the CPU core voltage through P_GPIO[42], programmed by U-Boot
during `otp_init`, and the board runs on what it outputs. Its clock gate, its divider, its pad
mux, and the PWM reset *shared by every channel* therefore sit in the same registers a servo
channel needs. Three of those words are never written; the rest are read-modify-write on one
channel's bits; the enable, disable, and load registers are write-1-to-set, so other channels are
unreachable through them. Four readings are asserted before the first write and after the last,
and the probe ends by resetting the board and requiring the firmware banner.

**The pad is a bench fact, and the plan says so.** The vendor pinmux table the operator supplied
settles what each pad *can* be — PWM0 through PWM5 reach P_GPIO[0..5], which are 3.3 V and which
nothing in this board's configuration claims — but no source on this host says which of them
reaches a connector. The channel, pad, and header pin are therefore absent from the pins until
the probe reports them, exactly as `reset_write_width` is.

**Evidence class is stated up front.** A motor arming and changing speed is not serial-observable.
The exit conditions rest on what the transcript can carry — the block reporting back the values
it was given, and the firmware returning afterwards — and the motion is recorded as an operator
observation. The repository has no precedent for an instrument reading, and this lane does not
introduce one.

## Open risks and follow-ups

- **Pad reachability is unresolved and is the lane's one go/no-go.** If no P_GPIO[0..5] reaches a
  connector, the fallbacks are D_GPIO[2..3] (PWM4 and PWM5's second pad function, also 3.3 V) or
  a soldered lead; the DSI_GPIO alternates are 1.8 V and belong to the display interface.
- **Whether the GPIO block reports a pad held by another function is unknown.** The probe samples
  it and reports the count without asserting it. If it does report, a later session could let the
  board observe its own output; if it does not, that idea is closed.
- **The driver's page will carry more than its channel.** A 4 KiB grant of the PWM block includes
  channels 8 through 12 and the shared enable words, and no capability bound expresses a
  per-channel restriction. That is driver policy, and it will be recorded as a known limit rather
  than claimed as containment.
- The probe has not been run. Nothing here is observed evidence about this board.

## Artifacts and provenance

- [`plan.md`](plan.md) — the lane's plan of record: the durable board and repository facts with a
  source for each, the fixed names, the reuse map, the session split, and the risks. Kept beside
  this entry so a later session re-verifies one line instead of re-exploring the vendor BSP.
- Vendor sources for every register fact are cited inline in `plan.md`; the SoC pinmux table the
  operator supplied is at `/srv/novatek/sdk/docs/NT98690_PinMux_table_20231101-MainPinMux.csv` on
  the development host.
- The bench modes' write sequence is reproducible with no board:
  `python3 scripts/check/check-nt98690-boot.py --dry-run`.

## Corrections

- **2026-09-08** — The pad is settled: PWM0 on P_GPIO[0], reached on the board's 40-pin GPIO
  header, and a drone ESC on it responded. The lane's "one go/no-go" above is answered. The
  pinmux words at handoff read exactly as A2 predicted (no routing; every P_GPIO in GPIO mode),
  and the GPIO block does not report a FUNCTION-muxed pad, so the "board samples its own output"
  follow-up is closed rather than deferred. Evidence and the seven pinned facts:
  [the probe's audit entry](../2026-09-08-h1v1-pwm-probe/index.md).
