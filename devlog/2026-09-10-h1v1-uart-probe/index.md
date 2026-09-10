# The H1V1 UART7 probe: heartbeats decoded on the ground radio, and the pad placed

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Audit |
| Status | Verified |
| Scope | `scripts/check/check-nt98690-boot.py`, `scripts/check/check-nt98690-bench-probe.py`, `scripts/check/check-sel4-pins.py`, `sel4/pins.toml` |
| Work items | 01a08ac5-14a7-757b-a456-c91868d651c8, 01a08ac5-1496-7988-a11a-bb5a3744456c |
| Gates | `just nt98690_bench_probe_check`, `just sel4_pin_check` |
| Trigger | [The lane decision](../2026-09-10-h1v1-mavlink-lane/index.md) and its bench probe, run by the operator on the named board |
| Baseline | Every UART7 register, clock, and pinmux fact in the lane's plan was read from the vendor BSP; the pad's header position came from the board's pinout diagram; neither had been observed on the board, and no Slime-side code had ever transmitted on a serial port |

## Summary

The operator ran the bench probe on the named Novatek NT98690 H1V1 from its unmodified vendor
U-Boot, with the air radio wired to header pin 13 and a second RFD900x on the development host.
The first run stopped at line configuration: the probe required the interrupt-identity register
to read the generic 16550A's `0xC1` after enabling the FIFOs, and this UART answers `0x81`.
The check was loosened to the bit that matters and the second run went the distance: the
survey read exactly what the plan predicted, the port took its divisor and line settings and
read them back, ten MAVLink heartbeats went out a byte at a time and all ten were decoded on the
ground radio with valid checksums and consecutive sequence numbers, every shared word the probe
had changed was written back and read back, and the vendor banner returned after `reset`. The
210-byte radio capture is byte-identical to what the encoder produces for sequences 0 through 9.

The pad's position on the connector is settled by that run's wiring rather than by a meter:
the frames reached the radio through a wire the operator reports was on pin 13. The meter step
the bench sheet asked for was not performed. That is recorded below rather than smoothed over.

## Observable symptom

- Command: `python3 scripts/check/check-nt98690-boot.py --serial /dev/ttyUSB0 --uart-probe --uart-receiver /dev/ttyUSB1 --transcript uart-probe.log --uart-receiver-capture radio-capture.bin`, from the standalone bench bundle built at `3148e1c3`; air radio's RX on header pin 13, its GND on pin 14, its 5 V from a bench supply; ground radio on `/dev/ttyUSB1`
- Expected: the survey reads, the core-rail invariants hold, UART7 takes every programmed value and reads it back, each of ten heartbeats is decoded on the ground radio within its window, every shared word is restored with readback, and the vendor banner returns after `reset`
- Observed, first run (bundle at `f513e437`): everything through the FIFO enable, then `register verification failed at 0x2f0136008: expected 0x000000c1, actual 0x00000081`; all five restorations verified, core rail re-checked, banner returned; `frames_sent=0`
- Observed, second run: the whole sequence; `frames_sent=10 frames_decoded=10 crc_failures=0 other_msgids=0 settings_restored=True firmware_banner=True`
- Exit/fault/serial evidence: [`uart-probe.log`](uart-probe.log) (the second run's raw UART0 bytes; the first run's transcript was written to the same name and overwritten by the second, so it survives only in the operator's terminal, quoted in the lane record), [`uart-probe-stdout.log`](uart-probe-stdout.log) (the probe's own lines, as pasted by the operator), [`radio-capture.bin`](radio-capture.bin) and its decode [`radio-capture.log`](radio-capture.log), [`gpio-probe-8.log`](gpio-probe-8.log), [`ground-radio-ati5.txt`](ground-radio-ati5.txt)

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | First run: survey `CG+0x60` = `0x09090909`, `CG+0x7C` = `0x00007060`, `CG+0x9C` = `0xffffffb7`, `TOP+0x34` = `0x00000001`, `TOP+0x38` = `0`, `TOP+0xA8` = `0xffffffff`, `PAD+0x08` = `0xaa555555`, `PAD+0x210` = `0x00000024`; all four core-rail readings held; port `LCR/LSR/IER/MCR/IIR` = `0/0x60/0/0/0x01` | Every prediction in the plan's A2 held: every UART divider field already 9 (48 MHz), UART7 gated with its reset released, nothing routed, every pad in GPIO mode, the pull-down on P_GPIO[8] at the default, the rail at 3.3 V. The "unknown value in the divider field" risk is retired: the loader sets them all |
| 2 | First run: after `FCR = 0x07`, `IIR` read `0x81`; the probe required `0xC1` and stopped before writing a byte. Cleanup restored the pad word, the mux, and the gate with readback, verified the unchanged divider and reset, re-checked the rail, `reset`, banner | The generic 16550A encoding is not this UART's: bit 7 alone is what it reports, the pattern Linux's port classifier files under `16550`. Neither U-Boot nor Linux reads the register for this port. The probe now requires bit 7 only (`3148e1c3`). Incidentally, the cleanup-after-failure path the PWM lane's reopened milestone is waiting to see was exercised here first |
| 3 | Second run: identical survey; `IIR` `0x81` accepted; `DLL` read back `0x34`; `LCR` `0x03` | The port holds divisor 52 at 8N1 |
| 4 | Second run: 210 writes to the transmit register, the first six `34 fd 09 00 00 00` (the divisor load, then the pinned frame's opening bytes); 32 line-status reads, 31 of them `0x60`, one `0x79` immediately after the pad-function write | Ten frames of 21 bytes, two THRE polls and one TEMT poll each. The single `0x79` is break, framing-error, and data-ready together: the instant P_GPIO[8] was routed to the UART, its receive input -- P_GPIO[9] still a GPIO with its pull-down -- read as a held-low line. Harmless to transmit, and the reason the probe masks to the two transmitter bits; the first run's clean `0x60` had suggested the mask was unnecessary |
| 5 | Ground radio: ten frames decoded, `seq` 0..9, `crc_ok` on every one; the capture is 210 bytes and equals `encode_heartbeat(0..9)` byte for byte; no other message id arrived | The pad transmits at the programmed rate and the far radio receives it. The radio injected no status frame of its own during the two-minute run, so the decoder's pass-through of unverified frames is exercised only by the host model, not yet by the wire |
| 6 | Second run cleanup: `TOP+0xA8` → `0xffffffff`, `TOP+0x34` → `0x1`, `CG+0x7C` → `0x7060`, each read back; divider and reset verified unchanged; rail re-checked; `reset`; `U-Boot 2021.10` | Every word restored to its surveyed value, and observed to be |
| 7 | `--gpio-probe 8` was refused by the bench bundle: `channel 8 is outside 0..5`. The operator applied a one-line sed to the copy and ran it: 20 SET and 20 CLR writes to bit 8, the data register reading the requested level after each; direction restored; the function word untouched because the pad was already GPIO. **No meter was on the header during this run** | The GPIO probe had borrowed the PWM probe's channel list, a bound about which channels sit off channel 12's divider and nothing to do with a plain output; fixed in `4da5a0f3`, bounded now by the header map. The transcript is pad-level evidence: the pad toggles and reads back. It says nothing about the connector |
| 8 | `ATI5` on the ground radio: `SERIAL_SPEED=57`, `AIR_SPEED=64`, `NETID=25`, `TXPOWER=10`, `MAVLINK=1`, `RTSCTS=0`, 920–925 MHz | The pair's configuration is what the plan assumed. Transmit power at 10 dBm on the bench, well under the current the plan budgeted for |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4/pins.toml` | `uart7_pad`, `uart7_pad_header`, `uart7_clock_at_prompt`, `uart7_mux_at_prompt`, `uart7_probe_observed` pinned from steps 1, 3, and 5; the comment says the header position is the decoded run's wiring, not a meter reading | A later bring-up reads what the board reported rather than what the BSP implies |
| `scripts/check/check-sel4-pins.py` | `observed_uart_routes` gains UART7 on P_GPIO[8] at pin 13, dated; the header string must agree with `header_pins`; the two survey tables parse as hex | The route cannot be repinned to a pad no run has driven, and the map and the route cannot disagree |
| `scripts/check/check-nt98690-boot.py` | `UART_IIR_FIFO_BIT = 0x80` replaces the datasheet's `0xC1`, with the board's answer recorded beside it (`3148e1c3`); `validate_gpio_pad` bounds the GPIO probe by the header map and the first bank word (`4da5a0f3`) | A check is as strict as the hardware, not stricter; a pad that is on the connector can be probed and one that is not cannot |
| `scripts/check/check-nt98690-bench-probe.py` | The 16550 model answers `0x81`; `channel_allowlist` proves both halves of the new bound, including that a wrong header map could not make the core-rail pad drivable | — |

## Verification

| Check | Result |
|---|---|
| `--uart-probe` with the ground radio, second run, steps 3–6 | Direct; `uart-probe.log`, `radio-capture.bin` |
| Ten heartbeats decoded, sequence consecutive, checksums valid, capture identical to the encoder | Direct; `radio-capture.log` regenerated from the immutable capture |
| Air radio's RX on header pin 13, GND on pin 14 | **Operator report** |
| Pin 13 alternates during `--gpio-probe 8` | **Not observed**: no meter was used. `gpio-probe-8.log` establishes the pad-level toggles only |
| Every shared word restored and read back; banner after `reset` | Direct, both runs; `uart-probe.log` for the second |
| First run's IIR reading and cleanup | Operator's terminal, quoted in the lane record; that run's raw transcript was overwritten |
| `ATI5` on the ground radio | **Operator report**; `ground-radio-ati5.txt` |
| `just nt98690_bench_probe_check` (40 scenarios) | Exit 0; each scenario confirmed to fail against its reverted fix |
| `just sel4_pin_check` with the five observed keys | Exit 0 |
| `just sel4_gate_control_check` | Exit 0; 48 gates, `nt98690_boot` still 25 -- a bench mode has no marker contract |
| `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Exit 0 |

## Open risks and follow-ups

- **The connector position of P_GPIO[8] has one witness, the wiring.** The frames arrived
  through the wire the operator placed on pin 13, and the diagram agrees, but the meter reading
  the bench sheet asked for was skipped. If a later session finds anything odd about that pin,
  this is the first thing to redo: forty seconds with a meter on pin 13 during `--gpio-probe 8`.
- **The decoder's handling of the radio's own frames is model-proven only.** No status frame
  was injected during the run, so pass-through of an unverified frame has not been seen on the
  wire. A longer listen would show one.
- **The FIFO is enabled but its usability is unknown.** `IIR` bit 7 alone is what Linux would
  call a 16550 whose FIFO it does not trust. Nothing in this lane depends on the FIFO; the
  later driver polls the holding register per byte regardless.
- **The first run's transcript is gone.** Both runs wrote `uart-probe.log`. The bench sheet
  should name the transcript per run; the operator's pasted terminal output is what survives.
- **The receive input floats low when the transmit pad is routed.** One line-status read
  showed a break. A lane that ever receives on this port must route P_GPIO[9] before enabling
  the receiver, or it will start with a break condition latched.

## Artifacts and provenance

- [`uart-probe.log`](uart-probe.log) -- the second run's raw UART0 bytes; sha256 `04390888807eba9cde450b7e8419dffb22492fb7cb390c7c9b73b7216b4066ce`
- [`uart-probe-stdout.log`](uart-probe-stdout.log) -- the probe's own lines for the second run, pasted by the operator; not a wire capture
- [`radio-capture.bin`](radio-capture.bin) -- the ground radio's raw bytes, 210 of them; sha256 `a39ee2729ec28e75822637c31e3d66bed0c4db4730211fb312b21f4dbbeff51e`
- [`radio-capture.log`](radio-capture.log) -- decoded from the capture by `scripts/lib/mavlink.py`; regenerable, never edited
- [`gpio-probe-8.log`](gpio-probe-8.log) -- the GPIO probe's raw UART0 bytes, run with the operator's sed on the allowlist; sha256 `29174ee12c12beae7a65c862a4a109818c03b3ab437e37754337cce93043f80a`
- [`ground-radio-ati5.txt`](ground-radio-ati5.txt) -- the ground radio's parameters, pasted by the operator
- The bench bundle was built from `3148e1c3` for the passing run and `f513e437` for the first; the probe's write sequence for either is reproducible with no board: `python3 scripts/check/check-nt98690-boot.py --dry-run --uart-probe`
