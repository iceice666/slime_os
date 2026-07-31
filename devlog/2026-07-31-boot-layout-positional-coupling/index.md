# Boot capability layout is a positional convention, not generation data

| Field | Value |
|---|---|
| Date | 2026-07-31 |
| Kind | Decision |
| Status | Proposed |
| Scope | `kernel/src/runtime/bootstrap.rs`, `kernel/src/capability/mod.rs`, `contracts/generation/v1/fixtures/valid.zti`, `scripts/build/build-generation.py`, `scripts/check/*.py`, every `SLIME_*_CHECK` gate |
| Roadmap | B10, B11, P1, P2, RP2, C8, C8.10 |
| Gates | none |
| Trigger | Reviewing whether the repository has accumulated too many subsystems ahead of the RPi5 ROS 2 pivot |
| Baseline | C8.10 full-graph boot passes with its own `launch_fabric_boot_init` layout; every earlier fabric gate still observes its declared serial markers |

## Summary

A survey asking whether Slime OS has too many subsystems found the kernel decomposition unremarkable and located the real debt in one mechanism: the boot capability vector in `kernel/src/runtime/bootstrap.rs` is a *positional* convention rather than generation-declared data. Init's capability slots are written by index (`caps[46] = ...`), gates read those indices positionally, and `MAX_CAPS = 64` is 61 occupied before C8.10. A new participant set therefore cannot be appended; it must either squat on another profile's slots or fork a whole new `launch_*_init`. The escape hatches this forced — 21 distinct `option_env!("SLIME_*")` compile-time flags across 70 sites, nine `generation.number ==` branches inside `launch_init`, three `launch_*_init` forks, and one 42-component `valid.zti` holding 16 scaffolding participants — are symptoms of that single constraint, not independent sprawl. Because the selection happens at kernel compile time, each gate builds a *different kernel binary*. This is recorded as `Proposed`: nothing is changed by this entry, and the two follow-on backlog items (B10, B11) carry the work.

## Changes

No code, contract, or gate changed. This entry records a diagnosis and its proposed cut order; the executable work is filed as B10 and B11 in [the backlog](../../roadmap/00-backlog.md).

| Area | Change | Restored invariant |
|---|---|---|
| Backlog | Opened B10 (positional boot layout and compile-time gate selection) and B11 (test scaffolding declared in the product boot generation). | Backlog again names the debt that stands between the current tree and P1/P2, per this repository's backlog-before-roadmap rule. |
| Roadmap sequencing | Recorded that B10 is a prerequisite for P1, not cleanup that can follow it. | P1's requirement that architecture-neutral code type-check for AArch64 cannot hold while boot paths are selected by x86-gate build flags. |
| Devlog | Recorded the mechanism and the evidence for it before any refactor rewrites the callsites that show it. | The reasoning behind the cut order stays inspectable after the code that motivated it changes. |

## Decisions

- Decision: The load-bearing defect is that init's capability slot layout is a positional convention in kernel source rather than named grants resolved from the generation. Fabric surface area, broker duplication, and the probe-heavy fixture are downstream of it and are not addressed first.
- Rationale: The source says so directly. `bootstrap.rs:176-182` explains that the vector is "61 of `MAX_CAPS = 64` before this milestone adds anything", that the three new C8.10 roles "need nine slots against three free", and that the vector "is also the layout six passing QEMU gates read positionally — the `caps[46] = ...` blocks below rewrite it per generation number — so renumbering it to fit would rewrite C8.3-C8.8's evidence rather than extend it". Every workaround observed follows from that sentence: 13 distinct positional writes over slots 46-59 across 26 statements, `if generation.number == 14` reassigning slots 46/47/49 with the comment that "the call gate reuses the executable/control slots occupied by three stream participants in every other generation profile", and the mutually exclusive call/operation profiles at lines 793 and 828 sharing one slot range.
- Rejected alternative: Treating fabric's mass as the primary finding. Fabric does hold 10 of 31 `contracts/` schemas, 20 of 43 component binaries, and roughly 5.7k of 15k lines in `components/bins`, but that depth is the milestone content of C8.1-C8.10 and cutting it would discard delivered capability. The constraint that makes it *unextendable* is the slot layout.

- Decision: Boot-path selection must stop being a kernel build-time input. Named grants resolved from the generation replace `caps[N] = ...`, which removes the `option_env!` branches and the `generation.number ==` branches together.
- Rationale: `option_env!` is evaluated at compile time, and Cargo's dep-info for the kernel records `env-dep:SLIME_DANGO_CHECK`, `env-dep:SLIME_GENERATION_CMD_CHECK`, `env-dep:SLIME_POWERBOX_CHECK`, and siblings — so changing a gate flag invalidates the kernel build. The check scripts set these per run (`check-fabric-stream.py` sets `SLIME_FABRIC_STREAM_CHECK=1` with `SLIME_GENERATION_NUMBER=12`; `check-fabric-qos.py` sets `SLIME_FABRIC_QOS_CHECK=1` with 13; `check-data-fabric-boot.py` sets `SLIME_FABRIC_BOOT_CHECK=1`, matched in the kernel against `generation.number == 17`). Eleven distinct generation numbers appear across the check scripts (6, 7, 8, 9, 10, 11, 12, 13, 14, 16, 99). The consequence is that no single kernel binary satisfies the gate suite, and "the kernel that passed the gates" is not one artifact.
- Rejected alternative: Renumbering the slot vector so every profile fits. `bootstrap.rs:180-182` states this rewrites C8.3-C8.8's evidence rather than extending it. Any accepted fix must resolve each existing profile to the *same* slot numbers it occupies today, so positional gate reads keep observing what they observe now.

- Decision: Component-set selection reuses the existing `fabricGraph.profiles` mechanism rather than introducing a second selector.
- Rationale: `build-generation.py` already resolves a named profile (`resolve_fabric_graph`, line 616; `selected_profile_name`, line 563; `SLIME_FABRIC_PROFILE`), and `valid.zti` already declares `default`, `visibility`, and `unified`. That mechanism currently governs interposition chains only, not which components a generation declares, which is why all 42 components — including `storage-probe`, `fabric-intruder`, `fabric-observer`, `fabric-proxy`, `sample-lender`, and 11 other scaffolding participants — live in one manifest with `-control` endpoints and real capability grants.
- Rejected alternative: A separate test-generation file. Two manifests would duplicate the route, QoS, and budget declarations that the fabric graph already resolves, and would let the product and test paths drift.

- Decision: Broker consolidation (`call_broker.rs` at 1299 lines and `operation_broker.rs` at 1418 lines share a near-identical skeleton) is sequenced last and is optional.
- Rationale: Both brokers' callers depend on fixed control slots. Merging them before the layout is named would relocate the coupling instead of removing it.
- Rejected alternative: Leading with the broker merge because it is the largest single duplication by line count. Line count is not the constraint; slot addressing is.

## Open risks and follow-ups

- [ ] B10 must demonstrate that named-grant resolution yields byte-identical slot assignments for every profile in use today, or the six positional gates lose their meaning. An equivalence check comparing resolved slots against the current vector is the minimum evidence.
- [ ] The proposed cut has no gate yet. `just architecture_contract_check` and `just x86_portability_check` are named in P0/P1 as planned targets and do not exist; B10's exit condition needs a target that exists when it is claimed.
- [ ] `generation.number` also selects *storage* identity at `bootstrap.rs:571` and `bootstrap.rs:595` (numbers 2, 3, 4 pick different capabilities and a different storage component). That is the same pattern as the fabric branches but on a different axis; B10 should state whether it is in scope.
- [ ] Whether ~2.4k lines of *service* in the kernel (`runtime/generation_service.rs` 803, `runtime/generation_manager.rs` 286, `storage/object_store.rs` 466, `storage/recovery.rs` 400, `storage/store_service.rs` 261, `storage/block_service.rs` 212) violate this repository's policy-free-kernel rule is an open question raised by the same survey. It is deliberately **not** folded into B10 or B11: no evidence was gathered on it, and it needs its own audit. **[INFERENCE]** that it is a problem at all.
- [ ] Component-side flag use (52 `option_env!` sites in `components/`) may not fall out of B10 automatically; a component reading `SLIME_FABRIC_VISIBILITY_CHECK` at 9 sites is making its own build-time decision independent of the kernel's layout.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none. Every figure in this entry is read from source at the commit named in *Baseline*; no guest run was performed for it.
- Related roadmap item: [B10 and B11](../../roadmap/00-backlog.md), [P1 and P2](../../roadmap/07-architecture-portability.md), [C8.10](../../roadmap/02-core-runtime.md), [RP2](../../roadmap/09-rpi5-ros2-demo.md).
