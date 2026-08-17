# B57, B58 — a rights mask that admitted a bit nobody named, and the gate that found a third defect

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v5/{schema,gen_rust}.zt`, `boot-contracts/src/generation.rs`, `boot-contracts/src/generated/generation.rs`, `scripts/lib/boot_contracts.py`, `scripts/build/build-generation.py`, `scripts/check/check-generation.py`, `scripts/check/check-architecture-contract.py` |
| Roadmap | B57, B58, B59, B67, B40 |
| Gates | `just generation_check`, `just contracts_check`, `just architecture_contract_check`, `just test_host`, `just sel4_boot_check` |
| Trigger | The structural audit at `35a95b2` computed `RIGHT_ALL` both ways and found them one bit apart |
| Baseline | Every rights validator masked with `(1 << 26) - 1`, which had been the spelling since v3 introduced u64 rights |

## Summary

`RIGHT_ALL` had two definitions: an enumerated union in the builder
(`0x3fdffff`) and a bit-width mask in the root and the oracle (`0x3ffffff`). The
difference is bit 17 — a reserved gap between `spawn` (16) and `supervise` (18)
that nothing names and nothing uses. Because admission masked with the wider
spelling, a grant carrying bit 17 passed every rights check in both the root and
the oracle. The rights vocabulary is now declared once in the v5 schema and
generated into both bindings, with `RIGHT_ALL` folded from the named bits, and
three restatements deleted. B58 — three hand-copied header offsets in
`check-architecture-contract.py` — was fixed in the same pass because it is the
same failure class one layer over. Running B57's verification sweep then exposed
a third defect, B67: the capability-layout gate's `extra` mutation has never
perturbed anything, because it selects slot 4, which the audit declares.

## Observable symptom

- Command: none — no gate could observe this. Both spellings were internally
  consistent, so every check passed while disagreeing about what a valid right is.
- Expected: one predicate for "is this a right some contract defines."
- Observed: two predicates, one bit apart, with the wider one gating admission.
- Exit/fault/serial evidence: none for B57/B58 (both are admission-time
  predicates, not runtime behaviour). B67's evidence is a serial transcript
  reaching the terminal marker where a refusal was required — quoted below.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Recomputed both spellings: python union `0x3fdffff`, rust width mask `0x3ffffff` | One bit apart; bit 17 is the difference |
| 2 | `grep "1 << 17"` over `slime-root`, `components`, `boot-contracts`, `scripts` returned nothing | The bit is a reserved gap, not an undocumented right |
| 3 | Traced which spelling gates admission: `generation.rs:1775,1933,2152` and `check-generation.py:377,426,545` all mask with `!RIGHT_ALL` | The wider spelling is the one that decides, so bit 17 was admissible |
| 4 | Checked whether a fixture could emit it: `build-generation.py:3146` rejects unknown right names, `validate_capability_rights:432-470` masks per kind | Not reachable from any `.zti` today; the defect is builder/admission asymmetry, not a live exploit |
| 5 | Read how generated constants reach the root: `boot-contracts/src/generation.rs:9` is `include!("generated/generation.rs")` | Generated consts are already in scope, so the hand-written ones were shadowing duplicates to delete |
| 6 | First generator attempt used inline lambdas (`map (bit => …)`) | `zutai::parse::generic` — unexpected `>`; the dialect requires named helpers, matching `base.zt:12`'s `map _.name` style |
| 7 | Looked for `shiftLeft` in `stdlib.num`; only `pow` exists | Union built as `l.sum (map rightValue bits)`; bits are distinct by construction, so summing `2^bit` is exactly the bitwise union |
| 8 | Regenerated: `RIGHT_ALL = 66977791`, bit 17 clear | Matches the builder's union exactly |
| 9 | Noticed the builder's `RIGHT` dict keys are manifest spellings (`bufferMap`), a second contract surface | Added `manifest` to the schema's `RightBit` so that table is generated too, not just the constants |
| 10 | Verified the new guard bites: rewrote generated `RIGHT_ALL` to `67108863`, ran the test | Aborts (`panic = "abort"`); restored value passes. The guard is not vacuous |
| 11 | Ran `just sel4_capability_layout_check` as part of the sweep | `the audit accepted a mutated CSpace: a capability was installed into an undeclared slot (--cfg slime_b40_mutate_extra)` |
| 12 | Stashed every working change and re-ran at `beff860` | Same failure — pre-existing, not caused by B57/B58 |
| 13 | Read `task.rs:1060-1064` against the audit's declared set at `:1075-1078` | The mutation excludes `{0,1,2,3}` and picks slot 4 = `CHILD_SLOT_CNODE`, which the audit *declares* → B67 |

## Root cause

**B57.** A bit-width mask is not a vocabulary. `(1 << 26) - 1` asserts "below the
highest defined bit"; the intended predicate is "one of the rights a contract
names." Those coincide only when the numbering is dense, and it is not — bit 17
is a deliberate gap. The two spellings could drift because neither was generated:
`docs/capability-matrix.md` claimed rights numbering was "generated-contract
truth," but `boot-contracts/src/generated/generation.rs` contained only
`FORMAT_VERSION`, and every rights bit was hand-authored in one file and re-typed
in others.

The innocent-looking site is `capability_rights_valid`, which masks per capability
kind and would have caught bit 17 on a *typed* capability. It is not the root
cause: the grant, mapping, and minted-binding walks apply `& !RIGHT_ALL`
independently, and that is the check bit 17 survived.

**B58.** Same class, one layer over: a hand-copied offset where a generated name
existed and was already imported. Its comment recorded a prior drift, which is the
evidence that the coupling does not hold by discipline.

**B67.** The mutation restates a subset of the predicate it is trying to violate.
The audit declares `{service 1, tcb 2, fault 3, CHILD_SLOT_CNODE 4, console 32}`;
the mutation excludes only `{0, 1, 2, 3}`. `find` therefore returns 4 — declared
— so the copy lands where occupancy is expected and the audit correctly stays
silent. `console` (32) is missing from the exclusion list too, latent only because
4 is selected first.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v5/schema.zt` | Declared `RightBit :: {name, bit, manifest}` and a 25-entry `rightBits` vocabulary; wired it into `format` | Rights numbering is schema-owned, as the capability matrix already claimed |
| `contracts/generation/v5/gen_rust.zt` | Renders `RIGHT_*` (Rust `u64`), `GENERATION_RIGHT_*` (Python), `GENERATION_RIGHT_BY_MANIFEST_NAME`, and `RIGHT_ALL` as `l.sum (map rightValue bits)` | `RIGHT_ALL` is a union of named bits; a gap cannot be admitted |
| `boot-contracts/src/generation.rs` | Deleted hand-written `RIGHT_TRANSFER`/`RIGHT_EXEC`/`RIGHT_SPAWN`/`RIGHT_ALL`; `capability_rights_valid` now reads generated names instead of `1 << 9`-style literals | One definition per right in the root |
| `scripts/build/build-generation.py` | `RIGHT`, `RIGHT_TRANSFER`, `RIGHT_ALL` now alias the generated table | The builder no longer owns the numbering it compiles against |
| `scripts/check/check-generation.py` | Four constants now alias `GENERATION_RIGHT_*` | The oracle and the root cannot disagree about the mask |
| `scripts/check/check-architecture-contract.py` | Literals `112`/`200`/`368` replaced with generated `GENERATION_HEADER_*_OFFSET` names; lockstep comment removed (B58) | No hand-written offset for a generated layout |
| `roadmap/00-backlog.md` | B57 and B58 moved to the resolved log with observed exit conditions; B67 opened | The backlog records what was proven, and the new defect is tracked rather than absorbed |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| `RIGHT_ALL` regresses to a width mask, re-admitting bit 17 | `just test_host` → `right_all_is_a_union_of_named_bits_and_excludes_the_gap_at_17` | Union ≠ `RIGHT_ALL`, or bit 17 set, or a capability kind accepting `1 << 17` |
| A new right is added to one binding only | `just contracts_check`, `just boot_gen` | Generated tree differs from the generator's output |
| A rights bit changes value, silently re-authorizing components | `just generation_check` | Generation identity changes; the two isolated builds' byte-identical assertion covers the encoding |
| A generation header field is added, shifting offsets | `just architecture_contract_check` | Reads through generated names, so a shift moves with the schema instead of decoding the wrong field |
| The B57 guard becomes vacuous | Verified directly this session by mutating generated `RIGHT_ALL` to `67108863` and observing the abort | — |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_host` | pass — 207 + 20 + 19 + … across the host crates | Direct |
| `cargo test -p boot-contracts` | pass — 182 tests, including the new guard | Direct |
| Guard bites: generated `RIGHT_ALL` → `67108863` | `cargo test -p boot-contracts right_all_is_a_union` aborts (SIGABRT under `panic = "abort"`); restored value passes | Direct |
| `just test_sel4_root` | pass — 118/118 across 14 modules | Direct |
| `just contracts_check` | pass — 31 seL4 manifests encode SLIMEG5 v5; fabric-ring bindings current | Direct |
| `just generation_check` | pass — two isolated builds byte-identical, generation `fa4c9a36…` | Direct |
| `just architecture_contract_check` | pass — B58's gate, including its 181 boot-contracts tests | Direct |
| `just sel4_root_boot_check` | pass — ordered generation, timer, task, IPC, fault, ready markers | Direct |
| `just sel4_boot_check` | pass — 30 markers, 5 causal chains, 21-slot layout, 19 composition tasks, none exited | Direct |
| `just ruff`, `just typos`, `just fmt_check_all`, `just lint_all` | pass | Direct |
| `just sel4_capability_layout_check` | **FAIL** on the `extra` arm — reproduced at `beff860` with all working changes stashed, so pre-existing. Opened as B67 | Direct |

## Decisions

- Decision: declare the rights vocabulary in `contracts/generation/v5/schema.zt`
  rather than adding a new `contracts/rights/v1/`.
  Rationale: rights are a field of the v5 generation records that already live
  there (`grantLayout`'s `rights 8`), and `generate-boot-bindings.py` already
  wires v5 into both bindings. A separate contract would add a generator and a
  second version axis for a vocabulary that has no independent lifetime.
  Rejected alternative: a standalone rights contract — deferred to B59, which has
  to create `contracts/syscall-abi/v1/` anyway for the label and error tables; the
  rights vocabulary can move there if it turns out to want its own version.

- Decision: put the `.zti` manifest spelling (`bufferMap`) in the same schema
  record as the constant suffix (`BUFFER_MAP`).
  Rationale: the builder resolves grants by manifest spelling, so that table is a
  contract surface too. Generating only the constants would have left the
  name-to-bit map as a 24-entry hand-written dict — the same defect one field over.
  Rejected alternative: deriving the manifest spelling from the constant name by
  case conversion. `mapMmio`/`MAP_MMIO` and `irqAck`/`IRQ_ACK` are not a
  mechanical round-trip, and inventing one would make a renaming bug silent.

- Decision: `RIGHT_ALL` as `l.sum (map rightValue bits)` rather than a bitwise
  fold. Rationale: `stdlib.num` has no shift or bitwise-or; the bits are distinct
  by construction, so the sum is exactly the union. Documented at the definition
  so the equivalence is not left implicit.

- Decision: record B67 rather than fix it in this change.
  Rationale: it is a pre-existing defect in a different mechanism
  (`audit_child_cspace`'s negative controls), proven independent by reproducing it
  with every working change stashed. Folding it in would have mixed an unrelated
  fix into B57's diff and made the "did B57 cause this" question unanswerable from
  the history.

## Open risks and follow-ups

- [ ] B59 remains the real closure for this defect class: 97 `RIGHT_*` declaration
  sites outside `boot-contracts` still restate bits the schema now owns, and the
  syscall label table, error table, and spawn-grant record are still hand-synchronized.
  B57 fixed the *predicate*; the duplication is untouched.
- [ ] B67 — `just sel4_capability_layout_check` is red on the `extra` arm and has
  been proving nothing for that mutation. Next item.
- [ ] **[INFERENCE]** No `.zti` fixture could emit bit 17, so this was not
  exploitable in a shipped generation. That is a source reading of
  `build-generation.py:3146` and `validate_capability_rights`, not an observed
  refusal of a hand-forged generation. A generation-level negative control that
  forges an undefined rights bit and observes admission refusing it does not exist;
  B57's guard is a host unit test over the predicate instead.
- [ ] `docs/capability-matrix.md`'s "generated-contract truth" sentence is now true
  for rights numbering, but the doc's tables are still hand-maintained prose. B59
  covers generating them.

## Artifacts and provenance

- Focused report: none; the audit that found B57 and B58 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md).
- Raw transcript: none preserved; every measurement here is reproducible from the
  cited file and line, and each gate result from its named `just` target.
- Serial/debugger/model output: B67's failing marker is quoted inline above; the
  full transcript is regenerable with `just sel4_capability_layout_check`.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B57 and B58 in the resolved log, B59 and B67 open.
