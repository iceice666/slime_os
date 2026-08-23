# C10.2: a second budget rather than a fifth column, and the gate that was mutating its own prose

| Field | Value |
|---|---|
| Date | 2026-08-23 |
| Kind | Change |
| Status | Verified |
| Scope | New `contracts/private-memory-budget/v1`; new `boot-contracts/src/private_memory_budget.rs` and its generated bindings; `contracts/generation/v1/schema.zt`; `slime-root/src/{generation,main,private_memory}.rs`; `components/runtime/src/{lib.rs,syscall.rs,syscall/sel4_transport.rs}`; new `components/bins/private-memory-probe/`; `components/bins/init/src/main.rs`; `boot-contracts/src/generation.rs`; new `contracts/generation/v1/fixtures/sel4-private-memory.zti`; `scripts/build/{build-generation,build-sel4}.py`; `scripts/generate/generate-boot-bindings.py`; new `scripts/check/check-sel4-private-memory-plane.py`; `scripts/check/{check-sel4-component-graph,check-sel4-gate-controls}.py`; `Justfile`; `Cargo.toml` |
| Roadmap | C10.2, C10, C10.1, C10.3, C7.3, B5, B8, B55, B63, B68 |
| Gates | `just private_memory_check`, `just sel4_component_graph_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just test_host`, `just contracts_check`, `just generation_check` |
| Trigger | C10.2 was the next roadmap milestone with met dependencies: the backlog is empty, C10.1 closed the previous day, and every other open item is an undecomposed parent (C9) or gated on physical hardware (M5.7, P4/RP3) |
| Baseline | C10.1 shipped the mechanism and deliberately hardwired every launch site's quota to `0`, so no component could grow a page and the operation was exercised only by the root's own embedded fixture against a compiled-in ceiling |

## Summary

A generation now declares which components may hold private memory and how
much. The declaration is `contracts/private-memory-budget/v1`, a `KIND_RESOURCE`
object authenticated by the generation's existing digest table, validated
eagerly inside `Admission::admit`, and installed at construction on both launch
paths. A component the budget does not name carries no window at all.

Two decisions did most of the work. The resource is a **sibling** of
`shared-buffer-budget/v1` rather than a fifth column on it, with its own
identity domain — the two bound unrelated mechanisms, most components use one
and not the other, and merging them would tie their versions together so a
private-memory change rewrote every shared-buffer budget on disk. And a
**malformed** budget fails the whole generation, which is deliberately
asymmetric with the C7.3 path that treats one as absent: deny-by-default makes
an undecodable shared-buffer budget harmless, but a private-memory budget that
silently read as absent is indistinguishable from a quota a component was
promised and never got, and the boot would look healthy.

The live evidence is a new plane and a new component, because B5's lesson
applies exactly here: C10.1's evidence ran on an ELF the root embeds at compile
time, which no manifest can name, so it could not answer whether a quota
*declared in a generation* reaches the component the generation names. The probe
discovers its own ceiling by growing one page at a time until refused and never
reads the manifest; the gate reads the declared number out of the fixture. The
agreement between those two independent facts is the milestone.

The review found the one real defect, and it was in the gate rather than the
mechanism: my `REQUIRED_MARKERS` table was `(regex, description)`, inverting the
order `sel4_gate_markers.chains_from_gate` reads, so `sel4_gate_control_check`
had been synthesizing its baseline transcript from the *prose* and mutating
that. The newly pinned count guarded nothing.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| `contracts/private-memory-budget/v1` (new) | 32-byte header, 36-byte sorted-unique entry, `maxHolders = 32`; publishes the root's `regionPages`/`totalPages` ceilings | Zutai owns the wire layout; the builder can refuse an over-declared budget using the root's own numbers rather than a copy |
| `boot-contracts/src/private_memory_budget.rs` (new) | Decoder plus `validate_against`, `holder_identity` under its own domain tag, 10 host tests | Structural validity and globally-possible bounds, enforced identically by both readers |
| `slime-root/src/private_memory.rs` | Two `const _: () = assert!` pinning `MAX_REGION_PAGES`/`MAX_TOTAL_PAGES` against the contract | Builder/root ceiling drift is a build failure, not a runtime refusal against a promised quota |
| `slime-root/src/generation.rs` | `private_memory_budget_object`, `private_memory_budget_is_satisfiable`, `private_memory_budget_admission`, `GenerationError::UnsatisfiablePrivateMemoryBudget`, `Admission::private_memory_holders`, 3 tests | One validation, before any launch, against this root's live constants |
| `slime-root/src/main.rs` | `declared_private_memory_pages`; budget resolved once per boot; the quota passed to `TaskTable::create` on both paths; `SLIME_MEM budget` and per-instance `SLIME_MEM quota` records | The quota is resolved *before* construction because `create` feeds it into the arena plan, and an arena never grows |
| `contracts/generation/v1/schema.zt` | `PrivateMemoryBudgetEntry` and an optional `privateMemoryBudget` list | A composition granting nobody working memory says so by omission |
| `scripts/build/build-generation.py` | `private_memory_holder_identity`, `build_private_memory_budget`, `validated_private_memory_quotas`, payload emission with a both-direction guard, boot-profile filter | A manifest error fails the build; holders without the object, or the object without holders, are both refused |
| `components/runtime/src/syscall.rs` | `private_memory_grow` + `PrivateMemory` | A component learns its base and page count in one badge-scoped call |
| `components/bins/private-memory-probe/` (new) | Measures its own ceiling, reads zeros, keeps a pattern across growth, checks the refusal had no effect | The probe never reads the manifest, so agreement with the root is evidence rather than restatement |
| `boot-contracts/src/generation.rs`, `components/bins/init/src/main.rs` | `BootAction::PrivateMemory = 30` across all five touch points, plus init's compile-time ABI pin | A renumbering fails to compile rather than silently composing another plane |
| `contracts/generation/v1/fixtures/sel4-private-memory.zti` (new) | One executable, two **root-autostart** instances, one in the budget | Root-launched rather than init-spawned: C10.2's subject is a declared instance's quota, and it keeps the plane clear of the 64-entry boot capability layout |
| `scripts/check/check-sel4-private-memory-plane.py` (new) | 11 markers across 4 causal chains, plus three cross-checks | Declared-vs-installed, measured-vs-declared, and per-holder charge attribution |
| `scripts/check/check-sel4-component-graph.py` | `SLIME_MEM budget holders=0 declared=0` plus two failure markers | The "no budget at all" case is asserted on a generation that actually has none |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A declared quota stops reaching its holder | `just private_memory_check` | `private-memory-granted: the generation declares 3 page(s) but the root installed N` |
| The ceiling stops binding at the declared number | `just private_memory_check` | `the granted probe grew to N page(s) against a declared quota of 3` — observed by injection |
| Omission stops denying | `just private_memory_check` | `private-memory-denied: no declared quota but a reserved window at 0x…` |
| Pages are charged to the wrong holder | `just private_memory_check` | `private-memory-denied: the root charged 3 page(s) against a declared quota of 0` — observed by injection |
| A query or refusal charges a page | `just private_memory_check` | the same per-holder charge comparison |
| A no-budget generation grants anything | `just sel4_component_graph_check` | failure marker `SLIME_MEM quota … declared=0 installed=[1-9]\d*` or `SLIME_MEM grown … delta=[1-9]\d*` |
| An over-committed budget is admitted | `just test_sel4_root` | `an_over_committed_private_memory_budget_is_refused` |
| A quota above the reservation is clamped instead of refused | `just test_sel4_root` | `a_quota_above_the_task_reservation_is_refused` |
| The two identity domains collide | `just test_host` | `holder_identity_is_domain_separated_from_the_shared_buffer_budget` |
| The gate's markers are weakened | `just sel4_gate_control_check` | pinned 11 and 31; the control rejects a table that shrank |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` | Passed. 11 markers across 4 causal chains. Observed `declared=3 installed=3 base=0x400000`, growths `0→1→2→3`, `cause=quota detail=QuotaExceeded { pages: 3, delta: 1, quota: 3 }`, `granted pages=3 … zeroed=1 survived=1 refused=1`, and for the omitted holder `declared=0 installed=0 base=0x0` then `cause=reservation detail=ReservationExceeded { pages: 0, delta: 1, reservation: 0 }` and `denied pages=0 base=0x0 refused=1` | Direct |
| Injection: fixture `pageQuota` 3 → 2 | Gate passed reporting `the declared quota (2 page(s)) is the measured ceiling` — the measurement tracked the declaration, which is C10.2's "lowering one holder's quota lowers exactly that holder's ceiling" | Direct |
| Injection: `declared_private_memory_pages` returns a constant | Gate failed: `missing marker: init declares no quota`, with `declared=2 installed=2` visible on `init` and on the omitted holder | Direct |
| Injection: transcript charging the omitted holder the granted holder's pages | `check_only_declared_pages_were_charged` refused it by name | Direct |
| Pre-fix vacuity check | Loading the gate through `chains_from_gate` returned the prose descriptions as patterns; after the fix it returns the regexes | Direct |
| `just sel4_component_graph_check` | Passed, 31 markers, including `SLIME_MEM budget holders=0 declared=0` on a generation declaring no budget | Direct |
| `just sel4_gate_control_check` | 34 gates reject 1318 mutated transcripts and layouts (1295 before this milestone) | Direct |
| `just test_sel4_root` | 149/149 across 16 modules; count pin raised 146 → 149 | Direct |
| `just test_host` | Passed, including 10 new `private_memory_budget` tests | Direct |
| `just contracts_check` | Passed; 30 declared operations documented | Direct |
| `just generation_check` | Passed; two isolated builds produced byte-identical `generation.bin` and `boot-store.bin` | Direct |
| `just sel4_boot_layout_check` | 26 plane layouts unchanged — the new plane adds no capability-layout row | Direct |
| `just sel4_root_boot_check`, `sel4_reclamation_check`, `sel4_sample_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_supervision_check` | Passed | Direct |
| `just lint_all`, `fmt_check_all`, `ruff`, `typos` | Passed | Direct |
| "Malformed, unsorted, duplicated, over-bound budgets fail generation decode" | Covered by the decoder's own tests and `private_memory_budget_admission`'s error path, **not** by a booted malformed-budget fixture — see *Open risks* | Inherited from code reading plus host tests |

## Decisions

- Decision: a sibling contract, not a fifth column on `shared-buffer-budget/v1`.
- Rationale: the two bound unrelated mechanisms — a shared buffer is a nameable, transferable object under a root-wide page ceiling; private memory is one task's own window no other component can see. Merging them would make every existing holder entry restate a quota for a mechanism it does not use, and would tie the formats' versions together so a private-memory change rewrote every shared-buffer budget on disk. The roadmap asked for this explicitly ("rather than widening that contract"), and the reasons are worth recording because the merged shape is the cheaper-looking one.
- Rejected alternative: add `pageQuota` to `SharedBufferBudgetEntry`. One fewer contract, one fewer decoder, and a permanent coupling between two independent authority questions.

- Decision: a distinct identity domain tag, `slime-private-memory-holder-v1`.
- Rationale: an identity computed for one budget must never be a valid identity in the other. `generation.rs` already records this rule for the fabric graph's identity domain; a host test now asserts the two never collide on the same component name, so the rule is checked rather than documented.

- Decision: a malformed budget fails the whole generation, unlike the C7.3 path.
- Rationale: the asymmetry is the interesting part. Deny-by-default makes an undecodable shared-buffer budget harmless — every holder is refused, which is conservative. But C10.2's exit condition requires every malformed budget to fail closed, because a budget that silently read as absent is indistinguishable from a quota a component was promised and never got: the component simply cannot allocate and the boot looks healthy. Failing at admission names the real fault before anything launches.

- Decision: the schema publishes the root's ceilings, and the root pins them with `const _: () = assert!`.
- Rationale: `validated_private_memory_quotas` must refuse exactly what `validate_against` refuses. Without a shared source the two drift, and the drift surfaces as either a builder rejecting budgets the root would honour or a builder emitting ones it would not — the second being a runtime refusal against a ceiling the generation promised. The `fabric-graph` contract already publishes ceilings this way.

- Decision: resolve the quota *before* `TaskTable::create`, not after it like the shared-buffer quota.
- Rationale: `create` feeds the quota into the arena plan through `private_memory::arena_reservation`, and an arena is fixed at `begin_task_arena` and never grows. A quota installed after construction would be a ceiling whose frames the arena has no room for — a number the task could never reach.

- Decision: the plane's two instances are **root-autostart**, not init-spawned.
- Rationale: C10.2's subject is whether a quota declared in a generation reaches the component the generation names. Routing the launch through an `init` spawn adds a parent whose own authority could be mistaken for the mechanism under test. It also keeps the plane clear of the boot capability layout, which is at its 64-entry ceiling: an init-spawned probe needs an `executable` row per instance and this needs none, which is why `sel4_boot_layout_check` still reports 26 unchanged layouts.

- Decision: the probe discovers its ceiling instead of being told it.
- Rationale: found while writing the gate. A probe that read the declared number and asserted it could pass against a root that had stopped honouring declarations entirely — both sides would be quoting the same manifest. Growing one page at a time until refused makes the probe's number a *measurement*, and the gate compares it against the fixture. The two facts are independent, which is the only reason their agreement means anything.

- Decision: causal chains rather than one flat marker sequence.
- Rationale: found by the round-2 review. My flat table asserted that the granted probe's records precede the omitted probe's, and justified it by launch order — but the root constructs `denied` first and seL4's `tcbSchedEnqueue` is LIFO at equal priority, so `granted` actually runs first. The order held, but for a reason the mechanism does not promise, which is precisely the defect B68 found in the determinism gate. Restructuring into four causal chains plus one `EXPECTED_UNORDERED` marker keeps all 11 markers while asserting only orders the mechanism guarantees.

- Decision: attribute charged pages per holder, not per task id.
- Rationale: also from round 2. `SLIME_MEM grown` carries a task id but no instance name, so summing by task proved the right *total* was charged without proving it went to the right holder — the omitted holder growing a page the granted holder then did not would reach the same sum. The gate now resolves each task id to an instance name through the root's own `SLIME_MEM quota` records. Verified by constructing that exact wrong distribution and observing the refusal.

## Open risks and follow-ups

- [ ] No gate boots a *malformed* private-memory budget. The decoder's ten host tests cover every structural refusal and `private_memory_budget_admission` maps each to a closed generation, but "malformed, unsorted, duplicated, over-bound budgets each fail generation decode" is observed at the unit level only. Closing it needs a corrupted-resource fixture variant, of the kind `sel4-matrix-unsatisfiable` is for the fabric graph — a declared byte-level delta rather than a second copy of the fixture.
- [ ] The aggregate ceiling's *live* refusal is unobserved. `MAX_TOTAL_PAGES` is 2048 and no composition's declared quotas come near it, so the aggregate arm is unit-tested but never reached on a boot. It becomes reachable when several components hold real quotas at once, which is C10.3's territory.
- [ ] `construct_child` re-locates and re-decodes the budget object on each spawn, twice — once for the quota and once for its marker — where the boot path resolves once into `private_budget`. Correct (admission has already validated it, and the object is immutable for the generation's lifetime) but wasteful, and a second lookup is the shape B71 records drifting. Worth folding into whatever C10.3 does to that function.
- [ ] The probe's `MAX_PROBE_PAGES` bound (64) is a runaway-loop guard, not a declared limit. A fixture declaring a quota above it would fail with `ceiling never reached` rather than a useful message. Fine while every plane quota is small; it should read the contract's `regionPages` if a fixture ever declares a large one.
- [ ] C10.2 leaves `TaskTable::create`'s `private_memory_pages` a `usize` parameter, now fed from the generation on both paths. C10.1's follow-up asked that it not outlive the generation resource as a caller-chosen number; it is now only ever generation-derived or the fixture's compiled-in constant, but the parameter itself remains.

## Artifacts and provenance

- Focused report: none; every finding and decision is recorded above with its source cross-reference.
- Raw transcript: not retained. Every result in *Verification* is reproducible from the named `just` target, and each injection is described precisely enough to repeat (the mutation, the file, and the observed failure string).
- Serial output: not retained as a sibling; the load-bearing marker lines are quoted verbatim in *Verification* and reproducible with `just private_memory_check`.
- Review: two rounds of a read-only reviewer pass over the uncommitted diff. Round 1 returned one P1 — the inverted `REQUIRED_MARKERS` tuple order defeating `sel4_gate_control_check` — and confirmed the eager-validation trace, the `u32`→`usize` bound, the identity domains, and the builder/decoder rule comparison. Round 2 confirmed that fix complete and returned two non-blocking findings: the marker order pinning a scheduling artifact (P2) and a `delta=[1-9]` inconsistency with the sibling gate (P3). All four are applied and recorded in *Decisions* and *Changes*.
- Related roadmap items: [C10.2](../../roadmap/02-core-runtime.md), [C10](../../roadmap/02-core-runtime.md)
- Predecessors: [`devlog/2026-08-23-c10-1-private-memory-mechanism/`](../2026-08-23-c10-1-private-memory-mechanism/index.md) (the mechanism this supplies a budget to), [`devlog/2026-07-28-c10-private-component-memory/`](../2026-07-28-c10-private-component-memory/index.md) (the design decision), [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../2026-07-26-b7-b8-budget-hygiene/index.md) (B8's aggregate rule, reproduced here for the new mechanism)
