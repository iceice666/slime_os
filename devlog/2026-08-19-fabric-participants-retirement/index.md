# Retiring FABRIC_PARTICIPANTS, and the gate that outlived what it guarded

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/bin/fabric-service.rs`, `components/bins/src/bin/fabric-publisher.rs`, `scripts/build/build-generation.py`, `scripts/check/check-data-fabric-profile.py`, `components/bins/src/default_fabric_profile.rs` |
| Roadmap | B70, CP2, B74 |
| Gates | `just data_fabric_profile_check`, `just generation_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_traffic_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_fault_check`, `just sel4_fabric_aggregate_check`, `just fmt_check_all`, `just lint_all`, `just ruff` |
| Trigger | `48f5876` retired the last two live `FABRIC_PARTICIPANTS` consumers |
| Baseline | Both components cross-checked a graph read against the compiled-in participant table; `check-data-fabric-profile.py` compared that table against the canonical profile |

## Summary

`FABRIC_PARTICIPANTS` was the last generated participant table with live
consumers, and B70/CP2 requires no component source to compile in a
manifest-derived constant table. The two remaining consumers were assertions
that compared a `GRAPH_READ` reply against the table, and `48f5876` retired
both along with the table's emission. The interesting part was not the deletion
but what the assertions could still honestly claim afterwards: with one
statement of the graph instead of two, a row *count* can only be re-derived from
the reply being checked, so the cardinality claims were dropped rather than
restated circularly, and replaced with content claims that stay independent
because `component_identity` is a locally-computed hash. Following up on that
commit, the `check-data-fabric-profile.py` assertion retargeted in the same
change was tested and found decorative — it passed with a participant silently
dropped from the rendered Rust — and has been rewritten to fire on both a
missing row and a duplicate masking one, each arm observed failing.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `fabric-service::prove_graph_read` | Dropped the hand-rolled paging loop and `total != FABRIC_PARTICIPANTS.len()`; reads through `fabric_self_view::rows` and asserts no returned row carries its own identity | The holder's whole-table scope is asserted without re-deriving the expectation from the reply |
| `fabric-publisher::prove_graph_self_view` | Replaced `count != own` and the byte-slice sibling comparison with "every returned row is mine" | A leaked row fails regardless of which sibling it names |
| Both | Reject `count == 0` | A collapsed scope and an empty graph are different failures, and only refusal surfaces itself |
| `render_fabric_profile_rust` | Removed the `FABRIC_PARTICIPANTS` emission and its row builder; removed the write-only `participant_rows_data` accumulator left behind | No component compiles in a manifest-derived participant table |
| `check-data-fabric-profile.py` | Rewrote the retargeted participant assertion to slice each table body and assert membership *and* row count | A dropped or duplicated participant fails the gate again |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A participant silently dropped from the rendered profile | `just data_fabric_profile_check` | `rendered Rust FABRIC_QOS has 14 rows for 15 declared participants` |
| A duplicated participant masking a dropped one | `just data_fabric_profile_check` | `rendered Rust FABRIC_QOS diverges from the canonical profile participants` |
| The holder's graph scope collapsing to its own rows | `just sel4_stream_check` and every plane launching `fabric-service` | `graph read answered the declared holder no rows` |
| A non-holder's scope leaking another component's rows | `just sel4_stream_check` | `graph read disclosed a component this one shares no edge with` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_profile_check` | ok | Direct |
| Perturbation: drop `participants[0]` from `qos_rows`/`visibility_rows`, regenerate | **Old check passed** — only the staleness arm fired | Direct |
| Same perturbation against the rewritten check | Failed at the row-count arm, naming the generator | Direct |
| Perturbation: duplicate `participants[1]` over `[0]`, preserving cardinality | Failed at the membership arm | Direct |
| `just generation_check` | `65f60c1163a7…`, unchanged from baseline | Direct |
| `just sel4_stream_check` | 57 frozen markers plus 4 declared seL4-only | Direct |
| `just sel4_qos_check` | 14 markers, 9 chains, six participants clean | Direct |
| `just sel4_visibility_check` | 26 markers, 7 chains, 12 view records | Direct |
| `just sel4_matrix_check` | incompatible QoS pair fails closed at admission | Direct |
| `just sel4_traffic_check` | 19 participants across three planes concurrently | Direct |
| `just sel4_call_check` | 47 markers, 10 chains plus 1 order-independent | Direct |
| `just sel4_operation_check` | 53 markers, 15 chains, six tasks clean | Direct |
| `just sel4_fault_check` | 10 markers, 8 isolation markers intact | Direct |
| `just sel4_fabric_aggregate_check` | 280 byte-identical trace records — after two flaky failures, see B74 | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | clean | Direct |

## Decisions

- **Decision:** Drop the cardinality claims rather than restate them against the reply.
  **Rationale:** `prove_graph_read`'s own doc named its condition verbatim — the
  table was compared "because agreement between two independently-derived
  statements of the graph is the only evidence available *before the consumers
  migrate*." Once the table is gone there is one statement, and comparing a reply
  against a number read out of that same reply asserts nothing.
  **Rejected alternative:** Keeping a count by summing rows from the reply, which
  would have preserved the shape of an assertion with none of its content.

- **Decision:** Assert "no row is my own" for the holder and "every row is mine"
  for the non-holder.
  **Rationale:** `component_identity` is a hash of a name each component spells
  itself, so the expectation never passes through the root. Verified across all
  fixtures that `fabric-service` appears as `fabricComponent` in 9 and as a
  `component` participant in none, so the holder declares zero own rows on every
  plane it runs on; `fabric-publisher` declares exactly 1 row in each of its 7.
  **Rejected alternative:** Comparing against a sibling's name, which the old
  publisher check did and which only catches the one leak it names.

- **Decision:** Retarget the profile gate's participant assertion rather than
  delete it, then test that the retarget works.
  **Rationale:** The first retarget searched the whole rendered file for a
  `(b"component", "route", ` prefix, which `FABRIC_NOTIFICATION_BINDINGS` also
  renders — so the substring survived the rows being dropped from `FABRIC_QOS`
  and `FABRIC_VISIBILITY` entirely. Scoping the search to one table body at a
  time, and adding a row count, makes both failure modes visible.
  **Rejected alternative:** Trusting the staleness comparison alone, which does
  catch a regenerated mistake but reports it as "checked-in profile is stale",
  pointing at the file rather than the generator.

## Open risks and follow-ups

- [ ] `just sel4_fabric_aggregate_check` failed twice in this session on the
      traffic schedule's boot 2 with two different signatures, then passed on the
      committed baseline and again with changes restored. Filed as B74.
- [ ] 17 dead symbols remain in `render_fabric_profile_rust` (16 consts/types
      plus the private `konst_str_eq`), each re-verified to have zero live
      external uses. `FabricNotificationBindingRow` and `FabricQosRow` must stay —
      they type consts with live uses.
- [ ] The two `include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"))` lines —
      literally what B70's exit clause forbids — remain at `fabric-service.rs:107`
      and `fabric-publisher.rs:50`. Both files still reference
      `GENERATION_BOOT_ACTION` from that include, so removal is blocked until that
      is resolved.
- [ ] Retired-context comments still naming `FABRIC_PARTICIPANTS` in
      `visibility_broker.rs`, `fabric-service.rs`, and `fabric-publisher.rs` are
      intentional historical notes, but worth a consistency pass.
- [ ] CLAUDE.md states `just test_sel4_root` runs 118 tests across 14 modules;
      the actual count is 130 across 15.

## Artifacts and provenance

- Focused report: none; the decisive reasoning is in this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: gate summary lines quoted inline under *Verification*.
- Related roadmap item: [B70](../../roadmap/00-backlog.md#b70--component-definitions-and-slotroute-bindings-are-compile-time-coupled-to-one-crates-private-manifest-parser-blocking-out-of-tree-components), [B74](../../roadmap/00-backlog.md)
