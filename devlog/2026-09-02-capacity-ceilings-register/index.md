# Capacity ceilings: what a raise actually costs, and why single-core is now written down

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Decision |
| Status | Proposed |
| Scope | `docs/directions/34-capacity-ceilings.md`, `docs/directions/README.md`, `sel4/config/qemu-arm-virt.cmake` |
| Roadmap | none |
| Gates | none |
| Trigger | A design conversation asking whether the memory and thread ceilings can be raised, and observing that `KernelMaxNumNodes > 1` will eventually be wanted |
| Baseline | Three ceilings — 2 MiB per task, 8 MiB system-wide private, 2 threads per component — and `KernelMaxNumNodes 1` on all four targets, each correct but only one of them carrying a written reason |

## Summary

Three of Slime's capacity bounds are set well below the hardware they run
on, and a fourth — single core — is a config value with no recorded
rationale. Reading the code to answer "can these be raised?" turned up a
coupling that is not visible from the constants themselves: every page of
a task's private reservation costs one root CSlot, so raising
`MAX_REGION_PAGES` to a useful size overruns a 4096-slot root CNode
sixteen times over for a single task. The repository has already hit that
wall twice and kept both measurements. The fix is not a larger constant
but a larger *frame* — both architectures expose 2 MiB and 1 GiB frame
objects that `slime-root` has never used — which changes the slot cost by
512x and changes no contract at all.

This entry registers the whole question as `docs/directions/34` and, in
the same change, writes the single-core deferral into
`sel4/config/qemu-arm-virt.cmake` beside the option, in the form that
file already uses for MCS. Nothing is implemented; no ceiling moves.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `docs/directions/34-capacity-ceilings.md` | New register entry: the five ceilings with their sources, the root-CSlot and `.bss` couplings with their two recorded failures, the four separable implementation pieces, and the open questions | A direction that follows from the vision but is not committed work is registered rather than rediscovered |
| `docs/directions/README.md` | Entry 34 added to the index, a new `capacity` route, an unblocked-work row naming the large-page half, and entry 34 placed in sequencing wave 3 | The register's four cross-reference surfaces stay consistent with the entry table |
| `sel4/config/qemu-arm-virt.cmake` | `KernelMaxNumNodes 1` now carries the same deferral discipline the adjacent `KernelIsMCS OFF` already had: the CAVEATS terms, the per-target cost, the `tcb_set_affinity` coupling to MCS, and the root-invariant audit a multi-core boot requires | An assurance decision states its terms where it is taken, rather than reading as an unexamined default |

## Decisions

- **Decision:** Register the capacity question as one direction entry
  covering memory, threads, and cores together, rather than three.
  **Rationale:** They share one mechanism — the root's per-task CSlot
  budget — and a reader who finds only the memory half will re-derive the
  same coupling when they reach threads. The entry keeps them separable
  as four implementation pieces while stating the shared cost once.
  **Rejected alternative:** A backlog item. `roadmap/00-backlog.md` tracks
  defects and regressions in implemented code; nothing here behaves
  incorrectly. Every ceiling is declared, bounded, enforced, and
  reclaimed, and the roadmap rule for `docs/directions/` — promote only
  once there are dependencies, bounded deliverables, required checks, and
  an observable exit condition — is exactly what this entry does not yet
  satisfy for its ceiling and SMP halves.

- **Decision:** Name large-page allocation as the only unblocked half, and
  state explicitly that a ceiling must not be raised before it.
  **Rationale:** The slot arithmetic is decisive. 256 MiB backed by
  granules is 65536 root CSlots against a default CNode of 4096; the same
  region backed by 2 MiB frames is 128. Both `LargePage`/`MegaPage` and
  `HugePage`/`GigaPage` already exist in `rust-sel4` for both
  architectures, `slime-root` uses neither, and the private window is
  already aligned to its own 2 MiB span. Raising a constant first produces
  `PlanExceedsRootSlots` at boot — a message that names the plan, not the
  constant responsible.
  **Rejected alternative:** Raising `KernelRootCNodeSizeBits` instead. It
  is a legitimate independent margin (7–26 permitted, default 12, verified
  configs use 19, no Slime config sets it) but not a substitute: slots are
  backed by the rootserver allocation, which is not negligible on a 63 MiB
  Duo, and it leaves the per-page frame-capability cost untouched.

- **Decision:** Record the single-core deferral in the config file rather
  than only in the register.
  **Rationale:** `sel4/config/qemu-arm-virt.cmake` already establishes
  that an assurance decision belongs beside its option — its
  `KernelIsMCS OFF` comment names the CAVEATS terms, the per-target cost,
  the coupling to `ScheduleRecord`'s zeroed budget fields, and the two
  validators a change must edit in the same commit. `KernelMaxNumNodes 1`
  sat directly beneath it with no such note, which read as an unexamined
  default rather than a decision. The new comment carries two couplings a
  future change would otherwise discover mid-flight: that `rust-sel4`
  gates `tcb_set_affinity` on `all(not(KERNEL_MCS), not(MAX_NUM_NODES = "1"))`
  so SMP and MCS are not independent options, and that `slime-root`'s
  bounded tables hold several invariants today only because one child runs
  at a time.
  **Rejected alternative:** Commenting all four configs. The MCS precedent
  in this same file argues against it — the decision is per-target, and
  the load-bearing one is `bcm2712-rpi5.cmake`, which includes upstream's
  own verified profile. Writing the same paragraph four times would
  suggest a single global decision that has not been taken. The register
  entry carries the cross-target picture.

- **Decision:** Leave the exit condition deliberately unspecified for the
  SMP half.
  **Rationale:** A multi-core claim needs a named target, a recorded
  verified-set departure for that target, and a re-read of the root's
  concurrency assumptions. The register can fix what the decision must
  *contain* without pre-deciding it, which is the same posture
  `qemu-arm-virt.cmake` takes for MCS.

## Open risks and follow-ups

- [ ] The `.bss` coupling runs the other way for shared buffers and is
      recorded but unquantified: `MAX_FRAME_ANCHORS = MAX_TOTAL_PAGES`
      feeds `MAX_PHYSICAL_PROVENANCE`, which is rounded to a power of two
      and lives in `.bss`, so raising the shared ceiling takes back root
      CSlots. The register states the direction of the effect and one
      worked example; it has not been measured.
- [ ] `private_memory.rs:58` argues the 2 MiB window is chosen to be
      defensible on a small target, which is sound and argues for a
      per-target-profile ceiling. But `private_memory.rs:59-62` argues for
      one constant because it is coupled to the arena and slot bounds.
      Both are correct and the register does not resolve them; a
      per-target ceiling would make the arena plan target-dependent.
- [ ] The claim that a 2 MiB block fills an AArch64 L2 entry directly and
      removes the leaf table for that span is labelled `[INFERENCE]` in
      the entry. This repository has never mapped a large frame, so the
      table saving is unverified.
- [ ] Whether any realistic composition reaches `MAX_TASKS = 48` before
      the memory ceilings is unexamined.
- [ ] Entry 34's ceiling and SMP halves should be promoted only when a
      workload needs the headroom. A ceiling raised for its own sake is a
      bound nothing charges — the objection A3 already records against
      inheriting a conserved CPU account with no MCS to charge it.

## Artifacts and provenance

- Register entry: [`docs/directions/34-capacity-ceilings.md`](../../docs/directions/34-capacity-ceilings.md)
- Config record: `sel4/config/qemu-arm-virt.cmake`, the comment block above `KernelMaxNumNodes`
- Related roadmap items: none. The entry is unpromoted by design; its
  eventual consumers are [Authority A3](../../roadmap/06-authority-trust.md)
  and [Foreign workloads](../../roadmap/05-foreign-workloads.md), neither
  of which can be sized while a task's working set is capped at 2 MiB.
- Verification: `python3 scripts/check/check-sel4-pins.py` passed after
  the config edit, confirming the added comment is inert — the parser
  skips `#` lines before matching `set(...)`, so the recorded
  `kernel_config_sha256` is unaffected. No runtime tests were run; this is
  a documentation and comment change that alters no compiled value.
