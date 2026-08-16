# Post-seL4 documentation reconciliation, and RP2 rescoped to what seL4 does not already supply

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Decision |
| Status | Proposed |
| Scope | `README.md`, `roadmap/README.md`, `roadmap/01-foundations.md`, `roadmap/02-core-runtime.md`, `roadmap/09-rpi5-ros2-demo.md`, `contracts/fabric-graph/v1/schema.zt`, `contracts/component/v1/README.md`, `contracts/bootstate/v1/gen_rust.zt`, `contracts/store/disk/v1/gen_rust.zt`, `contracts/generation/v1/fixtures/{sel4,sel4-supervision}.md`, `components/component{,-aarch64}.ld`, doc comments in `slime-root/src/`, `components/`, `stage0/src/arch/aarch64.rs` |
| Roadmap | RP2, RP3, C8.2, C7.7, M5.1, M5.4, M5.6, P5 |
| Gates | none |
| Trigger | Reading project status found `README.md` last updated 2026-07-30, entirely predating the `84c75f5` seL4 substitution, and `roadmap/README.md:18` carrying its own unactioned note that RP2's deliverables describe the retired custom kernel |
| Baseline | `84c75f5` retired `kernel/` and made `aarch64-sel4-qemu-virt` the product, but documentation outside `roadmap/07-architecture-portability.md` and `docs/` was not reconciled with it |

## Summary

`84c75f5` substituted seL4 for the custom microkernel and deleted `kernel/`,
but the reconciliation stopped at the tracks that slice touched directly.
`README.md` — the repository's front door — still opened with "built from a
new kernel", named `x86_64-qemu-virtio` as the Tier 0 automated target, and
listed a `kernel/` directory that no longer exists. Several contracts asserted
live invariants against deleted files, one of them wrongly: the
`fabric-graph` schema claimed `kernel/src/runtime/generation.rs` pinned its
ceilings with `const _: () = assert!`, and that compile-time assert died with
the kernel. Two decisions were made rather than merely transcribed. First,
retained-but-superseded formats are now labelled as such at the top of their
own documents rather than reading as current, because `contracts/component/v1/`
described itself as the live component encoding while `v2` is what
`just component_gen` renders and the seL4 product uses `v2`'s ELF-carrying
revision. Second, RP2 is rescoped: seL4 supplies the privileged mechanism its
old deliverables asked us to write, so RP2 now owns only the demo-scoped
replay plus the two arms no gate exercises — AArch64 rollback and wrong-target
rejection.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `README.md` | Rewrote the lead, status list, architectural direction, reference targets, generation/component boundary, layout, vertical-slice retrospective, and command set for the seL4 product | The front door describes the system that exists |
| `README.md` reference targets | Replaced the `x86_64-qemu-virtio` Tier 0 claim with `aarch64-sel4-qemu-virt` and the five admitted profiles' real build status | An admitted profile is not confused with a built one |
| `README.md` vertical slice | Marked complete and split its criteria into seL4-observed (1–8, 11) versus custom-kernel-only historical (9, 10 Framework) | Retired evidence is not read as current |
| `contracts/fabric-graph/v1/schema.zt` | Retargeted the control-message and shared-buffer ceiling comments from the deleted compile-time assert to `slime_root::generation::fabric_graph_is_satisfiable`'s admission-time re-check | The stated enforcement mechanism is the one that runs |
| `roadmap/02-core-runtime.md` C8.2 | Same correction in the milestone's own status text, naming the live constants passed to `validate_against` | A completed milestone's evidence names live code |
| `contracts/component/v1/README.md` | Reframed as retained format 1, pointed at `v2` and the `SLIMECME` ELF revision, and corrected the build pipeline's target selection | A superseded contract does not read as current |
| `roadmap/09-rpi5-ros2-demo.md` RP2 | Rescoped around P5 (see *Decisions*) | A milestone's deliverables name work that is actually outstanding |
| `roadmap/09-rpi5-ros2-demo.md` RP3 | Named the unused `sel4/config/bcm2712-rpi5.cmake` as the concrete starting artifact and moved device discovery to seL4 bootinfo | Board bring-up does not re-derive a device-tree parser in the root |
| `roadmap/README.md` | Corrected the RPi5 row and demo-sequencing steps 3–4, and added the missing `P5` node to the track map | The index agrees with its tracks |
| `roadmap/01-foundations.md` | Repointed M5.1/M5.6 verification targets at the gates they now resolve to and M5.4's unit evidence at `boot-contracts` | A named verification target exists |
| `roadmap/02-core-runtime.md` C7.7, boundaries | Repointed C7.7 at `just sel4_sample_check`; replaced the "C7 and B2 continue on the x86-64 reference path" boundary | No milestone cites a deleted test binary or a retired path as current |
| `components/component{,-aarch64}.ld`, `slime-root/src/{console,fault,generation,main,supervision}.rs`, `components/runtime/src/syscall/sel4_transport.rs`, `components/bins/src/bin/sample-lender.rs`, `stage0/src/arch/aarch64.rs`, `contracts/{bootstate,store/disk}/v1/gen_rust.zt`, `contracts/generation/v1/fixtures/{sel4,sel4-supervision}.md` | Retargeted comments that asserted a live counterpart in `kernel/`, and removed two dead ABI names (`SYS_SHARED_BUFFER_*`, "whichever kernel is under it") | A cross-reference resolves, or is explicitly historical |

Deliberately preserved: comments of the form "the retired kernel did X, here we
do Y" in `slime-root/src/main.rs`, `boot-contracts/src/{store_disk,
component_image}.rs`, and the seL4 check scripts. Those are design provenance
whose whole content is the contrast; rewriting them would delete the reasoning.

## Decisions

- Decision: RP2 is rescoped from "AArch64 QEMU kernel and component vertical
  slice" to "AArch64 QEMU product vertical slice", explicitly superseding P2's
  bring-up deliverables, and owes only a demo-scoped replay plus the AArch64
  rollback and wrong-target-rejection arms.
- Rationale: RP2's deliverables asked for exception vectors, `svc` syscalls,
  context and address-space switching, translation tables, TLB maintenance,
  GICv3, the generic timer, and PL011 — all of which seL4 already provides on
  the profile the product boots, with `just sel4_root_boot_check` observing EL0
  components, fault isolation, timer delivery, and reclamation there. Leaving
  the old text in place invited re-implementing formally verified mechanism.
  Auditing what the current gates do *not* cover left exactly two arms: no gate
  drives an AArch64 generation pair through rollback, and none rejects a
  wrong-architecture artifact from that admission path. `roadmap/README.md`
  already carried this as an unactioned note; this records the resolution.
- Rejected alternative: marking RP2 complete by inheritance from P5. The two
  named arms are real exit-condition content, not bookkeeping, and P5 was
  scoped to substitution rather than to demo rollback evidence.
- Decision: a retained-but-superseded contract states that at the top of its
  own document, with a pointer to the current version.
- Rationale: `contracts/component/v1/README.md` read as the live component
  encoding, including a build pipeline claiming `x86_64-unknown-none`, while
  `just component_gen` renders `v2` and the product uses `v2`'s ELF-carrying
  revision on a JSON target. A reader following v1 would have built the wrong
  thing. `contracts/component/v2/schema.zt` already defines v1's retained
  meaning precisely; the README simply had not adopted it.
- Rejected alternative: deleting the v1 README. The bounded rollback window
  must still decode those bytes, so its layout stays normative.
- Decision: no new gate accompanies these edits, and RP2's planned target
  (`just rpi5_arm_slice_check`) is named as planned rather than created.
- Rationale: every change is documentation or comment text; the Rust diff
  contains no non-comment line, confirmed by filtering the diff. Creating a
  gate here would attach a QEMU cost to a documentation change and imply the
  RP2 arms are implemented.

## Open risks and follow-ups

- [ ] `just rpi5_arm_slice_check` does not exist; RP2's exit condition is
  unobservable until the slice implementing it lands the target.
- [ ] `queueDepth` and `capabilitySlots` remain declared without any runtime
  cross-check (`devlog/2026-08-16-c8-13-declared-fields-audit/index.md`);
  C8.13.3 owns the capability-slot half. Unchanged by this entry.
- [ ] `roadmap/07-architecture-portability.md` retains many `kernel/`
  references inside P5's own completion record. Those are the frozen account of
  the retirement and were left alone; a reader could still mistake P2's
  historical deliverables for open work.
- [ ] `docs/directions/` was not audited against the seL4 cutover. None of its
  entries is a committed milestone, so no claim there is load-bearing yet.

## Artifacts and provenance

- Focused report: none; the reasoning is in *Changes* and *Decisions* above.
- Raw transcript: none retained.
- Serial/debugger/model output: none. Verification was
  `just contracts_check`, `just generation_check` (identity
  `ba924e2a49987aade0684f5d84f65bbce6857318973da3be8d845ff8d8501af6`,
  byte-identical across two isolated builds, so no contract edit perturbed a
  wire format), `just test_sel4_root` (114/114 across 13 modules),
  `just devlog_check`, `just typos`, `just fmt_check_all`, and `just lint_all`.
- Related roadmap item: [RP2](../../roadmap/09-rpi5-ros2-demo.md),
  [P5](../../roadmap/07-architecture-portability.md).
</content>
<parameter name="i">Writing devlog entry for the doc reconciliation and RP2 decision