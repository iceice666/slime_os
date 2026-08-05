# P5.5.1 — Narrow-on-transfer provisioning on seL4

| Field | Value |
|---|---|
| Date | 2026-08-05 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,transfer_window,parked}.rs`, `components/bins/src/bin/{init,fabric-subscriber}.rs`, `components/bins/src/{call_broker,operation_broker,default_fabric_profile}.rs`, `contracts/generation/v1/fixtures/sel4-fabric.zti`, `scripts/build/{build-generation,build-sel4}.py`, `scripts/check/check-sel4-{fabric,spawn,channel}-plane.py`, `Justfile` |
| Roadmap | P5.5.1, P5.5, B15, B17, B16, B12 |
| Gates | `just sel4_stream_check`, `just sel4_spawn_check`, `just sel4_channel_check` |
| Trigger | P5.5 opened as the next uncompleted milestone after P5.3 closed |
| Baseline | P5.3.4 (`44d273d`): six seL4 gates green, `Operation::CapTransfer` answering `unimplemented` |

## Summary

`Operation::CapTransfer` — C8.3's narrow-on-transfer move — was root-mediated
but had no handler, so no component could hand a capability to a task that
already existed. This slice implements it and observes P5.5's exit condition:
one declared `telemetry` route carries a sample from `fabric-publisher` to
`fabric-subscriber` over seL4, with both route endpoints provisioned by
`fabric-service` from the generation's declared edges, a re-delegation refused,
and `fabric-intruder` denied despite holding a real control endpoint.

Three of the four participants run unmodified. The slice also closed **B15** and
found two defects latent since P5.3.1, neither observable from any graph that
existed before it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `main.rs::serve_cap_transfer` | New handler: four rules restated from `sys_cap_transfer` — transfer authority at the source, narrow-only, transfer not inherited, descriptor names the real kind | A broker can provision a role to a running task without the root knowing what a route is |
| `main.rs::serve_cap_transfer` | A moved endpoint's channel *holder* moves with its capability | The receiver resolves a live queue rather than a capability naming nothing |
| `main.rs` `Recv` arm | `recv` answers `ERR_WOULDBLOCK` instead of parking; `wait` remains the only parking operation | Matches `kernel/src/ipc/mod.rs`; a component sweeping N sources no longer freezes at the first empty one |
| `main.rs::resolve_channel` | Every failure answers `BadCapability` (-1), not `InvalidOperation` (-4) | Matches `sys_send`/`sys_recv`; components compare against the literal |
| `transfer_window.rs` | `MAX_STAGED_ARRAY_BYTES` (1024) and `read_staged_array`, beside the message bound | **B15**: a spawn carries 64 grants, the oracle's number |
| `parked.rs` | `ParkReason::Receive` removed; `deliver_wake` no longer re-implements `serve_recv` | One dequeue/land/deliver path instead of two that had to stay in step |
| `build-generation.py` | `build_sel4_generation` resolves a declared `fabricGraph` through `resolve_fabric_profile` and carries the C8.2 resource | A seL4 route identity is folded from the same schemas and validation as an x86 one |
| `default_fabric_profile.rs`, both brokers | `WORKER_ABSENT` instead of a `const` panic when a graph declares no route for a worker | A stream-only graph compiles; every graph that declares the plane is checked exactly as before |

### Applied from the review pass

A fresh read-only pass over the finished diff. Every finding was verified against
the code before acting.

| # | Priority | Finding | Resolution |
|---|---|---|---|
| F2 | P2 | `restore_transferred` discarded three failures with `let _`. If `install` refused, the capability had already left transit and was in *neither* table — a loss the terminal `transit=0` marker cannot show, because a capability that left transit is exactly what that count stops seeing | Both failure paths now emit `SLIME_GRAPH FAIL capability lost`, which the gate's `FAILURE_MARKERS` already greps for. Matches `land_caps`'s existing shape |
| F3 | P2 | `valid_transfer_descriptor` dropped the oracle's `is_object_kind` check. `descriptor_names` refused an undefined kind anyway, so behaviour agreed — but the *error code* diverged: `ERR_BAD_CAP` where `sys_cap_transfer` answers `ERR_INVALID_ARG` | `is_object_kind` restated, so a malformed descriptor and a capability failure stay distinguishable exactly as the oracle has them |
| F9 | P2 | `check_no_participant_failed` allowed *at most* one failure per component. If an unconfigured instance ever stopped failing, a real participant's failure would land in its budget and pass | Tightened to `!= 1`: the unconfigured failure must be present, so its disappearance fails the gate rather than silently widening it |
| F1 | P2 | The rollback re-derived the channel key from `original.resource` while the forward path used `moved.resource`. Equal by construction today, but the asymmetry was undocumented and load-bearing | The key is passed in as `Option<(ChannelKey, TaskId)>`, recorded where the reassign happens |
| F8 | P3 | Two comments in the spawn gate still said `ERR_INVALID_ARG` after this change made `ERR_BAD_CAP` correct — misleading on the exact ABI point the milestone is about | Corrected |

The reviewer separately confirmed no defect in the `MAX_STAGED_ARRAY_BYTES`
stack frame and bounds checks, the non-blocking `recv` callsite audit (every
`recv` in `components/` already had an `ERR_WOULDBLOCK` arm), the
`resolve_channel` error-code change against both oracle syscalls, the
`WORKER_ABSENT` change against the x86 profile, and that the gate is not
vacuous.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A provisioned role becomes re-delegable | `just sel4_fabric_check` | `[fabric-publisher] re-delegation denied` missing |
| A role carries both directions | `just sel4_fabric_check` | `rights=0x1`/`rights=0x2` markers, and the publisher's/subscriber's own denial arms |
| An undeclared component obtains an edge | `just sel4_fabric_check` | `[fabric] ungranted component denied: fabric-intruder` missing |
| A moved endpoint resolves to no queue | `just sel4_fabric_check` | The graph deadlocks; no sample is delivered |
| A spawn silently drops grants past four | `just sel4_spawn_check` | `grants=6 channels=6` and `[init] six grants delivered` |
| `recv` parks again | `just sel4_fabric_check` | The fabric parks holding samples its subscriber waits for |
| A component grows an undocumented seL4 branch | `just sel4_fabric_check` | `check_components_are_minimally_branched` asserts exact counts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_fabric_check` | Pass — 33 markers across 7 causal chains | Direct |
| `just sel4_spawn_check` | Pass — six-grant spawn observed | Direct |
| `just sel4_channel_check` | Pass — park markers updated to `reason=wait` | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_loan_check`, `sel4_sample_check` | Pass, unchanged | Direct |
| `just fabric_stream_check`, `just data_fabric_boot_check` | Pass — the x86 fabric gates the `WORKER_ABSENT` change touches | Direct |
| `just contracts_check`, `generation_check` | Pass | Direct |
| `just fmt_check_all`, `lint_all` | Pass | Direct |
| `just test`, `test_host` | Pass — the x86 oracle is untouched | Direct |
| `just ruff`, `typos`, `machete` | Pass | Direct |
| Dispatcher headroom, instrumented | 136 of `MAX_GRAPH_ITERATIONS = 512` on the densest declared graph | Direct |

### Fault injection

A passing gate does not by itself prove a denial arm fires, so each was removed
and the gate re-run.

| Injection | Result |
|---|---|
| `RIGHT_TRANSFER` retained at the destination unconditionally | **Gate fails** — re-delegation succeeds, `fabric-publisher` exits non-zero |
| Rule 1 (transfer authority at the source) removed | **Gate fails** |
| The endpoint holder reassign skipped | **Gate fails** — the receiver holds a capability naming no queue |
| The narrow-only **subset test** removed | **Gate still passes** — see below |
| The wide staged reader reverted to the message bound | `just sel4_spawn_check` fails — the six-grant spawn is refused outright |

## Decisions

- **Decision:** Decompose P5.5 into P5.5.1 and P5.5.2 rather than landing the
  full stream plane.
- **Rationale:** P5.5's exit condition is C8.3-shaped — where route authority
  comes from — and needs one route, one publisher, one subscriber. The full
  C8.4 plane additionally needs a second publisher for the `>MAX_MSG` path, a
  stalled subscriber for eviction, and a second route for the fan-in: twice the
  graph, none of it required by the exit condition.
- **Rejected alternative:** Landing both, which would make the reviewable claim
  depend on the unreviewable one — the same reason P5.3 and C8.9 were split.

- **Decision:** Fix B15 in this slice, with its observation arm in
  `sel4-spawn.zti`'s scenario rather than this one.
- **Rationale:** B15's exit condition names "at least six declared grants
  observed under a named seL4 gate". The fabric graph's largest list is five, so
  closing B15 on it would claim an unobserved exit condition — the failure mode
  P5.3.4's review caught. A spawn-ABI bound belongs in the spawn gate.
- **Rejected alternative:** Deferring B15 a third time. Both prior deferrals
  rested on "the largest grant list is three, 48 bytes against 64"; the fabric's
  nine-grant x86 spawn makes that false.

- **Decision:** Make `recv` non-blocking rather than keeping P5.3.1's park.
- **Rationale:** P5.3.1's reasoning — a component blocked in a call is blocked
  either way, and answering would make it spin — holds for a one-source
  component and is wrong for any component that sweeps several sources before
  parking. The oracle splits poll from park, and the split is the component's to
  make. It reintroduces no spin: the component's next call is `wait`, which
  parks.
- **Rejected alternative:** A `recv` that parks only when the caller has one
  source, which would make the ABI depend on a fact the caller never stated.
- **Cost, measured rather than argued:** a blocking component now costs two
  dispatcher iterations where it cost one. The typed fabric — nine tasks, four
  provisioned roles, seven samples — used 136 of 512, so the change spent
  headroom rather than exhausting it. Instrumented once and removed; the number
  is recorded on `MAX_GRAPH_ITERATIONS`.

- **Decision:** Record B17 rather than construct a probe for the subset test.
- **Rationale:** Two probes were written and withdrawn, each caught by a
  different *earlier* rule — a factory by rule 1, an endpoint by the per-kind
  rule — so each looked like coverage it did not provide. The subject the rule
  needs is a capability holding transfer authority while narrower than its kind
  admits, and no graph this cutover can declare produces one.
- **Rejected alternative:** Keeping the withdrawn arm. It would have read as
  coverage in the transcript and in the roadmap.

## Open risks and follow-ups

- [ ] **B17**: the transfer's subset test has no coverage. P5.5.2's two-broker
      composition is the shape that produces the subject; the exit condition is
      recorded in the backlog.
- [ ] **P5.5.2**: the full stream plane with every component unmodified,
      including `fabric-subscriber`'s one branch.
- [ ] **B16** re-deferred: nine tasks against `MAX_RECORDS = 32`.
- [ ] **B12** re-deferred: the seventh seL4 generation uses the same build path,
      whose rustflags are keyed by triple.

## Artifacts and provenance

- Generation fixture: `sel4-fabric.zti` and its rationale `sel4-fabric.md`,
  both superseded by [`sel4-stream.zti`](../../contracts/generation/v1/fixtures/sel4-stream.zti)
  and [`sel4-stream.md`](../../contracts/generation/v1/fixtures/sel4-stream.md)
- Gate: `check-sel4-fabric-plane.py`, superseded by
  [`check-sel4-stream-plane.py`](../../scripts/check/check-sel4-stream-plane.py).
  The retired gate's module doc recorded the B17 coverage gap and both withdrawn
  probes; see the Corrections section for what that analysis got wrong
- Related roadmap item: [P5.5.1](../../roadmap/07-architecture-portability.md)

## Corrections

**2026-08-05 — P5.5.2 retired this entry's artifacts, and disproved one of its
conclusions.** Appended rather than edited into the body above, which stays as
it was written.

- The three paths this entry's **Artifacts and provenance** names no longer
  exist. `sel4-fabric.zti` and `sel4-fabric.md` became
  `contracts/generation/v1/fixtures/sel4-stream.{zti,md}`;
  `check-sel4-fabric-plane.py` became
  `scripts/check/check-sel4-stream-plane.py`; and the `Gates` field's
  `sel4_fabric_check` is now `just sel4_stream_check`. P5.5.2's graph is a
  strict superset of this one's, so every assertion recorded above is still
  observed — by that gate rather than this one.
- **The B17 analysis in this entry is wrong**, and so is the version of it in
  the retired gate's module doc. Both argued the subset test was unreachable
  from any graph this cutover could declare, on the grounds that only a
  `cap_transfer` retaining its transfer bit produces a capability holding
  transfer authority while narrower than its kind admits. A plain **spawn
  grant** produces one: `preflight_spawn_grants` installs the requested mask
  verbatim, and `init.rs` already granted `DANGO_OUTPUT_SLOT` at
  `RIGHT_SEND | RIGHT_TRANSFER` on x86 at the time this was written.

  The reasoning enumerated what `cap_transfer` itself could emit and treated
  that as the set of capabilities that could exist. It was checking one
  producer rather than every path that installs a rights mask.

  B17 is closed in P5.5.2 with a one-line fixture grant and one denial arm — not
  the two-broker composition this entry proposed. See
  [`2026-08-05-p5-5-2-stream-plane`](../2026-08-05-p5-5-2-stream-plane/index.md).
- The **front matter's `Gates` field and the artifact links** were repointed to
  the superseding gate and fixture, because `just devlog_check` requires every
  one to resolve and the originals were deleted. That is the only edit made
  below the correction line, and it changes no claim: the prose naming the old
  artifacts is left exactly as written.
