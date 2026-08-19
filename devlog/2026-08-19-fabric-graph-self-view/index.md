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

**2026-08-19 — the blocked site closes, and needs no contract change.** The
correction above records `operation_broker` as needing "a graph-model change:
either the worker declared as a holder of the routes it brokers, or a read
scoped to components on routes I broker". Both were more than the problem
required.

The worker already holds a declared capability edge to every component whose row
it needs. `fabric-op-worker` on `sel4-boot` binds `fabric-op-client-control`,
`fabric-op-client-b-control`, `fabric-op-server-control` and the rest — the root
placed those endpoints from the manifest before the worker ran. Verified against
both manifests before writing any code: the peers reachable from the host's own
bindings cover the components `declared_history_depth` asks about, on
`sel4-operation` (host `fabric-service`) and `sel4-boot` (host
`fabric-op-worker`) alike.

So `GRAPH_READ` gained a second scope rather than a new model: a caller reads
the rows of components it *shares a declared grant edge with*. That grants
nothing new — the caller already holds an endpoint to that component — and it
still refuses enumeration, so a component with no edge to a participant reads
nothing of it and C8.8's filtering stays the fabric's. The edge is read from the
caller's own binding list, the same per-instance scoping every other axis uses.

The worker-as-holder alternative was rejected on inspection: `fabricComponent`
is a single field naming one component, so making the worker a holder would
either need a second field or would move the holder away from `fabric-service`,
which brokers the stream plane in the same generation.

**`FABRIC_HISTORY_DEPTHS` is now deleted** — the table, its `depth_rows`
generator in `build-generation.py`, and its entry in the checked-in
`default_fabric_profile.rs`. `fabric_profile` drops 49 → 48 constants. Both
compilation paths were booted: `just sel4_operation_check` for the holder path
and `just sel4_boot_check` for the worker-as-peer path.

**2026-08-19 — the ABI text was stale, and the edge scope was untested for what
it refuses.** Two gaps in the commit above, both about evidence rather than
behaviour.

`contracts/syscall-abi/v1/schema.zt` and `docs/syscall-abi.md` still said "every
other instance reads its *own* rows **and nothing else**". That became false the
moment the edge scope landed. The contract is the authority on what this
operation may disclose, and `generate-syscall-abi-bindings.py --check` only
verifies that a declared label is documented, not that the prose matches the
code — so nothing would have caught it. Both now describe the two scopes and why
the second discloses nothing new.

More importantly, the edge scope was verified only where it *answers*. Every
gate run showed a caller getting rows it was entitled to; none showed a caller
being refused rows it was not. A scope that never refuses is enumeration wearing
a filter, and it would have passed every one of those gates.

`fabric-publisher` now checks it directly: its only declared edge is to
`fabric-service`, so `fabric-subscriber` — a sibling on the same route, sharing
no grant with it — must not appear in its read. Verified non-vacuous by pointing
the check at the publisher's own identity, which fails the plane with `graph read
disclosed a component this one shares no edge with`. That makes C8.8's
"an ungranted caller inferred nothing" rest on an observation rather than on the
absence of a counterexample.

**2026-08-19 — `fabric-service`'s provisioning walk: attempted, reverted,
partially understood.** Migrating the two remaining `FABRIC_PARTICIPANTS`
consumers in `fabric-service` was attempted and backed out. Recording it because
the half that worked and the half that did not are both useful to the next
attempt.

`declared_edges` migrated cleanly and was proven non-vacuous: forcing it to
return zero denies every participant with `ungranted component denied`, so the
migrated function does drive authority rather than passing silently. That part
is sound and could land on its own.

The provisioning walk did not. With it migrated, three of six edges provision —
`fabric-publisher-b` and `fabric-subscriber-b` succeed, `fabric-publisher` and
`fabric-subscriber` never do — and the plane fails at `reject: descriptor
validation`. The root's own markers show the holder receiving all six rows
(`rows=6`), so the read is not the problem; the loss is on the component side.

Established along the way, and worth keeping:

- A resource row's `route_index` indexes `ordered`, the route list sorted by
  route identity (`build-generation.py:2140`), while a broker's `ROUTE_NAMES` is
  its own hand-written dispatch order. The two genuinely differ and the
  translation through `graph_route_index` is the right instrument.
- Direction encodings agree: `FABRIC_DIRECTION` and the generated
  `DIRECTION_PUBLISH`/`SUBSCRIBE` are both 1 and 2.
- Reading the graph inside the client loop puts a syscall in a hot path; the
  read belongs at the top of `provision`, and `declared_edges` should take the
  rows rather than re-read them.

What was *not* established is why two specific components fail to match. Several
hypotheses were tried without confirming any — the honest state is that the
cause is unknown, not that a fix is close. The attempt is reverted rather than
left half-applied, so the tree is at the last green commit.

**2026-08-19 — second attempt: the main bug is found, a second one is not.**
Reverted again, but two things are now established rather than guessed, and the
first was the actual cause of the earlier failure.

**Measuring first was what found it.** A temporary probe printing every row the
component receives, booted directly under QEMU rather than through the gate
(which suppresses serial on success), showed the data is clean: all six rows
arrive, route indices map identity on this plane (`local 0 -> graph 0`,
`local 1 -> graph 1`), and every direction matches the compiled table. So the
read, the translation, and the encoding were all correct all along — every
hypothesis about them was wrong.

**The bug was where the read is called.** With the read hoisted above the client
loop, `fabric-publisher` and `fabric-subscriber` provision correctly and **all
six edges provision**, where before only three did. `graph_read` stages its reply
through the component's single transfer window, which `provision_edge` also uses
to hand out role descriptors; calling it inside the loop had the two contending.
That was not visible from any hypothesis about identities or orderings — only
from putting the read somewhere else and watching six edges appear.

**A second failure remains and is not understood.** After all six provision, the
plane still fails at `[fabric] reject: descriptor validation`, in the loan-admit
path (`fabric-service.rs:1518`), which checks a sample descriptor against
`type_tags[route]`. The plausible reading is that a row's local route index is
reaching a path expecting a different domain, but that was not confirmed, and
guessing at it is what cost the previous attempt.

Worth noting for the next attempt: `-b` components now provision **diagnostics
before telemetry**, where the generated table gave telemetry first, because the
resource sorts rows by grant identity. Nothing has been shown to depend on that
order — the subscriber matches its rings by route identity — but it is the one
observable difference between the two row sources, and it is where I would look
first.

`declared_edges` remains cleanly migratable and independently proven
non-vacuous; it does not depend on any of the above and could land alone.

**2026-08-19 — `declared_edges` lands.** The half of the `fabric-service`
migration that was independently sound is committed: `declared_edges` reads the
graph instead of `FABRIC_PARTICIPANTS`, which drops 18 → 17 live uses.

It ships with the placement fix the second attempt found, because it needs it:
the read is hoisted above the client loop, since `graph_read` stages its reply
through the component's single transfer window and `provision_edge` uses the
same window to hand out role descriptors. A read inside the loop leaves three of
six edges unprovisioned.

Re-proven non-vacuous in this exact arrangement rather than inherited from the
earlier run: forcing the count to zero denies every participant with
`ungranted component denied`, so the migrated function does gate authority.

**The provisioning walk stays on the generated table.** It is the site whose
descriptor-validation failure is still unexplained, and landing it would mean
shipping a change whose failure mode I cannot account for. Splitting the two was
possible because they share only the hoisted read, which is now in place — so
the walk's next attempt starts from a green tree with the placement bug already
fixed.

**2026-08-19 — the descriptor rejection is a route/type mismatch, and the two
route orderings are *not* the cause.** Instrumenting the four conditions at
`fabric-service.rs:1518` and booting directly under QEMU named the failing arm
and its values:

```
[fabric] reject: descriptor validation valid=0 kind=1 len=8192
         want_type=2845549244 got_type=148575099
```

`kind` and `len` are fine; only the type check fails. Resolving both tags against
`components/proto/src/interface_schema.rs`: the fabric expected
`diagnostics_stream` (`0xc5508e6f_a99ba2bc`) and the sample carried
`telemetry_stream` (`0x11641539_08db137b`). So a telemetry sample is being
admitted against the diagnostics route — the two are transposed somewhere on the
migrated path.

**The obvious suspect is eliminated.** The resource's route order was computed
directly from the manifest: route identities sort to `route_index 0 = telemetry`,
`1 = diagnostics`, which is exactly `ROUTE_NAMES`' order, and the boot probe
already showed the mapping is identity on this plane (`local 0 -> graph 0`,
`local 1 -> graph 1`). The `routes` and `type_tags` arrays agree with that order
too. So the earlier guess that the identity-sorted resource disagrees with
`ROUTE_NAMES` is **wrong on this plane**, and the transposition is elsewhere.

That leaves the direction/route pairing inside the migrated walk as the place to
look: a participant on two routes gets two rows, and if a row's route reaches
`provision_edge` while its direction selects the other row's handling, the
publisher and subscriber halves of one component would be crossed exactly this
way. Not confirmed — stated as the next thing to measure, not as the answer.

The mechanism is still open. What has changed is that the failure is now a named
transposition between two specific interfaces rather than an unexplained
validation error, and one hypothesis is closed off with evidence.

**2026-08-19 — the pairing is correct; the order is what differs.** Probing the
migrated walk at its `provision_edge` call site prints exactly what it pairs:

```
[pair] fabric-publisher    graph_route=0 local=0 dir=1
[pair] fabric-subscriber   graph_route=0 local=0 dir=2
[pair] fabric-publisher-b  graph_route=1 local=1 dir=1
[pair] fabric-publisher-b  graph_route=0 local=0 dir=1
[pair] fabric-subscriber-b graph_route=1 local=1 dir=2
[pair] fabric-subscriber-b graph_route=0 local=0 dir=2
```

**Every route/direction pair is right**, and every graph index translates to the
right local index. So the route/direction hypothesis this entry proposed last is
also wrong — nothing is transposed in the pairing.

What differs is the *sequence*: the two-route components are provisioned
`diagnostics` first and `telemetry` second, where the generated table gave
telemetry first. That is the resource's grant-identity sort showing through, and
it is the only observable difference left between the two row sources.

Two candidate consequences were checked and **eliminated**:

- The receivers do not assign by arrival. `fabric-publisher-b` and
  `fabric-subscriber-b` both match each arriving role on `descriptor.route_identity`
  and would fail loudly (`role names no declared route`) otherwise.
- `Publisher`/`Subscriber` each carry their own `route`, and `pump_publisher`
  reads `publisher.route`, so a table position does not decide a type tag.

What remains unexamined is `provision_edge`'s `ordinal` — `ring_base = RING_BASE +
ordinal * PAGE`, where `ordinal` counts already-provisioned entries. Reordering
provisioning therefore assigns different ring *addresses* to the same routes on
the fabric's side. Whether that is the mechanism is not established; it is simply
the one order-dependent quantity found so far, and the next thing to measure.

Exactly one rejection occurs, on the large-sample loan path, after all six edges
provision successfully.

**2026-08-19 — `ordinal` measured on both sides: no collision, but route and
ring address are transposed.** Probing `provision_edge` under both row sources
gives the exact difference:

```
baseline : pub-b route0@ordinal2  route1@ordinal3 | sub-b route0@4 route1@5
migrated : pub-b route1@ordinal2  route0@ordinal3 | sub-b route1@4 route0@5
```

Ordinals are dense and distinct in both, so `ring_base = RING_BASE + ordinal *
PAGE` allocates six non-overlapping pages either way — the collision this probe
was looking for does not happen. What changes is *which route lives at which
address*: the same `ring_base` holds telemetry in one ordering and diagnostics
in the other.

That is internally consistent on the fabric's side — `Ring::format` writes the
type tag and slot count from `route_index`, and `Publisher.ring_base` travels
with `Publisher.route` — so no single-sided reasoning explains a failure. Three
further order-dependent candidates were checked and eliminated:

- The receivers map by matched slot, not arrival: `fabric-publisher-b` pairs
  each arriving role on `descriptor.route_identity` before mapping, and its two
  ring bases are its own constants.
- `ring_slots` is derived per route (telemetry 4, diagnostics 2) and
  `Ring::attach` compares the header's count against the caller's expectation,
  so a mis-paired depth fails loudly rather than silently.
- The copy path uses a fixed `COPY_BASE`, not an ordinal.

So the measurement closes the `ordinal` hypothesis as stated — the addresses are
sound — while confirming the route/address transposition is real. The mechanism
connecting that transposition to a `telemetry` sample arriving where
`diagnostics` was expected is still not established, and the honest position is
that four hypotheses have now been eliminated with evidence and none confirmed.

The remaining untested surface is narrow: the fabric maps each ring at
`ring_base` *before* the participant maps its own copy, and both sides then
attach independently. Whether anything outside this file caches a base→route
association across that boundary is the next thing to look at, and it is the
first hypothesis so far that reaches beyond `fabric-service.rs`.
