# P6.2-P6.4: x86-64 seL4 boots, runs the root, and serves the product graph

| Field | Value |
|---|---|
| Date | 2026-09-03 |
| Kind | Change |
| Status | Verified |
| Scope | `sel4/config/qemu-pc99.cmake`, `sel4/pins.toml`, `flake.nix`, `scripts/lib/{pc99_media,sel4_boot}.py`, `scripts/build/build-sel4.py`, `scripts/check/check-sel4-{pins,root-boot,wait-set-plane,sample-plane,component-graph,boot-layout,x86-64-image}.py`, `slime-root/src/{thread_abi,platform_timer,device,console,child_vspace,object_allocator,main}.rs`, `slime-root/src/graph_runtime/console_runtime.rs`, `deps/rust-sel4` |
| Roadmap | P6.2, P6.3, P6.4 |
| Gates | `just x86_64_sel4_root_boot_check`, `just x86_64_qemu_check`, `just x86_64_sel4_image_check`, `just x86_portability_check`, `just sel4_gate_control_check` |
| Trigger | P6.1 left an admitted, reproducible pc99 build with no boot claim and no file tree to boot from |
| Baseline | AArch64 and RV64 reference lanes; P6.1's byte-reproducible pc99 kernel, root, child, and generation |

## Summary

Pinned QEMU q35/OVMF now boots upstream seL4 and the x86-64 `slime-root`
through one GRUB Multiboot2 EFI tree, the root's native task/IPC/fault/timer/
reclamation slice runs to `SLIME_ROOT READY`, and the resident
`init`/`console`/`spawn-service`/Slisp graph evaluates a Slisp expression typed
into live COM1 input and spawns `sysinfo` through its declared context
endpoint. Reaching that required five distinct defects to be root-caused: three
inherited configuration or upstream bugs that made the platform unbootable, and
two Slime-side bugs that a second architecture exposed. The same EFI tree P6.5
will write to removable media is what these gates boot today.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4/config/qemu-pc99.cmake` | `KernelSupportPCID` off, `KernelMaxNumBootinfoUntypedCaps` and `KernelRootCNodeSizeBits` restored to seL4's own defaults from the inherited proof profile's values | The kernel reaches userspace, hands the root a bootinfo with kernel untypeds, and presents a CSpace the root's slot bitmap accepts |
| `deps/rust-sel4` | x86-64 `stack_init` drops a `push rbp` that over-corrected once the arm was routed through a second `call`; new `IOPort` cap type with `ioport_in8`/`ioport_out8` | Rust is entered with the stack pointer the SysV ABI requires; a root holding port authority can use it |
| `slime-root/src/thread_abi.rs` | New module owning the entry stack pointer for every thread the root starts directly, with the x86-64 offset the absent `call` would have pushed | A kernel-entered thread begins at the alignment its ABI specifies |
| `slime-root/src/platform_timer.rs` | HPET status cleared between arming the comparator and enabling delivery; `TIMER_IRQ` moved from IOAPIC pin 2 to 20 | The root services only expiries it armed, on a pin no legacy device drives |
| `slime-root/src/device.rs`, `console.rs` | `Com1Input` polls the 16550 over an I/O-port capability; the console dispatcher's IPC buffer is named through `next_event` | Live product input on a machine whose serial port is not memory-mapped |
| `slime-root/src/object_allocator.rs` | Untyped table bounds derived from the kernel's own `MAX_NUM_BOOTINFO_UNTYPED_CAPS` | No machine can declare more untypeds than the root will accept |
| `scripts/lib/pc99_media.py` | Assembles and digests the GRUB EFI tree; verifies pinned firmware and module bytes before writing anything | A boot claim names the exact bytes it booted |
| `scripts/lib/sel4_boot.py` | One platform-aware identity check and boot path for every plane gate | A plane supports every architecture or none, not one by accident |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A boot that reads bytes the identity does not describe | `just x86_64_sel4_root_boot_check` | tree digest or per-file digest mismatch before QEMU starts |
| Unpinned firmware, bootloader, or module set | `just sel4_pin_check` | firmware hash, GRUB version, or module-list digest drift; module reordering is caught because the digest is order-sensitive |
| The kernel configuration drifting back toward the proof profile | `just sel4_pin_check` | exact override table mismatch on `KernelSupportPCID`, `KernelMaxNumBootinfoUntypedCaps`, `KernelRootCNodeSizeBits` |
| The emulator gaining or losing a feature the profile depends on | `just sel4_pin_check` | TCG's refusal set no longer matches `cpu_features` / `qemu_features_unavailable` |
| W^X silently becoming unenforced on an architecture that has the attribute | `just x86_64_sel4_root_boot_check`, `just sel4_root_boot_check` | per-platform `WX_PROBES` verdict mismatch |
| Boot media losing reproducibility | `just x86_64_sel4_image_check` | two normalized builds differ in boot media or GRUB config |
| A gate's marker chain weakening | `just sel4_gate_control_check` | 45 gates × mutated transcripts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just x86_64_sel4_root_boot_check` | pass — ordered generation, timer, task, IPC, fault, ready markers on qemu-pc99 | Direct |
| `just x86_64_qemu_check` | pass — root boot, wait-set, sample, product graph, 3 boot layouts, portability | Direct |
| `just x86_64_sel4_image_check` | pass — two normalized builds byte-identical across boot media, GRUB config, kernel, root, child, component, generation, identity | Direct |
| `just sel4_pin_check` (with and without the shell's store paths) | pass — verifies firmware and module digests when exported, reports the skip when not | Direct |
| `just test` (AArch64 root boot, component graph, gate controls) | pass | Direct |
| `just sel4_boot_layout_check` | pass — 31 AArch64 plane layouts unchanged | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just test_host`, `just contracts_check`, `just architecture_contract_check` | pass | Direct |
| `cargo test -p slime-root --lib` against the pc99 prefix | 217 passed | Direct |
| `just test_sel4_root` | not run — fails identically on a clean tree in this environment (host clang parses the AArch64 prefix's `syscalls.h`); the same 217 tests were run directly against the pc99 prefix instead | Direct (baseline confirmed by stashing the change) |

The five defects, in the order the boot surfaced them:

| # | Symptom | Root cause |
|---|---|---|
| 1 | `PCIDs not supported by the processor`, kernel halts before `boot_pml4` | The inherited X64 proof profile enables `KernelSupportPCID`; QEMU's TCG accelerator implements neither `pcid` nor `invpcid` on any model, so no CPU choice satisfies `head.S`'s checks |
| 2 | `user exception 0xd` at the root's first `movaps` | Upstream `stack_init` adjusts by 8 *and* pushes `rbp` while reaching Rust through a second `call`, leaving Rust entered with the stack pointer 16-byte aligned where SysV requires it plus 8 |
| 3 | `allocator rejected bootinfo: NoKernelUntyped`, then `DeviceTableFull`, then `SlotRangeTooLarge` | The proof profile's `MaxNumBootinfoUntypedCaps` of 50 is exhausted by q35's device regions before any kernel memory is emitted, its root CNode of 2^19 exceeds the root's slot bitmap, and the root's own 64-entry table is smaller than the machine's 78 device regions |
| 4 | Unbounded `SLIME_CLOCK expired due=0 delivered=0 live=0` | Two independent faults: `Tn_INT_ROUTE_CNF` pointed at IOAPIC pin 2, which the firmware's MADT also assigns to the still-enabled 8254 PIT; and enabling delivery while `GINTR_STA` held a retired expiry re-asserted the line immediately |
| 5 | `user exception 0xd` inside the root's console dispatcher | The console thread's stack pointer was aligned to 16 for a kernel-entered thread, the same ABI mismatch as #2 in a second place — now both go through `thread_abi` |

## Decisions

- **Decision:** Weaken the pc99 kernel profile rather than the gates, and pin
  each divergence with the emulator fact that forces it.
  **Rationale:** All three settings are proof-configuration choices, not
  product ones; seL4's own defaults are what the other Slime platforms already
  run. PCID is a TLB-flush optimization with no capability or mapping
  semantics.
  **Rejected alternative:** Running the gates under KVM, which would make the
  boot depend on the host CPU and break the pinned-machine reproducibility the
  whole pin system exists for.

- **Decision:** Fix the x86-64 entry-stack alignment in the rust-sel4 fork and
  re-pin, rather than working around it in Slime.
  **Rationale:** The fork already exists for platform fixes, and the bug is in
  upstream's own assembly; a Slime-side workaround would leave every future
  component subject to it.
  **Rejected alternative:** A local trampoline, which would have to be
  duplicated for the root and every component runtime.

- **Decision:** Route the HPET to IOAPIC pin 20 rather than enabling legacy
  replacement mode to silence the PIT.
  **Rationale:** Legacy replacement would route comparator 0 to pin 0 and take
  over the RTC, claiming two devices this milestone does not own.
  **Rejected alternative:** Filtering unexpected interrupts in the root, which
  would hide a misrouted line instead of not sharing one.

- **Decision:** Extend the existing plane gates through two shared libraries
  instead of adding x86-specific checkers.
  **Rationale:** AGENTS.md's verification-code discipline: the invariants are
  already owned by `check-sel4-root-boot.py` and the plane gates, and the only
  thing that differs per architecture is the boot route.
  **Rejected alternative:** A parallel `check-x86-64-*.py` family, which would
  let the two architectures' assertions drift apart.

- **Decision:** Record x86-64 boot-layout fixtures under their own directory
  even though the observed blocks are byte-identical to the AArch64 ones.
  **Rationale:** That identity is the cross-architecture result the gate
  exists to observe; sharing one file would let whichever run came last
  overwrite the other's evidence.

## Open risks and follow-ups

- [ ] The HPET frequency is still the pinned 10 MHz emulator fact rather than
      read from the HPET capability register. A physical machine reports its
      own period; H1 owns that inventory.
- [ ] `just test_sel4_root` cannot run in this environment for a reason that
      predates this change: it builds `sel4-sys` for the host against the
      AArch64 prefix, and host clang rejects that prefix's inline asm register
      names. The 217 tests were run directly against the pc99 prefix instead.
- [ ] The x86-64 boot-layout corpus covers three planes (`sel4`, `sel4-sample`,
      `sel4-wait-set`) rather than the AArch64 table's 31; the remainder build
      only for platforms later milestones own.
- [ ] `deps/rust-sel4`'s two commits are local to the fork branch and must be
      pushed before another checkout can resolve the pinned commit.

## Artifacts and provenance

- Serial capture of the first full root-boot slice on x86-64:
  [`first-root-boot-serial.log`](first-root-boot-serial.log)
- Frozen x86-64 capability layouts: `contracts/boot-layout/v1/fixtures/x86_64/`
- Related roadmap items:
  [P6.2](../../roadmap/07-architecture-portability.md#p62--shared-grub-multiboot2-boot-contract),
  [P6.3](../../roadmap/07-architecture-portability.md#p63--native-x86-64-root-and-component-execution),
  [P6.4](../../roadmap/07-architecture-portability.md#p64--x86-64-product-graph-and-semantic-corpus)
