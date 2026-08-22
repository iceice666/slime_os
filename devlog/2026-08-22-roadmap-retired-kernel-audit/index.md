# The open milestones still specified against the kernel P5 deleted

| Field | Value |
|---|---|
| Date | 2026-08-22 |
| Kind | Audit |
| Status | Verified |
| Scope | `roadmap/{README,01-foundations,02-core-runtime,07-architecture-portability,08-native-development,09-rpi5-ros2-demo,10-component-platform}.md` |
| Roadmap | C9, C10, C10.1, C10.2, C10.3, P3, P4, D2, D4, M1, M2, RP2, CP2, CP5, B70 |
| Gates | `just devlog_check` |
| Trigger | A progress review found `roadmap/README.md` still declaring B70 open one commit after `b9553b7` closed it, which raised the question of what else in `roadmap/` had not followed the P5 seL4 cutover |
| Baseline | P5.4.final deleted `kernel/` on 2026-08-09 and every *completed* milestone that described it was labeled historical or superseded at that time |

## Summary

Completed milestones were migrated when the custom kernel died. **Open** ones
were not. C9, C10, P3, P4, D2, and D4 still specified their unbuilt work against
retired mechanism, and C10 was the worst case: its motivation and architecture
decisions were written against `SLIMECMP`, the component/v1 image magic that
`contracts/component/v2/schema.zt:41` records as `legacyMagic` and that the
product path refuses. An implementer following C10 would have read a header
field from a format `boot-contracts/src/component_image.rs` rejects.

The distinction that made the audit tractable is that retired-kernel prose in a
`**Status:** Complete` milestone is the record P5.4.final deliberately preserved,
while the same prose in a `Not started` milestone is a specification defect. Four
tracks (03, 04, 05, 06) needed no change at all, and four surviving occurrences
were left deliberately untouched because they sit inside frozen observed results.

## Observable symptom

- Command: none — this is a documentation audit with no runtime reproduction.
- Expected: an open milestone's Deliverables, Required checks, and Exit condition
  name mechanism that exists on the `aarch64-sel4-qemu-virt` product path.
- Observed: C10's motivation sourced a component's stack from "the `SLIMECMP`
  header"; C9 required "keep scheduling mechanism in the kernel"; P3 required
  "implement S-mode kernel and U-mode component execution with Sv39 ... trap
  decoding, `ecall` syscalls, saved user context, address-space switching"; D4
  required "an immutable kernel-backed `Executable` object".
- Exit/fault/serial evidence: not applicable. The evidence is source
  cross-reference, recorded per row below.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Extracted every `` just <target> `` mention in `roadmap/` and diffed against the Justfile's 157 real targets: 42 unresolved | Weak signal. Nearly all are *planned* gates for unstarted milestones, which the roadmap format permits. Dropped this as a primary detector |
| 2 | Grepped `roadmap/` for retired artifacts (`kernel/src`, `SLIMECM`, `int 0x80`, `KERNEL_HALF_START`, `MAX_CAPS`, `SYS_WAIT`) | Hits concentrated in `02-core-runtime.md` and `08-native-development.md`; `07` hits were mostly inside P5's own completed sub-slices |
| 3 | Fanned three read-only scouts over the seven track files not already in hand, each told to record a milestone's `**Status:**` *before* judging its prose | 03, 04, 05, 06 came back with zero defects; 01 and 09 with cosmetic items only; 07 and 08 with four real defects |
| 4 | Checked whether `SLIMECMP` exists anywhere in code | 5 occurrences, all describing component/v1 as legacy: `contracts/component/v1/schema.zt:34` defines it, `contracts/component/v2/gen_rust.zt:91` emits it as `COMPONENT_IMAGE_LEGACY_MAGIC`, and `contracts/component/v2/schema.zt:46` names `SLIMECME` as the ELF-carrying revision P5.2 introduced |
| 5 | Checked C8's declared wait bound against the contract | The text said "the existing `SYS_WAIT` bound of eight"; the real value is `MAX_INGRESS_SOURCES = 9` (`boot-contracts/src/generated/fabric_graph.rs:14`). Both the mechanism and the number were stale |
| 6 | Confirmed the crate name C10.3 would extend | `components/runtime/Cargo.toml:2` is `slime-rt`; C10.3 said "add a `GlobalAlloc` to `components/runtime` ... matching the audited kernel heap", naming a heap that no longer exists to match |
| 7 | Checked whether the `SYS_WAIT` park lint C8.3 cites still exists | `grep -rn 'SYS_WAIT' scripts/` returns nothing, and `fabric_authority_check` is now an alias for `sel4_stream_check`. The citation is inside a frozen exit condition, so it stays as recorded history rather than being rewritten |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/README.md` | Backlog row said "**B70 open**" and the Component platform row said CP2's migration was unfinished, one commit after `b9553b7` closed both; mermaid node likewise | The index agrees with the committed evidence it summarizes |
| `roadmap/10-component-platform.md` | Track status `CP0, CP1, CP3, CP4 complete; CP2 partial; CP5 not started` → CP0–CP5 complete; CP2's status records the three-way split that retired the last nine `include!` sites | A track's own status matches its milestones' recorded outcomes |
| C10 motivation | `SLIMECMP` header → the `SLIMECME` header's `stack_bytes`, bounded by `MAX_STACK_BYTES` and validated in `boot-contracts/src/component_image.rs`; added B70's stack-overflow incident as the standing evidence that build-time-sized buffers are a live hazard | The milestone's premise cites a format the product admits, and its motivation rests on an observed failure rather than only on argument |
| C10 architecture decisions | "kernel object", "frames drawn individually", "pages return to the kernel" → no seL4 object kind and no root-tracked object; frames retyped individually into the child VSpace (`slime-root/src/child_vspace.rs`); frames return through the task-arena revocation B9 established | The mechanism split names seL4, `slime-root`, and `slime-rt` as they actually divide the work |
| C10.1–C10.3 | "growth syscall" → one operation declared in `contracts/syscall-abi/v1`; `kernel-wide ceiling` → root-wide; `builder/kernel drift` → builder/root; "only by kernel tests" → `slime-root`'s host unit tests; "matching the audited kernel heap" deleted | An implementer is pointed at the contract that must declare the operation, not at a deleted crate |
| C9 | Depends on P1 → P5; "keep scheduling mechanism in the kernel" → seL4 TCB priorities and, where its proof permits, MCS, with B48's deferral named so a class contract must state which it rests on; wait sets built on the native Endpoint/Notification mechanism B46 established | C9's unbuilt contracts sit on mechanism that exists, and its one real open question is stated rather than implied |
| C8 architecture decision | "the existing `SYS_WAIT` bound of eight" → `MAX_INGRESS_SOURCES`, declared in `contracts/fabric-graph/v1` and currently 9 | A declared bound is cited from its contract, so it cannot silently drift again |
| C8 depends-on, track sequencing, track tail | `x86_64-qemu-virtio` as a current path, "non-x86 boot", and "P1, P2, or P3 gate" → the product path, "physical-board boot", and "P4 or P5" | The track stops describing x86-64 as the default and points at the gates that exist |
| P3 | Rescoped from a custom RV64 kernel port (S-mode kernel, Sv39 tables, trap decoding, `ecall` entry, context switching) to a seL4 configuration port: pin the upstream kernel config and artifact hashes, add the P0 target profile, build `slime-root`, replay the corpus | P5's decision that architecture bring-up is upstream's problem is applied to the milestone that most contradicted it |
| P4 | Depends on P2 → P5; added building the `bcm2712` upstream seL4 kernel/loader from existing pins, and sourcing memory map, UART, GIC, and timer facts from seL4's BootInfo rather than a Slime-side board table | The board milestone qualifies the kernel it will actually run |
| D-track boundaries, ownership, D2, D4 | `SLIMECMP` → `SLIMECME` throughout, with component/v1 named as refused rather than as a target; "unread by the kernel loader" → the loader in `slime-root/src/child_vspace.rs`; "kernel-backed `Executable` object" → root-tracked record; `kernel-wide` limits → root-wide | The deferred language track targets the one revision the product admits |
| `roadmap/01-foundations.md` | M1 and M2 statuses labeled "Complete on the retired custom-kernel path ... historical record", following the precedent P2.1/P2.2 set | A reader cannot mistake x86 APIC bring-up or root-owned channels for current proof |
| `roadmap/09-rpi5-ros2-demo.md` | Sequencing line 26 "RP2 proves the architecture-neutral kernel" → the capability, component, and generation semantics on `aarch64-sel4-qemu-virt` | The summary agrees with RP2's own body, which was already correct |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just devlog_check` | Passed: 206 entries, 206 indexed. Validates this entry's `Roadmap` ids against real roadmap headings, its `Gates` against real Justfile targets, and every `devlog/` path in the repository | Direct |
| Retired-vocabulary sweep over `roadmap/` for 11 terms, each surviving hit tested for a self-labelling marker (`retired`/`historical`/`superseded`/`deleted`/`Was:`) | 7 occurrences remain, 3 self-labeled, 4 unlabeled and all inside frozen records — see *Open risks* | Direct |
| `SLIMECMP` occurrences in `roadmap/` | 9 before (3 in `02`, 6 in `08`), 1 after, and that one explicitly names component/v1 as refused | Direct |
| Roadmap-named `just` targets vs the Justfile's 157 real targets | 42 unresolved, all confirmed to be `Planned verification target` entries for unstarted milestones, which the roadmap format permits | Direct |
| Runtime tests | Not run. Documentation-only change; no Rust, contract, or script source touched, so no behavior can regress | Direct |

## Open risks and follow-ups

- [ ] Four retired-vocabulary occurrences remain and were left deliberately, because rewriting them would falsify an observed result: `00-backlog.md:318` and `:1319` (inside the Resolved log), `02-core-runtime.md:358` (C8.3's frozen exit condition, which cites a `SYS_WAIT` park lint that `grep -rn 'SYS_WAIT' scripts/` shows no longer exists), and `07-architecture-portability.md:371` (P5.4.1's kernel-test equivalence map, whose whole subject is the deleted tests). A future audit that wants these readable should append a correction, not edit the claim.
- [ ] C9's scheduling-class contract has a real unresolved dependency this audit only documented: B48 deferred AArch64 MCS until its proof is complete, so C9 must either rest on plain TCB priorities or wait. Owner: C9.
- [ ] `02-core-runtime.md` line 27 still says H2 consumes "P1's extracted architecture/platform boundary". P1 is complete and x86-specific; whether H2's userspace-driver boundary should now cite P5 was not investigated, because the Framework track is deferred and 04 audited clean.
- [ ] This audit judged specifications, not gates. It did not check whether any *completed* milestone's cited gate still observes what its exit condition claims; `2026-08-17-structural-audit/` is the precedent for that separate question.

## Artifacts and provenance

- Focused report: none; every finding is recorded in the *Changes* and *Investigation log* tables above with its source cross-reference.
- Raw transcript: not retained. The three scout transcripts were read-only investigations whose conclusions are reproduced above; the sweeps are reproducible from the terms listed in *Verification*.
- Serial/debugger/model output: none. No guest code ran.
- Related roadmap item: [C9](../../roadmap/02-core-runtime.md), [C10](../../roadmap/02-core-runtime.md), [P3 and P4](../../roadmap/07-architecture-portability.md), [D2 and D4](../../roadmap/08-native-development.md)
- Predecessors: [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../2026-08-09-p5-4-final-kernel-retirement/index.md), [`devlog/2026-08-22-b70-profile-include-closure/`](../2026-08-22-b70-profile-include-closure/index.md)
