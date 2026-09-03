# P6.A — the H1V1 handoff probe, its builder, and its gate

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Change |
| Status | Verified |
| Scope | `tools/nt98690/payload/{probe.S,probe.ld}`, `scripts/build/build-nt98690-payload.py`, `scripts/lib/{arm64_image,uboot_console}.py`, `scripts/check/check-nt98690-boot.py`, `scripts/check/check-sel4-gate-controls.py`, `sel4/pins.toml`, `just/hardware.just` |
| Roadmap | P6.A |
| Gates | `just nt98690_payload_check`, `just nt98690_boot_check`, `just sel4_gate_control_check` |
| Trigger | [The P6 lane decision](../2026-09-01-p6-nt98690-h1v1-lane/index.md) |
| Baseline | No NT98690 payload, builder, gate, or board profile existed; `sel4_gate_control_check` covered 45 gates |

## Summary

This lands everything P6.A needs except the board: a 5703-byte AArch64 probe that
measures the H1V1's firmware handoff, a deterministic builder that cannot emit an
image whose header disagrees with where it is linked, a fail-closed serial gate
that drives the unmodified vendor U-Boot over a tty or a TCP bridge, and that
gate's registration in the shared tamper control. The probe has been executed —
under QEMU, retargeted from the same source — so the instruction stream, the
register decoding, and the verdict logic are known to work rather than assumed to.

**P6.A's exit condition is not met.** No H1V1 has been booted. Everything below is
either host-verified or inherited from source reading; the board's own facts are
what the gate exists to collect and they do not exist yet. Status is Proposed, and
becomes Verified when a transcript is recorded beside this entry.

The QEMU run was not ceremony. It found a real defect: `putc` scratches `x9`, and
the GIC read block parked the distributor base there across the prints between its
reads, so `gicd_typer` and `gicd_iidr` were loads from a line-status value. They
printed as zeroes while the verdict line — which happened to re-derive the base —
still said `ok`. That combination is exactly what a bench session cannot debug: a
plausible transcript with two wrong numbers and a passing check.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `tools/nt98690/payload/probe.S` | AArch64 probe: arm64 `Image` header, EL/ID/timer/GIC/device-tree reads, a counter calibration against the line rate, six literal `check … = ok` verdicts, fault vectors, PSCI reset exit | The handoff is measured, and the measurement can be read in full |
| `tools/nt98690/payload/probe.ld` | Links at `0x10000000` and derives `text_offset`/`image_size` from that one symbol | The header cannot disagree with the link address |
| `scripts/build/build-nt98690-payload.py` | Deterministic build; checks the linker base against the pins, the address against the vendor memory map, and the emitted header against both | An image that would run from the wrong address cannot be built |
| `scripts/lib/arm64_image.py` | The arm64 `Image` header as a parseable format, with `decode_branch` | A format this repository emits can be read back and asserted |
| `scripts/lib/uboot_console.py` | Console over tty or `tcp:HOST:PORT`, autoboot capture, command/prompt round-trips | Physical gates fail closed on every absence; a bridged console reports framing evidence as unobservable rather than as zero |
| `scripts/check/check-nt98690-boot.py` | The P6.A gate: identity, slot probe, device-tree probe, byte comparison, launch, recovery, 25 ordered markers, 14 failure markers | Board claims rest on ordered observed evidence or fail |
| `scripts/check/check-sel4-gate-controls.py` | Registered `nt98690_boot` with its marker count pinned at 25 | A physical marker chain is proven to reject deleted, transposed, and failure-marked evidence |
| `sel4/pins.toml` | `[ns02201_h1v1]`, board facts only, each with its vendor source named | Board facts are sourced, never asserted |
| `just/hardware.just` | `nt98690_payload_check`, `nt98690_boot_check`, `nt98690_serial_monitor` | Each recipe says whether it qualifies hardware or only aids bring-up |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A marker is deleted, reordered, or contradicted, weakening the chain | `just sel4_gate_control_check` | The mutated transcript is accepted, or the pinned count of 25 no longer matches |
| The linker base, the pinned load address, and the header's `text_offset` drift apart | `just nt98690_payload_check` | Named mismatch naming both values |
| The load address moves onto vendor firmware, the CMA pools, or U-Boot | `just nt98690_payload_check` | Named region and its bounds |
| The header stops being executable — data, ELF magic, or a branch off the end | `just nt98690_payload_check` (`decode_branch`) | `first word … is not an unconditional b` |
| `booti` relocates the image away from the pinned address | `just nt98690_boot_check <serial>` | `Moving Image from` as a failure marker |
| A card carries a previous build | `just nt98690_boot_check <serial>` | `fatload` byte count, then head/tail `md.l` comparison against the built bytes |
| A label's column in `probe.S` drifts from the gate's fixed-width regexes | The marker regexes were matched against the probe's own emitted lines | A marker that cannot match any line the probe can print |
| The gate reports success with no board attached | `just nt98690_boot_check` with no `--serial` | Exit 1 naming P6.A |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just nt98690_payload_check` | Pass — 5703-byte image, header `text_offset` `0x10000000`, `image_size` `0x2650`, magic `ARMd`, ELF entry `0x10000000` | Direct |
| Rebuild and compare digests | Byte-identical: `1046ed432ceea7fda174a0db8086b7df1853edd89344e865c4b3ed0555c86d8e` | Direct |
| Linker base moved to `0x20000000`, pins unchanged | Refused, naming both addresses | Direct |
| Load address moved to `0x05000000` in both | Refused: lies inside the vendor media CMA pools | Direct |
| `decode_branch` on ELF magic (`0x7f454c46`) | Refused as not a branch | Direct |
| Probe executed under QEMU `virt` at EL2 | Ran to completion; `el = 2`, `fdt_magic = d00dfeed`, `midr_part = 0xd08` (A72, correctly decoded), `parange = 4`, `gicd_typer = 0x8` → `gic_irqs = 0x120` (288), `gicd_iidr = 0x43b` (ARM GIC-400), five checks `ok` and `placement` `FAIL` as predicted | Direct, but about QEMU: see the note below |
| Each gate marker regex matched against the QEMU probe output | 15 of 19 `SLIME_` markers match. Placement, base, `midr_part`, and `PAYLOAD_OK` differ by construction and are labelled as QEMU-only differences | Direct |
| `just nt98690_boot_check` with no `--serial` | Exit 1 naming P6.A and both endpoint forms | Direct |
| `just sel4_gate_control_check` | Pass — 46 gates reject 1788 mutated transcripts, up from 45 | Direct |
| **The H1V1 itself** | **Not run.** No board attached | — |

The QEMU row is direct evidence about the *code* and no evidence at all about the
*board*. It proves the instruction stream executes, the sysreg decoding is right,
and the verdict logic can both pass and fail. Two of its values are meaningless by
construction and are labelled as such in the transcript: `check placement` fails
because QEMU loads at its own RAM base plus `text_offset` rather than at the link
address, and `cnt_hz_est` is far below the real `CNTFRQ_EL0` because QEMU does not
throttle its console to the line rate the estimate assumes.

## Decisions

- **Decision:** Give the probe a QEMU `virt` build variant behind one define.
  **Rationale:** the alternative was carrying never-executed hand-written assembly
  to a board whose only diagnostic is the console that assembly drives. The variant
  changes three addresses and the two UART primitives; every register read, the
  calibration, the verdicts, the fault vectors, and the reset path are the same
  code. It immediately paid for itself by finding the `x9` clobber.
  **Rejected alternative:** disassembly review alone, which would not have caught a
  register clobbered across a call boundary.

- **Decision:** Derive the GIC distributor base before every access instead of
  holding it in a register across the prints.
  **Rationale:** `putc` scratches `x9`, so a base parked there survives no print.
  Re-deriving costs three instructions and removes the whole class rather than the
  one instance; `putc`'s clobber is now documented at its definition so the next
  edit does not reintroduce it.
  **Rejected alternative:** moving the base to a callee-saved register, which fixes
  this site and leaves the trap set for the next one.

- **Decision:** Re-establish the stack pointer at the top of the fault handler.
  **Rationale:** the handler reports by printing, and printing calls, so it needs
  a stack — while one plausible reason to be in it is that the stack pointer was
  what went wrong. Without this, a misaligned or wandered `sp` faults again inside
  the handler and the board loops silently at the one moment it has something to
  say. Three instructions.
  **Rejected alternative:** a reserved fault stack, which costs a page to defend
  against the same single case.

- **Decision:** State the "non-zero" and "grew" properties as literal `check … = ok`
  lines emitted by the probe, rather than as regexes in the gate.
  **Rationale:** the gate's chain is exercised by `check-sel4-gate-controls.py`,
  which must be able to instantiate every pattern to prove the chain rejects
  tampering. Constraining the vocabulary to literals and counted classes keeps that
  control working; the properties a regex cannot express are decided in the probe,
  where the values actually are.
  **Rejected alternative:** richer regexes plus a bespoke per-board control script,
  which would have added a second control mechanism to keep honest.

- **Decision:** Report the board's measurements without failing on them.
  **Rationale:** `cntfrq`, `parange`, and `gicd_typer` are inputs to P6.B's kernel
  configuration, not properties P6.A asserts. A zero `CNTFRQ_EL0` is a plausible
  outcome on this firmware and means the timer frequency must be pinned and
  overridden — a design consequence, not a boot failure. The gate prints them with
  that consequence named.
  **Rejected alternative:** failing on a zero frequency, which would report a
  successful boot as a broken one.

## Open risks and follow-ups

- [x] **The board has been booted.** `nt98690_boot_check --serial /dev/ttyUSB0`
      passed on the named H1V1 on 2026-09-02: all 25 markers in order, no failure
      marker, 0 framing errors on a local tty. Transcript: `probe-boot.log`.
- [x] Before the first scored run, the board was stopped at its U-Boot prompt
      and surveyed read-only. `uboot-survey.log` confirms the pinned prompt
      `nvt: `, `mmc dev 0`, and `mmc 0:1`: `mmc list` reports MMC0 as SD and
      MMC2 as eMMC, so no gate command addresses eMMC. `${fdtcontroladdr}` is
      `0x7f9c5ea0` and holds `d00dfeed`. U-Boot's `lmb_dump_all` reserves
      `0x1f00000-0x1ffffff`, `0x4800000-0xabfffff`, and
      `0x7f9c4a40-0x7fffffff`; none contains the pinned load address
      `0x10000000`. No separate vendor-autoboot transcript was retained, so no
      autoboot framing-error claim is made here.
- [x] `parange`, `cntfrq`, `gicd_typer`, and `gic_irqs` are tightened to the values
      the board reported, so the gate now asserts this board rather than a shape. A
      different H1V1 revision would fail it, which is the intent.
- [ ] `md.l` head-and-tail comparison samples the loaded image rather than digesting
      it, because this U-Boot has no `crc32`. If the survey shows `crc32` after all,
      P6.B should use it; if `ping` works, `tftpboot` would remove the card swap
      from the loop entirely.
- [ ] The probe assumes it can `smc` for PSCI `SYSTEM_RESET`. If firmware refuses,
      it prints `reset failed` and spins, and the board needs a power cycle — which
      would also mean P6.B needs a different autonomous-recovery mechanism.
- [ ] The Milk-V Duo gates still carry their own copy of the console machinery now
      in `scripts/lib/uboot_console.py`. Migrating them needs a Duo on the bench.

## Artifacts and provenance

- Scored physical-board transcript: [`probe-boot.log`](probe-boot.log)
- Read-only U-Boot survey: [`uboot-survey.log`](uboot-survey.log)
- QEMU self-test transcript, with its by-construction differences labelled: [`qemu-self-test.log`](qemu-self-test.log)
- Lane decision and P6.B/P6.C outlines: [`devlog/2026-09-01-p6-nt98690-h1v1-lane/`](../2026-09-01-p6-nt98690-h1v1-lane/index.md)
- Vendor U-Boot consulted for every expected console string: `/srv/novatek/sdk/worktrees/h1v1-dev/BSP/u-boot` (`cmd/mmc.c`, `cmd/booti.c`, `arch/arm/lib/{image,bootm}.c`, `common/image-fdt.c`, `fs/fs.c`)
- Related roadmap item: [P6.A](../../roadmap/07-architecture-portability.md#p6a--h1v1-environment-bootstrap-and-firmware-handoff-evidence)

## Corrections

Appended 2026-09-02 after the first board session. The survey runs below were
observations rather than scored runs.

1. **Recovery capture scope.** The board's firmware reaches its kernel handoff
   about 700 characters after the U-Boot banner and prints `Moving Image from
   0x7c700040 to 0x0`. The gate therefore retains only the recovered banner
   line; otherwise the vendor's next boot would match a failure marker intended
   for this gate's payload.
2. **Device-tree pre-flight.** `md.l ${fdtcontroladdr} 1` must contain the
   little-endian `edfe0dd0` view of the FDT magic before `booti`; a missing tree
   now fails before spending a board boot.
3. **Read-only survey.** `--survey` obtains the SD slot, device-tree address,
   and prompt string that an uninterrupted vendor autoboot cannot supply. All
   commands are read-only.

4. The vendor kernel's own relocation confirms the placement model the payload's
   header depends on: loaded at `0x7c700040` with `text_offset` 0, U-Boot moved
   it to `0x0`, which is `ALIGN(ram_base, 2 MiB) + text_offset`. The probe pins
   `text_offset` to its load address, so the same arithmetic is a no-op and
   `Moving Image from` should not appear.

5. **The run passed.** `probe-boot.log` records the whole of it. `check placement`,
   `check el2`, `check fdt_magic`, `check mmu_off`, `check cnt_advance`, and
   `check gicd` all returned `ok`; `PAYLOAD_OK`; PSCI `SYSTEM_RESET` returned the
   board to its loader, TF-A, and U-Boot with no operator action. The transcript
   ends at the banner, which is correction 1 working on hardware: the vendor's
   next kernel handoff, and its `Moving Image from`, are outside the evidence.

6. **The timer risk closed the other way.** This entry's rationale, and the P6.B
   plan, assumed `CNTFRQ_EL0` might read zero on the primary core because TF-A's
   `plat_helpers.S` programs it on secondaries. It reads 12 MHz, and the probe's
   independent estimate against the 115200 line rate agreed to 0.33%. P6.B takes
   the timer frequency from the register and needs no pinned override.
