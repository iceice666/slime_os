# Matrix plane reads its visibility policy from the graph

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/matrix_broker.rs`, `roadmap/00-backlog.md` |
| Roadmap | B70, CP2, B73 |
| Gates | `just sel4_matrix_check`, `just sel4_fabric_aggregate_check`, `just fmt_check_all`, `just lint_all` |
| Trigger | Continuing the B70/CP2 migration off `build.rs`-private tables after `bfb9264` moved the visibility plane |
| Baseline | `bfb9264` — visibility plane migrated; `matrix_broker` still read `FABRIC_VISIBILITY` at three sites |

## Summary

`matrix_broker` decided who may see which route from `FABRIC_VISIBILITY`, a
table `components/bins/build.rs` generates from the manifest and keys by route
*name*. Three use sites now read the participant rows the generation already
hands this component through `CAPABILITY GRAPH READ`. This was the last use of
`FABRIC_VISIBILITY` anywhere in the tree outside its own definition. The declared
policy is unchanged and was modeled against the fixture before the change to
confirm that. Verified on QEMU and mutation-tested; the mutation campaign also
surfaced a gate gap, filed as B73.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `matrix_broker::GraphView` | Added `rows_on`, `sees_graph`, `sees_private` | Route rows are selected by resolved graph index, not by `ROUTE_NAMES` position |
| `matrix_broker::nth_visible_route` | Reads graph rows instead of `FABRIC_VISIBILITY`; takes `&GraphView` | Visibility policy is answered from the generation's own participant table |
| `matrix_broker::answer_view` | Threads `&GraphView` through to `nth_visible_route` | — |
| `assert_declared_composition` | Observer's private-grant assertion reads graph rows | The negative assertion and the thing it constrains read one source |
| `matrix_broker` | Deleted local `VISIBILITY_PRIVATE`/`VISIBILITY_GRAPH` `u8` copies | The visibility vocabulary is `boot_contracts::fabric_graph`'s, not a component-local duplicate |
| `roadmap/00-backlog.md` | Filed B73 | — |

### Why the route index cannot be a `ROUTE_NAMES` position

The graph orders its participant table by grant identity; this plane declares
three routes (`telemetry`, `telemetry-alt`, `diagnostics`) in an order that need
not agree. `nth_visible_route` returns a `ROUTE_NAMES` position and its callers
index `ROUTE_NAMES` and `routes` with it, so joining that position directly
against `row.route_index` would have been exactly the name/position coupling B70
targets. `GraphView::route_indices` already resolves each declared name through
`graph_route_index`, which folds the interface identity and contract kind into
the route identity — so `telemetry` under a different contract resolves to no
index at all rather than matching by string. All three new methods key off that
array.

An earlier reading of this migration recorded a blocker: that `ROUTE_NAMES` had
two entries against the graph's three. That was a misread of
`fabric-service`'s two-route table; `matrix_broker` declares its own three-entry
`ROUTE_NAMES` with a comment saying exactly why it cannot borrow the stream
plane's. No blocker existed.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The private branch stops scoping to the holder | `just sel4_matrix_check` | `SLIME_ROOT FATAL` — `fabric-service` exits 1 on the observer's over-wide view |
| A declared route name is absent from the graph | `just sel4_matrix_check` | `matrix graph read did not complete` / route-identity resolution fails at startup |
| Cross-plane regression from the shared broker module | `just sel4_fabric_aggregate_check` | Semantic-trace records diverge between boots |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_matrix_check` | Pass, exit 0, including the unsatisfiable-QoS admission refusal | Direct |
| `just sel4_fabric_aggregate_check` | Pass, exit 0; 280 byte-identical semantic-trace records over two boots | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just lint_all` | Pass | Direct |
| Mutation: private branch drops the holder-identity check | **Fails the gate** — `SLIME_ROOT FATAL SLIME_GRAPH FAIL required instance fabric-service exit status=1` | Direct |
| Mutation: `telemetry-alt`'s two participants flipped `graph` → `private` in the fixture | **Survives** on migrated code *and* on unmodified pre-migration code | Direct |

The declared policy was modeled in Python against `sel4-matrix.zti` before the
change and the migrated code reproduces it exactly, including the cases the
plane exists to draw:

| Component | Visible route positions |
|---|---|
| `fabric-publisher`, `fabric-publisher-b`, `fabric-subscriber-b` | 0, 1, 2 |
| `fabric-subscriber` | 0 |
| `fabric-observer` | 2 |
| `fabric-probe`, `fabric-proxy` | none |

`fabric-probe` is the load-bearing case: the fixture declares it a control
client with no participant edge at all, so a graph-row rewrite had to keep
yielding nothing for it rather than an empty list of known routes.

## Decisions

- **Decision:** the second mutation's survival is recorded as a gate gap (B73),
  not fixed in this change.
- **Rationale:** the same mutation was run against unmodified pre-migration code
  and survived there too, so it is not a regression this change introduced.
  Fixing it means adding an assertion to a component or gate script, which is
  its own change with its own evidence.
- **Rejected alternative:** widening `nth_visible_route` to compensate. There is
  nothing to compensate for — the function reproduces the declared policy; it is
  the gate that never looks at a graph-visibility holder's view.

## Open risks and follow-ups

- [ ] B73 — no component on the matrix plane asserts the count or contents of a
      `graph`-visibility holder's paged view. Gate-side work.
- [ ] B72 — the visibility plane's QoS records are counted but never checked
      against the route they describe. Same shape, filed at `bfb9264`.
- [ ] `visibility_broker::qos_for`'s fallback arm changed selection order from
      route order to grant-identity order during the `bfb9264` migration. The arm
      **is** reached on `sel4-visibility.zti`: `fabric-publisher` holds `graph`
      visibility, so it is shown `diagnostics`, on which it has no row of its
      own. It does not matter there, because both `diagnostics` participants —
      `fabric-publisher-b` and `fabric-subscriber-b` — declare byte-identical QoS
      on every axis, so either selection emits the same record. Not filed as a
      defect for that reason, but the guard is a coincidence of this fixture
      rather than a property of the code: a fixture whose two participants on one
      route declare different QoS would expose the ordering change. Revisit when
      one does. Note this is invisible to the gate either way while B72 is open.
- [ ] `FABRIC_CLIENTS`, `FABRIC_INTERPOSITIONS`, `FABRIC_NOTIFICATION_BINDINGS`
      remain on generated tables. `GENERATION_BOOT_ACTION` is bootstrap-only and
      is an authority question rather than a mechanical migration.
- [ ] `fabric-publisher.rs:128` and `fabric-service.rs:367/385` read both the
      graph and the generated table deliberately, as cross-checks. They should
      not be migrated; dual-sourcing is the assertion.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none retained; every result above is a `just` target
  reproducible at `265b7b5`
- Serial/debugger/model output: quoted inline under Verification
- Related roadmap item: [B70](../../roadmap/00-backlog.md), CP2 in the
  [Component platform track](../../roadmap/10-component-platform.md),
  [B73](../../roadmap/00-backlog.md)
