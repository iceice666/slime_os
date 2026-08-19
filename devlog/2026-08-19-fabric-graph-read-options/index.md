# Fabric-graph read: authority shape and what each option reaches

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Decision |
| Status | Proposed |
| Scope | `slime-root/src/generation.rs` (`fabric_graph_object`), `contracts/syscall-abi/v1`, `components/bins/src/{visibility_broker,matrix_broker,call_broker,operation_broker}.rs`, `components/bins/src/bin/fabric-*.rs` |
| Roadmap | B70, CP2 |
| Gates | `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_fabric_aggregate_check` |
| Trigger | The const-context blocker was removed (163d834), leaving the read buildable and its authority shape undecided |
| Baseline | 54 live graph-data uses across 9 files, all runtime reads; no component can reach resource bytes |

## Summary

With `DECLARED_RING_CAPACITY` gone, every remaining graph-data use is a runtime
read, so a `fabric-graph` read is buildable. What it may *answer* is the open
question, and it is not an implementation detail: C8.8 already treats the route
set as policy the fabric enforces per caller, and `just sel4_visibility_check`
asserts on a real boot that an ungranted caller infers nothing. A read that
returned the graph verbatim would bypass the gate rather than implement it.

Measuring who actually consumes graph data changes the trade-off:
**46 of 54 uses are compiled only into `fabric-service`**, which already holds
the graph as the component the generation designates. Four more are in broker
modules shared with the two workers, and the last four are participants asking
one question that the wire already answers. So the option that keeps C8.8 policy
in userspace is not the narrow one — it is the one that reaches most of the
surface.

## Changes

No code. This entry records the options and the measurements behind them so the
choice is made before an ABI exists that assumes an answer.

### What consumes graph data, measured

| Where | Uses | Compiled into |
|---|---|---|
| `visibility_broker.rs` | 19 | `fabric-service` only |
| `matrix_broker.rs` | 17 | `fabric-service` only |
| `fabric-service.rs` | 10 | `fabric-service` only |
| `call_broker.rs` | 2 | `fabric-service` **and** `fabric-call-worker` |
| `operation_broker.rs` | 2 | `fabric-service` **and** `fabric-op-worker` |
| 4 participant binaries | 4 | each its own component |

**46 uses (85%) never leave `fabric-service`.** That matters because
`fabric-service` is the graph's designated holder — `fabric_component_identity`
is a field of the graph itself — so serving it the graph grants it nothing it is
not already trusted with.

### The four participant uses need no graph read at all

`fabric-publisher`, `fabric-subscriber`, and their `-b` variants each call
`ring_slots(route)` against `FABRIC_HISTORY_DEPTHS` for one reason, stated in
their own comments: the fabric formats the ring at the declared depth, and
`Ring::attach` rejects a mismatch, so a hardcoded local guess was "a
disagreement waiting to happen — and it was one".

But the ring header **already carries `slot_count`**, and
`valid_ring_header` compares it to the caller's expectation
(`components/proto/src/lib.rs:437`). The participant is reading the graph to
predict a number the mapped ring will state authoritatively. Reading the header
instead removes the last generated-table use from all four binaries without any
new mechanism — and removes a class of bug the current shape allows, where a
component that guesses right for the wrong reason still attaches.

This is worth doing regardless of which option below is chosen.

## Decisions

### Option A — the root filters

The root answers a per-caller-filtered view, applying `FABRIC_VISIBILITY` itself.

- Reaches all 54 uses, and any future component's.
- **Moves C8.8 policy from userspace into the root.** The project rule is
  "keep mechanism in `slime-root`; component policy belongs in userspace
  components", and a visibility filter is policy by that reading — it is the
  thing C8.8 exists to demonstrate a *component* enforcing.
- Largest new attack surface: the root gains a filtered-projection operation
  whose correctness is what `sel4_visibility_check` currently proves about the
  fabric.

### Option B — the root serves the holder, the fabric serves everyone else

The root answers only the instance the graph names as its fabric component;
other components keep asking `fabric-service` over their existing control
endpoints, as they do today.

- Reaches 46 uses directly and the 4 shared-broker uses when those run inside
  `fabric-service`; with the participant fix above, that is **50 of 54** with no
  policy moved.
- **C8.8 policy stays exactly where it is.** The root's rule becomes a
  one-line authority test — "are you the graph's declared fabric component" —
  which is mechanism, not policy.
- Leaves `call_broker`/`operation_broker` still `include!`ing the profile when
  compiled into `fabric-call-worker`/`fabric-op-worker`, so B70's `include!`
  count does not reach zero by this route alone.

### Option C — B, plus a declared-participant self-view

As B, and additionally the root answers any instance with *its own*
participant rows — the same scoping `resolve_binding` already uses, where a
caller learns only what it was granted.

- Reaches the two workers too, so all 54.
- The self-scoping precedent already exists and is already gated, so this adds
  no new authority *concept*, only a second table it applies to.
- More surface than B, less than A, and the filter is "your own rows" rather
  than a policy judgement.

### Recommendation

**Option C, staged: participant fix first, then B, then C's self-view if the
worker uses justify it.** The participant fix is independently correct and
testable today. B is the smallest step that moves real surface without
relocating a milestone's policy. C's extension is the same self-scoping rule
CP2 already ships, so it is an increment rather than a new idea.

Option A is the only one that closes everything in one move, and is the one to
take deliberately if the project decides the visibility filter belongs in the
root after all — but that is a C8.8 decision, not a B70 one, and should be made
on its own terms rather than as a side effect of retiring a build table.

## Open risks and follow-ups

- [ ] None of this is built. The entry exists so the ABI is not drafted against
      an unstated authority assumption.
- [ ] `FABRIC_NOTIFICATION_BINDINGS` (2 uses) is the fabric's view of its peers'
      slots. Under B it is answered by the same read; under A it needs the same
      filtering question asked of notification bindings.
- [ ] Whichever option is chosen, one `ParticipantEntry` is 128 bytes against a
      64-byte message bound, so the reply is cursor-paged or record-at-a-time.
      `slime-root/src/directory.rs` and `console.rs` already do this with
      `transfer_window::write_staged_region_with`; no new transport is needed.

## Artifacts and provenance

- Predecessor: [`devlog/2026-08-19-fabric-graph-read-scope/`](../2026-08-19-fabric-graph-read-scope/index.md)
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)
