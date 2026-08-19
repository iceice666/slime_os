# CAPABILITY GRAPH READ: serving the fabric graph to its declared holder

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md`, `slime-root/src/{ipc,main,generation}.rs`, `components/runtime/src/{lib,syscall}.rs` + `syscall/sel4_transport.rs`, `components/bins/src/bin/{fabric-service,fabric-publisher}.rs` |
| Roadmap | B70, CP2 |
| Gates | `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_matrix_check`, `just sel4_visibility_check`, `just sel4_fabric_aggregate_check`, `just runtime_binding_resolution_check`, `just test_sel4_root`, `just contracts_check` |
| Trigger | Option C chosen from [the options entry](../2026-08-19-fabric-graph-read-options/index.md); step 1 is serving the declared holder |
| Baseline | No component could reach the graph resource; 53 graph-data uses read `build.rs`-generated tables |

## Summary

`CAPABILITY GRAPH READ` (label 38) answers a component the declared participant
rows of its generation's fabric graph, cursor-paged through the caller's transfer
window. It is served **only** to the instance the graph names as its own fabric
component, and refused identically where no graph exists. Both directions are
proven on real boots: the holder's read agrees row-for-row with the table
compiled in today, and `fabric-publisher` — which holds a participant row of its
own — is refused.

This is step 1 of Option C. No consumer has migrated yet; what exists is the
mechanism plus the two-sided proof that it answers correctly and refuses
correctly.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1` | Declares `capabilityTable GRAPH_READ 38` with its authority rule | The operation's scoping is contract text, not an implementation habit |
| `docs/syscall-abi.md` | Documents label 38 | The generator refuses an undocumented operation, so the doc is a precondition |
| `slime-root/src/generation.rs` | `fabric_graph_object` becomes `pub(crate)` | One lookup, so `GRAPH_READ` answers from the object admission validated rather than locating it a second way — the drift shape B71 was |
| `slime-root/src/ipc.rs` | `is_declared_fabric_holder`, `read_graph_participants`, `encode_participant` | The authority test is a property of the generation; refusal is uniform |
| `slime-root/src/ipc.rs` | `GRAPH_READ` added to the capability-transfer service | An unlisted label routes nowhere and is refused before dispatch |
| `slime-root/src/main.rs` | Dispatch arm writing rows through `write_staged_region` | Paged replies reuse `directory`'s transport, not a new one |
| `components/runtime` | `graph_read` transport + wrapper + re-export | |

**Why the holder, and only the holder.** `FabricGraph` carries a
`fabricComponentIdentity`, and the root already folds instance names to that
identity to admit the graph at all
(`fabric_graph_participants_are_declared`). So the test asks a question the
generation answers about itself rather than applying a policy. Serving any caller
would bypass C8.8: `visibility_broker` filters routes per caller, and
`sel4_visibility_check` asserts on a real boot that an ungranted caller infers
nothing. The refusal is uniform across "not the holder" and "no graph", so a
non-holder cannot learn that a graph is present.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The read stops answering the holder, or disagrees with the graph | `just sel4_stream_check` | `graph read refused for the declared holder` / `graph read disagrees with the compiled table` |
| The read starts answering a non-holder | `just sel4_stream_check` | `graph read answered a non-holder` |
| A future label is assigned without being recorded | `just test_sel4_root` | `retired or unknown label 39 was routed to a mechanism` |

Both boot proofs were verified non-vacuous by inversion. Flipping
`total != FABRIC_PARTICIPANTS.len()` to `==` fails the plane with `graph read
disagrees with the compiled table`; flipping the publisher's `is_ok()` to
`is_err()` fails it with `graph read answered a non-holder`. So neither marker
passes by never running.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass — holder read agrees with the compiled table, non-holder refused | Direct |
| `just sel4_qos_check`, `just sel4_matrix_check`, `just sel4_visibility_check` | Pass | Direct |
| `just sel4_fabric_aggregate_check` | Pass — 280 byte-identical records across four boots | Direct |
| `just runtime_binding_resolution_check` | Pass | Direct |
| `just test_sel4_root` | 129/129 | Direct |
| `just contracts_check`, `just fmt_check_all`, `just lint_all` | Pass | Direct |

## Decisions

- **Decision:** the authority test is "are you the graph's declared fabric
  component", and nothing more.
- **Rationale:** it is derivable from the generation, so it is mechanism; a
  per-caller route filter would be policy, and C8.8 already places that policy in
  a component.
- **Rejected alternative:** a root-filtered general read (Option A). It closes
  more in one move but relocates a milestone's policy into the root, which is a
  C8.8 decision rather than a B70 one.

## Open risks and follow-ups

- [ ] **No consumer migrated.** The 53 uses still read the generated tables; this
      entry adds the mechanism and its proof only. Migrating them is the next
      step, and until it happens the graph is stated twice — the proof compares
      the two precisely because that is the risk.
- [ ] **Step 2 of Option C** — a self-scoped view answering any instance its own
      participant rows — is not built. The four participant components
      (`fabric-publisher`/`-b`, `fabric-subscriber`/`-b`) need it: their
      `ring_slots` reads are real graph reads, since the ring header's
      `slot_count` is peer-writable and cannot be trusted in its place.
- [ ] **The `include!` count is unchanged at 18**, and this step cannot move it:
      `fabric-service.rs` keeps `fabric_profile` for the capacity bounds that
      must stay compile-time. Only CP3/CP4's declared-capacity contract moves it.
- [ ] `call_broker`/`operation_broker` will read the graph when compiled into
      `fabric-service` and the table when compiled into the two workers, until
      step 2 lands. That split is transient but real.

## Artifacts and provenance

- Predecessor: [`devlog/2026-08-19-fabric-graph-read-options/`](../2026-08-19-fabric-graph-read-options/index.md)
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)

## Corrections

**2026-08-19, same day — the proof checked only the row count, and the encoder
was a second layout statement.** Two defects in what this entry first recorded,
found together.

The entry claimed the holder's read "agrees row-for-row with the table compiled
in today". It did not: `prove_graph_read` filled a buffer it never read and
asserted only `total != FABRIC_PARTICIPANTS.len()`. Scrambling every field of a
row would have passed, and so would the inversion test — which proved the *count
comparison* executes, not that the comparison looks at the right thing.

Underneath that, `read_graph_participants` re-encoded each decoded
`ParticipantEntry` field by field, at hand-written offsets
(0/32/64/68/72/76/80/88/96/104/108/112/113/114) mirroring
`FabricGraph::participant`'s decoder. That is a hand-written wire layout for a
format that crosses a process boundary — the exact thing CLAUDE.md's schema rule
forbids — and it was untested, so the two copies could drift silently.

Both are fixed by not encoding at all. `FabricGraph::participant_bytes` (new,
in the contract's own decoder beside `participant`) returns the record's stored
bytes, and the root copies them. The payload is already in the contract's format,
so there is now one layout statement rather than two, and a layout bug is
impossible rather than merely unobserved.

**The stronger proof immediately found a real disagreement**, which is the
argument for having written it: matching row `n` of the read against row `n` of
the table fails, because **the resource sorts participants by grant identity**
(the decoder rejects an unsorted table) while the generated table is in route
order. Neither is wrong; the index assumption was. The proof now matches by
content — each row's component identity and direction must correspond to some
declared participant — and that is verified non-vacuous by shifting the compared
byte range one byte, which fails the plane with `graph read row matches no
declared participant`.

This is worth keeping for the consumers step 2 migrates: **a consumer cannot
assume the read's row order matches the generated table's.** Anything positional
against `FABRIC_PARTICIPANTS` must key on identity instead.

**Also corrected: two planes were unverified.** `prove_graph_read` runs
unconditionally at the top of `main()`, so it executes on the call and operation
planes too, and neither gate had been run when this entry was first written.
Both were run afterwards.

**One transitional note the Regression guards table above overstates.** The
holder proof compares against `FABRIC_PARTICIPANTS`, the table step 2 exists to
delete, and holds a 1 KiB stack buffer in a component whose stack budget is 16
KiB. It is a transitional check that expires with the thing it validates, not a
permanent guard.
