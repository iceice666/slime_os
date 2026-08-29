# P3.D: a Milk-V Duo boots a Slime-built S-mode payload, hands-off

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `sel4/pins.toml`, `scripts/build/build-duo-payload.py`, `scripts/check/check-duo-boot.py`, `scripts/check/check-duo-gate-control.py`, `tools/duo/payload/`, `just/hardware.just`, `roadmap/07-architecture-portability.md` |
| Roadmap | P3.D, P3.E |
| Gates | `just duo_payload_check`, `just duo_boot_check /dev/ttyUSB0`, `just duo_gate_control_check` |
| Trigger | Deciding to target a physical RISC-V board, with no SD card reader on the development laptop |
| Baseline | No physical RISC-V board in tree; `riscv64-qemu-virt` declared but unbuilt; P4's media-copy loop is the only physical-board precedent |

## Summary

A named Milk-V Duo (SOPHGO CV1800B, T-Head C906) now boots a Slime-built S-mode
payload from its own boot partition and prints ordered evidence that the
firmware handed control over with translation disabled, a hart id, a DRAM
device-tree pointer, and a readable timebase. The loop needs no physical contact:
the payload is deployed over the board's USB-NCM link into its FAT `/boot`, and
U-Boot is driven over serial. Three consecutive runs passed with zero framing
errors. This qualifies a board, a firmware handoff, and an iteration loop — it
does **not** claim seL4 or `slime-root` on this board, which is P3.E.

The reason this exists as its own milestone is a hardware constraint, not a
preference: the development laptop has no SD card reader, so P4's "copy the media
onto the FAT partition and reset the board" step cannot be performed at all. A
loop that re-flashes a card per iteration was never viable here.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4/pins.toml` | New `[cv1800b_duo]` table: SoC, CPU, ISA, MMU, DRAM window, SBI reservation, interrupt controller, timer frequency, SBI spec/impl, U-Boot version/prompt/launch, staging and payload addresses, boot partition, USB-NCM address | Board facts are pinned and sourced, never asserted by a Slime-side board table |
| `tools/duo/payload/` | `smoke.S`, `smoke.ld`, parameterized `smoke.its`, and the board's captured `duo.dtb` | The payload is source in-tree, and its FIT carries no hand-written address |
| `scripts/build/build-duo-payload.py` | Deterministic build via pinned `nix` toolchain attributes; cross-checks the linker base, the ELF entry, and the pinned load address; writes `identity.json` | A payload linked for the wrong address fails at build time, not as a hang on the board |
| `scripts/check/check-duo-boot.py` | Two-phase physical gate: identity + digest-verified deploy, then serial-driven U-Boot launch and ordered marker matching | Physical claims require observed board evidence; absence is failure, never a skip |
| `scripts/check/check-duo-gate-control.py` | Mutates the committed transcript — each required marker deleted, the terminal marker reordered, each literal failure marker appended | The board gate's assertions are proven load-bearing rather than decorative |
| `just/hardware.just` | `duo_payload_check`, `duo_boot_check`, `duo_serial_monitor`, `duo_gate_control_check` | Every recipe states whether it qualifies hardware or only aids bring-up |
| `roadmap/07-architecture-portability.md` | P3.D recorded as observed; P3.E added for the seL4 port | A board-and-loop claim is not confused with a kernel-port claim |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A stale or wrong-address FIT is booted | `just duo_payload_check` | Build fails naming the linker base / pin disagreement, or the identity digest mismatch |
| The gate "passes" with no board attached | `just duo_boot_check` with no `--serial` | `no serial device given, so no board evidence can be observed` |
| A short FAT write boots a truncated image | Digest read back from the target during deploy | `the payload's digest on the board does not match the built FIT` |
| Marker assertions rot into decoration | `just duo_gate_control_check` | Names the tamper arm that was accepted |
| A payload strands the board | Payload exits by returning into U-Boot | Absence of `SLIME_DUO returning to U-Boot` fails the ordered chain |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just duo_payload_check` | Pass — 533-byte payload at `0x82000000`, 22 104-byte FIT | Direct |
| `just duo_boot_check /dev/ttyUSB0` | Pass — every ordered marker, 0 framing errors | Direct |
| Same gate, three consecutive runs, no physical contact | Pass ×3 | Direct |
| `just duo_gate_control_check` | Pass — 11 deletion arms, 1 reorder arm, 13 failure-marker arms all rejected | Direct |
| `python3 scripts/check/check-duo-boot.py` with no `--serial` | Exit 1, fail-closed message | Direct |
| Board parked at U-Boot, deploy attempted | Exit 1 naming the unreachable USB-NCM link | Direct |
| `--serial /dev/null` | Exit 1, `is not a tty` | Direct |
| seL4 RISC-V platform support, C906 MAEE behavior | Not run — P3.E's scope; see Open risks | Inherited from upstream source reading |

## Decisions

- **Decision:** deploy over the board's own USB-NCM link into its FAT `/boot`,
  and never modify stock `boot.sd`.
  **Rationale:** no SD reader on the development laptop, and leaving the vendor
  image bootable makes autoboot the recovery path.
  **Rejected alternative:** P4's media-copy step — impossible here; and serial
  `loady`, which this U-Boot does not compile in.

- **Decision:** wrap the payload in a FIT with `type = "kernel"` and a
  `flat_dt` node, and launch with `bootm`.
  **Rationale:** this U-Boot is built without `go`, `booti`, and `bootelf`, so
  `bootm` on a FIT is the only compiled-in way to transfer control. Vendor
  `common/bootm.c` rewrites a `kernel_noload` image's entry to a FIT-relative
  offset, and `arch/riscv/lib/bootm.c` `hang()`s when no device tree is present.
  The resulting `kernel(hart_id, fdt_addr)` call is exactly the handoff an seL4
  elfloader expects, so P3.E reuses this path.
  **Rejected alternative:** `kernel_noload` with an absolute entry — silently
  wrong; and renaming the payload to `boot.sd` so autoboot runs it, which would
  remove the recovery path.

- **Decision:** the payload exits by restoring U-Boot's `ra`/`sp` and returning.
  **Rationale:** measured — SBI legacy `SHUTDOWN` halts the hart, and this
  board's OpenSBI 0.9 does **not** implement SRST, so the `ecall` returns and
  falls through. Both strand the board and cost a physical re-plug. Returning
  depends on no SBI extension at all.
  **Rejected alternative:** SBI SRST cold reboot, which was tried and observed
  to strand the board.

- **Decision:** record P3.D as a board-and-loop milestone, with the seL4 port as
  a separate P3.E.
  **Rationale:** a custom payload booting proves the handoff and the loop; it
  proves nothing about seL4 on this SoC. Collapsing them would let a payload's
  pass imply a kernel claim.
  **Rejected alternative:** folding this into P3, whose exit condition is the
  RV64 QEMU corpus and is not what was observed here.

## Open risks and follow-ups

- [ ] **64 MiB is the dominant P3.E risk.** The DRAM window is 63.25 MiB
  (`0x80000000 + 0x03f40000`) and the vendor Linux sees only ~28 MiB after its
  ION/multimedia carve-outs. A Slime port publishes its own memory description
  and can reclaim most of the difference, but the elfloader, kernel, and root ELF
  sizes must be measured against that window rather than assumed to fit.
- [ ] **CV1800B PLIC context layout is unconfirmed.** The board reports
  `compatible = "riscv,plic0"`, which is the family upstream seL4's
  `include/drivers/irq/riscv_plic0.h` implements, but that driver hardcodes
  SiFive U54/U74 offsets and S-context numbering and rejects unknown platforms at
  compile time. Confirm against the SoC TRM before selecting it.
- [ ] **T-Head C906 memory attributes are unaudited.** MAEE occupies PTE bits
  60–63 and is gated by custom `mxstatus`/`sxstatus` CSRs. Upstream seL4 contains
  no T-Head code, so the firmware's reset state must be measured before standard
  Sv39 page tables are trusted.
- [ ] **No upstream seL4 support for this SoC exists.** Searches of seL4 16.0.0,
  current master, and seL4_tools found no CV1800B/SG2002/Milk-V/C906 references;
  the only related community artifact is an unsuccessful Allwinner D1 thread.
  P3.E is a real port, not a configuration exercise.
- [ ] This board's `duo.dtb` is the vendor Linux device tree, captured from
  `/sys/firmware/fdt`. It is the correct hardware-description argument for the
  bring-up handoff, but P3.E should publish its own rather than inherit the
  vendor's Linux-specific reservations.
- [ ] The gate's deploy phase requires a key the board's dropbear already
  accepts, bootstrapped once over the serial console. That bootstrap is manual
  and not itself gated.

## Artifacts and provenance

- Raw transcript: [`duo-boot-serial.log`](duo-boot-serial.log) — the observed
  `just duo_boot_check /dev/ttyUSB0` console capture this entry's claims rest on,
  and the input `duo_gate_control_check` mutates.
- Focused report: none; the decisive facts are in this entry and in
  `sel4/pins.toml [cv1800b_duo]`'s comments.
- Serial/debugger/model output: as above.
- Related roadmap item:
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md)
  P3.D (observed) and P3.E (not started).
