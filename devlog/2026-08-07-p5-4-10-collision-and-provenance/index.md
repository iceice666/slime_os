# P5.4.10 — C8.1's tag collision and C8.3's graph provenance

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/generation.rs`, `Justfile`, `AGENTS.md` |
| Roadmap | P5.4.10, P5.4, P5.4.1, C8.1, C8.3 |
| Gates | `just test_sel4_root`, `just sel4_stream_check` |
| Trigger | P5.4.10's last two rows |
| Baseline | 107 host tests; C8.1 and C8.3 the only rows left open |

## Summary

The last two P5.4.10 rows. C8.1's collision rule — two distinct interfaces may
not share one generation-local type tag — was implemented in
`FabricGraph::decode` and tested by **nothing**, in either `boot-contracts` or
`slime-root`. C8.3's gap was narrower and worse: the fabric service answers
requests from a `@generated` build-time table (`FABRIC_CLIENTS` in
`default_fabric_profile.rs`), and nothing checked it still agreed with the
authenticated graph. Both are now enforced and tested, and admission gained a
provenance check that refuses a graph naming a component the generation lacks.

## Changes

| Area | Change | Effect |
|---|---|---|
| `generation.rs` | `two_schema_graph(first_tag, second_tag)` test builder | A tag collision is constructible; `validate_schemas` runs before any route references a schema, so no route is needed |
| `generation.rs` | `distinct_schemas_may_share_no_type_tag` | C8.1's rule is pinned, positive half included |
| `generation.rs` | `GenerationError::UndeclaredFabricParticipant` | Distinct from `UnsatisfiableFabricGraph`: the graph fits every ceiling and still promises an edge to nobody |
| `generation.rs` | `fabric_graph_participants_are_declared`, called from `fabric_graph_admission` | A graph naming a dropped component fails closed at admission |
| `generation.rs` | `participants_are_declared` split out over names | The property is testable without hand-building a `Generation` |
| `generation.rs` | `qos_graph`'s participants now use derived identities rather than `[0x41; 32]` | The provenance test can name the same components the fixture declares |
| `Justfile`, `AGENTS.md` | Host test count 107 → 109 | The gate's count assertion stays exact |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The tag-collision check is removed or weakened | `just test_sel4_root` | `distinct_schemas_may_share_no_type_tag` fails |
| The decoder starts refusing legal two-schema graphs | same | The positive half of the same test fails |
| A graph names a component the generation dropped | same, plus admission at boot | `UndeclaredFabricParticipant`; the generation fails closed |
| The provenance check starts refusing valid graphs | `just sel4_stream_check` | The real six-participant graph stops admitting |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Pass, 109/109 across 13 modules — [`tests.log`](tests.log) | Direct |
| Fault injection: collision branch replaced with `let _ = …` | `test_sel4_root` fails | Direct |
| Fault injection: `if !declared { return Err(…) }` replaced with `let _ = declared` | `test_sel4_root` fails | Direct |
| `just sel4_stream_check` | Pass — the real graph's six participants all resolve, so the check is not refusing valid graphs | Direct |
| The other ten seL4 gates | Pass | Direct |
| `just fmt_check_all`, `lint_all` | Pass | Direct |

The two fault injections abort with `SIGABRT` rather than a clean assertion
message. That is this host's known panic-abort defect (`fatal runtime error:
failed to initiate panic, error 5`), reproduced previously in a bare `cargo new`
crate — the tests do fail, and the exit code is what the gate reads.

## Decisions

- Decision: check provenance **participant → component**, not both ways.
- Rationale: a component with no participant is ordinary — most components are
  not on the fabric. A participant with no component is a graph promising an
  edge to nobody. Only one direction can be wrong, and the test asserts the
  other direction stays legal so the check cannot drift into refusing normal
  generations.

- Decision: split `participants_are_declared` out over component names.
- Rationale: the check needs a `Generation`, and no test in this module builds
  one. Hand-building the full binary layout would make the fixture the thing
  under test — the failure mode already seen where a hand-built graph's derived
  identities had to be recomputed to decode at all.

- Decision: drop a second collision test I had written.
- Rationale: `an_ambiguous_schema_table_never_reaches_admission` re-asserted
  `decode` returns an error, which the first test already covers. It read as two
  properties and was one. A test count is not a coverage measure.

## Open risks and follow-ups

- [ ] **Provenance is checked by name hash, not by the fabric's own table.** The
      strongest form would compare `FABRIC_CLIENTS` against the graph directly,
      but that table lives in a component crate the root does not link. The
      check as written catches the case that motivates C8.3 — a component
      dropped from the manifest while its participant remains — because both
      artifacts derive from the same fixture and the graph is the authenticated
      one.
- [ ] **P5.4.10 is now complete**: five rows closed, two reclassified as needing
      no seL4 gate, one partial for a structural reason recorded in its own
      entry.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`tests.log`](tests.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md),
  [C8.1](../../roadmap/02-core-runtime.md),
  [C8.3](../../roadmap/02-core-runtime.md).

## Corrections

- **2026-08-07.** The Changes table claims "`Justfile`, `AGENTS.md` | Host test
  count 107 → 109". Only the `Justfile` was updated; `AGENTS.md:76` still read
  "107 host unit tests" when this entry was committed. The earlier edit pass
  matched the string "107 host tests", which does not occur — the gate index
  words it "107 host **unit** tests". Now corrected to 109. This is the same
  count-drift failure mode a previous review round caught on this exact gate,
  which is why the entry recorded the change as landing in both places.

- **2026-08-07.** The Verification row citing `tests.log` for "Pass, 109/109
  across 13 modules" over-reads its artifact: `tests.log` is a *filtered* run of
  the two new tests ("2 passed … 107 filtered out"). The arithmetic corroborates
  the 109 total and the full suite does pass, but the cited log does not show
  it. The sibling QoS entry's log does show its full run; this one should have.

- **2026-08-07.** "The other ten seL4 gates | Pass" should read *nine*: the row
  above it already accounts for `sel4_stream_check` separately, and ten is the
  total including it.

  All three found by an independent documentation review of this milestone.

- **2026-08-07.** The C8.3 provenance check as first committed covered only
  `ParticipantEntry::component_identity`. An independent code review of this
  milestone found two further component identities in the same authenticated
  resource that cross the same trust boundary and were never compared against
  the generation:

  - `FabricGraph::fabric_component_identity()` — the fabric host. `decode` only
    rejects an all-zero value. A graph naming a host the manifest dropped fits
    every ceiling and passes the participant arm, and *no* participant would
    receive anything, because the component that mints every route half does
    not exist.
  - `InterpositionEntry::component_identity` — `validate_interposition` checks
    chain termination, revisits, and self-bypass, never membership. A hop is a
    mandatory proxy on its route, so a dropped one silently breaks the route it
    was added to mediate.

  Both are now checked by `participants_are_declared`, host arm first since it
  disables the whole graph rather than one edge. Not hypothetical: the retained
  generation this root boots by default carries `interpositions=1`, so the hop
  arm is exercised on every fixture-variant boot — confirmed by
  `sel4_root_boot_check` still admitting that graph after the change. The
  fixture's fabric identity became name-derived rather than `[0xab; 32]` so the
  test builds an admittable graph, and a fourth case pins the host arm
  specifically: every participant declared, host dropped, refused.
