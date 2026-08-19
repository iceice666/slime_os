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

## Corrections

**2026-08-19, same day — the bounds are not blocked, and the exit clause is
structural.** The follow-up above says the 39 capacity bounds "need a
declared-capacity contract an out-of-tree component can compile against — a
CP3/CP4 question". Half right and half wrong, and the wrong half would have
mis-sequenced a milestone.

Right: they cannot be runtime-resolved, because they size fixed arrays in a
`no_std` component. Wrong: the contract they need is not owed by anyone, it
already exists. `contracts/fabric-graph/v1/schema.zt` declares every structural
ceiling — `maxParticipants = 32`, `limitSampleBytes`, `limitCapabilitySlots`,
`maxIngressSources` — and `boot-contracts/src/generated/fabric_graph.rs`
publishes them as Rust constants generated from that schema
(`pub const MAX_PARTICIPANTS: usize = 32;`), which `FabricGraph::validate_against`
already enforces every generation's declared limits against at admission.

That distinction matters because B70's exit clause reads "no component source
file `include!`s a **`build.rs`-private**, manifest-derived constant table". It
constrains where the table *lives*, not whether the value is compile-time. A
component compiling against the published ceiling, with the per-graph value
checked at runtime against the capacity it was built to hold, satisfies the
clause with no syscall — so the bounds are closable by moving their home, and are
not a reason B70 must wait on CP3.

**`GENERATION_BOOT_ACTION` is not free either.** The follow-up says the root
"already delivers a boot action to the bootstrap instance", implying the path
exists. It does, and it is *bootstrap-only*: `slime-root/src/main.rs:1964-1968`
passes `boot_action.id()` as the startup argument when
`instance_index == generation.bootstrap()` and `0` otherwise, with the comment
"Only the bootstrap instance composes a boot graph, so only it is told which
one." `fabric-service` and the participants branching on this string are not the
bootstrap instance, so closing those 31 uses means widening that delivery or
answering it through the root — and the boot action is deliberately a fact only
`init` holds, so it carries an authority question rather than being plumbing.

**On this entry's gates.** `Status: Verified` normally requires a behavioural
gate, and `devlog_check`/`typos` are hygiene the README treats as assumed. Named
here because this entry changed no runtime behaviour: what it asserts is a
measurement over source, reproducible by the per-symbol counts in the
investigation log, and there is no plane whose boot would confirm or refute it.

**2026-08-19 — "compile against the published ceiling" is refuted on a real
boot.** The correction above argued the 39 capacity bounds are closable by moving
their home: compile against `boot-contracts`' published ceiling, check the
per-graph value at runtime. Tried against `MAX_PARTICIPANTS` — the smallest
useful case, 7 on the stream graph against a ceiling of 32 publishers + 32
subscribers — and it **faults the component**:

```
SLIME_ROOT FATAL SLIME_GRAPH FAIL required instance fabric-service fault
seL4 stream plane check: failure marker in serial transcript
```

The reason is that the arrays are `main`'s stack locals, not statics.
`.bss` was byte-identical between the two builds (65,624 either way) and the ELF
grew 32 bytes, so both of the measurements that were easy to take said "free";
the binding constraint is `COMPONENT_DEFAULT_STACK_BYTES = 16384`, against which
two ceiling-sized `[Option<Publisher/Subscriber>; 64]` arrays are roughly 10 KB —
over half the stack before `Frame` or anything else is placed. An ELF-size or
`.bss` comparison could never have shown this; only the boot did.

So the bounds are **not** closable by re-homing them, and the earlier correction
was wrong in the opposite direction to the note it corrected: the first said they
needed a contract that does not exist, the second said the existing contract was
enough. Neither holds. A component can compile against a ceiling only where the
ceiling is close enough to the real value to fit its budget, and for the
participant arrays it is 9× too large.

**What does work is already in the tree, and is the pattern to follow.**
`components/proto/src/trace_sink.rs:145` sizes its storage at the published
ceiling `[BLANK; MAX_TRACE_DEPTH]` and carries the per-graph depth as a *runtime*
`capacity` field, asserting `capacity <= MAX_TRACE_DEPTH` in a `const fn`. That
works there because `MAX_TRACE_DEPTH` is 64 records, small enough that the
ceiling-sized array is affordable. It is the right shape for the bounds whose
ceilings are near their real values, and the wrong shape for the participant
arrays — so the split is not "bounds vs graph facts" but **per bound, whether its
ceiling fits the budget**.

That makes this genuinely CP3/CP4 work rather than a filing change: an
out-of-tree component needs its *own* declared capacity, admitted against the
graph it is composed into, which is a contract question about component
specification and not a table that can be moved.
