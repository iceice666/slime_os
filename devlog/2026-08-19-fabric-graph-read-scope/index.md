# Scoping the fabric-graph read against measured consumer needs

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Audit |
| Status | Verified |
| Scope | `components/bins/src/default_fabric_profile.rs` consumers, `boot-contracts/src/fabric_graph.rs`, `scripts/build/build-generation.py` |
| Roadmap | B70, CP2 |
| Gates | `just devlog_check`, `just typos` |
| Trigger | B70's remaining surface was about to be closed by building a `fabric-graph` resource-read syscall, on a backlog figure of "44 graph facts" that had never been measured |
| Baseline | 18 `include!` sites over three `build.rs`-generated tables; `fabric_profile` at 49 constants after `FABRIC_SUPERVISION`/`FABRIC_SUBSCRIBERS` were deleted |

## Summary

The backlog recorded B70's remainder as graph facts waiting on a `fabric-graph`
read. Counting them by *what each symbol is* rather than by symbol count shows
that is true of well under half. Of 128 live uses of generated `fabric_profile`
symbols, **39 are declared capacity bounds that size fixed arrays at compile
time** and no runtime query can replace, **31 are one boot-action string** that is
not graph data at all, **55 are genuine graph data**, and 3 are slot-ish. The read
is still worth building, but it closes 55 uses, not the surface. No code changed;
this entry exists so the next milestone is scoped by what it can actually retire.

## Observable symptom

- Command: `grep` for each symbol `default_fabric_profile.rs` declares, excluding
  the profile itself and comment-only lines.
- Expected (from the backlog): the remaining `fabric_profile` surface is
  predominantly graph facts, closable by an authenticated graph read.
- Observed: 39 of 128 uses are bounds compiled into array types and
  `const _: () = assert!` guards; 31 are `GENERATION_BOOT_ACTION`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | An early count put `ROUTE_NAMES` at 31 uses among the graph facts | Wrong: it is a hand-written `const` in `fabric-service.rs` and `matrix_broker.rs`, not generated at all. Removed from the generated-symbol count |
| 2 | Enumerated the 49 symbols the profile actually declares and counted live uses of each | 128 total, not the ~47 the constant count suggested |
| 3 | `MAX_PARTICIPANTS = FABRIC_MAX_PUBLISHERS + FABRIC_MAX_SUBSCRIBERS`, `Trace::new(FABRIC_TRACE_DEPTH)` via a `const fn` | Bounds are array sizes in a `no_std` component with no allocator; a runtime value cannot stand in |
| 4 | `GENERATION_BOOT_ACTION` appears only as `== "traffic"`, `== "matrix"` branches | A schedule selector — "which composition am I booted into" — not a fact about the graph |
| 5 | `build-generation.py` already writes `resolved_profile.graph_bytes` as the `fabric-graph` resource object, and `FabricGraph` is a complete decoder the root uses for admission | The read is an access path, not a format: nothing new needs designing on the wire |
| 6 | No component can reach resource bytes: no `KIND_RESOURCE` path in `components/runtime/src/syscall.rs`, and no component decodes `FabricGraph` | The mechanism genuinely does not exist, as recorded |
| 7 | `ParticipantEntry` keys on `component_identity`/`route_index`; `declared_qos(component: &[u8], route: &str)` keys on strings | The gap is name-vs-identity, and it is bridgeable: `route_identity()` is already called by four components |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/00-backlog.md` | Replaced "44 graph facts" with the measured four-way split | The remaining surface is stated by what each part needs, so a milestone cannot be scoped against a figure that counts compile-time bounds as runtime-closable |

No source changed. The classification is the deliverable.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Per-symbol live-use count over the 49 declared symbols | 128 uses: 39 bounds / 31 boot action / 55 graph data / 3 slot-ish | Direct |
| `MAX_PARTICIPANTS`, `Trace::new` read at their definitions | Both consume bounds in const position | Direct |
| `grep` for `KIND_RESOURCE` in the component runtime, and for `FabricGraph::decode` under `components/` | No matches in either | Direct |
| `just devlog_check`, `just typos` | Pass | Direct |

## Open risks and follow-ups

- [ ] The graph read remains unbuilt and is now scoped: it answers the 55
      graph-data uses, keyed by identity rather than by name.
- [ ] The 39 bounds need a *declared-capacity contract* an out-of-tree component
      can compile against — a CP3/CP4 question, not a syscall. They should stop
      being counted as syscall-blocked surface.
- [ ] `GENERATION_BOOT_ACTION`'s 31 uses are their own smaller question: the root
      already delivers a boot action to the bootstrap instance, so this may need
      no new mechanism at all.
- [ ] `ROUTE_NAMES` is hand-written, so it is a component asserting which graph it
      belongs to. A graph read is what would let it stop.

## Artifacts and provenance

- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)
- Predecessor: [`devlog/2026-08-19-supervision-binding-naming/`](../2026-08-19-supervision-binding-naming/index.md)
