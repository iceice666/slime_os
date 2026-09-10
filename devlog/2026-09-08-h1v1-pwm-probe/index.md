# The H1V1 PWM probe: an ESC driven from the vendor prompt, and the pad found

| Field | Value |
|---|---|
| Date | 2026-09-08 |
| Kind | Audit |
| Status | Monitoring |
| Scope | `scripts/check/check-nt98690-boot.py`, `scripts/check/check-sel4-pins.py`, `sel4/pins.toml` |
| Work items | 01a07bac-a899-7eac-9575-62cb1ff23285, 01a07bac-83ad-7cfe-be0e-b589e74af74f |
| Gates | `just sel4_pin_check`, `just sel4_gate_control_check`, `just nt98690_bench_probe_check` |
| Trigger | [The lane decision](../2026-09-07-h1v1-esc-lane/index.md) and its bench probe, run by the operator on the named board |
| Baseline | Every PWM, pinmux, clock, and pad fact in the lane's plan was read from the vendor BSP; none had been observed on the board, and no source on the development host said which pad reaches a connector |

## Summary

The operator ran the bench probe on the named Novatek NT98690 H1V1 from its unmodified vendor
U-Boot. `--gpio-probe 0` toggled P_GPIO[0], and the operator found it on the board's 40-pin
GPIO header. `--pwm-probe` on channel 0 then programmed the clock, the period registers, and
the pad routing exactly as the plan's write list said it would; every register read back the
value it was given; a drone ESC on the pad armed at 1000 us and ran faster at 1600 than at 1200
(operator observation); and all four core-rail readings held before the first write. Two
predictions were confirmed against the board rather than the BSP: no firmware in the boot path
routes these channels, and the GPIO block does not report a pad held by another function. The
first ESC run was interrupted by the operator before its closing disable and reset; the reason —
a slow sampling loop whose reading the board had already answered — is fixed here, and a second
run with the fix then completed end to end: six pulse widths, the disable, every shared word
restored to its surveyed value, `reset`, and the vendor banner.

## Observable symptom

- Command: `python3 scripts/check/check-nt98690-boot.py --pwm-probe --pwm-channel 0 --pwm-pulse-us 1000,1600,1200,1000 --pwm-hold-seconds 10 --serial /dev/ttyUSB0 --transcript esc.log`, from the standalone bench bundle, ESC on P_GPIO[0] with common ground and its own battery, propellers off
- Expected: the survey reads, the core-rail invariants hold, channel 0 takes every programmed value and reads it back, the ESC follows the pulse widths, the channel is disabled and every shared word restored, and the vendor banner returns after `reset`
- Observed, first run: everything through the third pulse width; the transcript ends during that width's GPIO sampling, with no disable, no restore, no `reset`, and no closing banner
- Observed, second run (`--pwm-pulse-us 1000,1000,1000,1600,1200,1000 --pwm-hold-seconds 1`, sampling off): the whole sequence, ending in the vendor banner
- Exit/fault/serial evidence: [`esc-run.log`](esc-run.log) and [`esc-clean-run.log`](esc-clean-run.log) — the raw wire bytes; the probe's own `check … = ok` lines went to the operator's terminal and are not in the captures, so the register facts below are read from the `md.l` echoes in the transcripts themselves

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `--gpio-probe 0` toggled P_GPIO[0]; the operator found the pin on the 40-pin GPIO header (operator report; that run's transcript was not returned) | PWM0 is the channel; the lane's one go/no-go is settled |
| 2 | Survey at the prompt: TOP+0x18 = `0x00000000`, TOP+0xA8 = `0xffffffff`, TOP+0x1C = `0x00010000`, CG+0x30 = `0x01df01df`, CG+0x84 = `0x06000000`, CG+0x8C = `0x10400000`, CG+0xA4 = `0xffffffff`, PWM+0x100 = `0x00001000`, GPIO DIR = `0`, PAD+0x210 = `0x00000024` | The plan's prediction holds: no PWM channel 0–7 is routed at handoff and every P_GPIO is in GPIO mode, so the root must program both words. All four core-rail invariants hold: shared reset released, channel 12 clock on, channel 12 routed, channel 12 enabled. The P1 rail bit is clear: 3.3 V |
| 3 | Writes, in order: CG+0x84 → `0x06000001`; CG+0x30 → `0x01df0077`; PWM+0x104 = 1; PWM+0x00 = 0; PWM+0x04 = `0x0020e800`; PWM+0x230 = `0x004e0300`; TOP+0x18 → `0x1`; TOP+0xA8 → `0xfffffffe`; PWM+0x100 = 1 | Byte-for-byte the sequence `--dry-run` renders; every shared word changed only in channel 0's bits |
| 4 | Readbacks: PWM+0x100 = `0x00001001`; PWM+0x04 = `0x0020e800`; PWM+0x230 = `0x004e0300` | The block took every value; channel 0 enabled beside channel 12, which is untouched — the write-1-to-set reasoning held on hardware |
| 5 | Relatched 1000 (`0x0020e800`/`0x004e0300`), 1600 (`0x00204000`/`0x004e0600`), 1200 (`0x0020b000`/`0x004e0400`) with the load bit after each; ESC armed, ran fast, then slower (operator observation) | The encoding at 1 MHz and the free-running update path are what the driver will use; a standard drone ESC accepts 3.3 V logic on this pad without translation |
| 6 | GPIO DATA read `0xfffff000` in every sample across the 1000 and 1600 holds and the start of the 1200 hold — bit 0 never set | The GPIO block does not report a FUNCTION-muxed pad. The idea of the board sampling its own PWM output is closed |
| 7 | The transcript ends mid-sampling in the 1200 hold: one disable write in the whole run (the initial quiesce), no restore, no `reset`, no closing banner | The operator abandoned a run that each sample's serial round trip had stretched to minutes. The channel and routing were left live until the next power cycle. Fixed below |
| 8 | Second run, sampling off: identical survey and core-rail readings; the same enable sequence; six widths latched (1000 ×3, 1600, 1200, 1000); PWM+0x104 = 1 then PWM+0x100 read `0x00001000`; TOP+0xA8 → `0xffffffff`, TOP+0x18 → `0x0`, CG+0x30 → `0x01df01df`, CG+0x84 → `0x06000000`; core-rail readings re-checked; `reset`; `U-Boot 2021.10` banner. ESC armed during the idle widths, ran fast, then slower (operator observation) | The disable and restore paths work and put every shared word back exactly as found; the board returns to its own firmware afterwards. One second per width is enough for the speed steps once the ESC has had three at idle to arm |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4/pins.toml` | `pwm_channel`, `pwm_pad`, `pwm_pad_header`, `pwm_pinmux_at_prompt`, `pgpio_function_at_prompt`, `gpio_data_reflects_function_pad`, `pwm_probe_observed` pinned from steps 1, 2, and 6 | A later bring-up reads what the board reported rather than what the BSP implies |
| `scripts/check/check-sel4-pins.py` | The seven keys checked: channel 0–5, pad named for the channel, both pinmux words parse, the GPIO fact cannot be flipped back to `true` without contradicting this entry | — |
| `scripts/check/check-nt98690-boot.py` | `PWM_PROBE_SAMPLES` defaults to 0 and the sampling loop is skipped at 0; `--pwm-samples` stays available for a different pad group | A run holds each pulse width for its stated time and nothing more, so it reaches its own restore and reset |

## Verification

| Check | Result |
|---|---|
| `--pwm-probe` on the named board, steps 2–5 above | Direct; `esc-run.log` |
| ESC armed at 1000 us, ran faster at 1600 than 1200 | **Operator observation** — not carried by the transcript |
| P_GPIO[0] reaches the 40-pin GPIO header | **Operator observation** from `--gpio-probe 0`; that transcript was not returned |
| Channel disabled, shared words restored, banner after `reset` | Direct, second run; `esc-clean-run.log` lines 791–1030 in its CR-normalised form |
| ESC armed at idle, ran faster at 1600 than 1200, second run | **Operator observation** |
| `python3 scripts/check/check-nt98690-boot.py --dry-run` after the sampling change | Exit 0; the rendered sequence is unchanged |
| `just sel4_pin_check` with the seven observed keys | Exit 0 |
| `just sel4_gate_control_check` | Exit 0; `nt98690_boot` still pinned at 25 |
| `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Exit 0 |

## Open risks and follow-ups

- **The header pin position is unrecorded.** The operator reports the standard 40-pin GPIO
  header; which of its forty positions carries P_GPIO[0] was not written down. Harmless for the
  driver, which routes a pad, not a pin; worth adding to `pwm_pad_header` when known.
- **The GPIO-probe transcript was not returned**, so step 1 rests on the operator's word. The
  PWM run's own TOP+0xA8 reading (all GPIO) is consistent with the earlier toggle having been
  restored.
- Nothing here is a Slime claim. Slime has not driven this pad; the root's carve, bring-up, and
  the driver are the lane's later sessions.

## Artifacts and provenance

- [`esc-run.log`](esc-run.log) — raw wire capture of the ESC run, from the operator's bench
  bundle (`h1v1-pwm-probe.tar.gz`, built from commit `f9f4be9` of this branch). Unedited; the
  vendor loader and U-Boot banners at its head are the board's own power-on. SHA-256
  `8824bdc78acc9a1a777bf25c0b08aad17a1929866c653e872c6d3cca6b70797e`.
- [`esc-clean-run.log`](esc-clean-run.log) — raw wire capture of the second, complete run, from
  bundle v2 (built from commit `bfc2b7d`). Unedited; 13,432 bytes; SHA-256
  `00862eadd221a74af1d65bb723bc302e74b8bac1553b6a10471ecef46481909a`.
- The write list both runs were held to: `python3 scripts/check/check-nt98690-boot.py --dry-run`.
- The lane's plan of record and the vendor sources for every register:
  [`../2026-09-07-h1v1-esc-lane/plan.md`](../2026-09-07-h1v1-esc-lane/plan.md).

## Corrections

**2026-09-09 — the clean-run readback claim above is too broad.** The immutable
[`esc-clean-run.log`](esc-clean-run.log) directly records the initial PERIOD and EXT_PERIOD
readbacks, the later pulse-update write commands, the final DISABLE followed by an ENABLE-state
readback showing channel 0 off, the shared-setting restoration write commands, and the returning
U-Boot banner. It does **not** contain a post-write readback for every pulse update or for every
shared clock/pinmux restoration. Therefore the Summary's “every register read back the value it
was given”, Investigation step 8's “put every shared word back exactly as found”, and Verification's
“shared words restored” are not all established by those raw bytes. The historical board run still
establishes only what the log records plus the separately labelled operator observations.

The bench probe now verifies each stable R/W clock, divider, pinmux, control, PERIOD, and EXT_PERIOD
setting after writing; verifies target disable through the live ENABLE register; verifies each
applicable shared-field restoration independently; preserves a primary failure alongside cleanup
failures; and reports an unconfirmed disable as a possible live output requiring manual actuator
power removal or board handling. `PWM_LOAD` is self-clearing, so the checker does not fabricate a
LOAD readback claim: register readback still does not prove latch acceptance, waveform, actuator
response, or physical stop. The 2026-09-09 evidence is the host-only production-function regression
`just nt98690_bench_probe_check`; no revised `--pwm-probe` or `--gpio-probe` board run was performed.
The original UART logs remain byte-for-byte unchanged. IO8 and P6.D remain outside this correction
and are not completed by host tests.

**2026-09-09 — three review findings on the bench probe and its pins, fixed here.** All three are
host-observable; none changes what either board run established.

- *The GPIO probe returned a pad driven low, not as found.* Every `--gpio-probe` cycle ends on
  `GPIO_P_CLR`, and cleanup restored only direction and the pad function, so a pad that arrived as
  a GPIO output driven high left the probe driven low — a different electrical state than the
  survey, under a function that documents verified restoration. `gpio_probe` now snapshots
  `GPIO_P_DATA` beside direction and function, and restores the latch *before* direction, verifying
  the pad reads the restored level while the pin is still an output. A pad found as an input has no
  observable latch in this register and is never driven. Neither board run is believed to have taken
  the affected path: the PWM run's survey read `GPIO DIR = 0` after the earlier `--gpio-probe 0`
  had restored it, which under the old cleanup's `dir_before & bit` restore implies the pad was an
  input before that probe too — an inference, since the GPIO-probe transcript was not returned.
- *`sel4_pin_check` accepted any self-consistent divider.* The ratio check passed divider 239 with
  500 kHz, while `pwm_probe` hardcoded divider 119 and wrote `--pwm-period-us` and `--pwm-pulse-us`
  into the counter unconverted — so such a pins edit would have halved every pulse width silently
  for later consumers. The checker now requires divider 119 and exactly 1 MHz, and the probe's
  divider is the named `PWM_CLOCK_DIVIDER` that `check_pwm_pins` asserts against the pins along with
  the source and count clock, so the two sides can no longer drift.
- *Two CLI rejection cases proved nothing.* `channel_allowlist` built each invalid argument tuple
  but never installed it in `sys.argv`, so `main()` exited on the missing serial endpoint and that
  unrelated `SystemExit` was accepted as channel rejection. The cases now install the argv, supply
  `--serial`, require the failure text to name the channel bound, and raise a dedicated
  `BoardOpened` — not `SystemExit` — if the CLI opens a port before validating.

Evidence: `just nt98690_bench_probe_check` (14 scenarios, exit 0), whose three new cases
(`gpio_output_latch_restored`, `gpio_input_latch_untouched`, `pins_bind_the_count_scale`) were each
confirmed to fail against a reverted fix; `just sel4_pin_check` exit 0; and
`python3 scripts/check/check-nt98690-boot.py --dry-run` exit 0, its rendered write list unchanged
apart from now naming the divider from the constant. No board run was performed for this
correction, so nothing here is board evidence, and the exit conditions the bench milestone records
remain what the immutable logs and the labelled operator observations establish.

**2026-09-09 — the bench milestone is reopened, and three follow-up findings are fixed.**

`01a07bac-a899-7eac-9575-62cb1ff23285` (`P6.PWM.A`) is back to `open`. Its exit conditions require
that the probe "restores every register it changed", and the correction above already established
that `esc-clean-run.log` carries the four shared restoration *write commands* but no post-write
readback for any of them. The item was therefore closed on a condition nobody observed. The
2026-09-09 remediation makes the probe verify each restoration independently, so a future
transcript will carry that evidence — but a host model proves what the code does, not what the
board did, and cannot substitute. Closing it needs one further `--pwm-probe` run on the named board
whose transcript shows a readback for every shared restoration; the rest of the exit conditions are
already observed and that run does not re-litigate them. The item's own body now says this.

Three further findings, all host-observable:

- *A GPIO toggle was reported without confirming the pad moved.* `send_command` only established
  that U-Boot returned to its prompt, so an ignored SET or CLR — or a pad that cannot attain the
  level — still printed a successful cycle. That is the one failure mode `--gpio-probe` exists to
  rule out: a false toggle would have the operator strike a reachable header pin off the list. Each
  cycle now goes through `drive_gpio_level`, which reads back the target bit in `GPIO_P_DATA` after
  every SET and CLR, and the latch restoration shares that one verified path. Note the earlier
  finding's asymmetry is unchanged and deliberate: a pad found as an *input* is still never driven,
  since there is no latch to restore there.
- *`sel4_pin_check` accepted an unobserved PWM route.* The channel was validated as a `0..5` range
  with `pwm_pad` merely required to agree, so an edit to channel 1 and `P_GPIO1` passed while
  `pwm_pad_header` and `pwm_probe_observed` still described the channel-0 experiment — an
  unqualified route a later bring-up would then consume as observed fact. The checker now holds a
  table of routes some run actually qualified (channel 0 → `P_GPIO0`, 40-pin GPIO header,
  2026-09-08) and validates the pad, header, and date together against the entry. Channels 1-5
  remain safe for the bench probe to *drive* — that is the probe's own `PWM_MAX_PROBE_CHANNEL`, a
  different claim — but adding one here means adding its run's evidence, not widening a range.
- *A test comment narrated its own history.* The `channel_allowlist` comment recorded the two prior
  implementation mistakes, which belongs in this file and not in verification code. It now states
  only the invariant: the rejection must name the channel bound and must happen before `Console` is
  constructed.

Evidence: `just nt98690_bench_probe_check` (15 scenarios, exit 0), with the new
`gpio_dead_pad_detected` case — a model that drops the first SET must fail, stop driving further
levels, and still restore direction and pad function. Removing the per-level readback fails it;
repinning `sel4/pins.toml` to channel 1 with a matching `P_GPIO1` now fails `just sel4_pin_check`
with the unobserved-route message. Also `just tasks_check`, `just ruff`, `just typos`, and
`--dry-run`, all exit 0. Again no board run, so nothing here is board evidence.

**2026-09-10 — the header pin position, from the board's pinout diagram.** The run above left
P_GPIO[0]'s position on the connector unwritten, and no schematic, pinout table, or header note
existed anywhere on the development host. The operator has since supplied the board's 40-pin
header diagram, kept beside this entry as [`header-pinout.png`](header-pinout.png). It places
**P_GPIO[0] at pin 26**, and its alternate-function labels agree with the vendor pinmux table on
every pad that could be checked (the SPI2_2 and SPI_1 groups; P_GPIO[0] carrying PWM0), which is
what makes it this board's document rather than a look-alike. `pwm_pad_header` now says pin 26,
and the whole pad-to-pin map is pinned as `header_pins` so no later lane re-measures it.

The position was first recorded from memory earlier the same day, withdrawn when that memory was
doubted, and is now recorded from the document; the two earlier commits stand in the history as
the reason this one cites a source. The diagram settles one more thing the plan for a second lane
had assumed: P_GPIO[4] and P_GPIO[5] are **not** on this header, so UART8's data pins reach no
connector, and only its flow-control pins do. That lane's own entry records the consequence.

