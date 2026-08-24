# P4 Raspberry Pi 5 bring-up: a second seL4 platform, and the assurance it costs

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Change |
| Status | Verified |
| Scope | `sel4/config/bcm2712-rpi5.cmake`, `sel4/pins.toml`, `.gitmodules`, `deps/rust-sel4` (forked), `scripts/build/build-sel4.py`, `scripts/build/build-generation.py`, `scripts/build/build-rpi5-media.py`, `scripts/check/check-sel4-pins.py`, `scripts/check/check-rpi5-boot.py`, `scripts/check/check-sel4-gate-controls.py`, `Justfile` |
| Roadmap | P4 |
| Gates | `just sel4_rpi5_image_check`, `just rpi5_media_check`, `just rpi5_boot_check`, `just sel4_pin_check`, `just sel4_gate_control_check`, `just sel4_root_boot_check`, `just generation_check`, `just contracts_check`, `just test_sel4_root`, `just rpi5_artifact_check` |
| Trigger | A physical Raspberry Pi 5 and removable media became available, opening P4's hardware half |
| Baseline | `qemu-arm-virt` was the only seL4 platform the build could produce; `sel4/config/bcm2712-rpi5.cmake` existed but had never been built |

## Summary

**Status scope:** `Verified` refers to this entry's host and build evidence only.
The physical board boot is **not** observed, so **P4 remains open** and nothing
here may be cited as board qualification.

P4's board path is now buildable end to end: the pinned seL4 16.0.0 tree builds a
`bcm2712` kernel, the loader gained the RPi5 platform it was missing, and the
packaged ELF flattens into the exact `kernel8.img`/`config.txt` the firmware
loads. Three real blockers were found and closed rather than worked around — the
loader had no `PLAT_BCM2712` arm at all, `objcopy -O binary` silently discards
the loader payload, and the upstream *verified* kernel configuration cannot
print, which would have made P4's own exit condition unobservable. The board
boot itself is **not** observed: the artifacts are built and verified and
`just rpi5_boot_check` fails closed with a named missing prerequisite until a
USB-UART adapter is attached and the media written. No P4 claim is made here.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `deps/rust-sel4` | Forked to `iceice666/rust-sel4`, adding `crates/sel4-kernel-loader/src/plat/bcm2712/mod.rs` and a `PLAT_BCM2712` selector arm. PL011 console at `0x107d001000`, PSCI secondary-core startup | A pinned platform seL4 supports can actually be loaded |
| `sel4/pins.toml` | Repinned `[rust_sel4]` to the fork; added `[bcm2712_rpi5]` board facts and `[observed_prefix_bcm2712_rpi5]` artifact hashes | One commit and one hash set identify the built sources per platform |
| `sel4/config/bcm2712-rpi5.cmake` | `KernelVerificationBuild OFF` / `KernelDebugBuild ON` / `KernelPrinting ON`, each with `FORCE`; documented the memory window and the assurance cost | The board has a console, and the cost is stated where it is made |
| `scripts/build/build-sel4.py` | Introduced a `Platform` descriptor and threaded it through configure/install, generation, application, loader, packaging, and manifest. Per-platform prefix, cargo target dirs, generation dir, artifacts, image, and manifest | Two platforms cannot overwrite each other's artifacts or reuse each other's loader |
| `scripts/build/build-generation.py` | `SEL4_TARGET_PROFILES` admits `aarch64-rpi5` beside the QEMU profile | "is a seL4 build" stopped meaning "is the QEMU profile" |
| `scripts/build/build-rpi5-media.py` | New: flattens PT_LOAD segments by physical address into `kernel8.img`, renders `config.txt`, writes no block device | The firmware receives the whole payload, and no gate writes a user's disk |
| `scripts/build/build-rpi5-media.py` | **Defect found and fixed after first flash.** The first image began with `7f454c46` — the ELF magic — because the lowest PT_LOAD segment starts at file offset 0 and so contains the 64-byte ELF header; the firmware branched into it and executed the header as AArch64. Separately the real entry sits `+0x4838` because `.rodata` precedes `.text`. Fixed by overwriting the run-time-dead header with one `b` to the entry point, keeping `kernel_address` page-aligned. `check_entry_is_code` now pins both the branch and its landing instruction | The firmware's first instruction is `_start`, and the defect that produced a silent board cannot recur |
| `scripts/check/check-rpi5-boot.py` | Added `--monitor` (`just rpi5_serial_monitor`): prints raw serial, asserts nothing, runs before the build and identity checks | A silent wire can be diagnosed without a gate's assertions in the way |
| `scripts/check/check-sel4-pins.py` | `--platform` selects which prefix `--prefix` validates; `CMAKE_SET` now matches `FORCE`; the RPi5 required-config table pins the three printing options | A pinned option cannot become invisible to the gate that pins it |
| `scripts/check/check-rpi5-boot.py` | New physical gate: builds, proves media is this build's, reads a real tty via `termios`, asserts ordered markers | Board evidence is observed, never inferred or emulated |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Board kernel silently changes | `just sel4_pin_check --platform bcm2712-rpi5` (via `sel4_rpi5_image_check`) | Prefix SHA-256 mismatch naming the artifact |
| The board loses its console again | `just sel4_pin_check` | `bcm2712-rpi5 CMake config is incomplete: KernelPrinting: expected 'ON'` |
| A stale `kernel8.img` is booted and attributed to a new kernel | `just rpi5_boot_check` | `the media is stale`, with both digests |
| A QEMU image is mistaken for a board image | `just rpi5_boot_check` | `carries QEMU launch facts, so it was not built for a physical board` |
| The marker table silently weakens | `just sel4_gate_control_check` | Pinned count mismatch; 35 gates / 1363 mutations |
| The QEMU platform regresses under the refactor | `just sel4_root_boot_check` | Missing/out-of-order marker on the QEMU transcript |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_rpi5_image_check` | Wrote `build/slime-sel4-bcm2712-rpi5.elf` (789504 bytes) and its manifest | Direct |
| bcm2712 prefix rebuilt from scratch twice | Byte-identical all five pinned artifacts | Direct |
| Loader disassembly | `movk #0x7d00 lsl 16` / `#0x10 lsl 32` and `sel4_pl011_driver::write` linked — the RPi5 UART, not QEMU's `0x09000000` | Direct |
| `just rpi5_media_check` | `kernel8.img` 797696 bytes, load `0x314000` (page-aligned), first word `0x1400120e` = `b +0x4838` landing on `0xd53800a0` = `mrs x0, mpidr_el1`; 197 `SLIME` markers | Direct |
| `objcopy -O binary` comparison | 38312 bytes vs 797696 — payload dropped, proving the custom flattener necessary | Direct |
| `just rpi5_boot_check --no-build` | Fails closed: identity and media verified, then names the missing serial device | Direct |
| Marker table, 11 single-marker deletions | All 11 rejected; transposition reported out-of-order; appended failure markers rejected | Direct |
| `just sel4_gate_control_check` | 35 gates reject 1363 mutated transcripts | Direct |
| `just sel4_root_boot_check` | Passes — QEMU path unaffected by the platform refactor | Direct |
| `just generation_check` | Byte-identical isolated builds; 4 CPU-budget mutations refused | Direct |
| `just contracts_check`, `just test_sel4_root`, `just rpi5_artifact_check`, `just ruff`, `just typos` | Pass; 152/152 root tests | Direct |
| `check_entry_is_code` fault injection | Re-injecting ELF magic, a wrong branch distance, and a wrong entry offset each rejected with a distinct message | Direct |
| **Board boot on the physical Pi 5** | **Not observed.** First flash produced zero bytes, which led to the header defect above. After the fix the adapter itself proved unusable: `just rpi5_serial_monitor` opened the tty, configured 115200 8N1, and read 0 bytes across the window. P4 is deferred on a working adapter | None |

## Decisions

- **Decision:** Fork `seL4/rust-sel4` rather than patch it in tree.
  **Rationale:** `check-sel4-pins.py` fails closed on any uncommitted change in a
  pinned submodule, so a local edit would either break the gate or force it to
  expect a dirty tree — losing the property that a commit identifies the built
  sources. `deps/zutai` and `deps/dango` are already forks, so this follows
  existing repository shape.
  **Rejected alternative:** an in-tree patch mechanism applied at build time.

- **Decision:** Leave the verified kernel configuration on the Pi 5.
  **Rationale:** `AARCH64_bcm2712_verified.cmake` sets `KernelVerificationBuild ON`,
  which forces `PRINTING` and `DEBUG_BUILD` off; that kernel emits nothing on the
  UART and does not compile `sel4::debug_println`. P4's exit condition, RP3, and
  `contracts/rpi5-ros2-demo/v2`'s `serialPath`/`serialBaud` all require a recorded
  serial transcript, and a verified kernel that cannot be observed cannot qualify
  a board. `qemu-arm-virt.cmake` already flagged bcm2712 as "the config where the
  claim is load-bearing" — this is that claim being spent, deliberately.
  **Rejected alternative:** keep the verified build and write a `slime-root` UART10
  driver for evidence. Real work, does not exist today, and would have blocked P4
  indefinitely; recorded as a follow-up instead.

- **Decision:** Do not widen the RPi5 memory overlay.
  **Rationale:** upstream ships only `overlay-rpi5-2gb.dts`, claiming physical 0 up
  to the VideoCore base; seL4 reports 1069023232 bytes usable after the ATF
  reservation. That range exists on every model, so it is correct on the 4 GiB
  board, only conservative. BCM2711's high ranges cannot be borrowed — the Pi 5
  moves peripherals behind the RP1 southbridge — and the device tree on boot media
  carries a firmware placeholder (`memory@0 = 0..0x28000000`) rewritten at
  hand-off, so the real high map must be read from a booted board.
  **Rejected alternative:** transcribing RPi4's ranges, which would have been
  fabricated data in a pinned file.

- **Decision:** The media builder writes no block device.
  **Rationale:** copying onto removable media is the one step that can destroy an
  unrelated disk. The gate proves what was written matches what was built; the
  operator performs the write.

- **Decision:** Defer P4 on a working USB-UART adapter rather than build an
  alternative output path.
  **Rationale:** seL4 ships three driver families (`serial`, `timer`, `smmu`) and
  no display, framebuffer, or HDMI driver anywhere in `src/`/`include/`; even the
  serial driver is a debug facility selected by device-tree `compatible` string
  and compiled out by the verified configuration. The debug UART is therefore the
  only console the kernel has, and the blocker is a $10 part rather than a design
  gap. Recorded in P4's *Why serial is the only evidence path* because "just use
  HDMI" is the obvious question.
  **Rejected alternative:** a userspace display driver. On this SoC video sits
  behind the RP1 southbridge across PCIe, so it needs PCIe enumeration and
  address translation before a single pixel — and a framebuffer cannot report a
  fault that happens before it is mapped, which is precisely what boot evidence
  must capture. Roadmap invariant "framebuffer output alone is never milestone
  completion" already anticipated this: a display would not close P4 even if it
  existed.

## Open risks and follow-ups

- [ ] **P4 is deferred on hardware, not closed.** The board boot is unobserved
      and blocked on a working USB-UART adapter — the one on hand produced zero
      bytes at 115200 8N1 with a healthy tty. When a working adapter exists:
      copy `build/rpi5-media/*` onto the FAT32 boot partition, reset, and run
      `just rpi5_serial_monitor /dev/cu.usbserial-XXXX` first (it asserts
      nothing, so it separates a link fault from an image fault), then
      `just rpi5_boot_check /dev/cu.usbserial-XXXX`.
- [ ] There is no substitute evidence path. seL4 ships three driver families —
      `serial`, `timer`, `smmu` — and no display, framebuffer, or HDMI driver at
      all; even the serial driver is a debug facility the verified configuration
      compiles out. The reasoning is recorded in P4's *Why serial is the only
      evidence path* so it is not re-litigated: a userspace display driver on
      this SoC needs PCIe enumeration behind RP1 before a pixel, and a
      framebuffer cannot report a fault occurring before it is mapped.
- [ ] The Pi 5 kernel is outside the verified configuration. Closing this needs a
      `slime-root`-owned UART10 driver so evidence no longer depends on
      `seL4_DebugPutChar`. Until then, no P4/RP3 evidence may be read as evidence
      about a verified kernel.
- [ ] The board sees 1019 MiB regardless of its 4 GiB badge. Widening needs an
      observed board memory map via `KernelCustomDTSOverlay`.
- [ ] The bcm2712 loader platform is only in the fork. Upstreaming it to
      `seL4/rust-sel4` would let the pin return to upstream.
- [ ] `REQUIRED_MARKERS` in `check-rpi5-boot.py` is the QEMU chain narrowed by
      inspection; the first real boot may show the board's marker set differs
      (for example the generic-timer IRQ number), which is a gate edit backed by
      an observed transcript rather than a guess.
- [ ] `harness.py`'s `profile_text`/`profile_integer` still name `[qemu_arm_virt]`
      in their diagnostics, so they were not reused for the board profile.

## Artifacts and provenance

- Focused report: none; the decisive chain is in this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: none — the board has not been booted.
- Related roadmap item: [P4](../../roadmap/07-architecture-portability.md), [RP3](../../roadmap/09-rpi5-ros2-demo.md).
