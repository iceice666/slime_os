# P5.4.final — auditing whether `kernel/` can be deleted

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Audit |
| Status | Verified |
| Scope | `roadmap/07-architecture-portability.md`, `roadmap/00-backlog.md` |
| Roadmap | P5.4.final, P5.4 |
| Gates | `just typos`, `just devlog_check` |
| Trigger | P5.4.2 and P5.4.3 both completed; P5.4.final was the next item |
| Baseline | The roadmap recorded P5.4.final as "not started" with no coverage analysis |

## Summary

**`kernel/` cannot be deleted today, and the reason is specific rather than
cautious.** Two read-only audits against the 26 seL4 plane gates found six
acceptance properties with no seL4 equivalent, plus a structural coupling that
makes deletion a coordinated cutover rather than a directory removal.

This is a negative result, and it is the useful one. Every M5 and M6 slice now
has an observed seL4 gate — P5.4.2 and P5.4.3 are complete — and it would have
been easy to read that as "the oracle is redundant". Auditing what deletion
would actually remove says otherwise.

## Observable symptom

P5.4.final's exit condition is that every acceptance check the custom kernel
guards has an observed seL4 equivalent. With P5.4.2 and P5.4.3 both complete,
that condition *looks* satisfied — every M5 and M6 milestone has a named,
passing seL4 gate. Whether it actually is satisfied had never been checked
against the gates themselves.

## Investigation log

Two scouts, both read-only and both instructed to be adversarial: the question
was what deletion would *lose*, not what is already covered.

- One enumerated every Justfile recipe that builds or boots the custom kernel,
  read each check script's actual assertions rather than inferring from names,
  and mapped each against a named seL4 gate.
- One mapped structural dependencies: workspace membership, Rust imports,
  script references, and which components and contracts both sides share.

Their verdicts agreed. Two of the six findings I then verified directly rather
than taking on report — see below.

## Changes

None to code. The audit's output is the itemised list now recorded in
P5.4.final and tracked as B31.

## Verification

### Six acceptance properties would be lost

1. **`kernel/tests/task_reclamation.rs`.** Verified directly: it captures
   `FRAME_ALLOCATOR.free_frames()` before and after spawn/release cycles and
   requires the count to return to baseline with no drift.
   `sel4_root_boot_check` asserts that reclaimed CSlot ranges adjoin with no gap
   or overlap — a different property, on a deliberately monotonic allocator, and
   the seL4 root has no free-frame count to compare against.
2. **The component-image decoder's stack-size and header-shape corpus.**
   `boot_contracts::component_image` is host-tested, but the oracle's corpus
   covers shapes no seL4 plane loads.
3. **`storage_nvme_read_check`.** Verified directly: `grep -c nvme` over every
   `check-sel4-*.py` returns nothing. The port drives virtio-blk only, and
   M5.7's promotion gates depend on NVMe.
4. **`aarch64_boot_check`'s custom stage-0/EL1 vertical slice.** seL4's loader
   booting is not the same acceptance property as Slime's own stage-0 reaching
   EL1.
5. **Kernel-foundation PMM/VMM/heap/APIC assertions.** seL4 provides the
   mechanism, so these have no analogue rather than an unported one — but they
   are coverage that exists today and would not tomorrow.
6. **`just test`'s smoke, panic, and IPC fault-isolation assertions.** The seL4
   planes observe faults and clean exits, which is broader but not the same set.

### Deletion is not a self-contained change

No Rust crate imports `slime_os-kernel` — the dependency direction is
kernel → `boot-contracts`, not the reverse — so the coupling is entirely in
orchestration:

- `scripts/lib/harness.py` defines `RELEASE_KERNEL` as the oracle artifact, and
  every checker importing it breaks;
- roughly two dozen check scripts boot with `cwd=kernel`;
- `build-generation.py` requires a custom-kernel ELF to build a generation;
- `components/bins/src/bin/` holds binaries both sides run — `directory-probe`,
  `powerbox-chooser`, `dango`, `spawn-service` and others — which must survive.

### What was verified directly

| Command/scenario | Result | Evidence class |
|---|---|---|
| `kernel/tests/task_reclamation.rs` asserts a free-frame count | Confirmed by reading the test | Direct |
| No seL4 gate mentions NVMe | Confirmed by grep over `check-sel4-*.py` | Direct |
| `sel4_root_boot_check` asserts CSlot adjacency, not frame conservation | Confirmed by reading the gate's own comment | Direct |
| No crate imports `slime_os-kernel` | Reported by the dependency audit | [INFERENCE] — not re-verified |
| The remaining four lost properties | Reported by the gate audit | [INFERENCE] — not re-verified |
| `just typos`, `just devlog_check` | Pass | Direct |

## Decisions

- **Decision:** Record the verdict and stop, rather than delete what is covered
  and leave the rest.
  **Rationale:** the exit condition is "removed in one reviewable change". A
  partial deletion leaves the tree in a state where neither the oracle nor its
  replacement is the authority, which is the situation the frozen-oracle rule
  exists to prevent.

- **Decision:** Audit against the gates rather than against the roadmap's own
  P5.4 inventory.
  **Rationale:** that inventory was written before this session's twelve slices
  and would have reported gaps that are now closed — and, more dangerously,
  might not have listed gaps that were never inventoried. The gates are the
  evidence.

- **Decision:** Mark two of the six as *possible* non-goals rather than work
  items.
  **Rationale:** the PMM/VMM/heap/APIC assertions test mechanism seL4 supplies
  under formal verification. Porting them would be re-testing seL4. That is a
  judgement the next author should make deliberately, so it is recorded as a
  question rather than silently dropped.

## Open risks and follow-ups

- [ ] B31 tracks the six. Three look portable — the decoder corpus, the
      fault-isolation assertions, and frame conservation if the root gains page
      accounting. One needs new mechanism: an seL4 NVMe transport. Two may be
      genuine non-goals.
- [ ] The orchestration cutover is its own slice: retiring ~24 legacy checkers,
      removing `RELEASE_KERNEL`, and making `build-generation.py` build a
      generation without a custom-kernel ELF.
- [ ] `AGENTS.md` describes `kernel/` as the frozen oracle. That stays accurate
      until deletion, and becomes wrong the moment it lands.
- [ ] Four of the six findings are the scouts' reports rather than my own
      reading. I verified the two largest; the rest should be re-checked before
      anyone acts on them.

## Artifacts and provenance

- B31 in [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- The itemised list and the coupling analysis are recorded in P5.4.final in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
- The slices that closed P5.4.2 and P5.4.3 are indexed in
  [`devlog/README.md`](../README.md) under 2026-08-08.
