# Visibility plane reads its participant facts from the graph

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/visibility_broker.rs`, `components/bins/src/fabric_self_view.rs`, `roadmap/00-backlog.md` |
| Roadmap | B70, B72 |
| Gates | `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_fabric_aggregate_check`, `just sel4_qos_check` |
| Trigger | Continuing B70/CP2's migration of `FABRIC_PARTICIPANTS` use sites after `6e7b530` moved `matrix_broker` |
| Baseline | `51d8280^` — the visibility broker answered every view record from three `build.rs`-generated tables |

## Summary

`visibility_broker` answered each client's introspection view by joining three
tables that `components/bins/build.rs` generates by parsing the manifest:
`FABRIC_PARTICIPANTS`, `FABRIC_VISIBILITY`, and `FABRIC_QOS`. Two of them were
joined **positionally** — `FABRIC_QOS[offer_index]` was assumed to describe
`FABRIC_PARTICIPANTS[offer_index]` — a correspondence that held only because a
single `build.rs` pass emitted both in the same order. The fabric graph resource
carries all three facts as fields of one participant row, so the broker now
reads them from `CAPABILITY GRAPH READ` and the positional join disappears into
a field access. This removes the visibility plane's last compile-time coupling
to that private parser, which is B70's exit clause. All four seL4 planes that
consume the widened row still pass.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `fabric_self_view::Row` | Widened with `visibility: u32` and `qos: TransportQos`, decoded at the offsets `FabricGraph::participant` uses (`72..76`, `80..115`); `EMPTY_ROWS` extended in lockstep | A row carries every participant fact a broker asks about, from one read |
| `visibility_broker::GraphView` | New; reads the participant table once at startup and resolves each declared route name to its graph index | The whole-table read is hoisted out of the dispatch loop, which shares one transfer window with descriptor-granting |
| `nth_visible_route`, `qos_for`, `route_matched` | Read `visibility`, `qos`, and `direction` off the graph row instead of the three generated tables | The offer/request join is two fields of one row, not two arrays at one index |
| `route_interface` | Collapsed to the local stream match `route_identities` already folds | Interface and contract kind agree with the identity the route resolves by, or the route is unresolvable |
| `transport_qos` | Deleted — it existed only to map a `FabricQosRow` tuple onto `TransportQos` | — |
| `roadmap/00-backlog.md` | Added B72 for the gate gap found by mutation testing | — |

No root-side or protocol change was needed: `slime-root/src/ipc.rs:918` already
stages raw 128-byte `participant_bytes`, so visibility and QoS were on the wire
and only `Row`'s four-field decode was discarding them.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The view stops reading the graph and answers from stale state | `just sel4_visibility_check` | Mutation forcing `row_count` to 0 → `SLIME_ROOT FATAL SLIME_GRAPH FAIL` |
| A declared route name is absent from the graph | `just sel4_visibility_check` | `fail(b"a declared route name is absent from the graph")` at startup |
| An incomplete read is read as an empty table | `just sel4_visibility_check` | `fail(b"visibility graph read did not complete")` |
| The widened `Row` breaks its other consumers | `just sel4_fabric_aggregate_check`, `just sel4_qos_check` | Plane gate failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_visibility_check` | Pass — 25 markers across 7 causal chains; 12 view records and 2 distinct traces | Direct |
| `just sel4_matrix_check` | Pass — includes the unsatisfiable-QoS admission refusal | Direct |
| `just sel4_fabric_aggregate_check` | Pass — 280 byte-identical semantic-trace records | Direct |
| `just sel4_qos_check` | Pass — 14 markers across 9 causal chains | Direct |
| `just fmt_check_all`, `just lint_all` | Pass | Direct |
| Mutation: `row_count` forced to 0 | Gate **fails** — proves the view reads the graph | Direct |
| Mutation: route indices swapped (`index ^ 1`) | Gate **passes** — see B72 below | Direct |

The second mutation was re-run against unmodified pre-migration code, in the
equivalent form that swaps the QoS lookup's route while leaving every route
name intact. It passed there too, so the gap is pre-existing and this change
introduced no regression. Recorded as B72 rather than fixed here, because
closing it means strengthening a gate's assertions rather than finishing this
migration.

## Decisions

- Decision: keep `route_matched` despite admission making its QoS half a tautology.
- Rationale: `slime-root/src/generation.rs:257` refuses any generation where
  `all_pairs_qos_compatible` is false, so on a plane that boots no matched pair
  can be incompatible. But `route_matched` is an `any`/`any` — an *existential*
  question — and a route declaring only offers, or only requests, is admissible
  and matches nothing. That half still falsifies, and it is what the record's
  `matched` byte reports.
- Rejected alternative: replacing the body with `true`. It would agree with
  every current fixture and silently misreport the first half-declared route.

- Decision: keep `ROUTE_NAMES` local and translate name → identity → graph index.
- Rationale: a route identity is a one-way SHA256 fold of name, interface
  identity, and contract kind, so names are not recoverable from the graph.
  `matrix_broker` and `fabric-service` already resolve this direction only, and
  `ROUTE_NAMES` is a hand-written `const`, not a generated table — so it is not
  the coupling B70 names.
- Rejected alternative: adding names to the graph resource, which would make a
  component's assertion about which graph it belongs to unfalsifiable.

- Decision: mirror `matrix_broker`'s existing `GraphView` rather than design a
  new shape.
- Rationale: that struct already encodes the startup-read-once, resolve-indices,
  query-by-content pattern this broker needs, including the reason the read must
  not happen per request.

## Open risks and follow-ups

- [ ] B72 — a QoS record's payload is never checked against the route it
      describes on the visibility plane; `fabric-subscriber`'s `visibility_main`
      counts records without reading a field of either.
- [ ] `matrix_broker.rs:711`'s `nth_visible_route` still reads `FABRIC_VISIBILITY`;
      the widened `Row` now makes it migratable.
- [ ] `FABRIC_CLIENTS`, `FABRIC_INTERPOSITIONS`, and `FABRIC_NOTIFICATION_BINDINGS`
      remain compiled-in, as does `GENERATION_BOOT_ACTION` (bootstrap-only, an
      authority question rather than a table lookup).

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; every result above is reproducible from the
  `just` targets named in the Verification table.
- Serial/debugger/model output: quoted inline in Verification.
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [B72](../../roadmap/00-backlog.md)
