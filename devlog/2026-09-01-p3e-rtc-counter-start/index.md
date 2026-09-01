# P3.E left the CV1800B RTC seconds counter stopped

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/platform_timer.rs`, Milk-V Duo physical timer path |
| Roadmap | P3.E |
| Gates | `just duo_sel4_check` |
| Trigger | The rebuilt A/D-corrected physical image booted upstream seL4 and entered `slime-root`, then emitted no byte after acquiring RTC IRQ 17 |
| Baseline | The first physical image had stopped at the loader page-table switch, so the RTC path had never executed on the board |

## Summary

The first image rebuilt with eager RISC-V A/D bits crossed the loader transition, booted upstream seL4, entered `slime-root`, mapped the CV1800B RTC, and acquired PLIC IRQ 17, then stalled before either timer delivery or the three-second bounded diagnostic. `program_cv1800b_rtc` programmed the alarm but omitted the vendor driver's RTC initialization: clearing the external-pulse selector in both `SEC_PULSE_GEN` and `ANA_CALIB`. The seconds counter therefore did not advance, so neither the absolute alarm nor the timeout counter could make progress. The adapter now performs that read-modify-write before arming an alarm; the final four-boot physical campaign observed startup and post-graph IRQ delivery, the bounded early-fault path, and autonomous cold recovery after every boot.

## Observable symptom

- Command: `python3 scripts/check/check-duo-sel4.py --serial /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0 --evidence-dir devlog/2026-08-31-p3e-sel4-milkv-duo`
- Expected: a startup timer notification, generation admission, component evidence, and autonomous cold reset.
- Observed: the last line after 180 seconds was `SLIME_TIMER acquired irq=17 freq_hz=1`; the expected three-second timeout also did not print.
- Exit/fault/serial evidence: exit 1; [`timer-counter-stall.log`](timer-counter-stall.log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `SLIME_DUO loader page tables active`, upstream seL4 boot, root entry, device mapping, and `IRQControl_GetTrigger(17)` all completed | The previous A/D defect is physically fixed; this failure is after the root owns the RTC and IRQ |
| 2 | Neither the alarm notification nor the bounded timeout fired in 180 seconds | The shared `SEC_CNTR_VALUE` time source was not advancing, rather than only the PLIC notification being miswired |
| 3 | The CVITEK vendor driver calls `rtc_enable_sec_counter` during probe, clearing bit 31 in both `SEC_PULSE_GEN` and `ANA_CALIB`, before it uses the same alarm sequence | Slime copied `set_alarm` but omitted the block initialization that makes its counter advance |
| 4 | The low bits of both registers contain calibration state | The repair must be a read-modify-write of bit 31, not a literal register replacement |

## Root cause

`program_cv1800b_rtc` assumed firmware handed off a running internal seconds source. The board's vendor path does not make that an ABI guarantee: its Linux driver explicitly clears the external-pulse selector in `SEC_PULSE_GEN` and `ANA_CALIB` during probe. Without those writes, `SEC_CNTR_VALUE` remained constant. Slime then expressed both the deadline and its failure bound in that stopped domain, making the intended fail-fast path unable to fail.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| CV1800B timer programming | Read `SEC_PULSE_GEN` and `ANA_CALIB`, clear only bit 31 in each, then run the existing bounded settle and alarm sequence | The timer's monotonic source advances before any deadline or timeout depends on it |
| Register preservation | Retain every low calibration bit in both controls | Root startup selects the internal pulse without discarding firmware or factory calibration |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The physical counter remains stopped | `just duo_sel4_check /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` | No startup `SLIME_TIMER delivered` marker or no bounded timeout |
| The alarm or PLIC path remains broken after the counter starts | `just duo_sel4_check /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` | Counter advances but the ordered notification marker is absent |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Physical pre-fix rebuilt image | Booted loader, upstream seL4, and root; stalled after acquiring IRQ 17 for 180 seconds | Direct; [`timer-counter-stall.log`](timer-counter-stall.log) |
| Corrected sample and early-fault FIT builds | Passed; artifacts bind the repaired root image | Direct |
| Rebuilt physical campaign | Passed: three sample boots delivered startup and post-graph timer IRQs, reached ready, and cold-reset; the fourth boot emitted the bounded early-fault diagnostic and cold-reset | Direct; [`devlog/2026-08-31-p3e-sel4-milkv-duo/`](../2026-08-31-p3e-sel4-milkv-duo/index.md) |

## Decisions

- Decision: initialize the RTC source in `program_cv1800b_rtc`, immediately before the first alarm.
- Rationale: this is the first operation requiring the seconds domain, is idempotent, and keeps the board-specific ownership inside the existing timer adapter.
- Rejected alternative: use `rdtime` for Slime deadlines while leaving the RTC stopped. That would make the timeout work but would not repair the absolute RTC alarm that owns IRQ 17 and cold-reset timing.

## Open risks and follow-ups

- [x] The power-cycled board completed the four-boot `just duo_sel4_check` campaign.
- [x] Physical startup and post-graph timer markers passed; the stopped-counter failure no longer reproduces.

## Artifacts and provenance

- Focused report: this entry.
- Raw pre-fix transcript: [`timer-counter-stall.log`](timer-counter-stall.log).
- Passing physical transcripts and normalized evidence: [`devlog/2026-08-31-p3e-sel4-milkv-duo/`](../2026-08-31-p3e-sel4-milkv-duo/index.md).
- Related roadmap item: [P3.E](../../roadmap/07-architecture-portability.md#p3e--sel4-on-the-milk-v-duo).
