# B55 — the full-graph boot plane refused its own first spawn, then six more defects behind it

| Field | Value |
|---|---|
| Date | 2026-08-15 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `components/bins/src/bin/{init,fabric-service,fabric-observer,fabric-probe,fabric-proxy}.rs`, `components/bins/src/fabric_boot.rs`, `contracts/generation/v1/fixtures/sel4-boot.zti`, `scripts/build/build-generation.py`, `scripts/check/{check-sel4-boot-plane.py,check-sel4-gate-controls.py}` |
| Roadmap | C8.10, B55 |
| Gates | `just sel4_boot_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check` |
| Trigger | User asked to discuss B55; investigation found the backlog entry's diagnosis was one of seven defects, not the whole gap |
| Baseline | `just sel4_boot_check` failed at `SLIME_GRAPH spawn refused task=0 slot=2 ungranted`, before any fabric role was provisioned; identical on unmodified `master` at `84c75f5` |

## Summary

C8.10's exit condition — one seL4 generation booting every C8 role
simultaneously to healthy blocked idle — was unobserved since the native-seL4-
IPC cutover (`c8fc792`). The backlog's own diagnosis (init sending an empty
spawn-grant vector) was correct but covered only the first of seven defects,
each masking discovery of the next, plus a structural mismatch in the gate
itself that would have kept it failing even after every code defect was
fixed. All seven are root-caused and fixed; `just sel4_boot_check` passes
reproducibly, and the full regression suite (root/stream/qos/call/operation/
visibility planes, gate-control mutation testing, contracts, generation
determinism, host unit tests, fmt, lint, ruff, typos) is green on the same
tree.

## Observable symptom

- Command: `just sel4_boot_check`
- Expected: every declared C8 role spawns, all twenty instances reach healthy
  blocked idle, the supervisor emits its `live == idle` terminal.
- Observed: `SLIME_GRAPH spawn refused task=0 slot=2 ungranted` immediately
  after `SLIME_ROOT allocator baseline`, before any participant ran.
- Exit/fault/serial evidence:
  ```
  SLIME_GRAPH spawn preflight count task-instance=19 child=16 requested=0 parent=1 minted=5 respawn=false
  SLIME_GRAPH spawn refused task=0 slot=2 ungranted
  ```

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `drive_boot_plane` (`init.rs`) spawned every child with `&[]`; `fabric-service` spawned both route workers itself. | Root cause 1: neither shape is expressible under the native model — a worker's control endpoints are generation-declared Endpoints installed before any task runs, and a fabric-spawned worker's participant-supervision handles name tasks only `init` holds. |
| 2 | Fixed grants and worker ownership; boot advanced past the terminal but `fabric-subscriber-b` (a two-route subscriber) failed `declared participant was denied` with garbage `status`/`route_identity`. | Root cause 2: `receive_role` decoded any message on the control endpoint as a capability transfer with no magic check; `refresh_matches` interleaves a QoS `EVENT_MATCHED` record on the same endpoint between a multi-route participant's own replies. |
| 3 | Fixed the magic check; `fabric-observer`/`fabric-probe`/`fabric-proxy` then fell through to their standalone-plane logic instead of the boot arm. | Root cause 3: the three gated on `startup_arg != 0`, but `construct_child` (`main.rs`) always passes a spawned child `0` — only the root-launched bootstrap instance carries the boot-action argument. |
| 4 | Fixed the selector; `fabric-service` then crashed `stream notification binding missing` when `fabric-observer` requested its role. | Root cause 4: `notification_slots` failed hard on an undeclared ready/credit pair, even though the profile resolver's own comment documents that a boot-mode role provisioned without driving samples legitimately declares none. |
| 5 | Fixed the sentinel; the graph then hung — `fabric-service`'s idle marker never printed and the root eventually FATALed `graph iterations exhausted`. | Root cause 5: `boot_graph`'s provisioning sweep required every registered stream client to answer, but the declared proxy holds a real control endpoint and, under boot, parks without ever contacting the broker. Root cause 6: `MAX_GRAPH_ITERATIONS`'s wedge check fires for any run with a live task at the bound, which is correct for exit-based graphs but wrong for one whose declared success state is every task parked forever. |
| 6 | Fixed both; the graph settled, but `fabric-observer` never printed its own role confirmation. | Root cause 7: `fabric-observer` requested 2 narrowed capabilities per edge; `provision_edge` only ever delegates 1 (a v2 ring loan carries data and credit in one region), so its second `receive_role` blocked forever. |
| 7 | Fixed the count; the gate itself then failed marker-ordering and pinned-count assertions built for a topology (`fabric-service` spawning two workers, per-participant sequential provisioning) that no longer existed, and stopped reading before the graph's own provisioning traffic. | The gate's `boot()` stopped at the first `SLIME_GRAPH healthy` line, which fires the instant every declared task *exists* — causally before the twenty instances' own request/reply traffic, not after — so it could never have observed its required markers even with every code defect fixed. |

## Root cause

Two independent classes of defect, both never previously exercised because
the boot plane never advanced far enough to reach them:

**Code (seven defects).** The native-seL4-IPC cutover rewrote the full-graph
launcher for the new capability model — every control endpoint and every
worker's participant-supervision handle is a generation fact the root
installs or `init` alone can grant — but left behind code shaped for the
retired custom kernel's model (empty grant vectors, a fabric that mints its
own workers, `startup_arg` propagated to every child, notification pairs
assumed always-declared, a provisioning sweep assumed to always complete, a
graph assumed to always fully exit, and a stream participant assumed to need
two capabilities per route). Each defect's failure masked the next until
fixed.

**Gate protocol.** `SLIME_GRAPH healthy … required=N live=N idle=N` is
printed inside the same central dispatch loop that services every task's own
IPC, the moment the Nth declared task is constructed — `idle` is `live`
printed a second time, not a separately tracked blocked-state count. Twenty
instances' own provisioning traffic runs *after* the twentieth spawn returns,
so the gate's `boot()`, which stopped reading at the first sighting of that
line, could never see the causal chains it required.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `init.rs` | `drive_boot_plane` spawns all nineteen children itself, including both route workers, with each fixture-declared grant vector. | A dynamically spawned worker's control endpoints and participant-supervision handles are always satisfiable by its spawner. |
| `sel4-boot.zti` | Call/op control grants target their worker directly; minted-binding table widened from 6 to 14 rows (one per real handle, matching B46's derivation rule); added the missing `nav-backup` grant. | The fixture's declared authority matches what the generated profile and the components actually read. |
| `scripts/build/build-generation.py` | `_control_sources` takes a `holder` parameter instead of hardcoding `fabric-service`. | A bounded route worker's controls can terminate at the worker the graph declares owns them. |
| `fabric_boot.rs` | `receive_role` checks `CAPABILITY_TRANSFER_MAGIC` and drains anything else. | A control endpoint carrying more than one record kind cannot have an unrelated record misread as a role reply. |
| `fabric-observer.rs`, `fabric-probe.rs`, `fabric-proxy.rs` | Boot-arm selector changed from `startup_arg != 0` to `fabric_boot::active()`; observer's role count corrected from 2 to 1. | Every spawned child, not only the bootstrap instance, can determine which graph it is part of; a participant's requested capability count matches what the broker delegates. |
| `fabric-service.rs` | `notification_slots` returns a sentinel instead of failing when a pair is undeclared; `boot_graph` pre-marks the declared proxy answered; restored `[fabric] idle: parked on control endpoints`. | A boot-mode role provisioned without driving samples legitimately declares no Notification pair; a declared non-participant does not block the broker's completion. |
| `slime-root/src/main.rs` | `MAX_GRAPH_ITERATIONS`'s fatal also requires `!healthy_emitted`. | A graph whose declared success state is every required task parked forever is not a wedge merely because it never reaches `live == 0`. |
| `check-sel4-boot-plane.py` | `boot()` reads through a quiet settling period after the healthy record; `CHAINS`, `EXPECTED_INIT_CHILDREN`, `EXPECTED_ROLES`, `EXPECTED_IDLE_WITHOUT_ROLE` rewritten for the restored single-spawning-parent composition; five racy cross-task stream markers moved to order-independent membership checks. | The gate observes the graph's actual convergence, and asserts every required marker's presence rather than one specific scheduling interleaving. |
| `check-sel4-gate-controls.py` | Boot-plane transcript synthesis and pinned marker count (46 → 30) updated to match. | The mutation-testing harness's synthetic baseline matches what the real gate now requires. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future change reintroduces an empty spawn-grant vector or worker-spawned-by-fabric shape | `just sel4_boot_check` | `SLIME_GRAPH spawn refused … ungranted`, or `check_composition`'s single-spawning-parent assertion |
| An unrelated broker record gets misread as a role reply | `just sel4_boot_check`, `just sel4_gate_control_check` | `declared participant was denied` with a garbage status, or a gate-control deletion mutation silently accepted |
| `startup_arg` gating creeps back into a shared component | `just sel4_boot_check`, `just sel4_visibility_check` | The three components fall through to standalone-plane logic; `fabric-observer`'s filtered-view assertions fail under boot |
| `MAX_GRAPH_ITERATIONS`'s relaxation masks a genuinely wedged graph | `just sel4_boot_check` and every other plane gate, all of which still require `healthy_emitted` before the relaxation applies | A wedged non-boot graph still FATALs, since it never certifies healthy |
| The gate's own marker tables drift from the real composition again | `just sel4_gate_control_check` | Pinned required-marker count mismatch, or a mutation accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_check` (3 repeated runs) | Pass; 30 markers across 5 chains, 21-slot layout, 19 tasks, 5 checked roles, 10 declared idles, none exited, identical across runs | Direct |
| `just sel4_boot_layout_check` | Pass; 24 plane layouts match their fixtures, including the unchanged 21-slot boot layout | Direct |
| `just sel4_gate_control_check` | Pass; 28 gates reject 1082 mutated transcripts and layouts | Direct |
| `just contracts_check` | Pass | Direct |
| `just generation_check` | Pass; two isolated builds produced byte-identical `generation.bin`/`boot-store.bin` | Direct |
| `just sel4_root_boot_check`, `sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check` | Pass (three initially reported false failures from concurrent-CMake-build contention across parallel `just` invocations sharing one build directory; each re-ran clean in isolation) | Direct |
| `just test_sel4_root` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- Decision: init spawns both bounded route workers itself, rather than
  keeping `fabric-service` as their spawner with a runtime handoff mechanism
  for the endpoints and supervision handles it cannot legally receive.
  Rationale: no legal path exists under the current capability model for a
  worker to receive its control endpoints or participant-supervision handles
  from anyone but its own spawner — an endpoint grant cannot cross a spawn
  boundary (`grant_crosses_spawn`), and an endpoint-kind minted binding is
  excluded from `nth_declared_capability`. Rejected alternative: add a
  runtime capability-transfer handoff from `fabric-service` to each worker
  after spawn, which would re-introduce a second, ad hoc authority-
  distribution mechanism alongside the generation's declarative one for no
  observable benefit.
- Decision: call and operation plane participants stay on `park_only`
  (no negotiated role request) rather than resurrecting the x86 oracle's
  runtime role-request protocol for them. Rationale: under the native model a
  participant's control endpoint is already its whole authority, generation-
  declared and root-installed before any task runs; a request/reply
  negotiation on top of that would prove a binding that already holds by
  construction. Rejected alternative: port `fabric_call_scenario::boot_park`'s
  `request_role` unchanged, which would compile against a protocol these
  participants no longer speak.
- Decision: the gate's `boot()` reads through a bounded quiet-settling window
  after the healthy record rather than changing the root to gate the healthy
  print on a separate "idle" tracking mechanism. Rationale: the root's
  healthy print is shared by every seL4 plane gate; changing its semantics
  risks every one of them. The narrow, boot-plane-specific read-protocol
  change carries the same risk only for this one gate. Rejected alternative:
  track genuine per-task idle state in the root and gate the print on it,
  which touches supervision/scheduling code exercised by every other passing
  gate for a property only this one plane's timing exposed.

## Open risks and follow-ups

- [ ] C8.10's roadmap section still describes the bounded route-worker
  partition and wait-source bounds as declarative facts checked by
  `just data_fabric_profile_check`; that half was unaffected by this work and
  was not re-verified beyond its own already-passing gate.
- [ ] The seL4 boot storm of `<<seL4 [decodeCNodeInvocation…]: CNode
  Copy/Mint/Move/Mutate>>` kernel invocation errors during every init
  construction, on every seL4 plane including unmodified `master`, is not
  caught by `FAILURE_MARKERS`' `r"<<seL4\(CPU 0\) \[decodeInvocation"` pattern
  (an exact-prefix mismatch against the kernel's actual
  `decodeCNodeInvocation` label). Confirmed pre-existing and identical on
  master; out of scope for B55. Filed for a future backlog item — a real
  kernel-level condition is currently silently tolerated by every seL4 gate.

## Artifacts and provenance

- Focused report: none; the investigation log above is the decisive chain.
- Raw transcript: none retained.
- Serial/debugger/model output: quoted inline under *Observable symptom* and
  *Investigation log*, captured directly from repeated `just sel4_boot_check`
  and manual QEMU boots of `build/slime-sel4-boot.elf`.
- Related roadmap item: [C8.10 in the core-runtime track](../../roadmap/02-core-runtime.md)
- Related backlog item: [B55](../../roadmap/00-backlog.md)
