# C8.10 groundwork — Declared route-worker partition and wait-source bounds

| Field | Value |
|---|---|
| Date | 2026-07-30 |
| Kind | Change |
| Status | Verified |
| Scope | Resolved fabric-profile contract, generation resolver, generation fixture, generated userspace profile, three split participant binaries, kernel component-identity trackers |
| Roadmap | C8.10 |
| Gates | `just data_fabric_profile_check`, `just fmt_check`, `just fmt_check_components`, `just lint`, `just lint_components` |
| Trigger | C8.10 implementation |
| Baseline | C8.9 closed the typed full profile, but the fabric's stream, call, and operation planes remained mutually exclusive generation profiles physically aliasing one range of init capability slots, and nothing declared how the graph would be partitioned across `SYS_WAIT` sets. |

## Summary

This entry lands the declarative half of C8.10 and deliberately stops short of
the boot-path replacement. The resolver now sums each plane's control slots into
one disjoint layout instead of taking their maximum, computes a declared bounded
route-worker partition over whole routes, and rejects at build time any partition
a worker could not park on in a single `SYS_WAIT` set. The partition and its
per-worker wait-source count are emitted as a typed `ProfileWorker` list and
generated into the userspace profile.

The milestone's remaining deliverables — a collision-free fabric-only bootstrap
layout, three worker tasks, and one generation launching every role at once —
are **not** implemented here. Measurement during implementation showed they
require replacing the kernel boot path rather than extending it, which is a
larger change than this slice should carry; see *Open risks and follow-ups*.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract | Added `ProfileWorker` (name, routes, `waitSources`) to the resolved fabric-profile schema and its derive/export lists. | How the graph is partitioned across kernel wait sets is a schema-owned generation fact, not a runtime heuristic. |
| Slot layout | Replaced the plane-control `max()` with a `sum()` over stream, call, operation, and replacement controls. | Two planes coexisting in one boot are numbered into disjoint ranges instead of colliding on one aliased range. |
| Route workers | Added a declared `FABRIC_ROUTE_WORKERS` partition, validated for undeclared routes, incomplete coverage, and overlapping claims. | Every declared route is carried by exactly one worker; a silent gap or double-claim fails the build. |
| Wait bounds | Added `FABRIC_WORKER_WAIT_SHAPES`, distinguishing the graph-derived stream shape from the fixed-slot-array request/response shapes, and checked each worker's peak against the kernel bound. | A partition a broker could not register in one `SYS_WAIT` set is rejected at build time rather than polling or hanging at boot. |
| Participants | Split the unauthorized probe, declared interposition proxy, and filtered-introspection client into `fabric-probe`, `fabric-proxy`, and `fabric-observer`, declared in the fixture with their own control grants. | The three roles become distinct component identities with non-overlapping grants instead of one binary switching on an env flag. |
| Drift guard | Emitted a `const fn fabric_worker_wait_sources` accessor and bound each broker's own park-set arithmetic to it: the call broker's `SYS_WAIT` array is now sized from the declared peak instead of a literal `7`. | The resolved partition and the arrays that must hold it cannot disagree silently; a broker outgrowing its declared peak fails to compile. |
| Negative corpus | Added two mutators (crowding the graph-derived worker, raising a fixed shape's peak), a positive control asserting each is rejected *by the wait bound* rather than an unrelated guard, and resolution of every declared profile. | A wait-bound case cannot pass vacuously, and a profile no generation selects yet still has to resolve. |
| Graph-shape assertions | Updated the live-path participant count (14 → 15) and the telemetry subscriber count (2 → 3) to admit the declared introspection client. | The kernel's structural assertions keep naming the exact graph the generation admitted, rather than being loosened to tolerate a range. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A plane's control slots are sized from the largest plane and collide when planes coexist | `just data_fabric_profile_check` | `capability layout above declaration` mutator is accepted, or `requiredCapabilitySlots` regresses below the summed layout. |
| A declared partition leaves a route uncarried or claimed twice | `just data_fabric_profile_check` | Build fails with `route workers do not partition the declared routes` or `a route is claimed by more than one worker`. |
| A worker is admitted that cannot park on its whole live set | `just data_fabric_profile_check` | `route worker above wait bound` or `fixed-shape worker above wait bound` mutator is accepted. |
| A generated worker table drifts from the resolved profile | `just data_fabric_profile_check` | `checked-in default userspace profile is stale`. |
| A declared graph edge is added or removed without the live path noticing | `just test` | `fabric_manifest::booted_generation_declares_an_admitted_fabric_graph` or `fabric_stream::telemetry_route_declares_two_publishers_and_three_subscribers` panics on the exact count. |
| The declared wait shapes drift from the brokers they mirror | `const _: () = assert!(..)` in all three brokers against `fabric_worker_wait_sources(..)` | Compile error, e.g. `assertion failed: WAIT_SOURCES == CLIENTS * 2 + 3`. Verified by editing the generated peak 7→6 and observing the build fail. |
| A wait-bound mutator passes for the wrong reason | `just data_fabric_profile_check` | `<mutator> is rejected by <other check>, not by the wait bound it names`. |
| A declared profile no generation selects yet stops resolving | `just data_fabric_profile_check` | Resolution of that profile fails, or one of its workers exceeds the kernel bound. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_profile_check` | Passed: the two normalized-schema decoder tests (48 of the lib's 50 filtered out), both generation builds byte-identical, the whole negative corpus rejected — each wait-bound mutator additionally proven to be rejected *by the wait bound* — every declared profile resolved including the unselected `unified`, and the checked-in userspace profile matched the resolved value. | Direct |
| `just test` | Passed: 189 tests, zero failures, after updating the two hardcoded graph-shape assertions the new declared participant changed. The first run failed at `fabric_manifest.rs:69` with a participant count of 15 against a hardcoded 14, which is the guard working: adding a declared edge is exactly the change those assertions exist to catch. | Direct |
| `just contracts_check` | Passed: all four model-check scenarios, 50 contract tests, and every named contract including `fabric-graph` and `fabric-visibility`. | Direct |
| `just fabric_stream_check` | Passed; the declared introspection client is absent from the stream control-grant table, so the live fabric provisions no third route edge and the one-loan-per-matched-subscriber count is unchanged. | Direct |
| `just fabric_authority_check` | Passed. | Direct |
| `just fabric_manifest_check` | Passed: a deterministic 2548-byte resource with 4 schemas, 5 routes, 15 participants, and 1 interposition hop, plus authority tuples, distinct identity domains, bounds, and the negative corpus. The independent participant count corroborates the updated live-path assertion. | Direct |
| `just fmt_check`, `just fmt_check_components` | Passed. | Direct |
| `just lint`, `just lint_components` | Passed with warnings denied. | Direct |
| `cargo build --release -p slime-components` for `fabric-probe`, `fabric-proxy`, `fabric-observer` | Passed; each declared component needs a compiled image, so the fixture entries are not satisfiable without them. | Direct |
| Computed worker peaks vs. broker source | stream 8/9, call 7/9, operation 9/9. The call figure independently matches the hand-declared `[WaitSource; 7]` array in `call_broker::run`. | Direct |
| Dependent limits after `subscribers` 3→4 | Summed subscriber history is 18 against a frame capacity of 32; declared subscribers 4 against 14 loans and 14 mappings; `requiredCapabilitySlots` 37 against the declared ceiling of 48. Every dependent bound retains headroom, and the summing change raises the slot figure without breaching it. | Direct |
| Exhaustive branch enumeration of both request/response park sets | Every combination of client presence, receive/send readiness, server presence, backup-route fallback, clock state, and replacement-control state was enumerated. Maximum reachable set is exactly 7 for the call shape and exactly 9 for the operation shape; no branch exceeds either the declared peak or `MAX_WAIT_SOURCES`. Both sit at their bound with zero headroom, which is why the shapes are declared rather than inferred. | Direct |

## Decisions

- Decision: derive the stream worker's wait count from the graph, but take the request/response workers' peaks from a declared shape.
- Rationale: `park_on_streams` walks live participant tables, so its set scales with the graph, while `call_broker` and `operation_broker` park across fixed slot arrays (`CLIENTS = 2`, `supervision: [u32; CLIENTS + 1]`). A replaced client reuses its slot, and the operation shape's backup-route source is registered only while the server source is absent. Deriving those from edge counts double-counts both and rejects partitions the broker parks on comfortably.
- Rejected alternative: one uniform per-edge overhead model. Two variants were implemented and discarded after they predicted 15/9 and 13/9 for a worker whose true peak is 9/9 — the model was re-deriving broker internals and drifting from them.
- Decision: keep C8.10 open and land only the declarative half.
- Rationale: `launch_init`'s capability vector measures 61 of `MAX_CAPS = 64` — counted programmatically from the source — so the three split roles cannot be added as executable/control/service triples at all; that needs 9 slots against 3 free. A fabric-only layout is projected at 53/64 and looks viable, but reaching it means replacing the boot path for six generations, renumbering every `init.rs` slot constant, and rewriting six passing QEMU gates.
- Rejected alternative: claim the milestone on the declarative half alone; the required checks name concurrent launch and healthy blocked idle, which no gate here exercises.

## Open risks and follow-ups

- [ ] The bootstrap replacement remains open: one fabric-only capability layout reached by an early return in `launch_init`, renumbered `init.rs` constants, three distinct worker binaries wrapping the stream/call/operation brokers, deletion of the superseded `fabric-intruder`, and `just data_fabric_boot_check`. The 53-of-64 target is a projection from the participant and worker counts, not an observed layout — it must be re-counted against real code before being relied on.
- [ ] `fabric-probe`, `fabric-proxy`, and `fabric-observer` are declared and compiled but not yet launched by any gate; their logic still also exists in `fabric-intruder` and `fabric-subscriber`'s visibility arms. That duplication resolves when the boot path lands, and until then the originals remain the live path.
- [ ] The `unified` fabric profile is declared in the fixture but selected by no generation. It is now resolved and wait-bound-checked by the gate, so it cannot rot silently, but nothing boots it.
- [ ] `fabric-service.spawnBudget` stays 0. The boot-path slice must raise it to the number of workers it spawns, in the same change that introduces them, rather than granting spawn authority ahead of a consumer.
- [ ] `FABRIC_WORKER_WAIT_SHAPES` still mirrors broker constants by hand. Compile-time asserts now catch a broker and its declared peak disagreeing, but nothing checks the Python arithmetic against the broker's actual `wait` call sites — a broker could add a source and update both the assert and the shape while still being wrong about its true worst case.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: observed through `just data_fabric_profile_check`; no frozen sibling capture retained.
- Related roadmap item: [C8.10](../../../roadmap/02-core-runtime.md#c810--collision-free-full-graph-bootstrap-and-bounded-route-workers).
