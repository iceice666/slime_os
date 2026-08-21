# RP2: one generation carrying the data path and the component graph, and the two arms that were never observed

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/generation.rs`, `components/bins/src/bin/init.rs`, `slime-root/src/boot_selector.rs`, `contracts/generation/v1/fixtures/sel4-demo.zti`, `contracts/boot-layout/v1/fixtures/sel4-demo.layout`, `scripts/build/{build-sel4,build-generation}.py`, `scripts/check/{check-sel4-demo-plane,check-sel4-boot-layout,check-sel4-boot-selection,check-sel4-gate-controls,check-generation}.py`, `Justfile` |
| Roadmap | RP2 |
| Gates | `just sel4_demo_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just sel4_boot_selection_check` |
| Trigger | RP2's status recorded two arms — rollback on an AArch64 generation pair and wrong-target rejection — as owed to the demo, with no gate exercising either, plus a demo-scoped replay of the C7/C8 data path under one generation rather than across plane fixtures. |
| Baseline | `just sel4_sample_check` proved the C7 half, `just sel4_stream_check` the C8 half, and `just sel4_component_graph_check` the product graph — each over its own generation. `just sel4_boot_selection_check` paired two `sel4` product generations. Every wrong-target assertion in the repository was host-side or a unit test. |

## Summary

RP2 asked for one demo-scoped AArch64 generation running the component model and
the data path together, with its rollback and wrong-target arms observed on that
same profile. The three properties existed separately and the composition did
not: no generation carried the C7 exchange, the C8 route graph, and the product
graph at once, so "the component-launch and data path together" was an inference
across three images rather than an observation of one.

A `demo` boot action and `contracts/generation/v1/fixtures/sel4-demo.zti` make it
one manifest. Both compositions are the existing ones — what is new is the
generation, so the slice reuses the compositions the sample and stream gates
exercise (their provisioning, denial, and loan paths, not their scripted
mid-stream death, which is armed for `stream`/`qos`/`fault` only).
`just sel4_demo_check` observes all three parts in one transcript, plus a
failing pending demo generation rolling back to a verified demo known-good root,
plus a component image qualified for another admitted target being refused before
any of its bytes are mapped.

Three defects surfaced only on real boots, each after the change that exposed it
looked correct by reading: a double-freed supervision handle, a capability-role
query that correctly refused a question the new graph made ambiguous, and an 8 MiB
`.bss` buffer in the boot selector that was silently costing half the root CNode.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/generation.rs` | `BootAction::Demo = 29`, its `"demo"` spelling, and both frozen-numbering tests extended | The numeric ABI stays pinned; a manifest naming an unknown action still fails admission |
| `components/bins/src/bin/init.rs` | `boot_action::DEMO` mirror constant (compile-time asserted equal); `drive_demo_plane`; the `DEMO` dispatch arm *returns* so `main` launches the product graph | One generation composes all three parts; the ABI copy cannot drift from the contract |
| `components/bins/src/bin/init.rs` | `spawn_service_caps` and `launch_fabric_graph` resolve their factory by grant name instead of by `kind:` capability role | A component asks the question the manifest answers; the role query keeps refusing genuine ambiguity rather than guessing |
| `contracts/generation/v1/fixtures/sel4-demo.zti` | New: 13 executables, 13 instances, 26 grants, the `sel4-stream` fabric graph, the product graph, and the C7 pair | The demo's composition is authenticated generation data, not a build flag |
| `slime-root/src/boot_selector.rs` + `build-generation.py` | `SELECTOR_GENERATION_BYTES` 8 MiB → 4 MiB, both statements, with an agreement check | The selector can boot every generation the product can; two statements of one ceiling cannot drift |
| `scripts/build/build-generation.py` | `SLIME_WRONG_TARGET_EXECUTABLE=<executable>=<profile>` re-qualifies one declared executable for another *admitted* profile, body intact | Roadmap invariant 9 is observable on a boot, not only host-side |
| `scripts/check/check-sel4-demo-plane.py`, `Justfile` | New three-arm gate `just sel4_demo_check`, with `just rpi5_arm_slice_check` as RP2's roadmap alias | RP2's exit condition has a named gate |
| `check-sel4-boot-layout.py`, `check-sel4-gate-controls.py` | `sel4-demo` registered in `PLANES` (26 planes) and `GATES` (33 gates, 29 markers) | A new plane inherits the standing layout and mutation controls |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The three parts stop composing in one generation | `just sel4_demo_check` | A chain's marker missing, or `check_ordered_across_chains` reporting the stages out of order |
| A wrong-target image reaches the loader | same gate, wrong-target arm | `wrong_target=1` absent, or any `[sample-lender]` line proving it executed |
| Rollback stops returning to the known-good demo root | same gate, rollback arm | `number=99` surviving into the fallback boot, or a mutation outside BootState |
| The gate itself gets weaker | `just sel4_gate_control_check` | The pinned 29-marker count no longer matches |
| The demo plane's resolved slots move | `just sel4_boot_layout_check` | `sel4-demo.layout` disagrees with what the root resolved |
| The two selector ceilings drift apart | `just sel4_boot_selection_check` | `selector ceiling disagrees: builder … root task …` |
| A generation outgrows the selector's buffer | same gate, `assert_ceiling_holds_every_generation` | `<name>'s generation is N bytes against a selector ceiling of M` |
| The poisoned wrong-target image outlives a failed arm | `check_wrong_target`'s `finally` | A later gate booting an image qualified `aarch64-rpi5` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_demo_check` | Pass, all three arms | Direct |
| Slice arm terminal | `SLIME_GRAPH HEALTHY generation=1 required=4 live=0 completed=4 failed=0`, `loans=0 mappings=0 regions=0 orphans=0`, `tasks reclaimed live=0` | Direct |
| Slice arm composition | `executables=13 instances=13 grants=26`, `fabric graph=admitted schemas=2 routes=2 participants=6` | Direct |
| Wrong-target arm | `elf=12`, `loadable_executables=12 … wrong_target=1`, spawn refused, no byte of the image executed | Direct |
| Rollback arm | 99 selected twice (attempts 1, then 0), then 1 with `pending=0 attempts=0`, stable; only BootState sectors mutated | Direct |
| `just sel4_gate_control_check` | 33 gates reject 1279 mutated transcripts and layouts | Direct |
| `python3 scripts/check/check-boot-layout-resource.py` | 26 seL4 planes agree, 121 rows | Direct |
| `just sel4_boot_selection_check` | Pass with the 4 MiB selector buffer | Direct |
| `just sel4_sample_check` | Pass (pre-change baseline for the C7 half) | Direct |
| Selector `.bss` | 10.99 MB at 8 MiB, 6.79 MB at 4 MiB — an 8.39 MB / 4.19 MB delta matching the array exactly | Direct |
| Rollback assertion non-vacuity | `expect_selected` rejects a wrong generation number; `only_boot_state_changed` rejects both an out-of-range mutation and an unchanged BootState | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| `just test_sel4_root`, `just test_host` | 131/131 root, host suites pass | Direct |
| Read-only review, two rounds: a three-lens panel then a pass over the applied fixes | 13 findings then 7, all applied; see below | Direct |
| Ceiling assertion non-vacuity | Lowering the ceiling below a measured generation fails naming it; observed as `sel4-traffic-generation's generation is 1569445 bytes against a selector ceiling of 1048576` while the check still globbed the build tree | Direct |

### Review findings applied

Two rounds, 13 findings then 7, all applied. No defect in an executable path
either round. What the rounds did find was worth the passes: comments this
patch's own change falsified, an assertion sound only by statement ordering, and
two gate-integrity defects in the new gate itself — a cleanup that ran only on
success, and a headroom check that failed on a clean checkout.

#### Round 1 — three-lens panel (canonical, correctness, convention)

| Priority | Location | Finding | Resolution |
|---|---|---|---|
| P2 | `init.rs` `drive_demo_plane` | The second-status assertion was sound only because no spawn separated it from the collection loop. `free_slot_from(1)` returns the *lowest* free slot, so any later spawn — `launch_fabric_graph` makes six — reuses a just-released number and the assertion would pass reading another task's live handle | Moved the assertion inside the collection loop, per handle, and recorded why the placement is load-bearing |
| P2 | `init.rs` `compose_declared_graph` | Doc said only `PRODUCT` returns; the new `DEMO` arm returns too | Corrected |
| P2 | `init.rs` `main` | Comment claimed `sel4.zti` is the only generation reaching that body, which the returning `DEMO` arm falsified | Corrected to name both generations |
| P2 | `init.rs` `drive_demo_plane` | Comment claimed a declared interposition probe; `sel4-demo` declares `interpositions = []` and no `fabric-intruder-supervision`, so the launcher takes its `without_proxy` arm | Corrected; `fabric-intruder` is the denial control here, not a hop |
| P2 | `check-sel4-demo-plane.py` | `only_boot_state_changed` compared only *below* the BootState offset, so any write from sector 40 to the end of the disk passed — the gate could not observe the property RP2's exit condition records | Bounded to the two slots, matching `only_slots`; verified it now rejects a write past them |
| P2 | `check-sel4-demo-plane.py` | The poisoned wrong-target image was built unconditionally but the restoring rebuild was gated on `--no-build`, leaving a falsely qualified artifact for the next gate to boot | Cleanup made unconditional, and `--no-build` with that arm is now refused rather than silently ignored |
| P3 | `init.rs` `spawn_service_caps` | Comment said the demo generation grants init two factories; it binds three, contradicting this patch's own other comment | Corrected with slot numbers |
| P3 | `init.rs` factory resolvers | Both doc comments still stated the pre-migration rule about which resolver is safe where | Rewritten against the verified fixture set |
| P3 | `init.rs` `launch_fabric_graph` | Comment blamed RP2 for the role query's ambiguity and presented an incomplete fixture survey as exhaustive. `sel4-boot`/`sel4-traffic` already bound two, which is why `resolve_own_buffer_factory` predates this change | Corrected; survey extended to all six fixtures and verified |
| P3 | `check-sel4-gate-controls.py` | Comment implied the rollback arm had a guarded marker table; it has none | Restated to say plainly that both non-slice arms are uncovered by this control |
| P3 | `boot_selector.rs` / `check-sel4-boot-selection.py` | The agreement check pinned the two constants to each other but not to the artifact the constraint is about, while its comment positioned it as the latter | Added a derived assertion over every built generation, proven non-vacuous |
| P3 | `roadmap/README.md` | "Demo-first sequencing" item 3 still described RP2's three arms as outstanding | Updated |
| P3 | `Justfile` / gate naming | The plane introduced four spellings and no base gate, against the convention where roadmap names alias a `sel4_*` base | Renamed to `sel4_demo_check` / `check-sel4-demo-plane.py`, with `rpi5_arm_slice_check` as the roadmap alias |

#### Round 2 — on the applied fixes

Round 2 confirmed both round-1 logic fixes independently: the relocated
per-handle assertion is order-independent (`resolve_supervision` on a dropped
slot returns `BadCapability`, and no spawn intervenes), and
`only_boot_state_changed`'s window matches what `commit_state` writes
(`SLOT_BYTES = 512`, `BOOTSTATE_SLOT_COUNT = 2`).

| Priority | Location | Finding | Resolution |
|---|---|---|---|
| P2 | `check-sel4-boot-selection.py` | The headroom check globbed `build/*-generation/`, so it depended on what a previous run had left behind and failed outright on a clean checkout, where `--boot-selection` builds an image but no generation | Split into `assert_ceiling_agrees` (constants) and `assert_ceiling_holds_every_generation` (the three generations this run writes to the store, passed explicitly) |
| P2 | `check-sel4-demo-plane.py` | The wrong-target cleanup ran after the assertions, and every `fail()` raises — so the poisoned `aarch64-rpi5` image was left behind exactly when something had gone wrong, the hazard its own comment claimed to close | Cleanup moved into a `finally` around the arm |
| P3 | `init.rs` `spawn_service_caps` | The round-1 fix quoted `instance=2` from the *pre-merge* boot; the merged fixture reports `instance=8`, so the observation was unreachable | Marker quoted without the instance index, with a note saying why |
| P3 | `init.rs` `drive_demo_plane` | "the same evidence the stream gate froze" is falsified by `STREAM_DEATH_VARIANTS`, which arms the scripted mid-stream death for `stream`/`qos`/`fault` only | Claim narrowed to the provisioning, denial, and loan paths, in the source, the roadmap, and this entry |
| P3 | `init.rs` `launch_fabric_graph` | The survey implied a closed two-caller set; `resolve_own_buffer_factory` has eleven call sites | Scoped to the fixtures reaching this launcher, stating that other callers exist |
| P3 | `boot_selector.rs` | "not the store's" named a bound store v1 does not carry | Restated: store v1 declares no ceiling, so this constant is the only bound |
| P3 | this entry | Claimed 14 findings over a 13-row table | Counted |

## Decisions

- **Decision:** Reuse `drive_sample_plane`'s exchange and `launch_fabric_graph`'s
  graph rather than authoring a third scenario.
  **Rationale:** RP2 asks for the two paths "under one demo-scoped generation
  rather than across separate plane fixtures" — the *generation* is what has to
  be new. Reusing them means the slice's data-path evidence is the evidence two
  existing gates already exercise, minus their scripted mid-stream death, which
  `build-sel4.py` arms for `stream`/`qos`/`fault` only.
  **Rejected alternative:** A bespoke demo scenario, which would assert
  something no other gate has observed and therefore prove less.

- **Decision:** Give `demo` its own `BootAction` rather than a build flag.
  **Rationale:** The composition is then authenticated generation data delivered
  at activation, so two builds of the image cannot disagree about which graph
  they boot. `fabric-service` needed no new branch at all: `demo` matches none of
  its named actions and falls through to exactly the stream composition.

- **Decision:** Lower the selector's generation buffer to 4 MiB instead of
  shrinking the demo generation or raising the root CNode.
  **Rationale:** The buffer is `.bss`, so the loader creates one root CSlot per
  page *before* the root runs; 8 MiB spent ~2048 of the root CNode's 4096
  (`CONFIG_ROOT_CNODE_SIZE_BITS = 12`) on an array that is almost entirely zero.
  That is a capacity limit on what the selector can boot, not mere waste. 4 MiB
  still leaves 2.6× headroom over the largest generation this repository builds
  (`sel4-traffic`, 1.57 MB).
  **Rejected alternative:** Raising `CONFIG_ROOT_CNODE_SIZE_BITS`, which edits
  pinned kernel configuration for a userspace sizing mistake.

- **Decision:** Resolve the two factories by grant name rather than relaxing the
  `kind:` role query.
  **Rationale:** The refusal was *correct*. The demo generation grants init three
  factories, and which factory a given service allocates from is a graph fact the
  manifest states, not a property of the capability. This is the same remedy B70
  recorded for `spawn-service`'s RPC endpoint: ambiguity refusal beats a plausible
  wrong answer.
  **Rejected alternative:** A lowest-slot tiebreak in the role query, which would
  silently hand out a factory the manifest assigned to someone else.

- **Decision:** Have `--arm` print only the arms it actually ran.
  **Rationale:** A partial run printing the full three-arm sentence is a false
  evidence claim — the precise shape (`B67`, `B72`, `B73`, `B75`) this repository
  keeps finding in its own gates.

## Open risks and follow-ups

- [x] The 4 MiB selector ceiling was headroom stated in prose rather than a
      checked property. Closed during review: `assert_ceiling_agrees` pins the
      two constants, and `assert_ceiling_holds_every_generation` measures the
      generations this gate writes to the store against the ceiling, naming the
      offender. Scoped deliberately to those three: only a generation the store
      names ever reaches the buffer, so `sel4-traffic` — the largest fixture the
      repository builds, at 1.57 MB — is not in scope. A first attempt globbed
      `build/*-generation/` instead and was wrong twice over: it depended on what
      a previous run had left behind, and it failed on a clean checkout where
      `--boot-selection` builds an image but no generation.
- [ ] The wrong-target arm refuses at *spawn* rather than at whole-generation
      admission: `Admission::admit` counts `wrong_target` and returns `Ok`. That
      is the existing documented behavior (`PayloadFormat::WrongTarget` is
      non-loadable and reported), and RP2 asks only that the artifact be rejected
      before mapping, which is observed. Whether a generation containing a
      wrong-target executable should fail admission outright is a separate
      question this entry does not settle.
- [ ] RP2 is QEMU evidence for `aarch64-sel4-qemu-virt` only. Per roadmap
      invariant 8 it makes no Raspberry Pi 5 claim; RP3 owns the board.

## Artifacts and provenance

- Focused report: none; the investigation is short enough to live in this entry.
- Raw transcript: not retained. The three arms are reproducible with
  `just sel4_demo_check`, and the frozen layout is
  `contracts/boot-layout/v1/fixtures/sel4-demo.layout`.
- Serial/debugger/model output: the marker sets this entry cites are the gate's
  own `CHAINS` and `WRONG_TARGET_MARKERS` tables in
  `scripts/check/check-sel4-demo-plane.py`.
- Related roadmap item: [RP2](../../roadmap/09-rpi5-ros2-demo.md#rp2--aarch64-qemu-product-vertical-slice)
