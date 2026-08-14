# P5.1 — Substituting seL4 for the custom microkernel

| Field | Value |
|---|---|
| Date | 2026-08-03 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/`, `components/runtime/`, `sel4/`, `scripts/build/build-sel4.py`, `scripts/check/check-sel4-pins.py`, `scripts/check/check-sel4-root-boot.py`, `Justfile`, `flake.nix`, `deps/sel4`, `deps/rust-sel4` |
| Roadmap | P5.1, P5.2, P5.3, P5.4, P2 |
| Gates | `just sel4_pin_check`, `just sel4_qemu_image_check`, `just sel4_root_boot_check` |
| Trigger | P2.2–P2.6 each require hand-writing AArch64 exception vectors, isolation, GICv3, timers, and virtio — mechanism upstream seL4 already provides under formal verification |
| Baseline | The custom x86-64 kernel boots the full 25-component graph; `just test` and `just product_boot_check` are the retained oracle |

## Summary

Slime's differentiator is the capability/component/generation model, not a
hand-written microkernel. This entry replaces the kernel-side mechanism with
upstream seL4 (pinned 16.0.0) and re-hosts Slime's authority model as a Rust
root task, `slime-root`, on `aarch64-qemu-virt`.

What is observed: the pinned image boots, the root task admits the existing
verified generation and derives child authority strictly from declared grants,
a native AArch64 child runs under a root-mediated IPC surface, a real timer
interrupt is claimed and serviced, shared frames carry bytes both ways, seL4
refuses a read-only write and refuses execution from a data page, a deliberate
fault is supervised, and every resource is reclaimed to zero.

What is **not** observed: no legacy component image runs. The generation's 25
component payloads are the retired kernel's custom `SLIMECM` images, which the
root task admits for authority derivation but cannot load. The proof of this
slice is a native fixture, and the root task says so in its own serial record
rather than implying graph activation. Rebuilding the components as native ELF
is P5.2. The custom kernel is retained unchanged as the frozen oracle.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Pins | `sel4/pins.toml` + `check-sel4-pins.py`: exact submodule commits, origins, seL4 release, both Rust toolchains, target-spec bytes, CMake config field-by-field, and observed artifact hashes | An image's identity names the exact sources it was built from |
| Pins | Pin gate fails closed on a dirty `deps/sel4` or `deps/rust-sel4` working tree | A commit hash cannot certify sources that were edited in place |
| Build | `build-sel4.py`: deterministic kernel build (`dtb-randomness=off`, `-ffile-prefix-map`), identity manifest the boot gate re-verifies | Two builds from different source paths produce identical artifacts |
| Root task | `slime-root/src/{main,generation,object_allocator,task,child_vspace}.rs`: generation admission, deterministic untyped/CSlot allocation, child CSpace/VSpace/TCB construction from AArch64 ELF | Child CSpaces hold only grant-derived authority; no untyped, CNode, VSpace, ASID, or IRQ authority reaches a child |
| Root task | `slime-root/src/{ipc,fault}.rs`: bounded root-mediated operation surface and fault supervision over one badged endpoint | Every legacy syscall number resolves to a bounded answer, never an unimplemented panic |
| Root task | `slime-root/src/{timer,platform_timer,event}.rs`: bounded timer scheduler behind a `PlatformTimer` adapter, backed by the EL1 physical timer on PPI 30 | A wake is never lost to a platform error after the queue was mutated |
| Root task | `slime-root/src/{shared_buffer,buffer_adapter}.rs`: table-held rights/quota authority, global frame-anchor uniqueness, orphan retention, structural teardown ordering, real seL4 frame mapping | Authority is table state, never caller-supplied bearer data |
| Transport | `components/runtime/src/syscall/{sel4_transport,wire,legacy}.rs` + `runtime.rs`: `sel4` Cargo feature routing the unchanged Slime operation API through seL4 calls on child slot 1 | Public wrapper signatures and error semantics are identical on both transports; oversized payloads are rejected, never truncated |
| Harness | `Justfile`: `sel4_pin_check`, `sel4_qemu_image_check`, `sel4_root_boot_check`; `run` repointed to the seL4 image; legacy targets renamed `legacy_*`; `fmt_check_all`/`lint_all` extended | The product default is the seL4 path; the oracle is explicitly labelled legacy |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Sources drift from the pinned identity | `just sel4_pin_check` | Commit, origin, toolchain, target-spec, CMake, hash, or dirty-tree mismatch |
| A build stops being reproducible | `just sel4_qemu_image_check` | Identity-manifest digest disagreement |
| The slice regresses anywhere | `just sel4_root_boot_check` | An ordered marker missing, or a failure marker present |
| Legacy images silently start being activated | Boot marker asserts `slimecm=[1-9]\d*` with `unrecognized=0` and `tasks=2` | Marker mismatch |
| Child pages become executable again | `SLIME_BUF probe refused ... kind=wx-execute` | Probe returns without a fault |
| Read-only mappings become writable | `SLIME_BUF probe refused ... kind=ro-write` | Region observed holding the intrusion value |
| The custom kernel regresses | `just test`, `just product_boot_check` (unchanged) | Existing legacy gates |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_pin_check` | Pass | Direct |
| `just sel4_qemu_image_check` | Pass; identity manifest written | Direct |
| `just sel4_root_boot_check` | Pass, ~17 s; full ordered marker set | Direct |
| `just fmt_check_all` | Pass (includes `fmt_check_sel4_root`) | Direct |
| `just lint_all` | Pass (includes `lint_sel4_root`: root, child, `slime-rt --features sel4`) | Direct |
| Untouched `deps/sel4` file → pin gate | Fails with `deps/sel4 has uncommitted changes`; restored and re-verified green | Direct |
| Corrupted shared-buffer pattern → boot gate | Fails on the read-back marker; restored and re-verified green | Direct (BufferIntegration) |
| Two clean builds from different source paths | Byte-identical kernel ELF and all five tracked digests | Direct (ProductHarnessCutover) |
| Legacy x86 corpus (`just test`, `product_boot_check`) | Unchanged; not re-run for this entry | Inherited |

Observed serial evidence, abridged to the load-bearing markers:

```text
SLIME_ROOT generation admitted number=1 components=25 grants=38 health=3 kernel=1 bootstrap=1
SLIME_ROOT graph admitted; legacy SLIMECM images not activated components=25 slimecm=25 elf=0 unrecognized=0
SLIME_TIMER acquired irq=30 freq_hz=62500000
SLIME_TIMER serviced events=1 programming=Disarm
SLIME_TIMER advanced start=5006508 end=5692643 delta=686135
SLIME_BUF mapped buffer=1 vaddr=0x40000000..0x40001000 pages=1 rights=read-write holder=0
SLIME_BUF mapped buffer=2 vaddr=0x40010000..0x40011000 pages=1 rights=read-only holder=0
SLIME_CHILD shared read vaddr=0x40000040 observed=0x534255465f525721 expected=0x534255465f525721
SLIME_BUF probe refused task=0 kind=ro-write access=Write address=0x40010040
SLIME_BUF probe refused task=0 kind=wx-execute access=Execute address=0x40000000
SLIME_ROOT child exit observed task=0 role=clean-exit status=0
SLIME_ROOT child fault observed task=1 role=deliberate-fault kind=VirtualMemory { access: Write, ... }
SLIME_BUF teardown unmapped=1 revoked=2 released=2 live=0 pages=0 mappings=0 holder_pages=0 orphans=0
SLIME_ROOT READY tasks=2 grants=19 declared_grants=38 reclaimed_slots=96
```

### Review findings fixed before this entry

A read-only review of the uncommitted diff reported twelve findings. Six were
routed to the agents owning those files and six were fixed directly:

| Finding | Priority | Resolution |
|---|---|---|
| `SYS_TRANSFER_WINDOW_BIND` (label 31) had no root-side handler; every oversized payload would have failed | P0 | Added `Operation::TransferWindowBind = 31` with root-service mediation |
| Child pages mapped executable: `PF_X` was mapped onto `CapRights::grant`, but AArch64 `maskVMRights` reads only read/write and `VmAttributes::DEFAULT` omits `EXECUTE_NEVER` | P0 | Added `page_attributes()`; non-`PF_X` pages and the IPC buffer are execute-never. Now a live boot assertion (`kind=wx-execute`) |
| Code copied through a data-cached scratch mapping was never unified to the I-cache | P1 | `unify_instruction_cache()` after the child mapping is installed (an unmapped frame returns `IllegalOperation` — observed) |
| Pin gate accepted a dirty submodule tree | P1 | `git_dirty()` check; mutation-tested |
| `service_timer_source` dropped due-timer wakes when a platform call failed after the queue was mutated | P1 | Restructured; regression test added |
| Shared-buffer authority was caller-supplied bearer data (rights and holder) | P1 | `authorize()` keys rights off table state and narrows only; holder is a verified claim; quotas via `declare_quota()` |
| Frame-anchor uniqueness checked only within one `create()` call | P1 | Global uniqueness across live regions |
| Failed map rollback orphaned an unaccounted live mapping | P1 | Orphan recorded, `SharedBufferError::Orphaned`, `retry_orphans()` |
| Teardown emitted revoke/release interleaved per region | P2 | `build_actions()` derives all unmaps, then revokes, then releases |
| "Legacy images not activated" was a self-reported string | P2 | Marker now asserts `slimecm=[1-9]\d*`, `unrecognized=0`, alongside `tasks=2` |
| Partial ELF segment data left an unloaded tail | P2 | Not applicable: `object`'s `read_bytes_at` errors rather than short-reading, and `data.len() > mem_size` is rejected |
| seL4 transport wire format is hand-rolled, not a Zutai schema | P2 | Reviewed, not applied: the fast-path register encoding is an in-memory calling convention between two crates in this repo, exempt under AGENTS.md. Revisit at P5.2 if it becomes a cross-artifact format |

## Decisions

- Decision: substitute upstream seL4 for the custom kernel; keep Slime's authority model as a root task.
- Rationale: P2.2–P2.6 are four milestones of kernel mechanism seL4 already provides, verified. The capability/component/generation model — the actual product — is unchanged by the substitution.
- Rejected alternative: continue P2.2–P2.6 on the custom kernel. Rejected because each slice re-derives mechanism with no product differentiation, and the RPi5 demo needs the data path, not a second hand-written kernel.

- Decision: retain `kernel/` unchanged as the frozen oracle; do not delete it in this change.
- Rationale: seL4 currently activates zero of the 25 declared components. Deleting the only implementation that runs the full graph would destroy the regression baseline before its replacement exists. Retirement is P5.4, gated on P5.3.
- Rejected alternative: delete `kernel/` now. Rejected as unevidenced: it would convert a working oracle into a claim.

- Decision: the timer uses the EL1 **physical** timer (PPI 30, `CNTP_*`), not the virtual timer.
- Rationale: with `KernelArmHypervisorSupport ON` the kernel runs at EL2 and claims `CNTHP_*`/PPI 26 for its own tick, and `arch/arm/kernel/boot.c` marks PPI 27 (`CNTV_*`) `IRQReserved` unconditionally whenever hypervisor support is compiled in. PPI 30 is the only architected-timer PPI a root task can claim under this config.
- Rejected alternative: PPI 27. Rejected because `IRQControl_Get` returns `seL4_RevokeFirst` for a reserved IRQ.

## Open risks and follow-ups

- [ ] No legacy or native Slime component graph runs on seL4; the proof is a fixture. P5.2.
- [ ] The C7/C8 bounded data path is not replayed on seL4. P5.3.
- [ ] `kernel/` is still the only implementation of the full component graph. P5.4.
- [ ] Shared-buffer loans, seal-remapping of live writable mappings, and `advance_epoch` are unit-tested only, with no boot marker.
- [ ] Shared-buffer adapter failure handling (rollback, orphan, retry) is unit-tested but was never triggered by a real seL4 error in the boot record.
- [ ] Timer evidence establishes delivery and acknowledgement only — no temporal isolation, CPU reservation, or deadline guarantee, and lateness is neither bounded nor reported. The `SLIME_TIMER advanced` marker is corroborating only: `CNTPCT_EL0` free-runs, so a non-zero delta would hold even if IRQ delivery degraded. `SLIME_TIMER delivered` is the load-bearing assertion.
- [ ] Raspberry Pi 5 remains untouched: `sel4/config/bcm2712-rpi5.cmake` is a pinned profile, not an observed boot. P4/RP3.

## Artifacts and provenance

- Focused report: none; the findings table above is the curated record.
- Raw transcript: none retained — the boot transcript is regenerated on demand by `just sel4_root_boot_check`, which asserts it.
- Serial/debugger/model output: abridged marker set inline under *Verification*.
- Related roadmap item: [P5 — seL4 microkernel substitution](../../roadmap/07-architecture-portability.md#p5-sel4-microkernel-substitution)
