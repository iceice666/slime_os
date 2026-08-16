# C8.13 — concurrent cross-plane traffic, nine gaps C8.10's parked boot never exercised, and an honestly partial exit

| Field | Value |
|---|---|
| Date | 2026-08-15 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-traffic.zti`, `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}`, `components/proto/src/fabric_trace.rs`, `components/proto/tests/fabric_trace.rs`, `scripts/lib/fabric_trace_contract.py`, `components/bins/src/{fabric_boot.rs,call_broker.rs,operation_broker.rs}`, `components/bins/src/bin/{fabric-service,init,fabric-op-worker,fabric-observer,fabric-proxy}.rs`, `boot-contracts/src/generation.rs`, `scripts/build/{build-generation.py,build-sel4.py}`, `scripts/check/{check-sel4-traffic-plane.py,check-sel4-gate-controls.py}`, `Justfile` |
| Roadmap | C8.13 |
| Gates | `just sel4_traffic_check`, `just data_fabric_traffic_check`, `just sel4_gate_control_check`, `just sel4_trace_check` |
| Trigger | C8.13 was the next uncompleted milestone with C8.11 and C8.12 both complete |
| Baseline | C8.1–C8.12 complete; C8.10's `sel4-boot.zti` proves the three-worker stream/call/operation partition is collision-free, but every participant parks without ever exercising a broker's real relay loop |

## Summary

C8.13 asks for the stream, call, and operation planes to carry real,
concurrent traffic through C8.10's exact partition while every declared
resource ceiling emits bounded peak-and-baseline evidence. That partition —
`sel4-boot.zti`'s three workers, `fabric-service`/`fabric-call-worker`/
`fabric-op-worker` — was built to prove *shape*: every role fits in disjoint
slots with nothing colliding, under a scenario where every participant
requests its role and then parks. Nothing there had ever sent a real sample,
made a real call, or driven a real operation goal through it concurrently.

Making it do that (`bootAction = "traffic"`, `sel4-traffic.zti`) surfaced nine
distinct latent gaps the parked scenario could never reach: a missing
operation restart-release-barrier slot on the dedicated worker, missing
call-plane phase-barrier grants and a missing `transferable` flag on the call
control endpoints (loan delegation crosses authority and needs it), two
missing shared-buffer factory grants and two missing `sharedBufferBudget`
entries, and — the one that took the longest to isolate — the declared
interposition proxy and the filtered-view observer both permanently blocking
the plain stream broker's completion, because neither ever exits and the
broker's delivery loop retries a *blocking* send to whichever endpoint a
matched subscriber's role names until that task is observably gone. Each was
found by booting, reading exactly where the transcript stopped advancing, and
tracing the one syscall that never returned. `just sel4_traffic_check` now
boots the three-worker partition, drives real traffic on all of it
concurrently, and asserts interleaving, clean task exit (except the two
structural roles this milestone does not drive), and bounded resource
evidence for six of the eleven declared resource classes.

**This is a partial exit, not a complete one**, committed as `In progress`
after the roadmap's full scope turned out substantially larger once read in
full: 11 declared resource classes, not 6; QoS-timed stream traffic running
concurrently with call/operation, which this entry drops; and a saturation
scenario that deliberately drives every ceiling to its bound, which this
entry does not attempt. See *Open risks and follow-ups*.

A reviewer pass over the working tree before commit found nine further
issues, all in evidence quality and comment accuracy rather than the
concurrency-sensitive core (which it found sound): the stream plane's
`RESOURCE_RETRIES` counter could only ever record a structural zero, since
the `retry_count` field it samples advances only inside the QoS-timed arm
this entry does not drive — dropped rather than kept as fake evidence. The
new gate hardcoded `RESOURCE_COMPLETE` instead of importing it from the
generated contract binding the way its sibling gate does, anchored its
stream-concurrency check on provisioning markers that land before `broker()`
is ever entered rather than real traffic, used a failure-marker prefix
`drive_traffic_plane` never emits, and hard-failed a nonzero operation
`retained` baseline the broker's own close comment says is a legitimate
outcome. `BootAction::Traffic` was missing from `generation.rs`'s frozen ABI
numbering test, and three doc comments (module-level, on `drive_traffic_plane`,
and in the trace schema) made claims the code did not back, including a
reference to a helper function this patch no longer defines. All nine are
fixed; see *Changes* and the devlog `Decisions` for the retries removal.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}` | New `resourceBuffers`(6), `resourceRetries`(7), `resourceRetained`(8) counters; `resourceComplete`/`maxResourceCounter` renumbered to 9; Python bindings extended to export every `resourceX` code, not `resourceComplete` alone | Every counter this milestone adds has a schema-owned code and a documented peak/baseline convention; a host script asserting on a specific counter can import it instead of hand-copying its number |
| `components/bins/src/bin/fabric-service.rs` | New `traffic_graph()`: `boot_graph`'s exact partition and proxy pre-mark, but calling `broker()` for a real relay loop and waiting for every non-parked task to exit instead of looping forever. `RESOURCE_FRAMES` peak(+baseline) and `RESOURCE_BUFFERS` peak(+baseline, gated to `"traffic"`) emission in the close block. No `RESOURCE_RETRIES`: `Subscriber::retry_count` advances only inside `apply_time`, itself gated to `"qos"` alone, so a stream retries record under `"traffic"` would be a structural zero rather than evidence | The stream plane's own worker can carry real traffic under the C8.10 partition without disturbing C8.5/C8.11's standalone `sel4-qos.zti` gate, whose fixed `traceDepth=16` predates this evidence, and without claiming resource evidence for a ceiling this plane cannot yet drive |
| `components/bins/src/call_broker.rs` | `peak_buffers`/`peak_retries` fields, `Payload::buffer_slot()`, `RESOURCE_CALLS`/`BUFFERS`/`RETRIES` peak(+baseline) emission | The call plane's own worker reports its resource ceilings under real concurrent load, where the standalone `sel4-call.zti` fixture already carries enough `traceDepth` headroom |
| `components/bins/src/operation_broker.rs` | `peak_retained` field, `RESOURCE_OPERATIONS`/`RETAINED` peak(+baseline) emission; baseline for retained results is read fresh rather than assumed zero, since `finished()` does not require the retained table empty | The operation plane's own worker reports its resource ceilings, including the one table a client can still hold live at close |
| `components/bins/src/bin/fabric-op-worker.rs` | `RESTART_START_SLOT = 12`, `Some(RESTART_START_SLOT)` where the worker previously passed `None` | The declared operation-plane restart scenario has a real release barrier under `"traffic"`, instead of hitting `pump_replacement`'s "reached admission with no barrier declared" fail-closed guard |
| `boot-contracts/src/generation.rs` | `BootAction::Traffic = 28` variant, parse arm, and a `(BootAction::Traffic, 28)` row in `boot_action_numbering_is_frozen` | The new action's ABI number is pinned the same way every prior variant is, so a future renumbering-in-declaration-order mistake is caught by the same test C8.12 relied on |
| `components/bins/src/fabric_boot.rs` | New `full_graph_active()` (`"boot"` or `"traffic"`), used only by the observer and proxy | The two C8.10 structural roles this milestone does not drive stay parked under `"traffic"` exactly as they do under `"boot"`, instead of the observer's real subscription request permanently blocking the broker's delivery loop on its dead control endpoint |
| `components/bins/src/bin/{fabric-observer,fabric-proxy}.rs` | Park gate switched from `fabric_boot::active()` to `full_graph_active()`; observer keeps its C8.10 `provision_and_park` arm for `"boot"` unchanged and adds a `park_only` arm for `"traffic"` | Same as above, at the two call sites |
| `components/bins/src/bin/init.rs` | `BootAction::Traffic` (28) dispatch, `drive_traffic_plane()`: the identical `drive_boot_plane` spawn sequence, granting three previously-omitted buffer factories (publisher-b, call-client, call-server), `wait_clean` on every task except the two parked ones, and a healthy-idle check on those two instead | The full partition spawns and settles under real traffic with the same grant shape `drive_boot_plane` already proved collision-free |
| `scripts/check/check-sel4-traffic-plane.py` (new) | Boots the traffic image; asserts three short deterministic chains (admission, init's single-threaded spawn order, close), task lifecycle (17/19 clean exits, 2 healthy-parked), cross-plane interleaving anchored on each plane's own real relay-loop traffic markers (not provisioning), and per-family resource evidence imported from the generated `fabric_trace_contract` module (declared `RESOURCE_*` peak/baseline pairs, one terminal record last, nothing dropped or rejected; the operation `retained` baseline is asserted bounded by its own peak rather than zero, matching the broker's own documented invariant) | The milestone's required checks are each independently falsifiable rather than implied by a passing boot alone, B55's rule holds (no chain asserts an order across genuinely concurrent tasks), and the gate's own failure/resource-code literals cannot drift from the code they check |
| `Justfile`, `scripts/check/check-sel4-gate-controls.py` | `sel4_traffic_check`, `data_fabric_traffic_check` targets; `sel4_traffic_plane` registered in the gate-control table (10 required markers) | The new gate is reachable by its roadmap-named target and is itself proven to reject a deleted/transposed/appended-failure marker |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future change to the stream close block re-widens unconditional `BUFFERS`/`RETRIES` emission and silently drops records on `sel4-qos.zti`'s fixed `traceDepth=16` | `just sel4_trace_check` | `dropped=N` (N>0) in the stream family's summary, or a `FAILURE_MARKERS` hit |
| A future edit to `drive_traffic_plane`'s grant vectors desyncs from `sel4-traffic.zti`'s declared minted/crossing grants | `just sel4_traffic_check` | Spawn failure, `SLIME_GRAPH buffer create refused`/`loan refused`, or a boot timeout |
| The observer or proxy is changed to request a real role under `"traffic"` again | `just sel4_traffic_check`'s `check_task_lifecycle` | `fail(f"{component} task {task} exited with status(es) ..., but the milestone requires it to stay parked")`, or (if it instead never settles) the 240s boot timeout |
| A future participant change makes one plane's traffic markers collapse into a purely sequential phase | `just sel4_traffic_check`'s `check_concurrency` | `"... showed no marker from another plane between two of its own; the schedule looks sequential rather than concurrent"` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check` (×5, including 3 back-to-back repeats) | Pass every time; stream/call/operation each closed with 0 dropped, 0 rejected | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just lint_all` | Pass, zero warnings across stage0, boot-contracts, host components, and the seL4 product crates | Direct |
| `just ruff`, `just typos` | Pass | Direct |
| `just test_host` | Pass (boot-contracts + slime-proto host suites, including the 19-test `fabric_trace` suite) | Direct |
| `just test_sel4_root` | 112/112 across 13 modules | Direct |
| `just contracts_check`, `just generation_check` | Pass; `sel4-traffic.zti` validates and the pinned generation's determinism check still holds | Direct |
| `just sel4_gate_control_check` | 30 gates (was 29) reject 1158 mutated transcripts/layouts | Direct |
| `just sel4_matrix_check`, `just sel4_boot_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_trace_check` | All pass unchanged | Direct — confirms `fabric_boot.rs`/`fabric-service.rs`/`fabric-observer.rs`/`fabric-proxy.rs`/`operation_broker.rs` changes do not regress any standalone plane |
| `just data_fabric_profile_check` | Fails with `fabric graph: invalid control grant fabric-call-client-control`, identical to the pre-C8.13 baseline (`5896fb9`) | Direct — confirmed pre-existing and unrelated, not caused by this work |

## Decisions

- **Decision:** Commit C8.13 as `In progress`, covering 6 of the 11 declared resource classes (frames, buffers, retries, in-flight calls, in-flight operations, retained operation results) plus real concurrent stream/call/operation traffic, rather than continuing to the full exit condition in one pass.
  **Rationale:** The roadmap's full deliverable list (11 resource classes, timed-QoS stream concurrency, saturation testing) was substantially larger than initially scoped, and each additional piece was following the same pattern already observed nine times over — a hidden wiring gap C8.10's parked scenario never exercised, discoverable only by booting and tracing one blocked syscall at a time. Continuing to the full scope in the same session risked either a rushed, under-verified finish or an open-ended session with no natural stopping point. Asked the user directly; the user chose to stop and commit the verified partial subset with an honest roadmap status over continuing or silently narrowing the milestone's declared scope.
  **Rejected alternative:** Redefining C8.13's exit condition down to match what was built (asked, not chosen) — would have understated the roadmap's actual requirement rather than recording it as outstanding work.
- **Decision:** Drop QoS-timed stream traffic from the traffic plane rather than wire its missing clock grant.
  **Rationale:** `fabric-publisher-b`'s QoS-timed arm sends simulated clock advances directly to the stream fabric over a dedicated `fabric-publisher-b-clock` peer endpoint that only exists in the standalone `sel4-qos.zti` fixture; wiring it into the C8.10 partition means teaching `build-generation.py`'s `FABRIC_BOOT_STREAM_CONTROL_GRANTS` about a new per-worker clock edge, not just adding a fixture-level grant like the other eight gaps here. The call and operation planes' own clocks (`fabric-call-time`, `fabric-op-time`) already run unconditionally under `"traffic"` and supply real `RESOURCE_RETRIES`/timeout/expiry evidence, so the milestone's retry/deadline properties are not entirely unproven — only the stream-specific timed arm is missing.
  **Rejected alternative:** Extending `qos_check()` to `"qos" || "traffic"` unconditionally (tried first) — regressed `sel4_trace_check` by pushing `sel4-qos.zti`'s fixed `traceDepth=16` over capacity even before the clock-grant gap was reached; reverted.
- **Decision:** The observer and the declared interposition proxy stay permanently parked under `"traffic"` rather than joining real stream delivery.
  **Rationale:** The plain stream broker's `deliver()` retries a *blocking* native `Send` to whichever control endpoint a matched subscriber's role names, with no notion of "this peer will never consume again" — it relies entirely on the peer's task exiting to detect that a route is done. The observer's C8.10 role (`provision_and_park`) requests a real subscription and then never drains or exits, so `capability_delegate`'s blocking export send to it never returns once traffic tries to deliver the shared telemetry sample it matched — deadlocking the whole broker and starving every other subscriber behind it. Fixed by parking the observer *without ever requesting the role* (`park_only`, exactly the proxy's existing treatment), and pre-marking both as answered in `traffic_graph`'s `provision()` setup.
  **Rejected alternative:** `provision_and_exit` (accept the role, then exit immediately) — tried first; makes the observer a real, correctly-answered `Subscriber` that the broker still tries to deliver a shared sample to on a now-dead task's control endpoint, reproducing the identical deadlock.

## Open risks and follow-ups

- [ ] Queue, history, event, mapping, loan, and capability-slot resource evidence (5 of 11 declared classes) — not implemented. Each needs its own peak+baseline emission site in the relevant broker plus manifest-level verification that the emission does not regress the standalone C8.4-C8.9 fixtures' fixed `traceDepth`, the way `sel4-qos.zti` did here.
- [ ] QoS-timed stream traffic concurrent with call/operation — dropped; needs `build-generation.py`'s `FABRIC_BOOT_STREAM_CONTROL_GRANTS`/per-worker control-grant resolution taught about a `fabric-publisher-b`↔`fabric-service` clock edge under the `"traffic"` partition, analogous to the call/operation phase-barrier grants this entry already adds.
- [ ] A saturation scenario that deliberately drives every declared ceiling to its manifest bound at once, and asserts neither an exceeded bound nor a deadlocked route worker — not attempted; the current `sel4-traffic.zti` scenario exercises normal C8.4-C8.9 traffic volumes, not adversarial ones.
- [ ] `just data_fabric_profile_check`'s pre-existing `invalid control grant fabric-call-client-control` failure remains open and unrelated; not filed as a new backlog item here since it predates this work and was confirmed identical on baseline `5896fb9`.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in the *Changes*/*Decisions* tables.
- Raw transcript: none captured separately; the session's own tool-call history is the record.
- Serial/debugger/model output: `just sel4_traffic_check`'s own transcript output (reproducible by running the gate).
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
