# P5.4.1 — the oracle equivalence inventory

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Audit |
| Status | Verified |
| Scope | `Justfile`, `kernel/tests/*.rs` (19), `scripts/check/*.py` (43 before this slice, 44 after), `scripts/lib/harness.py`, `slime-root/src/{channel,graph,transit,main}.rs`, `components/bins/src/bin/{init,crossing-peer}.rs`, `contracts/generation/v1/fixtures/sel4-crossing.{zti,md}`, `scripts/build/{boot_layout,build-sel4,build-generation}.py`, `scripts/check/check-sel4-crossing-plane.py` |
| Roadmap | P5.4.1, P5.4, B22, B24, B16, B12, B23 |
| Gates | `just devlog_check`, `just sel4_crossing_check` |
| Trigger | P5.4's decomposition (2026-08-07) named this inventory as the artifact its exit condition asks for and no one has produced |
| Baseline | Eight seL4 gates passing; P5.4's text claiming "equivalents through C8.4, none for C8.5–C8.10" |

## Summary

Mapped every acceptance check the frozen `kernel/` oracle guards to its observed
seL4 equivalent or an explicit gap, across all three legacy surfaces P5.4 named.
The recorded belief is **half right and materially incomplete**: C8.5–C8.10
having no seL4 equivalent is confirmed, but "equivalents through C8.4" overstates
C8.1–C8.4, and the whole **M5.x/M6.x/B10/B11 class — nineteen closed oracle
milestones with named Justfile gates — is unmentioned by P5.4 and uncovered on
seL4** (ten M5 gaps, five M6 gaps, two M6 partials, B10 and B11; M5.2a, M5.6a,
and M5.6b are host or model checks and survive deletion).
Two of P5.4's own three figures were also wrong (11 direct kernel-test
recipes, not 8; 24 legacy checkers, not the 34 harness importers). The
lifetime-vs-live bounds audit found **a third defective table beyond B16 and
B22** — `SharedBufferTable::quotas`, which B16's sweep had implicitly cleared —
and B22 itself is now fixed and gated by `just sel4_crossing_check`, with both
fault injections confirmed failing.

## Observable symptom

- Command: `just devlog_check`, `just sel4_crossing_check`
- Expected: every acceptance check `kernel/` guards is named with an observed
  seL4 equivalent or a recorded gap, and lifetime-vs-live bounds in `slime-root`
  are audited as a class.
- Observed before this entry: no such mapping existed. P5.4's exit condition
  ("every acceptance check the custom kernel guards has an observed seL4
  equivalent") was a claim with no artifact behind it, and its supporting figures
  were counted by hand and partly wrong.
- Exit/fault/serial evidence:
  [`crossing-plane-boot.log`](crossing-plane-boot.log) for B22's arm;
  the tables below for the inventory itself.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `Justfile` has **11** `cargo test --test <kernel binary>` recipes, not 8 (`:183,189,194,198,202,215,223,232,295,302,350` in the post-change file; each was 10 lower before this slice inserted `sel4_crossing_check`) | Three oracle milestones — C7.6, C7.7, M5.1 — were invisible to P5.4's direct-surface count. The three `fabric_*` ones sit as second commands inside python-first recipes, which is likely how they were missed |
| 2 | `cd kernel` appears in **31** recipes, matching P5.4 | Figure confirmed |
| 3 | **34** of the 43 checkers then present import `harness`, matching P5.4 — but only **24** actually depend on the oracle | "Imports harness" is not the legacy set. `ROOT`/`SECTOR_SIZE`/`load_script` are portable; only `RELEASE_KERNEL`/`release_kernel`/`run_qemu(cwd=kernel)` or a literal `ROOT / "kernel"` is dependence |
| 4 | `check-aarch64-boot.py` and `check-generation-commands.py` import **only portable** symbols yet build and boot the oracle | A harness-symbol audit alone misclassifies both. The classifier must also grep `ROOT / "kernel"` |
| 5 | 19 `kernel/tests/*.rs` carry **151** test entities (150 `#[test_case]` + `should_panic.rs`'s expected-panic `_main`); 130 assert architecture-neutral semantics, 21 assert custom mechanism | The neutral 130 are the replacement obligation; the 21 die with `kernel/` and need nothing |
| 6 | The eight ungated files hold **51** neutral entities, 32 of them in `object_store.rs` (M5.4) | The single largest block of unreplaced semantics in the corpus, and it has no seL4 counterpart of any kind |
| 7 | `kernel/tests/fabric_stream.rs:553` and `:629` are **C8.5** assertions inside the C8.4-gated file | Worse than the recorded belief implies: deleting `kernel/` removes them without `fabric_qos_check` — a Python-only gate with no kernel arm — turning red |
| 8 | `slime-root` never decodes the fabric-graph resource; grep for `FabricGraph`/`SLIMEFG` returns only doc prose, against `kernel/src/runtime/generation.rs:105-119` which validates it against kernel ceilings before launch | C8.2's exit condition ("aggregate admission before component launch") is entirely unmet on seL4, not partially |
| 9 | `ipc.rs:204-215` classifies nine operations `Mediation::Unavailable`, asserted statically by `check-sel4-component-graph.py:406-442` | That is a rigorous **declaration** of the M5/M6 gap, not coverage of it. Reading `unsupported=0` as coverage reads it backwards |
| 10 | **Inherited evidence** ([`2026-08-07-b16-supervision-records/`](../2026-08-07-b16-supervision-records/index.md)): B16's sweep claimed `Terminations` and `ChannelTable` are the only two per-task tables that never free, naming "the shared-buffer orphan and charge tables" as correct. Re-derived here rather than accepted | `charges` is correct (`shared_buffer.rs:1782-1784`). Its sibling one line above, `quotas` (`:502`), has **no free path at all** and was not named — the third table |
| 11 | `push` derives `key = self.len` (`channel.rs:446`) | A precondition for the B22 fix, not tidying: the moment any free path decrements `len`, the next key aliases a live channel, and `Resource::Endpoint { channel }` is the only handle a component holds. Exhaustion would become confused-deputy redirection |
| 12 | First B22 fault injection 2 (drop `Transit::holds_endpoint`) **passed** | The arm was decorative: init kept the in-flight pair's other end, so `GraphTables::holds_endpoint` alone found a holder. Fixed by having init `cap_drop` its half, leaving the transit entry the only thing naming the channel. Re-injected: fails |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| this entry | The three-surface equivalence inventory below | P5.4's exit condition has an artifact rather than a claim |
| `channel.rs` | `sweep(&mut ChannelTable, &GraphTables, &Transit)` frees every channel no live holder can name; `MAX_CHANNELS` documented as a live bound | `MAX_CHANNELS` bounds channels *live at once*, not channels a boot may mint — **B22** |
| `channel.rs` | `next_key` monotonic counter replaces `key = self.len`, with `checked_add` refusal; `minted` splits cumulative from live | A reclaimed key names nothing rather than aliasing a live channel |
| `graph.rs` | `GraphTables::holds_endpoint(ChannelKey)` | The live half of the predicate |
| `transit.rs` | `Transit::holds_endpoint(ChannelKey)` | The in-flight half — without it the fix reintroduces the defect |
| `main.rs` | `mint_channel` sweeps lazily on `TableFull` and retries, reporting `SLIME_GRAPH channels swept freed=… live=… minted=…`; terminal channel marker gains `minted=` | A reclaimed table is observable, and the transcript still states what happened |
| `crossing-peer.rs` | New component: holds a transferred end in `Transit` across the crossing, then proves it still resolves | B22's transit arm is load-bearing |
| fixture, `boot_layout.py` | `sel4-crossing.zti` and row 62 | B10's fixture/layout agreement for the new executable |
| `check-sel4-crossing-plane.py`, `Justfile` | The ninth seL4 image and its gate | B22's exit condition is observable |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_crossing_check` | Pass — 33 pairs minted over a 32-entry table; the first sweep reports `freed=28 live=4 minted=32` and the terminal line `minted=37` | Direct |
| Fault injection 1: sweep call removed | Fails at `[init] crossing plane fail: loop pair mint`, `channels swept freed=0 live=32 minted=32` — [`fault-injection-1-no-sweep.log`](fault-injection-1-no-sweep.log) | Direct |
| Fault injection 2: `Transit` half of the predicate removed | Fails at `[crossing-peer] fail: the collected end no longer resolves for send`, every earlier marker passing — [`fault-injection-2-no-transit.log`](fault-injection-2-no-transit.log) | Direct |
| Fault injection 3: `key = self.len` restored in `push` | Fails the source check with `push no longer derives its key from next_key` — [`fault-injection-3-key-from-len.log`](fault-injection-3-key-from-len.log) | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_channel_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_sample_check`, `sel4_stream_check`, `sel4_supervision_check` | All pass — the `minted=` append did not disturb the two gates matching the channel line | Direct |
| `just contracts_check`, `just generation_check` | Pass — `default_boot_layout.rs` regenerated (one `CROSSING_PEER_SLOT` constant); 19 fixtures agree with the resource | Direct |
| `just devlog_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| The inventory's own equivalence claims | Read from source at the assertion level (`REQUIRED_MARKERS`, `#[test_case]` bodies, `Mediation` table), not from docstrings | Direct, static |
| The oracle's own gates (`just test`, the 11 named kernel targets) | **Not run.** This slice changes no `kernel/` file, and the inventory reads the corpus rather than executing it | Unobserved |
| `slime-root`'s unit tests, including none added here | **Not run — nothing runs them.** See B23 | Unobserved |

## Open risks and follow-ups

### The inventory: three surfaces

**Surface 1 — direct `kernel/tests/*` targets (11 recipes, corrected from 8).**

| Recipe | Test binary | Milestone | seL4 equivalent |
|---|---|---|---|
| `spawn_prereq_check` | `spawn_authority.rs` | M6.1 | **partial** — `sel4_spawn_check` covers narrow grants, factory minting, supervision shape; generation-v2 determinism is uncovered |
| `shared_buffer_factory_check` | `shared_buffer_authority.rs` | C7.2 | covered — `sel4_loan_check` `class=ungranted`, `sel4_component_graph_check` buffer lifecycle |
| `shared_buffer_accounting_check` | `shared_buffer_accounting.rs` | C7.3 | covered — `sel4_loan_check` `quotas declared=3 budgeted=3`, per-holder ceilings parsed from the fixture |
| `shared_buffer_mapping_check` | `shared_buffer_mapping.rs` | C7.4 | covered — `sel4_root_boot_check` exact ranges, ro-write and wx refusals, teardown to zero |
| `shared_buffer_loan_check` | `shared_buffer_loan.rs` | C7.5 | covered — `sel4_loan_check`, every clause including single-return and in-transit reclamation |
| `fabric_manifest_check` | `fabric_manifest.rs` | C8.2 | **gap** — see below |
| `fabric_authority_check` | `fabric_authority.rs` | C8.3 | **partial** — rights algebra covered by `sel4_stream_check`; graph provenance is not |
| `fabric_stream_check` | `fabric_stream.rs` | C8.4 **+ 2× C8.5** | **partial** — transcript covered by `sel4_stream_check`; the structural arm and the two C8.5 assertions are not |
| `sample_descriptor_check` | `sample_descriptor.rs` | C7.6 | covered — `sel4_sample_check`, `sel4_stream_check`'s shared-sample hop |
| `sample_plane_check` | `sample_plane.rs` | C7.7 | covered for 4 of 5; the retained-x86-v2 arm needs no equivalent |
| `storage_cap_check` | `storage_capability.rs` | M5.1 | **partial** — rights algebra covered; block-protocol and DMA-outstanding arms are not |

**Surface 2 — harness-mediated checkers (24 legacy of the 43 then present,
corrected from 34).** 21 boot the oracle; 3 need its binary or source without
booting (`check-release-trust.py`, `check-x86-portability.py`,
`check-no-storage-authority.py`); 10 harness importers are portable and survive
deletion; the 9 non-importers are the seL4 gates. Counted against the tree as it
stood when the inventory was taken; this slice then added
`check-sel4-crossing-plane.py`, making the current totals 44 checkers and 10
non-importers with the legacy 24 unchanged. The two that a symbol-only audit
misclassifies are named in investigation step 4.

**Surface 3 — the eight ungated `kernel/tests/*.rs`.** 67 entities, 51 neutral.

| File | Neutral | Milestone | seL4 |
|---|---|---|---|
| `object_store.rs` | 32 | M5.4 | **total gap** — GPT redundancy and recovery precedence, content-addressed integrity, crash-consistency at all five write/flush boundaries, monotonic sequence. No seL4 storage plane exists |
| `component_image.rs` | 11 | P0 + retained v1 | **gap** — P5.2 covers the positive and wrong-target paths; the malformed corpus (W+X, overlap, entry-outside-exec, footprint, truncation) is unexercised on seL4 |
| `task_reclamation.rs` | 5 | B9 | **partial** — teardown-to-zero covers the double-free hazard; per-cycle drift, cost scaling, and rejected-spawn conservation are uncovered |
| `generation_manager.rs` | 2 | M5.6 | **gap** — state-policy mapping and GC root reachability |
| `isolation.rs` | 1 | M2 | covered — `sel4_root_boot_check` fault attribution, `sel4_component_graph_check` bounded error with caller alive |
| `kernel_foundation.rs` | 0 (10 custom) | M1 | none needed — custom PMM/VMM/heap/APIC die with `kernel/` |
| `boot.rs`, `should_panic.rs` | 0 (2 custom) | harness | none needed, but see the negative-control risk below |

### Gaps assigned to P5.4.2+ slices

Ordered as P5.4 specifies — later slices compose earlier ones — with the
M-series prefixed because it is larger than the C-series remainder and P5.4's
text never named it.

- [ ] **P5.4.2 — the M5 storage class.** Ten gaps across M5.1–M5.9,
      zero seL4 coverage, structural rather than incidental (five of the nine
      `Mediation::Unavailable` planes are exactly M5's surface). Includes
      `object_store.rs`'s 32 ungated tests, which `roadmap/01-foundations.md:141`
      explicitly says cannot be QEMU evidence and must remain unit evidence.
- [ ] **P5.4.3 — the M6 service class.** M6.3–M6.7 are gaps (directory, dango,
      generation commands, powerbox, transfer); M6.1 and M6.2 are partials.
- [ ] **P5.4.4 — C8.2 aggregate admission.** `slime-root` decodes only
      `BootLayout` and `SharedBufferBudget`; a `sel4-stream` generation whose
      graph declared unsatisfiable limits would launch. The bytes ride along
      (`build-generation.py:2464`) and are never read.
- [ ] **P5.4.5 — C8.5 QoS.** Confirmed absent, and the two assertions inside
      `fabric_stream.rs` make it worse than "absent": no gate turns red when
      they go. `sel4-stream.zti` declares full QoS tuples and grants no time
      capability, so the simulated-time clause is structurally unreachable today.
- [ ] **P5.4.6 — C8.6 bounded native calls.**
- [ ] **P5.4.7 — C8.7 native operations.**
- [ ] **P5.4.8 — C8.8 filtered introspection and declared interposition.**
- [ ] **P5.4.9 — C8.9/C8.10 typed closure and full-graph bootstrap.** The
      largest seL4 graph is 7 components and 2 routes against
      `check-data-fabric-boot.py`'s 20 roles.
- [ ] **P5.4.10 — the smaller partials.** C8.1 collision rejection, C8.3 graph
      provenance, C8.4's structural arm, C7.1's retained-v2 arm, B10's missing
      seL4 layout fixture, B11's product-vs-test profile pair,
      `component_image.rs`'s malformed corpus, `task_reclamation.rs`'s three
      uncovered properties. M6.1's v2-determinism partial belongs to P5.4.3
      with the rest of M6, not here.

### Bounds class: the third table, and what stays open

- [ ] **`SharedBufferTable::quotas` has no free path** (`shared_buffer.rs:502`,
      cap `MAX_CHARGE_HOLDERS = 96`). `declare_quota` (`:574-587`) reuses a slot
      only for the same `HolderId`, and `construct_child` (`main.rs:3092`) keys
      it by task id while `TaskTable::next_id` never rewinds — so a spawn/reap
      graph consumes one slot per task, permanently. `commit_teardown`,
      `reclaim_holder`, and `advance_epoch` never touch it. **[INFERENCE]** not
      reachable by any declared generation today (96 is triple `MAX_TASKS`, and
      the deepest declared graph is the supervision plane's 35 spawns) — read
      from the fixtures, not observed as a boot that survives the bound. That is
      already 36% of the way there on the newest gate, which is B16's exact
      trajectory. **Opened as a new backlog item rather than fixed here**: it is
      a third derived predicate with its own gate and fault injection, and
      bundling it would repeat the mistake B22's own deferral note describes.
- [ ] **`SharedBufferTable::orphans`** (`:503`) is freed only by
      `retry_orphans` (`:1549`), which is called from nowhere outside
      `#[cfg(test)]`. Not the B22 shape — entries are minted only by an adapter
      unmap failure and `record_orphan` dedupes — but recorded so a future
      fault-injection plane does not find it the hard way.
- [ ] **`SlotCursors`/`LaunchedComponents` are sized `MAX_CHANNELS` (32) while
      `MAX_ADMITTED_COMPONENTS` is 48.** A 33-loadable-component generation
      admits and then fatals in the staging loop (`main.rs:1158`/`:1161`). Loud
      and deterministic, but the refusal arrives after admission rather than at it.
- [ ] **The base boot layout is now 63 of `MAX_BOOT_LAYOUT_ENTRIES` (64).** One
      append remains; past that a new plane needs an override or a replacement.

Everything else classified: **24 tables live-bounded** with a named freeing
function, **12 deliberately monotonic** (all but `task::next_id` guarding
overflow with `checked_add` and a typed error). B16's sweep was correct on every
table it named and correct within its literal scope of *per-task* tables; it
missed `quotas` because `quotas` is only *keyed* per-task and is declared
per-component at boot. That is precisely why P5.4.1 audits this as a class
rather than inheriting a sweep done while fixing something else.

### Other risks

- [ ] **No seL4 gate has a negative control.** `should_panic.rs` is the oracle's
      proof that a failing assertion is observable at all. The ten seL4 gates
      are marker-matching Python checkers, and nothing in-repo demonstrates that
      a missing marker fails one. Mitigated only by per-slice fault injection,
      which is per-change discipline rather than a standing guard.
- [ ] **`sel4_root_boot_check` boots no repository fixture.** Its generation is
      `slime-root/fixtures/generation.bin`, the retained x86 blob. P5.1's
      `slimecm=[1-9]\d*` non-vacuity assertion depends on that blob carrying
      legacy images; if `kernel/` deletion removes whatever produced it, the
      argument breaks. Must be resolved before P5.4.final.
- [ ] **B12** — the component build's stale `--remap-path-prefix`. Deferral
      re-reviewed 2026-08-07 before opening this gate, on the unchanged
      reasoning: `sel4-crossing.zti` is a ninth seL4 generation built through the
      same JSON-target path, whose rustflags are keyed by triple and match none
      of the stale literal's, so it neither touches the defect nor extends its
      reach.
- [ ] **B23** — `slime-root`'s unit tests run in no gate. This slice adds no unit
      test for exactly that reason; `just sel4_crossing_check` is the sole
      observation point for the B22 fix, which is why it carries two fault
      injections.
- [ ] The inventory's per-milestone verdicts are read from checkers' assertion
      lists and test bodies at one point in time. Nothing enforces that a later
      gate edit keeps a claimed equivalence true; whether a cross-referencing
      script is worth writing is left to P5.4.final, which is where the claim
      becomes load-bearing.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`crossing-plane-boot.log`](crossing-plane-boot.log) — the
  full passing B22 boot.
- Serial/debugger/model output:
  [`fault-injection-1-no-sweep.log`](fault-injection-1-no-sweep.log),
  [`fault-injection-2-no-transit.log`](fault-injection-2-no-transit.log),
  [`fault-injection-3-key-from-len.log`](fault-injection-3-key-from-len.log).
- Related roadmap item:
  [P5.4.1](../../roadmap/07-architecture-portability.md) (this slice),
  [P5.4](../../roadmap/07-architecture-portability.md) (the parent whose gaps
  this records),
  [B22](../../roadmap/00-backlog.md) (resolved),
  [B12](../../roadmap/00-backlog.md) and
  [B23](../../roadmap/00-backlog.md) (deferred, re-reviewed).
