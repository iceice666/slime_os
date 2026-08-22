# B70 closes: the last nine `include!` sites, and the stack the ceilings overflowed

| Field | Value |
|---|---|
| Date | 2026-08-22 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/fabric-graph/v1/{schema,gen_rust}.zt`, `contracts/syscall-abi/v1/schema.zt`, `boot-contracts/src/fabric_graph.rs`, `slime-root/src/{ipc,main,generation}.rs`, `components/runtime/src/{lib,syscall}.rs` + `syscall/sel4_transport.rs`, `components/bins/{fabric-service,fabric-publisher,fabric-publisher-b,fabric-subscriber,fabric-subscriber-b,fabric-call-worker,fabric-op-worker,spawn-service,dango}/`, `components/lib/src/{call_broker,operation_broker,matrix_broker,visibility_broker,fabric_occupancy_trace}.rs`, `components/build-support/src/lib.rs`, `scripts/build/build-generation.py`, `scripts/check/check-{data-fabric-profile,fabric-manifest,component-crate-split,component-sdk-out-of-tree}.py` |
| Roadmap | B70, CP2 |
| Gates | `just contracts_check`, `just generation_check`, `just data_fabric_profile_check`, `just fabric_manifest_check`, `just component_crate_split_check`, `just runtime_binding_resolution_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_boot_check`, `just sel4_traffic_check`, `just sel4_fault_check`, `just sel4_saturation_check`, `just sel4_trace_check`, `just sel4_fabric_aggregate_check`, `just sel4_dango_check`, `just sel4_spawn_check`, `just sel4_component_graph_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all` |
| Trigger | B70's remaining surface after the boot-action query landed: nine `include!` sites over `fabric_profile.rs`, `command_profile.rs`, and `dango_profile.rs` |
| Baseline | Nine component sources compiled a `build.rs`-private, manifest-derived constant table into their own images; `render_fabric_profile_rust` emitted a per-plane Rust profile the builder pointed every fabric component at |

## Summary

B70's exit clause — "no component source file `include!`s a `build.rs`-private,
manifest-derived constant table" — is met. The count is zero: every remaining
`include!` match in `components/` is prose in a doc comment describing the
mechanism that used to exist. `render_fabric_profile_rust`, the
`SLIME_DATA_FABRIC_PROFILE` handoff, `emit_fabric_profile`, both command-table
generators, `components/build-support`'s whole manifest parser, and the
checked-in `default_fabric_profile.rs` are deleted; `components/build-support`
is now `configure()` alone and reads no manifest at all.

What replaced them splits three ways rather than one. Per-generation *facts* —
the trace-sink shape — became fields of the authenticated `fabric-graph` header
and are queried at run time. Per-generation *ceilings* were already queryable
through `RuntimeLimits`. Structural *storage bounds* moved into
`contracts/fabric-graph/v1` as published constants, because they size fixed
arrays at compile time and no query can retire them.

The interesting part was not the migration. Sizing those arrays from contract
ceilings rather than per-plane constants overflowed the 64 KiB component stack
in three brokers, and it did not present as a stack overflow: `.data` sits
directly below the stack in these images, so `fabric-service` read a corrupted
`static` and refused its own factory capability, while `fabric-op-worker` faulted
executing at `0xfffffffffffffffd`.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-graph/v1/schema.zt` | Format 2 → 3, header 156 → 164 bytes, gaining `trace_depth` and `trace_overflow`; new query ids 24/25 in both `queryFields` and `limitQueryFields` | The sink shape a worker must honour is a field of the authenticated resource, not a constant a build script rendered per plane |
| `contracts/fabric-graph/v1/schema.zt` | New published ceilings `limitTraceDepth`, `traceTerminalReserve`, `traceOverflowSaturate`, `frameCapacity`, `maxRoleParticipants` | A component's fixed arrays and the graphs it may be handed agree by construction, stated once in the contract |
| `boot-contracts/src/fabric_graph.rs` | `GraphLimits`/`RuntimeLimits` gain the two trace fields; `validate_declared_limits` refuses a depth past `LIMIT_TRACE_DEPTH`, at/below `TRACE_TERMINAL_RESERVE`, or an unimplemented overflow, and bounds `publishers`/`subscribers` by `MAX_ROLE_PARTICIPANTS` | A graph naming a sink or a per-role fan-out no component could build is refused at decode rather than discovered by the worker that tried |
| `boot-contracts/src/fabric_graph.rs` | `RuntimeLimits::trace_sink_depth()` / `trace_overflow_is_saturate()` | Three workers stop restating the same two comparisons against the same two constants |
| `slime-root/src/ipc.rs` | The two new ids answer through `graph_query`'s existing holder-or-visible-participant gate | A participant may read the ceilings it admits against; nothing else widened |
| `contracts/syscall-abi/v1/schema.zt` | New self-scoped `SPAWN_BUDGET` (label 42), gated on `SERVICE_SPAWN` | `spawn-service` and `dango` read their own declared budget instead of compiling in another component's |
| `components/bins/*`, `components/lib/*` | Nine `include!` sites deleted; boot action via `generation_composition::is`, ceilings and trace depth via `graph_query`, slots via `resolve_binding` by declared name, QoS/history from decoded graph rows | A component's behaviour is selected by the generation it runs inside, not by the manifest a build script happened to see |
| `components/bins/fabric-service/src/main.rs`, `components/lib/src/{call,operation}_broker.rs` | The large fixed tables moved into claim-once `static mut` storage; the call and operation arms of `fabric-service::main` extracted behind `#[inline(never)]` | A broker's storage no longer lands in a caller's stack frame; every plane stops paying for brokers it does not run |
| `components/lib/src/call_broker.rs` | `call_deadline_ns` resolved once in `verify_graph` and cached | Admission and retry stop staging a graph read through the one transfer window the call path uses to move capabilities |
| `components/bins/spawn-service/src/main.rs` | A root spawn refusal answers `STATUS_SPAWN_REFUSED` with the root's code in `detail` | "your command is not declared" and "the root refused a spawn I authorized" stay distinguishable |
| `scripts/build/build-generation.py` | No Rust profile rendered or exported; `FABRIC_FRAME_CAPACITY` reads the contract; per-role ceilings enforced; header carries the trace fields | The builder and the decoder enforce one set of bounds, and no image is parameterized by which plane built it |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A graph declares a sink no worker can build | `just contracts_check` (host test `declared_trace_sink_outside_the_contract_fails_closed`), `just data_fabric_profile_check` | Decode answers `Impossible`; the builder refuses the manifest |
| A graph declares more stream participants than a broker sizes for | `declared_role_counts_above_the_broker_ceiling_fail_closed`, `just data_fabric_profile_check` | Decode answers `Impossible`; `fabric graph: limit subscribers exceeds the contract ceiling` |
| The builder's encoded header and the manifest disagree about the sink | `just fabric_manifest_check` | `built graph does not carry the manifest's declared trace depth` |
| A component regrows a private manifest derivation | `just component_crate_split_check` (`ALLOWED_BUILD_CALLS` is now `{configure}`) | `build.rs calls unknown helpers` |
| A second consumer aliases a claim-once table | the claimed-flag `fail()` in each claim function | The task exits rather than sharing storage |
| A broker's frame regrows past the stack | every fabric plane gate | The observed failure mode: a corrupted `static` or a VirtualMemory Execute fault |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check`, `sel4_qos_check`, `sel4_visibility_check`, `sel4_matrix_check` | pass | Direct |
| `just sel4_call_check`, `sel4_operation_check` | pass | Direct |
| `just sel4_boot_check`, `sel4_traffic_check`, `sel4_fault_check`, `sel4_saturation_check` | pass | Direct |
| `just sel4_trace_check`, `sel4_fabric_aggregate_check` | pass | Direct |
| `just sel4_dango_check`, `sel4_spawn_check`, `sel4_component_graph_check` | pass | Direct |
| `just contracts_check`, `generation_check`, `data_fabric_profile_check`, `fabric_manifest_check`, `system_spec_check`, `component_spec_check` | pass | Direct |
| `just component_crate_split_check`, `runtime_binding_resolution_check`, `sel4_gate_control_check` | pass | Direct |
| `just test_sel4_root` | 131/131 across 15 modules | Direct |
| `just test_host`, `fmt_check_all`, `lint_all`, `ruff`, `typos`, `machete` | pass | Direct |
| B70's exit clause: `include!` of a manifest-derived table in `components/` | zero sites; every match is doc-comment prose | Direct |

## Decisions

- **Decision:** the trace sink's depth and overflow become *header fields* of
  `contracts/fabric-graph/v1`, bumping the format to 3.
- **Rationale:** [`devlog/2026-08-19-fabric-graph-read-scope/`](../2026-08-19-fabric-graph-read-scope/index.md)
  established that
  `traceDepth` is a per-graph scalar reachable no other way, and that carrying it
  would need "a schema field, a regen, root staging, and a runtime read" — judged
  not worth doing then *because it closed zero `include!` sites*. That arithmetic
  changed once the boot action closed: the depth became the last symbol holding
  four participant files, so the same four-part cost now closes four sites.
- **Rejected alternative:** compiling the published ceiling. The trace gate
  asserts `capacity == declared_depth(fixture)` exactly, over three planes
  declaring 16, 64, and 64, so no single compiled constant satisfies all three.

- **Decision:** storage bounds that size fixed arrays move into the contract as
  published constants (`frameCapacity`, `maxRoleParticipants`), not into a query.
- **Rationale:** B70's clause constrains where a table *lives*, not whether a
  value is compile-time. A constant a component reads from the shared contract is
  reachable by an out-of-tree crate; one a build script renders per plane is not.
- **Rejected alternative:** sizing from the existing ceilings
  (`MAX_PARTICIPANTS` = 32, `LIMIT_IN_FLIGHT` = 32). Measured: that is what
  overflowed the stack. Two `[Option<Publisher/Subscriber>; 32]` arrays carrying
  a 64-entry `StreamHistory` each need roughly half a megabyte.

- **Decision:** `maxRoleParticipants` = 4 bounds `publishers`/`subscribers` only;
  `clients`/`servers` keep the table bound.
- **Rationale:** a stream broker holds one ring record with a full history per
  edge, so its storage scales with the declared per-direction count. A
  request/response broker holds one small record per in-flight call, so its
  storage does not — and `sel4-matrix.zti` legitimately declares seven clients.
  Bounding all four at 4 refused a real fixture.

- **Decision:** the oversized broker tables move to claim-once `static mut`
  rather than raising any declared stack.
- **Rationale:** the storage is per-task and single-owner by construction — one
  broker per worker task — so a static with a claimed-flag states exactly that,
  and a second claim fails loudly instead of aliasing. Raising the stack would
  have hidden the sizing mistake rather than fixed it.

- **Decision:** `spawn-service` answers a root refusal with its own status.
- **Rationale:** `ERR_BAD_CAP` is `STATUS_NOT_ALLOWED` on that wire, so
  forwarding the root's code told a client its command was undeclared when the
  service had already resolved and authorized it — and `dango` printed
  `resolve-denied` for it. The root's code now travels in `detail`, where it is
  diagnostic rather than policy.

## Open risks and follow-ups

- [ ] `components/lib/src/{call,operation}_broker.rs` carry comment prose
      rewritten during this task after two subagents each `git checkout`-reverted
      uncommitted work and reconstructed it. A code-only comparison against
      `HEAD` found no dropped logic, and every plane gate passes, but the prose is
      not the original author's and deserves a read.
- [ ] `MAX_PENDING_DELIVERIES` in `operation_broker` is still derived from
      contract ceilings (22,528 bytes of `.bss`), where every declared graph needs
      a fraction of it. The stack cost is gone; the image cost is not.
- [ ] `maxRoleParticipants` = 4 is the widest any current fixture declares. A
      composition wanting a fifth publisher must raise the contract constant and
      re-measure the stack, which is the trade this makes explicit rather than
      the ceiling it hides.
- [ ] The two reverts above were preventable: uncommitted work in a shared tree
      has no recovery path. Worth a convention that agents stage before probing.

## Artifacts and provenance

- Focused report: none; the decisive measurements are quoted inline.
- Raw transcript: none retained.
- Serial/debugger/model output: the two diagnosed failures are quoted inline —
  `SLIME_GRAPH buffer create refused task=6 class=ungranted` against a `static`
  reading `1075663531` where the image initialized `0xffffffff`, and
  `SLIME_GRAPH FAIL required instance fabric-op-worker fault kind=VirtualMemory
  { access: Execute, status: 64 } instruction=Some(1)`. Frame sizes measured by
  disassembly: `fabric-op-worker::main` 62,960 → 12,256 bytes;
  `fabric-call-worker::main` 48,032 → 8,944.
- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2 in the component
  platform track](../../roadmap/10-component-platform.md)
- Predecessor: [`devlog/2026-08-21-b70-boot-action-query/`](../2026-08-21-b70-boot-action-query/index.md)
