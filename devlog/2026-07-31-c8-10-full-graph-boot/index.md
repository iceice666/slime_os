# C8.10 — Collision-free full-graph boot and live bounded route workers

| Field | Value |
|---|---|
| Date | 2026-07-31 |
| Kind | Change |
| Status | Verified |
| Scope | Kernel boot path, generation fixture, resolver profile layout, fabric service and two new worker binaries, every fabric participant's boot arm, new `data_fabric_boot_check` gate |
| Roadmap | C8.10 |
| Gates | `just data_fabric_boot_check`, `just data_fabric_profile_check`, `just fabric_authority_check`, `just fabric_stream_check`, `just fabric_qos_check`, `just fabric_call_check`, `just fabric_operation_check`, `just fabric_visibility_check`, `just test`, `just fmt_check_all`, `just lint_all` |
| Trigger | C8.10 bootstrap replacement, the half deferred by the 2026-07-30 route-worker-partition entry |
| Baseline | The declarative half had landed: plane control slots summed into one disjoint layout, a validated route-worker partition, per-worker `SYS_WAIT` peaks. But the stream, call, and operation planes were still mutually exclusive generation profiles physically aliasing one range of init capability slots, selected by rewriting `caps[46..60]` per generation number, and no boot launched more than one plane. |

## Summary

This lands C8.10's bootstrap replacement. One generation now launches every C8
role at once — both stream routes, the call plane, the operation plane, plus the
unauthorized probe, the declared interposition proxy, and the filtered
introspection client as three distinct component identities — through a
fabric-only capability layout measuring **53 of 64** slots, and the whole graph
reaches healthy blocked idle with no traffic and no polling. The fabric is split
into three bounded route workers: the stream worker stays in `fabric-service`,
and `fabric-call-worker` and `fabric-op-worker` are new tasks it spawns.

The previous entry projected 53/64 and flagged the figure as "a projection to
re-count against real code, not an observed layout". The boot now prints the
count from the live path and the gate parses it back, so it is observed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Boot path | Added `launch_fabric_boot_init`, an early return in `launch_init` mirroring `launch_recovery_init`, building its own fabric-only capability vector. | The full graph fits without renumbering the layout six passing gates read positionally; no profile-dependent slot rewrite happens on this path at all. |
| Resolver | The `unified` profile resolves its own stream-plane control list (7 participants, `fabric-intruder` dropped, probe/proxy/observer added) instead of sharing one tuple. | Adding participants cannot renumber the subscriber supervision slots the older profiles grant positionally — `default` and `visibility` still resolve to `requiredCapabilitySlots` 37, byte-identical. |
| Route workers | Two new binaries wrapping the existing `call_broker` and `operation_broker`, spawned by `fabric-service` with `spawnBudget = 2`. | One task cannot park on 24 sources against `MAX_WAIT_SOURCES = 9`; the declared partition (stream 8, call 7, operation 9) becomes three real tasks that each block on their whole live set. |
| Worker control binding | The fabric passes on the service-side control endpoints init created, rather than minting new ones. | The spawn-time binding between a control endpoint and a component identity — the basis of every C8.3 authority claim — is preserved rather than re-established by a second party. |
| Participant boot arms | Each participant provisions its declared role, asserts the descriptor names the exact (route, direction, object kind) tuple, and parks. | The gate's exit condition is a provisioned graph *at rest*; the sample, call, and operation data paths stay the property of C8.4–C8.7's own gates. |
| Executable transferability | Boot-layout executables carry `RIGHT_TRANSFER`. | `spawn_from_cap` makes a child's supervision handle transferable only if its executable is, and the request/response brokers authenticate a participant by exactly that moved handle. |
| Idle sweep | Added `fabric-publisher-b`, `fabric-subscriber-b`, the three split roles, and both workers to `on_idle`'s `checks`, plus a boot arm treating a live fabric task as healthy. | A role that crashed was previously invisible: tracked by `record_spawn` but never swept, so the slice reported healthy while the transcript merely lost a marker. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A plane's slots collide when planes coexist | `just data_fabric_boot_check` | Init or a worker fails to spawn; the layout report exceeds its ceiling. |
| The layout silently grows to the kernel ceiling | `just data_fabric_boot_check` | The gate parses the live `N of 64` figure and fails at `N >= 64` rather than at the next participant added. |
| A fabric role crashes or exits instead of parking | `just data_fabric_boot_check` | Named per component: `<name> did not reach healthy blocked idle`. |
| A worker polls or outgrows one `SYS_WAIT` set | `just data_fabric_boot_check` | `live stream sources exceed one SYS_WAIT set` in the forbidden list. |
| The probe, proxy, and observer collapse back into one identity | `just data_fabric_boot_check` | Their three distinct markers are asserted separately. |
| The `unified` layout change disturbs an earlier profile | `just data_fabric_profile_check` plus all six fabric gates | A renumbered supervision slot makes a gate read a control endpoint where it expects a supervision handle. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_boot_check` | Passed: `53 of 64 init capability slots, 20 roles at healthy blocked idle, three bounded route workers`. | Direct |
| `just data_fabric_profile_check` | Passed; `default` and `visibility` resolve unchanged at 37 required slots. | Direct |
| `just fabric_authority_check`, `just fabric_stream_check`, `just fabric_qos_check`, `just fabric_call_check`, `just fabric_operation_check`, `just fabric_visibility_check` | Passed — see the run recorded in the commit; the six C8.3–C8.8 gates are the evidence that the new profile did not disturb the old layouts. | Direct |
| `just test`, `just contracts_check`, `just generation_check` | Passed. | Direct |
| `just fmt_check_all`, `just lint_all` | Passed with warnings denied. | Direct |
| Init capability peak | 53 of 64 at launch. Incremental release is load-bearing, not tidiness: 53 slots plus one supervision handle per participant is 69 against a ceiling of 64, so a leak of the bootstrap-only capabilities fails a spawn and takes the gate with it. That is the observed guard for "release bootstrap-only capabilities immediately after spawn". | Direct |
| Supervision-ordering determinism | The single `yield_now` between spawning and transferring supervision is deterministic rather than a winning race: scheduling is cooperative (the APIC timer handler advances ticks and never preempts), `SYS_YIELD` appends to a FIFO ready queue, and a send blocks only on a full queue — so every participant enqueues its role request before init resumes. Read from `task::yield_now`, `timer_handler`, and `ipc::send`. | Direct |

## Decisions

- Decision: add a seventh boot path rather than extend the existing `caps` vector.
- Rationale: that vector was 61 of 64 before this change and is rewritten per generation number by the `caps[46] = ...` blocks that six passing QEMU gates depend on. Renumbering it to fit three more roles would have rewritten C8.3–C8.8's evidence instead of extending it. `launch_recovery_init` already established the early-return precedent.
- Rejected alternative: appending to the shared layout and renumbering `init.rs`'s slot constants — the option the previous entry sized at "rewriting six passing QEMU gates", which is exactly what it would have cost.

- Decision: keep the stream broker in `fabric-service` and extract only two workers.
- Rationale: the milestone requires three bounded workers, not three new binaries. The stream plane's provisioning path is the one four gates exercise; moving ~1500 lines of it would have risked that evidence to gain nothing the declared partition does not already state.

- Decision: participants provision and park rather than exchanging traffic.
- Rationale: required check 4 names "healthy blocked idle with no traffic". Running the C8.4–C8.7 scenarios concurrently would make this gate fail on any regression in those planes, so a break in the call plane would surface as a C8.10 failure rather than a C8.6 one.

- Decision: the interposition proxy parks without a role in this profile.
- Rationale: the graph declares it as a chain hop, not a route participant, and the stream broker provisions participants. Its relay authority is provisioned by the C8.8 visibility broker, whose gate still proves it. Here it must be a distinct task with non-overlapping grants, which it is.

## Open risks and follow-ups

- [ ] `fabric-intruder` is retired from the new boot profile but still exists, and `fabric_visibility_check` still names it in both its required markers and its `check_source_authority()` source read. Deliberately deferred: deleting it would take that gate red for a component C8.10's own checks never mention. Its probe/proxy/observer roles are already split, so retiring it is now a rename-and-delete confined to the C8.8 gate.
- [ ] The boot profile provisions the declared interposition chain's *participants* but not the chain itself; a proxy relaying under the full graph is unproven. `fabric_visibility_check` proves the relay on its own profile.
- [ ] The stream worker's final state is parked inside `provision` on its *control* set, not on its stream sources. The declared graph includes the unauthorized probe and the interposition proxy, and neither ever answers — the probe is refused by design, the proxy is a chain hop rather than a participant — so `provision` never returns and the trailing `park_on_streams` loop is unreachable. Every declared stream edge is minted and announced before that point, so the graph is fully provisioned; but "the stream worker parks on its 8 declared stream sources" is established by the resolver's bound check and `just fabric_stream_check`, not observed by this gate.
- [ ] The boot gate observes each of the three workers *parked on its own live set*; it does not observe any of them parked at its declared peak. The boot graph is quiescent by design, so the call and operation workers have no in-flight calls or pending deliveries and their live sets sit well below 7 and 9. The peaks are covered elsewhere — the resolver rejects a partition above `MAX_WAIT_SOURCES` at build time, each broker binds its `SYS_WAIT` array to the declared value through a `const _: () = assert!`, and `just fabric_call_check` / `just fabric_operation_check` drive the full scenarios — but no single gate observes a worker at its bound on the full graph.
- [ ] `FABRIC_WORKER_WAIT_SHAPES` still mirrors broker constants by hand. Unchanged from the previous entry: compile-time asserts catch a broker and its declared peak disagreeing, but nothing checks the Python arithmetic against the broker's actual `wait` call sites.
- [ ] Init's boot arm parks on the fabric's supervision handle and never reaps. A participant that dies wakes it, but it takes no action beyond letting the kernel's sweep report the failure — adequate for a gate whose healthy state is total quiescence, not for a supervisor.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the gate's own serial assertions are the record.
- Serial/debugger/model output: observed through `just data_fabric_boot_check`.
- Related roadmap item: [C8.10](../../roadmap/02-core-runtime.md#c810--collision-free-full-graph-bootstrap-and-bounded-route-workers).
