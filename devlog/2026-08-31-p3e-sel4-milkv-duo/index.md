# P3.E — seL4 on the Milk-V Duo

| Field | Value |
|---|---|
| Date | 2026-08-31 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/target-profile/v1/`, `scripts/{build,check}/`, `sel4/config/cv1800b-duo.cmake`, `slime-root`, `deps/{sel4,rust-sel4}` |
| Roadmap | P3.E |
| Gates | `just riscv64_qemu_check`, `just duo_sel4_check`, `just duo_gate_control_check` |
| Trigger | P3.D established a repeatable physical handoff, making the upstream seL4 CV1800B port and first verified Slime generation the next architecture slice |
| Baseline | The board ran only the minimal P3.D S-mode probe; upstream seL4 had no CV1800B platform, C906 MAEE changed Sv39 memory attributes, and the loader's eager RISC-V leaves had not been exercised on a software-maintained A/D implementation |

## Summary

P3.E adds distinct RV64 QEMU and Milk-V Duo target profiles, an upstream seL4 CV1800B platform, C906 MAEE page-table encoding in the loader and kernel, the physical RTC/PLIC/root path, and a four-boot qualification gate. The observed MAEE smoke run established `sxstatus.MAEE=1`; physical campaigns then exposed and fixed the loader A/D defect recorded in [`devlog/2026-08-31-p3e-loader-ad-bits/`](../2026-08-31-p3e-loader-ad-bits/index.md) and the stopped RTC seconds counter recorded in [`devlog/2026-09-01-p3e-rtc-counter-start/`](../2026-09-01-p3e-rtc-counter-start/index.md). The final campaign completed three byte-identical sample runs plus the bounded early-fault control, with zero framing errors and autonomous recovery after every boot.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Target contracts and generation builder | Added distinct `riscv64-sel4-qemu-virt` and `riscv64-sel4-milkv-duo` identities and target-qualified generation construction | Physical and emulated RV64 artifacts cannot be interchanged |
| seL4 platform and pins | Added the CV1800B platform configuration, observed memory/PLIC/timer facts, isolated prefix, and pinned artifacts | Platform mechanism uses explicit board facts rather than QEMU defaults |
| Loader and kernel page tables | Applied XTheadMae normal-memory attributes to valid Sv39 leaf and branch descriptors on the Duo, and initialized every eager RISC-V loader leaf with `A\|D` | Every hardware page-table walk uses the encoding selected by observed MAEE state and begins with immediately usable leaf access state |
| Root timer and reset | Added the mapped CV1800B RTC one-shot path, PLIC IRQ, bounded early-fault control, and autonomous RTC reset | Timer and fault evidence fail explicitly and each run recovers without physical intervention |
| Device-untyped traversal | Retyped sparse device pages in CSpace-bounded chunks, retaining one last-child capability as the untyped watermark anchor while deleting each consumed prefix | Mapping a late MMIO page no longer requires one live CSlot per skipped granule, while the retained child prevents seL4 from resetting the device untyped's free index between chunks |
| Physical gate | Added digest deployment, ordered boot assertions, three normalized sample runs, one early-fault run, and evidence capture | The milestone requires repeated physical serial evidence rather than build success |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| RV64 reference behavior diverges before physical qualification | `just riscv64_qemu_check` | The reference plane fails its architecture-neutral corpus |
| MAEE evidence is weakened or reordered | `just duo_gate_control_check` | Deleting or reordering any marker is accepted |
| Physical seL4/root/component/timer/fault behavior regresses | `just duo_sel4_check /dev/ttyUSB0` | Any ordered marker, semantic repeat, zero-framing, fault, or recovery assertion fails |
| Sparse MMIO traversal resets or retains consumed caps | `just test_sel4_root` plus every seL4 plane mapping a device page | Anchor/planning unit tests fail, or an existing platform's device mapping is refused or maps the wrong physical granule |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just duo_boot_check /dev/ttyUSB0 --transcript /tmp/duo-maee.log` equivalent physical run | Observed `sxstatus=0x0000000040638000`, decoded `MAEE=1`, payload completion, return, and vendor recovery | Direct; raw capture below |
| `just duo_gate_control_check` | Passed; the observed P3.D transcript passes and all 27 missing, reordered, or explicit-failure mutations are rejected | Direct |
| `just riscv64_qemu_check` | Passed after rebuilding the full reference corpus from pinned Rust-seL4 commit `070c6a384a01ce0f6c60e081fddc8c40fb2a6132`; the rebuilt identity records that commit | Direct |
| First physical `check-duo-sel4.py --no-build` campaign | Reached loader `Entering kernel` and then timed out before the post-switch marker; root-caused to eager leaf PTEs missing `A\|D`, fixed and rebuilt | Direct; [`devlog/2026-08-31-p3e-loader-ad-bits/`](../2026-08-31-p3e-loader-ad-bits/index.md) |
| Second physical campaign | Crossed the repaired loader transition, booted upstream seL4, entered `slime-root`, mapped the RTC, and acquired PLIC IRQ 17; then timed out because the RTC seconds source was stopped, now fixed and rebuilt | Direct; [`devlog/2026-09-01-p3e-rtc-counter-start/`](../2026-09-01-p3e-rtc-counter-start/index.md) |
| Architecture-matched `slime-root` host tests under `qemu-aarch64` | 214/214 passed against the pinned AArch64 seL4 prefix; the first run caught and forced correction of the new fragmented-slot fixture | Direct; [`root-tests-aarch64-qemu.log`](root-tests-aarch64-qemu.log) |
| `just duo_sel4_check /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` | Passed after the RV64 reference corpus: three physical sample boots produced byte-identical normalized semantic traces with zero framing errors, the early-fault boot emitted its bounded diagnostic, and all four boots cold-reset to vendor Linux | Direct; [`sample-run-1.log`](sample-run-1.log), [`sample-run-2.log`](sample-run-2.log), [`sample-run-3.log`](sample-run-3.log), [`early-fault.log`](early-fault.log) |

## Decisions

- **Decision:** Treat MAEE as a platform page-table encoding requirement on every valid Sv39 descriptor, including non-leaf branches.
- **Rationale:** The physical probe observed `sxstatus.MAEE=1`, and C906 interprets PTE bits 63–59 during every page-table walk.
- **Rejected alternative:** Clear MAEE or rely on standard Sv39 encoding without an observed firmware transition; either changes firmware-owned state or reproduces the first silent instruction-fetch fault.

- **Decision:** Keep the post-activation timer proof before entering the component service loop.
- **Rationale:** Components are runnable but no request has yet been served, so the `ClockService` scheduler is empty and hardware ownership has not passed to it; the proof directly establishes the roadmap's post-activation IRQ requirement.
- **Rejected alternative:** Run a private timer scheduler after component requests begin, which could overwrite a live service deadline.

## Open risks and follow-ups

- [x] The final physical campaign recorded three byte-identical normalized sample traces and one bounded early-fault recovery trace.
- [ ] The completed claim remains deliberately limited to the RV64 architecture, root, timer, fault, and sample-plane behavior exercised here; it does not qualify storage, USB, network, display, sensor, actuator, ROS, or Framework behavior.

## Artifacts and provenance

- Focused report: the two physical defects have their own linked entries; this entry owns milestone integration.
- Raw transcripts: [`maee-smoke.log`](maee-smoke.log), the observed S-mode probe and subsequent autonomous vendor recovery; [`sample-run-1.log`](sample-run-1.log), [`sample-run-2.log`](sample-run-2.log), and [`sample-run-3.log`](sample-run-3.log), the three successful physical sample boots; [`early-fault.log`](early-fault.log), the bounded negative control; [`root-tests-aarch64-qemu.log`](root-tests-aarch64-qemu.log), the complete architecture-matched root-test result and the fixture defect it exposed.
- Normalized semantic traces: [`sample-run-1.normalized.log`](sample-run-1.normalized.log), [`sample-run-2.normalized.log`](sample-run-2.normalized.log), and [`sample-run-3.normalized.log`](sample-run-3.normalized.log), compared byte-for-byte by the gate.
- Generated build identities remain under `build/` and are not committed evidence.
- Related roadmap item: [P3.E](../../roadmap/07-architecture-portability.md#p3e--sel4-on-the-milk-v-duo).
