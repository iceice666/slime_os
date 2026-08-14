# P5.4.4 — C8.2 aggregate fabric-graph admission on seL4

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{generation,main}.rs`, `scripts/check/check-sel4-stream-plane.py`, `Justfile`, `AGENTS.md` |
| Roadmap | P5.4.4, P5.4, P5.4.1, C8.2 |
| Gates | `just sel4_stream_check`, `just test_sel4_root` |
| Trigger | P5.4.1's inventory, which found C8.2 had no seL4 equivalent at all rather than a partial one |
| Baseline | Nine seL4 gates passing; `slime-root` decoding only `BootLayout` and `SharedBufferBudget` |

## Summary

`slime-root` never decoded the fabric-graph resource. The retired kernel
validates it against its own ceilings before any component launches
(`kernel/src/runtime/generation.rs:105-119`), and P5.4.1 recorded that as C8.2's
exit condition being entirely unmet on seL4 — the bytes rode along in every
generation the builder emits and nothing read them. A generation whose graph
declared unsatisfiable limits would have launched. This slice adds the
admission: the same `boot_contracts` predicate the oracle uses, against
`slime-root`'s own ceilings, refusing the whole generation closed before the
fabric or any participant starts.

## Changes

| Area | Change | Effect |
|---|---|---|
| `generation.rs` | `fabric_graph_object` locates the `SLIMEFG` resource, matching the oracle's shape | The declared graph is reachable |
| `generation.rs` | `fabric_graph_is_satisfiable` applies `validate_against` with this root's ceilings | C8.2's aggregate check exists on seL4 |
| `generation.rs` | `Admission::admit` calls it before closure classification; `GenerationError::UnsatisfiableFabricGraph` | An impossible graph fails the generation, not a participant mid-boot |
| `generation.rs` | `Admission::fabric_graph_admitted` | The wiring is observable, not only the predicate |
| `main.rs` | `SLIME_ROOT fabric graph={admitted,absent}` | A gate can see the check ran |
| `check-sel4-stream-plane.py` | Requires `fabric graph=admitted` in the first chain | P5.4.4's exit condition is observable |
| `generation.rs` tests | Three cases: satisfiable admitted, every ceiling refused, self-contradiction refused | The predicate's wiring to *these* ceilings is checked |

The ceilings are `slime-root`'s — `MAX_WAIT_SOURCES`, `MAX_TASK_CAPS`,
`MAX_TOTAL_PAGES`, `MAX_SHARED_BUFFERS`, `MAX_MAPPINGS`, `MAX_LOANS`,
`MAX_MESSAGE_BYTES`, `CHANNEL_CAPACITY` — not the retired kernel's. The
predicate is shared byte-for-byte, so the two implementations can disagree only
where their mechanisms genuinely differ, which is exactly why validating in both
places is not redundant.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The admission stops being called | `just sel4_stream_check` | `missing marker: SLIME_ROOT fabric graph=admitted` |
| A ceiling is wired to the wrong constant | `just test_sel4_root` | `limit N at V exceeds this root's ceiling and was admitted` |
| The check starts refusing satisfiable graphs | `just sel4_stream_check` | The stream plane's real 2×2 graph stops booting |
| The marker appears without a graph to check | `just test_sel4_root` + the `absent` case on every other plane | The two states would stop being distinguishable |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass — the real two-publisher/two-subscriber graph is admitted and the plane runs unchanged — [`stream-plane-admission.log`](stream-plane-admission.log) | Direct |
| Fault injection: admission wiring removed | Fails with `missing marker: SLIME_ROOT fabric graph=admitted` — [`fault-injection-no-admission.log`](fault-injection-no-admission.log) | Direct |
| `just test_sel4_root` | Pass — 105/105, including the three new cases — [`admission-tests.log`](admission-tests.log) | Direct |
| The other eight seL4 gates | All pass; each reports `fabric graph=absent`, since the stream fixture is the only one declaring a graph | Direct |
| `just contracts_check`, `just generation_check`, `just devlog_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| An impossible graph reaching a boot | **Not observed** — `build-generation.py` refuses to encode one, so the runtime check cannot be reached through the fixture path. Covered by unit tests instead; see below | Unobserved, with reason |

## Decisions

- Decision: assert the **wiring** with a boot marker and the **predicate** with
  unit tests, rather than trying to boot an impossible graph.
- Rationale: I tried the direct route first. `build-generation.py` validates the
  same limits at encode time, so raising `ingressSources` past the ceiling in
  `sel4-stream.zti` fails the *build* and the root never sees it. That is
  correct defence-in-depth and not worth weakening to make a gate convenient.
  What remains is that the runtime check must exist, be wired in, and use the
  right ceilings — a marker covers the first two and unit tests the third.
- Rejected alternative: a hand-built malformed generation blob in a unit test.
  It would exercise `Admission::admit` end to end, but constructing a full
  generation — header, string table, objects, components, grants, health — to
  reach one branch is a large fixture whose own correctness would then need
  checking. The marker gets the same property from a real boot.

- Decision: `fabric_graph_is_satisfiable` is a separate `pub` function rather
  than inlined into the generation walk.
- Rationale: the interesting content is *which ceilings are passed*, and those
  are this implementation's. Splitting it makes that directly testable without
  a generation blob, which is what let the ceiling table be checked one field at
  a time.

- Decision: the ceiling test accepts refusal at **either** guard.
- Rationale: some over-ceiling values are also structurally impossible against
  the wire format's own maxima, so the decoder rejects them before
  `validate_against` runs — `ingress_sources` is one. What must hold is that no
  such graph reaches a running fabric, not which guard catches it; asserting on
  the guard would break the test if the format's maxima ever moved
  independently of this root's.

## Open risks and follow-ups

- [ ] **The refusal path is unobserved at boot.** Unit tests cover the
      predicate and the marker covers the wiring, but no gate boots a
      generation carrying an unsatisfiable graph, because the builder will not
      emit one. Closing that would mean a deliberately malformed fixture the
      builder is told to skip validating — worth doing if a future slice needs
      the runtime refusal to be load-bearing, not worth it for this one.
- [ ] **Only the stream fixture declares a graph**, so the `admitted` arm rests
      on one plane. The `absent` arm is exercised by the other eight, which is
      what makes the marker non-vacuous.
- [ ] **C8.2 is not fully closed by this.** P5.4.1 scoped P5.4.4 as the
      aggregate-admission gap, which this fills. The oracle's
      `kernel/tests/fabric_manifest.rs` also asserts route-authority tuples,
      interposition-chain termination, and per-pair QoS compatibility over the
      *booted* graph — those remain uncovered and belong to P5.4.10's partials
      list alongside C8.3's graph provenance.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`stream-plane-admission.log`](stream-plane-admission.log) —
  the passing stream-plane boot.
- Serial/debugger/model output:
  [`fault-injection-no-admission.log`](fault-injection-no-admission.log),
  [`admission-tests.log`](admission-tests.log).
- Related roadmap item:
  [P5.4.4](../../roadmap/07-architecture-portability.md) (this slice),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (the inventory that
  recorded the gap), [C8.2](../../roadmap/02-core-runtime.md) (the oracle
  milestone).
