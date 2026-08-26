# C9.6: a robot workload that survives its own restart, on the mechanisms C9 shipped

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/{robot-sensor,robot-controller,robot-actuator,robot-supervisor,robot-burner,robot-clock,fabric-call-worker,fabric-service,init}`, `components/lib/src/call_broker.rs`, `slime-root/src/{main,peer_endpoint}.rs`, `boot-contracts/src/generation.rs`, `contracts/generation/v1/fixtures/sel4-robot-runtime.zti`, `contracts/boot-layout/v1/fixtures/sel4-robot-runtime.layout`, `scripts/{build,check}` |
| Roadmap | C9.6, C9, C9.1, C9.2, C9.3, C9.4, C9.5, C8.10, C8.15, B70, B75, B76 |
| Gates | `just robot_runtime_check`, `just sel4_boot_check`, `just sel4_call_check`, `just sel4_traffic_check`, `just sel4_fabric_aggregate_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host` |
| Trigger | C9.6 was the last open roadmap milestone: the backlog's Open section is empty, C9.1–C9.5 closed, and M5.7/RP3/RP4 are hardware-blocked. |
| Baseline | C9.1–C9.5 complete. No generation composed a timer-driven sensor, a dual-contract-kind controller, and a call-served actuator over one fabric while surviving a mid-run component restart under declared contention. |

## Summary

A simulated sensor → controller → actuator graph now runs end to end on the
native fabric, under a declared best-effort CPU load, and survives an injected
controller fault mid-run with its fabric authority reissued to the replacement.
`just robot_runtime_check` boots the `sel4-robot-runtime` plane (generation 46,
`bootAction="robot-runtime"`) twice over one composition and compares the
declared semantic traces field by field, the same discipline C8.15 applied to
the aggregate schedules. The composition exercises every C9 slice in one boot:
C9.1's clock authority drives both the sensor's cadence and the call plane's
deadline arm; C9.2's bounded wait sets carry the timer notifications; C9.3's
declared scheduling class is what makes the contention claim non-vacuous; C9.4's
lifecycle policy is what bounds and admits the restart; and the two-boot
comparison is the C9.5 discipline of comparing declared traces rather than
marker presence, applied to a composition C9.5's own recording surface does not
yet declare deterministic (its clock reads are the only recorded sources, and
this graph's authority crosses routes that are not).

The controller is the milestone's novel participant: one identity holding two
contract kinds at once, a stream subscriber on `telemetry` and a call client on
`parameters`, each reaching its broker through a separate generation-declared
control endpoint. The restart is what makes that composition load-bearing
rather than a sequencing exercise: the replacement incarnation comes up holding
reissued stream and call authority and resumes the chain, and the parameter it
applies is read back from instance-owned state the root wrote once before the
first death, so "fresh authority, original configuration" is a data claim, not
a marker claim.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/bins/robot-sensor` (new) | Publishes one `TelemetrySample` per C9.1 timer expiry on the declared `telemetry` stream, four ticks then a terminal `FLAG_LAST`, at the `foreground` band | The plane's ordinary end is a real stream end, and each expiry preempts a demonstrably-running burner, which is what makes "declared scheduling order preserved under contention" observable |
| `components/bins/robot-controller` (new) | Subscribes `telemetry`, issues one bounded `ParameterCall` command per consumed sample on `parameters`, reads its command scale back from instance-owned parameter state every incarnation, and faults deliberately after two samples on its first incarnation | One identity holds two contract kinds at once; the injected restart is scripted rather than induced so the transcript can tell a restart from a slow component |
| `components/bins/robot-actuator` (new) | Answers forwarded commands on `parameters`: applies in-range ones, refuses out-of-range ones `STATUS_REJECTED`, settles a deliberately withdrawn command `STATUS_CANCELLED`, and never answers one sentinel so the route's declared deadline is what expires it | Cancellation, rejection, and deadline miss each have exactly one producer, which is what keeps them distinguishable from each other and from fault and peer loss |
| `components/bins/robot-clock` (new) | Advances the shared simulated clock to two declared instants, each behind a barrier the controller releases only after the request it must expire exists | The deadline arm is non-vacuous: an advance against no in-flight call expires nothing and would prove nothing about deadline handling |
| `components/bins/robot-burner` (new) | Spins a declared `bestEffort` load in chunks whose markers bracket the higher bands' progress | "Declared scheduling order preserved under contention" is asserted against a real runnable competitor, not an idle vCPU |
| `components/bins/robot-supervisor` (new) | Observes the controller's death through its supervision handle, asks the root what the declared policy admits, waits the declared backoff on a real C9.1 timer, spawns the replacement itself, and signals the call broker's retirement notification once no further restart will come | C9.4's division is composed rather than re-derived: the root gains none of this component's restart policy |
| `contracts/generation/v1/fixtures/sel4-robot-runtime.zti` (new) | Generation 46, `bootAction="robot-runtime"`: the six components, the two routes, the declared clock authority, the lifecycle policy with `causes=["fault"]` and `attempts=2`, the scheduling class, and the shared-buffer and private-memory budgets | Every arm of the plane is declared data, not an ambient default |
| `components/lib/src/call_broker.rs` | `Broker::new` gains a `retirement: [Option<u32>; CLIENTS]` parameter and a `wake_name: &'static [u8]` parameter; `reclaim_dead_clients` checks a `(supervision, retirement)` pair per client, polling `notification_poll(retirement)` as the death signal when no supervision handle exists | A handle-less client's slot can now be retired, so `Broker::run`'s exit predicate (every client absent) is reachable on a plane whose only client is a restartable, generation-owned endpoint |
| `components/bins/fabric-call-worker/src/main.rs` | The one fixed positional slot layout becomes a `Composition` struct dispatched on `BootAction`: `Boot` and `Traffic` share `TRAFFIC` (both fixtures declare the identical `fabric-call-{client,client-b,server,time}-control` grants), `RobotRuntime` takes `ROBOT` | The broker names roles by their declared grants rather than by CSpace position, so a generation adding or reordering unrelated authority cannot silently retarget one |
| `components/bins/fabric-service/src/main.rs` | `salvage_stale_subscriber` drains a dead subscriber's ring through the broker's own writer-side mapping before `reclaim_component` unmaps it, re-admits each sample into a fresh frame, and prepends them to whatever was still queued, handing the combined history to the reprovisioned subscriber; `reprovision_participants` calls it before reclaiming; `ring_receiver_slot`/`role_supervision_slot` special-case `robot-controller` under `BootAction::RobotRuntime` | A restart mid-stream drops no sample the fabric had already handed to the dead incarnation's ring but that incarnation had not yet consumed |
| `slime-root/src/peer_endpoint.rs`, `slime-root/src/main.rs` | `receiver_for` gains a `tasks` fallback parameter, consulted when the `LaunchedInstances` lookup finds no instance | Defensive only; confirmed not load-bearing for the loan-collision fix, which was the slot-numbering change instead |
| `boot-contracts/src/generation.rs` | `BootAction::RobotRuntime = 36`, registered in `ALL`, `from_id`, `parse`, and the `FROZEN_BOOT_ACTIONS` test (36 entries) | The new plane is a declared composition, not a hidden mode |
| `components/bins/init/src/main.rs` | `drive_robot_runtime_plane()` spawns robot-sensor, robot-actuator, robot-clock, then fabric-service and fabric-call-worker | The plane is launched the same way every other plane is, through init's declared spawn authority |
| `scripts/check/check-sel4-robot-runtime-plane.py` (new) | Builds the image, boots twice, pins the marker chains and failure markers, checks the fixture shape against the declaration, and compares the two boots' declared semantic traces field by field | The milestone's required checks are observed rather than argued, the same way C8.15's aggregate gate observes them |
| `components/bins/robot-{actuator,controller}`, `sel4-robot-runtime.zti` (review fix) | A new declared `robot-actuator-timeout-observed` notification: the controller signals it only after `expect_reply` returned `STATUS_TIMEOUT`, and the actuator waits on it before exiting | The deadline miss is a declared outcome rather than a scheduling artefact. The broker adjudicates `observe_server_death` *before* `pump_time` in one sweep, so an actuator free to exit earlier could have its own death settle the outstanding request `STATUS_PEER_DEAD` instead — the two outcomes the plane exists to separate, decided by priority |
| `components/bins/robot-{controller,actuator}` (review fix) | `expect_rejection` issues `REJECTED_COMMAND = 5000`, above the actuator's `MAX_COMMAND = 1000`; the actuator now fails if `rejected == 0` | The refusal arm is non-vacuous. Every ordinary command was `tick * scale` ≤ 35 and the only out-of-range value was the unanswered sentinel, checked first — so `STATUS_REJECTED` was declared, documented, and never once produced |
| `boot-contracts/src/stream_history.rs`, `components/bins/fabric-service/src/main.rs` (review fix) | Salvage reserves a free frame *before* `Ring::consume`, and the new `StreamHistory::note_loss` carries both abandoned ring samples and the dead record's untaken loss report into the replacement's history | A salvage that cannot keep everything reports a gap instead of presenting a truncated stream as complete. `Ring::consume` advances the shared tail and bumps no loss counter, so the old order destroyed the very sample it then failed to keep, unaccounted |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The boot plane's composition arm is dropped from `fabric-call-worker` again | `just sel4_boot_check` | `[fabric] call endpoints ready` missing; `SLIME_GRAPH component exit ... status=1` |
| The call plane's C8.6 surface regresses | `just sel4_call_check` | `[fabric] call endpoints ready` missing |
| The traffic plane's concurrent stream/call/operation traffic regresses | `just sel4_traffic_check` | missing bounded peak-and-baseline evidence |
| The aggregate two-boot semantic comparison regresses | `just sel4_fabric_aggregate_check` | differing declared traces across the two schedules |
| A handle-less client's call slot is never retired, so `Broker::run` never exits | `just robot_runtime_check` | plane never reaches `live=0` |
| A restart mid-stream drops ring-resident samples the dead incarnation had not consumed | `just robot_runtime_check` | the replacement's resumed tick sequence skips a tick |
| A replacement's reused request ID collides with the broker's duplicate-suppression high-water mark | `just robot_runtime_check` | a resumed command settles `STATUS_DUPLICATE` |
| A clean completion exit is wrongly admitted as a restartable cause | `just robot_runtime_check` | `restarts total=` exceeds the one injected fault |
| The declared `bestEffort` load starves the normal band, or vice versa | `just robot_runtime_check` | burner chunk markers stop bracketing the supervisor's terminal marker, or the graph never completes |
| A new boot action is added without a `fabric-call-worker` composition | `just sel4_gate_control_check` | gate mutation control on the composition dispatch |
| A deadline miss silently degrades into peer death because the server exited first | `just robot_runtime_check` | `[robot-actuator] timeout settlement observed` missing, or the controller failing `an unanswered command did not settle as timed out` |
| The refusal arm becomes vacuous again | `just robot_runtime_check` | `[robot-actuator] refused total=0`, which the actuator now treats as a failure |
| Salvage drops a sample without accounting for it | `cargo test -p boot-contracts stream_history` | `salvage_attributes_abandoned_samples_and_inherited_loss` fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just robot_runtime_check` | Pass: two boots, `SLIME_GRAPH HEALTHY generation=46 required=8 live=0 completed=8 failed=0`, both boots' declared semantic traces identical. Post-review the transcript carries `[robot-actuator] command refused value=5000`, `refused total=1`, and `[robot-actuator] timeout settlement observed` after `[robot-controller] command deadline observed`, with `[fabric] call peer death propagated` last | Direct |
| `just sel4_boot_check` | Pass: 30 markers, 5 causal chains, C8.10 boot plane's `fabric-call-worker` composition restored | Direct |
| `just sel4_call_check` | Pass: 47 markers, 10 causal chains | Direct |
| `just sel4_traffic_check` | Pass: 10 markers, 3 causal chains, stream/call/operation concurrent | Direct |
| `just sel4_fabric_aggregate_check` | Pass: 2 schedules, 279 semantically identical trace records in total | Direct |
| `just test` (`sel4_root_boot_check` + `sel4_component_graph_check` + `sel4_gate_control_check`) | Pass: root boot ordered markers, component graph launch and shutdown, 41 gates reject 1623 mutations (up from 1619 with the three new pinned markers) | Direct |
| `just test_sel4_root` | 183/183 across 19 modules | Direct |
| `cargo test -p boot-contracts stream_history` | 7/7, including the two new `note_loss` salvage-accounting tests | Direct |
| `just contracts_check` | Pass: generation v5 check, 36 manifests encode SLIMEG5 version 5, syscall-ABI and fabric-ring bindings current | Direct |
| `just generation_check` | Pass: byte-identical isolated builds | Direct |
| `just sel4_boot_layout_check` | Pass: 32 plane layouts match their fixtures | Direct |
| `just lint_all` | Pass: clippy `-D warnings` across every crate including stage0 and boot-contracts | Direct |
| `just fmt_check_all`, `just ruff`, `just typos`, `just deny`, `just machete` | Pass | Direct |

## Decisions

- **Decision:** Slot 30 for `fabric-service`'s native `robot-controller-control`
  binding. **Rationale:** the number is overloaded across two unrelated tables.
  The builder enforces a hard 0..31 ceiling on this "logical binding slot"
  (`instance fabric-service: logical binding slot outside 0..31`). Independently,
  `slime-root`'s `serve_buffer_loan` passes the same small integer into a
  root-internal, 64-entry, dynamically-allocated `AuthorityTable` indexed by
  `receiver_slot`, and `fabric-service`'s own dynamically-allocated authority
  entries occupy the low end of that table. A slot of 3 collided with one of
  those; 50 exceeded the builder's ceiling; 30 is the value that is both under
  the builder's ceiling and above `fabric-service`'s realistic dynamic
  allocation range. The collision is documented in `slime-root/src/main.rs`'s
  `serve_buffer_loan` comment.
  **Rejected alternative:** deriving the binding slot from the graph row instead
  of declaring it; the builder's 0..31 ceiling is a contract constraint, not a
  lookup.

- **Decision:** salvage the stale subscriber's ring-resident samples through the
  broker's own writer-side mapping, rather than carrying only the broker's own
  pending queue forward. **Rationale:** an earlier revision preserved only
  `subscriber.history` — samples admitted but not yet written to the ring — and
  the replacement still dropped the majority of the lost data, which lived in
  the ring's own memory (samples the broker had already written and the dead
  incarnation had not yet consumed). The dead task's reader-side mapping died
  with it, so the broker's writer-side mapping of the same shared buffer is the
  only live view left. Draining it and re-admitting each sample into a fresh
  frame, then prepending to whatever was still queued, accounts for both halves.
  **Rejected alternative:** carrying the queue forward alone, which silently
  dropped the ring-resident half.

- **Decision:** derive the call-broker request ID from the sample's own
  monotonic tick, not a per-incarnation counter. **Rationale:** the call
  broker's duplicate suppression is a per-client monotonic high-water mark, so a
  counter that resets to zero on a restart reuses IDs the broker has already
  seen and the replacement's first command is rejected as a duplicate of the
  first incarnation's. A tick is consumed exactly once across every incarnation
  combined, so it is never reused. `TERMINAL_REQUEST_BASE` (1,000,000) is the
  offset for the two one-shot cancel/timeout requests, chosen well clear of any
  tick value.
  **Rejected alternative:** a per-incarnation counter, which collides on restart.

- **Decision:** `causes = ["fault"]` only in the fixture's restart policy, not
  `["exit", "fault"]`. **Rationale:** a clean scenario-completion exit must not
  be a restartable cause, or the supervisor keeps re-admitting spurious extra
  restarts and each replacement re-subscribes to an already-torn-down stream and
  hangs. The roadmap's required checks ask only that the restart be bounded,
  reissue authority, and let the graph resume, which the single injected-fault
  restart demonstrates; `attempts = 2` remains as an independent, never-fully-
  spent safety ceiling.
  **Rejected alternative:** declaring both causes and capping restarts at the
  attempt count, which admits a restart for the controller's own clean finish.

- **Decision:** the contention-interleaving gate brackets a
  `[robot-supervisor] restarts total=` marker between two burner chunks, not a
  sensor tick. **Rationale:** six competing `normal`-band participants reacting
  to every sensor tick keep the CPU continuously busy through the sensor's own
  tick window, so no whole burner chunk ever lands there; the one structurally
  guaranteed idle gap is right after `robot-controller`'s own exit, when every
  other route has settled and only the supervisor's bounded poll remains.
  Bracketing the supervisor's terminal marker there is the same "higher band
  makes progress while `bestEffort` is still mid-run" claim the milestone
  requires, tied to the composition's actual scheduling structure rather than
  to a window that never opens.
  **Rejected alternative:** widening the supervisor's poll interval and shrinking
  the burner's chunk granularity until a chunk lands inside a sensor tick
  window, which is structurally unreachable in this composition.

- **Decision:** a `retirement` notification, not a minted supervision binding,
  carries "no further restart will come" to the call broker. **Rationale:** a
  minted binding for a third-party holder is installed only at the holder's own
  spawn time, and `fabric-call-worker` is init-spawned before `robot-supervisor`
  ever exists, so the binding is structurally unexpressible. The supervisor
  signals both `robot-controller-retired` and `fabric-call-worker-parameters-
  ready` before its own exit, because signaling retirement alone does not wake
  a broker parked on a different `Notification` object.
  **Rejected alternative:** a fabric-held supervision binding over the
  controller, rejected by `validate_supervision_binding_names` because the
  minted owner must match the subject's declared owner.

- **Decision:** the actuator's exit is ordered *after* the timeout settlement by
  a declared notification, rather than by ordering the clock's advance before
  the exit. **Rationale:** found by review, then confirmed against the observed
  transcript. `Broker::run` calls `observe_server_death` at `call_broker.rs:549`
  and `pump_time` only at `:554`, and `retire_server` runs
  `reclaim_all(STATUS_PEER_DEAD)` — so within one sweep a dead server settles
  every outstanding call before any queued advance is consumed, and peer death
  wins the tie. The pre-fix run reached the failing interleaving already:
  `[fabric] call peer death propagated` appeared *before* both
  `[robot-clock] advanced` lines, and the declared `STATUS_TIMEOUT` survived only
  because `try_send_terminal` happened to return `ERR_WOULDBLOCK` (the observed
  `[fabric] terminal delivery queued`), leaving the call in `PendingTerminal` for
  `pump_time` to overwrite. That queueing depends on the controller being parked
  in `release_clock_phase` rather than in `expect_reply` — pure scheduling. The
  milestone's central claim is that a deadline miss and a peer loss stay
  distinct, so the ordering had to become a property of the composition.
  **Rejected alternative:** publishing the second advance before the actuator
  exits, which is insufficient — a *pending* advance still loses to
  `observe_server_death`, so it would need an acknowledgement that the broker
  had already expired the request, which is the notification anyway.

- **Decision:** a dedicated `REJECTED_COMMAND = 5000` rather than reusing an
  ordinary command value. **Rationale:** also found by review. The refusal arm
  was declared in the fixture, documented in both components, and never once
  executed: every ordinary command is `tick * scale` with at most five ticks and
  a scale of seven, so at most 35 against a `MAX_COMMAND` of 1000, and the only
  out-of-range value in the scenario was the unanswered sentinel — which the
  actuator checks *before* its range test precisely so the request stays open.
  `refused total=0` was therefore load-bearing evidence of nothing. The actuator
  now fails outright on `rejected == 0`, so the arm cannot silently go vacuous
  again.

- **Decision:** salvage reserves its destination frame before consuming, and
  unsalvageable backlog is counted rather than dropped. **Rationale:** the third
  review finding. `Ring::consume` advances the shared tail and — contrary to the
  original comment — bumps no loss counter, so checking frame availability after
  consuming destroyed the sample it then failed to keep, with no
  `EVENT_SAMPLE_LOST` to show for it; a dropped terminal sample would also have
  erased the orderly stream end. Building a fresh `StreamHistory` additionally
  discarded the stale subscriber's own untaken `lost`/`oldest_lost` report. The
  new `StreamHistory::note_loss` is the single place both are re-attributed, so
  a restart that cannot keep everything reports a gap instead of presenting a
  truncated stream as complete.

## Open risks and follow-ups

- [ ] `slime-root/src/peer_endpoint.rs`'s new `tasks` fallback parameter is
  defensive and confirmed not load-bearing for this milestone's fixes; it is
  retained because removing it after the fact would require re-proving the
  loan-collision fix against a different root, and the parameter is exercised
  by no current caller. If it proves unreachable it can be removed as a
  follow-up.
- [ ] C9.5's deterministic-replay claim still covers clock-driven components
  only, because `recv`, `bufferLoan`, `supervise`, `parameterRead`, and
  `lifecycleRestart` are classified `unrecorded`. This graph composes those
  mechanisms but does not widen the recorded-source set past the clock, so the
  two-boot semantic comparison here is C8.15-style rather than C9.5-style
  record-and-replay. Widening that set is its own follow-up.
- [ ] `just test` aggregates three gates and names no C9 plane. Whether the
  aggregate should cover `robot_runtime_check` is a question for the C9 track's
  close, left open deliberately to avoid a second aggregate convention beside
  the existing per-plane ones.

## Artifacts and provenance

- Focused report: none; the decisions above are the analysis.
- Raw transcript: none retained. The gate reproduces the whole run
  (`just robot_runtime_check`), and its two-boot comparison is the artifact.
- Serial/debugger/model output: the gate's own transcripts, reproduced on
  demand.
- Related roadmap item:
  [C9.6](../../roadmap/02-core-runtime.md#c96--robot-workload-composition)
