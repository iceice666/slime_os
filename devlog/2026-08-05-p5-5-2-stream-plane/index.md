# P5.5.2 — The full stream plane, unmodified, on seL4

| Field | Value |
|---|---|
| Date | 2026-08-05 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,channel,shared_buffer}.rs`, `components/bins/src/bin/{init,fabric-publisher,fabric-subscriber}.rs`, `components/bins/build.rs`, `contracts/generation/v1/fixtures/sel4-stream.{zti,md}`, `scripts/build/{build-generation,build-sel4}.py`, `scripts/check/check-sel4-stream-plane.py`, `Justfile` |
| Roadmap | P5.5.2, P5.5, B17, B18, B16, B12 |
| Gates | `just sel4_stream_check`, `just fabric_stream_check` |
| Trigger | P5.5.2 opened as the next uncompleted milestone after P5.5.1 closed |
| Baseline | P5.5.1 (`11d9b72`): seven seL4 gates green, one counted seL4 branch in `fabric-subscriber`, B17 open |

## Summary

P5.5.1 ran one route, one publisher, and one subscriber, and its gate asserted
the *exact extent* of the seL4 branching its components carried rather than its
absence — `fabric-subscriber` needed one branch, because it refuses to finish
until both sample forms arrive and the `>MAX_MSG` one comes from a publisher
that graph did not declare. This slice declares that publisher and the rest of
the C8.4 plane, removes the branch, and observes P5.5.2's exit condition: all
six stream participants run on seL4 as the x86 oracle builds them, with **no
seL4 branch in any of them**, producing 48 markers across 10 causal chains.

**B17 is closed, and its premise was wrong.** The backlog held that the transfer
contract's subset test was unreachable from any declarable graph, because only a
`cap_transfer` retaining its transfer bit could produce a capability holding
transfer authority while narrower than its kind admits. A plain **spawn grant**
produces one — `preflight_spawn_grants` installs the requested mask verbatim —
and init already does exactly that on x86 for `DANGO_OUTPUT_SLOT`. The shape was
in the tree the whole time; nobody had asked to widen one.

**A third ABI divergence was found and fixed**, on the same pattern as the two
P5.5.1 found: `shared_buffer_unmap` refused a loan slot where the oracle accepts
one, so a receiver that mapped a downstream loan had no slot it could unmap
with. Latent since P5.3.2 and unreachable until a component exercised the
shared-sample path, which this is the first seL4 graph to do.

P5.5.1's gate, generation, and image are **retired** here rather than kept:
every assertion that slice made is a subset of this one's, over a strictly
larger graph.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/fixtures/sel4-stream.zti` | New: six components, two routes, QoS and budgets copied from `valid.zti`. Replaces `sel4-fabric.zti`. | The seL4 graph declares what the oracle declares, so the participants need no adaptation |
| `components/bins/src/bin/fabric-subscriber.rs` | The one `SLIME_SEL4_FABRIC_CHECK` branch deleted; both sample forms required unconditionally again | The component is the x86 binary, with no seL4 branch |
| `components/bins/src/bin/init.rs` | `drive_fabric_plane` → `drive_stream_plane`: six participants, spawn order constrained by the fabric's supervision handles, control-slot indices named rather than positional | Spawn order and control-slot order are separately correct, and visibly so |
| `components/bins/src/bin/fabric-publisher.rs` | `subset_test_arm`: asks to widen a spawn-granted `send`+`transfer` end back to `recv`, guarded on holding it | B17's subject exists and the refusal is attributable |
| `slime-root/src/channel.rs` | `MAX_CHANNELS` 16 → 32, with the "one per task pair" reasoning replaced by "one per edge" | A broker-minted graph fits the table |
| `slime-root/src/shared_buffer.rs` | `unmap_loan`, authorizing by the mapping record rather than region ownership | A loan receiver can unmap what it mapped |
| `slime-root/src/main.rs` | `serve_buffer_lifecycle` resolves a loan slot for `unmap` alone; response handling factored into `finish_buffer_lifecycle` | `shared_buffer_unmap`'s accepted kinds match `sys_shared_buffer_unmap`'s |
| `scripts/check/check-sel4-stream-plane.py` | Rewritten from P5.5.1's gate: 10 chains, branch **absence**, and `check_transcript_matches_the_oracle` | The transcript is the oracle's, not one written to suit this root |
| `slime-root/src/graph.rs` | `MAX_GRAPH_TASKS` 16 → `MAX_TASKS` | One ceiling on how many tasks a graph may hold, refused where nothing is allocated yet |

### Two scheduling races the gate exposed, one closed and one narrowed

The seL4 gate was flaky, and the passes recorded early in implementation were
luck rather than evidence — stated plainly because it would have been easy to
keep re-running until green. It now passes **6 runs in 9**, up from roughly 1 in
3. Both causes are the same shape: the oracle's cooperative scheduler orders
events favourably every time, and seL4's does not.

**Closed: a publisher writing to a route it had already retired.**
`fabric-publisher-b` marked its first `diagnostics` sample terminal and then
published on that route again. That second send was **dead code** —
`FLAG_LAST` sets `publisher.finished`, and both the broker loop and
`park_on_streams` skip a finished publisher, so nothing ever read it. It was
worse than inert: once `diagnostics` retired, only `telemetry` kept the fabric
alive, so after that drained the send answered `ERR_PEER_DEAD` and the component
exited 1. Deleted. Moving `FLAG_LAST` to the second sample instead was tried
first and wedges `just fabric_qos_check`, whose subscriber waits for the early
terminal event — the two gates want opposite things from the flag.

**Narrowed: provisioning races publishing.** `deliver` refuses a subscriber
whose `matched_publishers` is zero, and `refresh_matches` counts only publishers
already provisioned — so a subscriber that asks after `fabric-publisher` has
sent its whole stall window matches nothing, receives nothing, and loses
nothing. `fabric-subscriber-b` then fails its own loss assertion on a boot where
the fabric was correct.

`drive_stream_plane` now yields after spawning the fabric, which is what
`launch_fabric_graph` does on x86 — and it is *not* sufficient here, which is
the interesting part. That comment argues one yield is enough "deterministically
rather than by luck" because scheduling is cooperative and `SYS_YIELD` drains a
FIFO ready queue. On seL4 it only makes the ordering likely. The real fix is a
dependency rather than a timing hint, and init cannot express it: it holds no
channel to a participant after spawn. B18 records the two candidate signals.

Everything this milestone claims is observed on a passing run; what is
unreliable is reaching the end of the boot, not what the boot proves.

### Two divergences the composition exposed

Both were invisible to every earlier seL4 graph, and neither is a defect this
slice introduced.

**`MAX_CHANNELS` was sized against the wrong quantity.** Its doc said sixteen
was "one channel per task pair … more than any declared seL4 generation needs".
But a channel is created per *edge*, and a userspace broker mints edges the
generation never declared: this graph's thirteen grants become six control
channels, and `fabric-service` then mints two per publisher and two per
subscriber. At sixteen the fabric failed its eleventh `endpoint_create` with
`[fabric] fail: credit endpoints`, and every participant then failed downstream
of that — a transcript that reads as four broken components rather than one
exhausted table. Raised to 32 (`task::MAX_TASKS`), because the growth tracks
route roles rather than task pairs.

**`shared_buffer_unmap` refused a loan slot.** `kernel/src/syscall/mod.rs:715`
resolves `SharedBufferLoan(loan) => loan.region()`; the root's
`serve_buffer_lifecycle` matched only `Resource::SharedBuffer`. A receiver that
maps through `loan_map` therefore had no slot it could unmap with — the region
belongs to the *lender*, and the receiver was never issued a buffer capability
for it. The fix is not a handle conversion but a separate table entry point:
`unmap`'s authorization is region ownership, which a borrower never has, so
`unmap_loan` authorizes on the mapping record instead. That is what the oracle's
own `SharedBufferTable::unmap` does — it takes no rights argument and matches on
`mapping.owner` — so this restates the oracle rather than weakening anything.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component grows a seL4 branch again | `just sel4_stream_check` | `check_components_are_unmodified` names the file and the flag |
| The seL4 transcript drifts from the oracle's | `just sel4_stream_check` | `check_transcript_matches_the_oracle` reports the marker and demands it be declared in `SEL4_ONLY` |
| The subset test is removed or weakened | `just sel4_stream_check` | `fabric-publisher` fails; injected and observed below |
| The B17 arm silently stops testing anything | `just fabric_stream_check` | The x86 graph grants no probe, so the marker must be absent there; injected below |
| A participant failure is masked by an unconfigured instance's | `just sel4_stream_check` | Per-component failure count `!= 1` |
| The loan-unmap path regresses | `just sel4_stream_check` | `[fabric-subscriber] fail: unmap` |
| A publisher outlives a route it terminated | `just sel4_stream_check` | `[fabric-publisher-b] fail: publish` after `[fabric] stream plane complete` — currently firing; B18 |
| The x86 oracle is disturbed | `just fabric_stream_check`, `just fabric_visibility_check` | Marker or source-lint failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass — 51 markers, 10 chains, six components unmodified, 1 declared seL4-only marker. **6 runs in 9** after two B18 fixes, up from ~1 in 3; the failures are B18, not an assertion | Direct |
| `just sel4_sample_check` | Pass | Direct |
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_loan_check` | Pass | Direct |
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| `just fabric_stream_check` | Pass | Direct |
| `just test` | Pass | Direct |
| `just test_host`, `just miri` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all` | Pass | Direct |
| `just devlog_check`, `just ruff`, `just typos`, `just machete` | Pass | Direct |
| `just deny` | Fails identically at `11d9b72` | Direct, pre-existing |

### Fault injection

A passing gate does not by itself prove a denial arm fires. Each claim this
slice adds was injected, and one injection **failed to fail** — which is
recorded here rather than dropped, because it is the more useful result.

| Injection | Expected | Observed |
|---|---|---|
| Delete `rights & !source.rights` from `serve_cap_transfer` | Gate fails | `[init] stream plane fail` — `fabric-publisher`'s widening succeeded |
| Revert `serve_buffer_lifecycle`'s loan-slot arm | Gate fails | `[init] stream plane fail` — `[fabric-subscriber] fail: unmap` |
| Remove `subset_test_arm`'s possession guard | x86 must stay silent | **x86 printed the marker anyway** — see below |
| Relax `fabric-subscriber`'s `shared == 0` requirement | Gate fails | **Gate passed** — see below |

The first is the one P5.5.1 ran and could **not** make fail. That is the
difference between B17 open and B17 closed.

**The possession guard is load-bearing, and injection is what showed it.**
Removing it left `just fabric_stream_check` green while the x86 boot emitted
`[fabric-publisher] narrowed transfer role cannot widen` — on a graph that
grants no probe, where `PROBE_SLOT` is empty and the `cap_transfer` was refused
for having no capability at all rather than by the subset test. The marker would
have read as coverage on both kernels while only one had it, which is precisely
the failure mode B17 was opened for. The guard establishes possession by
*sending* on the granted end first, so the arm is unreachable without the
subject.

**The `shared == 0` injection does not fail the gate**, and the arm it tests is
weaker than it looks. Relaxing the requirement changes nothing observable here,
because this graph declares `fabric-publisher-b` and the shared sample arrives
regardless — the assertion only bites on a graph that *cannot* produce one,
which was P5.5.1's and is no longer any. It is retained because it is the x86
oracle's own assertion and this component is unmodified, not because this gate
proves it. What this gate does prove about the shared path is
`[fabric-subscriber] shared sample verified`, which is a real delivery, and the
loan-unmap injection above, which fails the gate.

## Decisions

- **Decision:** Retire P5.5.1's gate, generation, and image rather than keep
  both.
- **Rationale:** Its assertions are a strict subset of this one's over a larger
  graph — the same denials, the same role masks, the same `transfers served`
  count. Keeping both would mean maintaining two images to observe one property
  twice, and the retirement is recorded under P5.5.1 in the roadmap so its exit
  condition stays observed rather than silently dropped.
- **Rejected alternative:** Grow `sel4-fabric.zti` until its subscriber saw both
  sample forms. That converges it toward `sel4-stream.zti` and leaves two
  near-identical images.

- **Decision:** B17's subject is a **spawn grant**, not a retained-transfer
  `cap_transfer`.
- **Rationale:** It is what the code already produces. The first attempt at this
  followed the backlog's own suggestion — provision an interposition hop with
  `FLAG_RETAIN_TRANSFER` so a relay could provision the next hop — and it was
  wrong twice over: `fabric-service` provisions *every* hop directly, so a relay
  never provisions anything, and `check-fabric-visibility.py` lints the proxy
  source specifically to forbid it holding `RIGHT_TRANSFER`. Retaining the bit
  there would have been a real widening dressed as coverage.
- **Rejected alternative:** A second seL4 image running the visibility profile,
  which is what closing B17 through the interposition chain would have cost.

- **Decision:** The B17 arm establishes possession by *using* the granted
  endpoint before attempting the widening.
- **Rationale:** An empty slot answers the same `ERR_BAD_CAP` the subset test
  answers. A bare widening arm would therefore pass identically in a graph that
  never granted the endpoint — coverage that looks real and is not, which is the
  exact failure mode B17 was opened for. The arm sends on the end first, so a
  graph without one skips silently and claims nothing. `valid.zti` grants no
  probe, and the x86 transcript confirms the skip.
- **Rejected alternative:** Gate the arm on a compile-time check flag, which
  would make the component carry a scenario branch — the thing this milestone
  exists to remove.

- **Decision:** `unmap_loan` is a separate table entry point rather than a
  handle conversion at the call site.
- **Rationale:** The two authorize differently. `unmap` requires region
  ownership, which a loan receiver never has, so converting a loan handle into a
  buffer handle and calling `unmap` would be refused by `authorize`. Splitting
  makes the difference explicit and keeps the loan path from silently acquiring
  owner authority.

## Open risks and follow-ups

- [ ] **B16** stays open and its margin is now the narrowest recorded: thirteen
      tasks against `MAX_RECORDS = 32`. Still latent, because every declared
      generation runs to completion and exits — a quantifier that stops being
      safe at P5.4.
- [ ] **B12** stays open, re-reviewed before this gate on unchanged reasoning.
- [ ] **B18** partly fixed and still open: the gate now passes 6 runs in 9. Two
      residual causes — a publisher that can still start before its subscribers
      are provisioned (needs a real dependency, not a yield), and a marker lost
      to non-atomic `debug_write` on an otherwise correct boot. Resolve before
      the gate is relied on as a regression guard.
- [ ] `just deny` fails at HEAD and failed identically before this change. Not
      in CLAUDE.md's required list; unaddressed here rather than silently
      ignored.
- [ ] The gate's `transfers served` assertion is `[1-9]\d*` rather than an exact
      number, unlike P5.5.1's `served=4`. An exact count here would encode this
      composition's arithmetic rather than the property; the chains assert
      *which* roles crossed.
- [ ] `fabric-subscriber`'s "both sample forms arrived" assertion is now inert:
      every graph that declares the component also declares the publisher that
      produces the shared form, so relaxing it fails nothing. Kept because it is
      the unmodified oracle's own assertion, but it should not be counted as a
      guard. See the fault-injection table.

## Artifacts and provenance

- Generation fixture and rationale: `contracts/generation/v1/fixtures/sel4-stream.{zti,md}`
- Gate: `scripts/check/check-sel4-stream-plane.py`
- Related roadmap item: `roadmap/07-architecture-portability.md` P5.5.2
- Backlog: `roadmap/00-backlog.md` B17 (resolved), B16, B12
- Predecessor: `devlog/2026-08-05-p5-5-1-typed-fabric/`
