# B89 — fourteen renderers, one 82-line codec block, three drifts

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/_shared/{codec.zt,zutai.zti}`, fifteen `contracts/*/gen_rust.zt` renderers, regenerated `components/proto/src/{fabric_qos,fabric_time}.rs` |
| Roadmap | B89, B88, IO0 |
| Gates | `just contracts_check`, `just test_host`, `just kani_io_proofs` |
| Trigger | B88's own follow-up list: adding a fifteenth local copy of `offsetConsts` for the same forced reason made the duplication worth measuring rather than repeating |
| Baseline | Every generated binding correct and every gate green; the mechanics producing them duplicated fourteen times |

## Summary

`wire.rust`'s `WireField` carries `name`, `width`, and `signed` — no
`byteArray`. Its shared `offsetConsts` and `wireStruct` therefore cannot
describe a `[u8; N]` wire field, and every contract with an array field had to
restate the entire offset/struct/encode/decode emitter block locally. Fourteen
did. Once the label baked into their `expect(...)` strings is parameterised, the
blocks are **byte-identical**: 82 lines, fourteen times, ~1150 lines of clone.

The duplication was forced, not careless — which is precisely why it survived —
but it had already drifted in the three ways clones drift. `fabric-qos` and
`fabric-time` emitted `expect("generated fabric-stream layout")`, naming the
contract they were copied *from* rather than themselves, so a panic in a QoS
record would have sent a reader to the wrong schema. `fabric-trace` emitted a
three-line `if` body where twelve siblings emitted one line. `block/v2` had
collapsed its source formatting and used the label `block-v2` where its sibling
used `block`. None of that was visible to any gate, because each copy was
internally consistent and every generated file was correct.

`contracts/_shared/codec.zt` now owns the emitters as `wire.codec`. All fifteen
renderers delegate: **1382 lines deleted, 189 added.** Every generated artifact
was regenerated and diffed against a pre-migration snapshot with zero
differences, apart from the two mislabelled `expect` strings, which now name
their own contracts.

## Observable symptom

No failing command; this is debt, and the mislabel is a latent diagnostic defect
rather than a wrong output.

- Command: `grep -rn 'generated fabric-stream layout' components/proto/src/`
- Expected: matches only in `fabric_stream.rs` and `fabric_ring.rs` (which is legitimately generated from `fabric-stream/v2`)
- Observed: also `fabric_qos.rs` and `fabric_time.rs` — nine and six occurrences respectively, each a panic message pointing at the wrong contract
- Exit/fault/serial evidence: `contracts/fabric-qos/v1/gen_rust.zt` and `contracts/fabric-time/v1/gen_rust.zt` both hard-coded `"generated fabric-stream layout"` in their local `decodeExpr`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | 18 renderers declare a local `offsetConsts`; 50 import `wire.rust` | The scale is larger than B88's follow-up note claimed ("three contracts"); that estimate was wrong and is corrected here. |
| 2 | 18 declare their own `WireField`; **zero** use `r.WireField` | The shared type is dead as a type. 28 contracts pass `wire.python`'s shape instead. |
| 3 | Extracting `constName` through `offsetConsts` and normalising the join alias: 14 files hash identically, 2 differ | The identical group is a genuine consolidation target, not superficially similar code. |
| 4 | Widening extraction to the whole block (`constName` → `rustBindings`) and parameterising the label: 12 files hash identically at 82 lines | The duplication is the entire codec block, not just offsets. |
| 5 | `fabric-qos` and `fabric-time` extract with label `fabric-stream` | Copy-paste provenance, and a wrong panic message in shipped generated code. |
| 6 | `fabric-trace` differs by 2 lines; `block/v2` by whitespace and label spelling | Both semantically identical; the divergence is in emitted formatting and source layout. |
| 7 | Checked which brace style is actually *shipped*: `io_queue.rs` has the three-line form | `fabric-trace`'s renderer was right and twelve siblings' collapsed source produced the same output anyway. The shared module adopts the three-line emitter. |
| 8 | `contracts/zutai.zti` maps alias `wire` → `_shared`, whose `zutai.zti` lists `base`/`rust`/`python` | A new module is a one-line manifest addition, not a build-system change. |

## Root cause

A shared helper whose type could not express the data its callers had. `wire.rust`
predates the array-carrying protocols; when the first contract needed a
`[u8; 32]` field, extending the shared `WireField` would have touched every
contract importing it, so the contract copied the mechanics instead. The
fourteenth copy was made for exactly the same reason as the first. The drift is
the symptom; the missing `byteArray` capability is the cause.

The fix does not widen `wire.rust`'s `WireField` — that remains the risky change
B88 declined — but adds a second module whose `WireField` is `wire.python`'s
shape, which is what every schema's `*Layout` list already declares. No adapter
is needed at any call site.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/_shared/codec.zt` | New 175-line module: `constName`, `offsetConsts`, `offsetConstsWithEnds`, `rustType`, `fieldDecls`, `decodeExpr`/`decodeFields`, `encodeField`/`encodeFields`, `wireStruct`, plus `layoutNames`/`wireBytes`/`validField`/`allValid` | One definition of the byte-array-aware Rust codec mechanics |
| `contracts/_shared/zutai.zti` | Registers `codec` | Importable as `wire.codec` |
| 14 renderers | Local block replaced by four delegating aliases plus a label-bound `wireStruct` | The emitters have one source; each contract keeps only its own label |
| `contracts/network-destination/v1/gen_rust.zt` | B88's local copy also delegates, via `offsetConstsWithEnds` | The fifteenth copy does not survive its own defect fix |
| `contracts/fabric-qos/v1`, `contracts/fabric-time/v1` | Label corrected from `fabric-stream` to their own contract names | A panic message names the schema it came from |
| `components/proto/src/{fabric_qos,fabric_time}.rs` | Regenerated: 15 `expect` strings corrected | Generated, and now truthful |

Validity predicates deliberately stay with each contract: what makes a layout
admissible is contract-specific (signedness rules, size equalities, field-count
bounds), and centralising those would hide real per-format differences behind a
shared name.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A renderer's emitters drift from the shared module again | Structural: there is no local copy left to drift | — |
| The shared module changes emitted output | `just contracts_check` (every generator's `--check`) | `generated … bindings are stale` |
| A codec change breaks a protocol's wire layout | `cargo test -p slime-proto` (the protocol test suites) | Round-trip or bounds assertion |
| A change breaks the proved wire arithmetic | `just kani_io_proofs` | Harness counterexample |
| A change breaks a live protocol | `io_queue_check`, `io_link_check`, `io_network_check`, `sel4_qos_check`, `sel4_stream_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_powerbox_check` | Missing plane marker |

## Verification

The load-bearing evidence is the byte-identical diff: a refactor of code
generators is only safe if the generated bytes do not move.

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre-migration snapshot of `components/proto/src` + `boot-contracts/src/generated`, all generators re-run, `diff -rq` | **Zero differences** across every generated artifact | Direct |
| `just contracts_check` | PASS — every generator's own `--check` agrees | Direct |
| `cargo test -p slime-proto` | PASS — all protocol suites | Direct |
| `cargo test -p boot-contracts --all-features` | PASS — 335 passed | Direct |
| `just test_host` | PASS — 20 suites | Direct |
| `just kani_io_proofs` | PASS — 18 harnesses (io-queue's generated codec is under proof) | Direct |
| `just kani_virtio_proofs` | PASS — 13 harnesses | Direct |
| `just io_queue_check`, `io_link_check`, `io_network_check`, `sel4_qos_check` | PASS | Direct |
| `just sel4_stream_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_powerbox_check` | PASS | Direct |
| `just sel4_gate_control_check` | PASS — 45 gates, 1768 mutations | Direct |
| `just generation_check` | PASS — two isolated builds byte-identical | Direct |
| `just fmt_check_all`, `just lint_all`, `just machete`, `just typos`, `just devlog_check` | PASS | Direct |
| `grep 'generated fabric-stream layout'` after the fix | Only `fabric_stream.rs` and `fabric_ring.rs`, both legitimately from `fabric-stream` | Direct |

## Decisions

- **Decision:** Add `wire.codec` rather than widen `wire.rust`'s `WireField`
  with `byteArray`.
  **Rationale:** 50 contracts import `wire.rust` and 28 pass `wire.python`'s
  `WireField` into it. Changing that type is a repository-wide edit whose blast
  radius is every boot and protocol contract; a new module is additive and
  touches only the callers that opt in. B88 flagged the widening as
  "worth doing as its own change" — this keeps that judgement.
  **Rejected alternative:** widening the shared type. Still open, still
  reasonable, and now less urgent because nothing is forced to duplicate.
- **Decision:** Keep the label a parameter of `wireStruct` rather than deriving
  it from the contract path.
  **Rationale:** The renderer does not know its own path, and threading one in
  would add a field to fifteen `Protocol` types to serve a panic string.
  **Rejected alternative:** dropping the label entirely and emitting a generic
  message. It would make every generated `expect` identical across fifteen
  protocols, which is worse than the mislabel this entry fixes.
- **Decision:** Migrate with the label preserved verbatim per contract, verify
  byte-identical output, and only then correct the two wrong labels.
  **Rationale:** Two changes that both touch generated bytes cannot be
  distinguished if made together. Sequencing them means the zero-diff run
  proves the refactor and the second run's diff is exactly 15 lines of
  corrected strings — reviewable in isolation.
  **Rejected alternative:** fixing labels during the migration. Faster, and it
  would have destroyed the only evidence that the refactor changed nothing.
- **Decision:** Leave per-contract `valid`/`validField` predicates in place.
  **Rationale:** `allValid` is shared because the field-admissibility rule is
  genuinely common, but the record-level predicates encode format-specific
  facts. A shared `valid` would need a configuration record per contract, which
  is the duplication back in another shape.

## Open risks and follow-ups

- [ ] `wire.rust`'s `WireField` still lacks `byteArray`, and `r.WireField` is
  now referenced by nothing. Either widening it or deleting the unused type is a
  reasonable follow-up; neither is needed to keep the duplication from
  returning.
- [ ] `fabric-operation` and `fabric-call` retain local emitters, deliberately:
  their `constName` takes one argument and hard-codes a fixed prefix
  (`OFF_OPERATION_`, `OFF_CALL_`) rather than accepting one, so they are a
  different signature, not a copy of this block. Migrating them means changing
  their emitted constant names, which changes generated bytes and their
  consumers. Out of scope here.
- [ ] `fabric-visibility` has a `byteArray`-aware local `rustType` but no
  `offsetConsts`; not examined.
- [ ] The generated bindings are proved correct only where IO6/IO7 harnesses
  reach (`io-queue`, and `virtio_mmio` which is hand-written). The other twelve
  protocols' generated codecs rest on their own test suites and plane gates, as
  before; this change did not alter that and does not claim to.

## Artifacts and provenance

- Focused report: none; the module header comment in `contracts/_shared/codec.zt`
  records why the duplication existed and what had drifted.
- Raw transcript: none retained; the migration is reproducible by re-running
  every `*_gen` target and diffing against `git`.
- Serial/debugger/model output: the plane markers under *Verification*.
- Related roadmap item: [IO0 — Queue, identity, and buffer-lease contract](../../roadmap/11-io-substrate.md#io0--queue-identity-and-buffer-lease-contract) as the proved consumer, with the resolved entry at [B89](../../roadmap/00-backlog.md#resolved) and its predecessor [B88](../2026-08-29-b88-network-destination-generated-offsets/index.md).
