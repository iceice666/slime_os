# Self-scoped graph rows, and the first consumers off the generated table

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md`, `slime-root/src/{ipc,main}.rs`, `components/runtime/src/{lib,syscall}.rs` + `syscall/sel4_transport.rs`, `components/bins/src/fabric_self_view.rs`, `components/bins/src/bin/fabric-{publisher,subscriber,publisher-b,subscriber-b}.rs` |
| Roadmap | B70, CP2 |
| Gates | `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_fabric_aggregate_check`, `just runtime_binding_resolution_check`, `just test_sel4_root`, `just contracts_check` |
| Trigger | Option C step 1 landed the holder-scoped read (17d436c); step 2 is the self-scoped view the four participants need |
| Baseline | `GRAPH_READ` answered only the declared fabric holder; participants read `FABRIC_HISTORY_DEPTHS` |

## Summary

`GRAPH_READ` now answers any instance its *own* participant rows, and the four
stream participants derive their ring depth from it instead of from the
generated table. The holder still reads every row; everyone else reads only the
rows naming them, which is the scoping `resolve_binding` already applies to
bindings. A second operation, `GRAPH_ROUTE_INDEX` (39), resolves the route
identity a participant folds locally to the index its rows carry, so no
component assumes the resource's sort order.

C8.8 is untouched: a participant still cannot enumerate the graph, so per-caller
route filtering remains the fabric's to enforce.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1` | `GRAPH_READ` rescoped; `GRAPH_ROUTE_INDEX 39` declared | The scoping rule is contract text |
| `slime-root/src/ipc.rs` | `read_graph_participants` filters by caller identity; `route_index_for` added | A component learns what the generation declares about it, and nothing else |
| `slime-root/src/ipc.rs` | `cursor` counts rows *the caller may see* | A participant cannot infer where its rows sit among everyone else's |
| `components/bins/src/fabric_self_view.rs` | New: fold identity, ask the root, read own row | One implementation for four components |
| 4 participant binaries | `ring_slots` resolves through the root, cross-checked against the table | The depth is generation data, not a compiled constant |

**Why a participant cannot derive `route_index` locally.** It knows its route by
identity -- it folds name, interface identity, and contract kind exactly as the
builder does -- but a row names the route by index into a table the *resource*
sorts by identity, which is not the order the generated table uses. Deriving the
index locally would be assuming that sort order, so the root answers the fold.
The operation is unscoped and safe for any caller: the identity is one the asker
already holds, so the answer confirms a fold it computed itself.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The self-scope leaks another component's rows | `just sel4_stream_check` | `graph self view returned rows this component does not declare` |
| The root's depth disagrees with the declared graph | `just sel4_stream_check` | `graph read ring depth disagrees with the compiled table` |
| A future label is assigned unrecorded | `just test_sel4_root` | `retired or unknown label 40 was routed to a mechanism` |

Both boot checks were verified non-vacuous by inversion: flipping
`resolved != compiled` fails the plane with the depth-disagreement marker, and
the self-view probe was observed failing before the scoping landed (`graph read
answered a non-holder`) and passing after, with the root's own markers showing
the split -- `rows=6` for the holder, `rows=1` for the publisher.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass -- self-view scoped, all four depths agree | Direct |
| `just sel4_qos_check`, `sel4_visibility_check`, `sel4_matrix_check` | Pass | Direct |
| `just sel4_call_check`, `sel4_operation_check` | Pass | Direct |
| `just sel4_fabric_aggregate_check` | Pass -- 280 byte-identical records | Direct |
| `just runtime_binding_resolution_check` | Pass | Direct |
| `just test_sel4_root` | 129/129 | Direct |
| `just contracts_check`, `fmt_check_all`, `lint_all` | Pass | Direct |

## Decisions

- **Decision:** a non-holder reads its own rows; `cursor` counts its own rows.
- **Rationale:** it is `resolve_binding`'s rule applied to a second table, so it
  adds no new authority concept. A component with no rows reads nothing, so
  nothing is disclosed that its holder did not already know.
- **Rejected alternative:** overloading `GRAPH_READ`'s cursor word as a mode
  selector to avoid a second label. A hidden mode switch is harder to audit than
  a declared operation, and the ABI's shape is one label per question.

## Open risks and follow-ups

- [ ] **The generated-table count has not dropped.** `FABRIC_HISTORY_DEPTHS`
      still shows 6 live uses and `FABRIC_PARTICIPANTS` 18, because each migrated
      participant now holds *both* paths: the root's answer and the compiled
      lookup it is checked against. That cross-check is deliberate transitional
      scaffolding -- it is the only evidence available while two statements of the
      graph coexist -- and deleting it is what actually retires the table.
- [ ] The remaining consumers (`visibility_broker` 19, `matrix_broker` 17,
      `fabric-service` 10, the two shared brokers 4) are unmigrated. They read
      the graph as a whole, which the holder-scoped read already answers.
- [ ] `call_broker`/`operation_broker` compiled into the two workers read as
      non-holders, so they see only those workers' own rows. Whether that is
      sufficient for them is unexamined.
- [ ] The `include!` count is unchanged at 18.

## Artifacts and provenance

- Predecessor: [`devlog/2026-08-19-fabric-graph-read/`](../2026-08-19-fabric-graph-read/index.md)
- Decision record: [`devlog/2026-08-19-fabric-graph-read-options/`](../2026-08-19-fabric-graph-read-options/index.md)
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)

## Corrections

**2026-08-19, same day — the cross-checks are deleted, and one site cannot
migrate.** The follow-up above records the generated-table count as unchanged
because each migrated participant held both paths. Those cross-checks are now
gone: the four participants read only the root's answer, and
`FABRIC_HISTORY_DEPTHS` drops from **6 live uses to 1**.

`fabric-service`'s `declared_history_depth` migrated too, and it needed the
*holder*-scoped read rather than the self view — the fabric provisions a ring
*for* each participant, so it asks about another component. The row's
`qos.historyDepth` is the same number the table carried: `depth_rows` in
`build-generation.py` is generated from the same `participants` list the resource
encodes, so this is one source replacing a copy of itself.

**`operation_broker` cannot migrate, and the reason is structural.** It reads a
*client's* depth, so it needs holder scope, but the module is compiled into two
binaries: `fabric-service`, which is the graph's declared holder, and
`fabric-op-worker`, which is not. On `sel4-boot` the worker runs that path as a
non-holder and would read only its own rows, resolving nothing exactly where
C8.10 needs it. Nothing in scope lets the module branch on which binary hosts it
either. This was checked against both manifests before attempting it —
`fabricComponent` is `fabric-service` on `sel4-operation` and `sel4-boot` alike
— rather than discovered by a failing boot. The site now carries that
explanation.

Closing it needs a graph-model change: either the worker declared as a holder of
the routes it brokers, or a read scoped to "components on routes I broker".
That is the same shape as the `call_broker`/`operation_broker` question the
follow-up above already flags as unexamined, and it is now examined: the answer
is that a non-holder read is *not* sufficient for them.

Deleting the cross-checks also made `COMPONENT` dead in three of the four
participants — it existed only to key the compiled lookup. Clippy caught it,
which is the guard working: the constant named a component's own identity for a
table that no longer exists.
