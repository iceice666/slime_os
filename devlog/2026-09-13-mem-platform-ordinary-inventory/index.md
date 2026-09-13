# MEM-PLATFORM: the kernel's own memory window becomes observable, and a probe reaches past the retired one

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,object_allocator}.rs`, `scripts/build/build-sel4.py`, `scripts/check/{check-sel4-pins,check-sel4-root-boot,check-sel4-gate-controls}.py`, `sel4/pins.toml` |
| Work items | 01a07a2d-0081-7df4-b7fe-38b933057b4a |
| Gates | `just sel4_pin_check`, `just sel4_root_boot_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check` |
| Trigger | MEM-PLATFORM's premise: the AArch64 product started QEMU with 2048 MiB while the pinned kernel DTB was generated from a separate literal, so runtime `-m` was the only evidence of RAM |
| Baseline | `fc86cc7d` had already moved `memory_mib`, the DTB tuple, and the pinned `platform_gen.yaml` window to 2048 MiB / `0x60000000..0xc0000000`; no per-range evidence existed and no validator read the installed window |

## Summary

The ARM platform was already 2 GiB by the numbers; what was missing was any way to
*tell*. RAM size reached the generated kernel DTB through a literal in
`build-sel4.py` that no gate compared against `sel4/pins.toml`, the installed
prefix's `platform_gen.yaml` was hashed but never read, and the root reported one
summed byte count that a smaller kernel window and a larger `-m` produce
identically. This change removes the duplicate literal, validates the installed
window against the pinned DRAM base plus RAM size, and has the root enumerate every
admitted ordinary range and probe a frame from the highest one that can hold a
granule. On `qemu-arm-virt` the inventory now reaches `end=0xbf94c000` with an
admitted range beginning exactly at the retired platform's `0x80000000` end, and the
probe verified a frame at `0xbf948000` — roughly 1 GiB past what the 1024 MiB
platform could contain.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-sel4.py` | `QEMU_DTB_PARAMETERS` no longer carries a RAM literal; `dump_device_tree` reads `memory_mib` from the platform's pins profile | One platform fact has one source, so a one-sided edit cannot leave the kernel describing a different machine than the launcher boots |
| `scripts/check/check-sel4-pins.py` | Refuses a RAM literal in the DTB tuple; parses the installed `platform_gen.yaml` and requires its window end to equal `dram_base + memory_mib`; runs in the default flow when a prefix exists and skips cleanly when it does not | A blessed hash proves the artifact did not change, not that it describes this platform; the window is now read rather than only hashed |
| `sel4/pins.toml` | `dram_base` pinned for both QEMU profiles | The assertion has a pinned source rather than a magic number, and RV64's `0x80000000` base is not ARM's |
| `slime-root/src/object_allocator.rs` | `OrdinaryRange`, `ordinary_ranges`, `ordinary_physical_end`, and a root-only `allocate_last_ordinary_granule` that bypasses first-fit | A capacity probe must touch RAM the previous window could not contain, not observe that a high range exists while allocating from the first |
| `slime-root/src/main.rs` | Per-range and summary `SLIME_ROOT ordinary` markers, then a probe that maps, writes, reads back, unmaps, deletes, and releases; `beyond_legacy` is per-platform and `None` where no window was superseded | `-m` moves no marker here: every range comes from BootInfo, which the kernel derives from its own pinned description |
| `scripts/check/check-sel4-root-boot.py` | `check_ordinary_memory` ties the summary to the enumerated ranges, the probe to the highest granule-capable range, and `beyond_legacy` to the platform's own superseded window | The markers are load-bearing rather than well-formed |
| `scripts/check/check-sel4-gate-controls.py` | `literal_for` instantiates enumerated digit classes; `check_root_memory_runtime_control` drives `check_ordinary_memory` with a synthetic inventory and seven mutations | The claim is tested where it lives; the marker table alone could only check field shape |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The DTB RAM size and the pinned profile drift apart | `just sel4_pin_check` | `QEMU_DTB_PARAMETERS[...] must contain only executable, machine, and CPU; RAM comes exclusively from pins.toml memory_mib` |
| A prefix whose kernel window describes another platform passes on a blessed hash | `just sel4_pin_check` | `stale 1024 MiB platform window rejected` is the control; a real mismatch names the observed and expected window |
| A larger `-m` is read as a larger kernel window | `just sel4_gate_control_check` | `root memory evidence rejected 7 launcher-only and inventory mutations` |
| The probe stops leaving the retired window, or RV64 is credited with crossing one it never had | `just sel4_root_boot_check` | `the probe took 0x…, which the superseded 0x80000000 platform already contained`; `qemu-riscv-virt reported beyond_legacy=1, expected 0` |
| The summary stops agreeing with the ranges it summarizes | `just sel4_root_boot_check` | `ordinary inventory end is 0x…, expected 0x… from the enumerated ranges` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_pin_check` | Passed: `qemu-arm-virt installed memory window verified`, `qemu-riscv-virt installed memory window verified`, `stale 1024 MiB platform window rejected` | Direct |
| One-sided DTB edit (added a fourth `"1024"` tuple item, observed the refusal, reverted) | Rejected, exit 1, with the message quoted in the guards above | Direct |
| `just sel4_root_boot_check` | Passed. 34 ranges, `SLIME_ROOT ordinary ranges=34 bytes=1584454000 end=0xbf94c000`, range 11 at `paddr=0x80000000 bytes=536870912`, probe `paddr=0xbf948000 bytes=4096 beyond_legacy=1 verified=1`. Kept as [`ordinary-inventory-arm.log`](ordinary-inventory-arm.log) | Direct |
| `check-sel4-root-boot.py --platform qemu-riscv-virt` | Passed. `ranges=40 bytes=3195192256 end=0x13f95a000`, probe `paddr=0x13f950000 beyond_legacy=0 verified=1`. Kept as [`ordinary-inventory-riscv.log`](ordinary-inventory-riscv.log) | Direct |
| `just sel4_gate_control_check` | Passed: 48 gates, 1937 mutated transcripts, and the seven ordinary-inventory mutations | Direct |
| `just sel4_boot_layout_check` | Passed: 31 plane layouts match their fixtures | Direct |
| `just ruff`, `cargo fmt -p slime-root --check` | Passed | Direct |
| Prefix hashes | Unchanged; no re-blessing was required, because the pinned RAM sizes were already 2048/3072 MiB and this change only removed the duplicate literal and added validation | Direct |

## Decisions

- Decision: the probe allocates from the highest ordinary range that can still place a granule, through a root-only allocator entry point, rather than reading the provenance of a first-fit allocation.
- Rationale: the exit condition is that a frame is allocated *from the added range*. A first-fit allocation plus a range-existence assertion would satisfy the words and not the claim. The inventory's tail is a run of sub-page descriptors the kernel publishes for leftover bytes, so "highest" is qualified by granule capability — requiring the numerically highest range would demand a 4 KiB object from a 256-byte untyped.
- Decision: `beyond_legacy` is a per-platform claim, absent where no window was superseded.
- Rationale: `0x80000000` is the AArch64 `virt` platform's retired RAM end and the RISC-V `virt` platform's DRAM *base*. A shared constant would sit below every RV64 ordinary address and report `1` by arithmetic rather than by RAM. RV64 reports `0` and the checker requires exactly that.

## Open risks and follow-ups

- [ ] The pinned window check reads `platform_gen.yaml`; the closure's `platform_gen.json` carries the same facts and is not cross-checked against it.
- [ ] `ordinary_physical_end` is the end of the highest *admitted* range, which excludes device untypeds by construction. It is not the platform's physical RAM end and must not be quoted as one.
- [ ] The probe proves allocate/map/write/read/unmap/delete and release of a root CSlot. The ordinary untyped watermark remains monotonic, so those bytes are not reusable through that path — the pre-existing limitation recorded at `slime-root/src/buffer_adapter.rs:320-323`.

## Artifacts and provenance

- Raw transcripts: [`ordinary-inventory-arm.log`](ordinary-inventory-arm.log), [`ordinary-inventory-riscv.log`](ordinary-inventory-riscv.log).
- Related work item: MEM-PLATFORM, `.tasks/items/01a07a2d-0081-7df4-b7fe-38b933057b4a.md`.
