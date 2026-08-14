# B50 — a minted endpoint named an object nobody could create

| Field | Value |
|---|---|
| Date | 2026-08-14 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/{schema.zt,fixtures/*.zti}`, `boot-contracts/src/generation.rs`, `scripts/build/build-generation.py`, `scripts/check/check-generation.py`, `slime-root/src/main.rs`, `components/bins/src/bin/{init,directory-probe,sel4-*-probe,sel4-generation-*,sel4-filesystem-service,dango}.rs`, eleven plane gates |
| Roadmap | B50, B46 |
| Gates | `just generation_check`, `just sel4_gate_control_check`, `just contracts_check`, `just sel4_spawn_check`, `just sel4_supervision_check`, `just sel4_generation_check`, `just sel4_filesystem_check`, `just sel4_directory_check`, `just sel4_input_check`, `just sel4_storage_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just sel4_recovery_plane_check`, `just sel4_transfer_check` |
| Trigger | `SLIME_GRAPH spawn preflight count … requested=0 parent=0 minted=N` on ten plane gates after B50's `endpointCreate` deletion (`ecfc99d`) |
| Baseline | Before the native IPC cutover, an endpoint was a root-owned logical channel a component created with `EndpointCreate`, so deferring its object identity to a runtime minter was meaningful |

## Summary

A `MintedBinding` of kind `endpoint` named an object nobody could supply. The
record exists to defer *object identity* while fixing owner, holder, slot, and
rights ceiling — which was exactly right when a component could create a channel
at runtime. B46 replaced that with a generation-owned seL4 Endpoint the root
materializes and installs into both declared ends, and B50 deleted the
`endpointCreate` right that had authorized creation; but 63 minted endpoint
bindings survived across eleven fixtures, and `preflight_spawn_grants` counted
every one in the total a parent must satisfy. Ten plane gates failed at `spawn
refused … ungranted` for a count their `init` had no way to meet. Every fixture
is now converted to authority its plane can actually transfer, `minted` is gone
from `CapabilityGrant` entirely, and two root defects the conversion exposed are
fixed. Twenty-four gates pass, including all three of B50's exit-condition gates.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Six probe fixtures (`sel4-{storage,store,rollback,recovery,transfer,directory}.zti`) | Run token declared as an ordinary grant, with a loopback endpoint for the idle instance | The discriminator is a declared edge, and the idle instance's receive returns rather than blocking |
| Six probe components | `startup_arg == 0` replaced with a bounded non-blocking receive on the run token | A spawned instance can tell itself from an autostarted one; the root delivers a nonzero boot action only to the bootstrap instance, so `startup_arg` could not |
| `sel4-spawn.zti` | Seven minted endpoints replaced by two declared control endpoints plus seven narrowed transferable directory views; both children non-autostart | A parent hands its child capabilities at spawn, using authority it holds *and may pass on* |
| `sel4-generation.zti`, `sel4-filesystem.zti` | Minted RPC endpoints become ordinary grants; the service learns its client's death from a supervision handle or a declared close edge | A native Endpoint carries messages; supervision carries death |
| `sel4-boot.zti` | 41 minted endpoint bindings and 16 minted grants converted to declared grants with a binding per end | The full-graph plane declares its control edges the same way every other plane does |
| `contracts/generation/v1/schema.zt`, `boot-contracts/src/generation.rs`, `scripts/build/build-generation.py`, `scripts/check/check-generation.py` | `CapabilityGrant.minted` and `GRANT_MINTED` deleted; `flags` still refuses unknown bits | A format field has a producer, or it does not exist |
| `slime-root/src/main.rs` | `grant_crosses_spawn` shared by the spawn count and the spawn ordering | A declaration the count skips is not numbered in the order requests are paired against |
| `slime-root/src/main.rs` | `supervision_derive` adds `RIGHT_TRANSFER` to the derived handle | Derivation is the "I intend to hand this on" operation, so its result can be handed on |
| `sel4-filesystem-service.rs`, `directory-probe.rs`, `dango.rs` | Transferred capabilities claimed with `capability_import` instead of read from `caps[0]` | The received-capability array carries native Endpoint handles only (B46) |
| `init.rs`, `check-sel4-supervision-plane.py` | B25's derive scenario restored: derive, cross the allocation bound, query both handles | A derived handle outlives both its task and the source handle it came from |
| `scripts/check/check-sel4-gate-controls.py` | Marker-count pins updated for eleven gates, with the reason each moved | A gate that silently lost a marker lost coverage |
| `scripts/build/build-generation.py` | `channel_aliases` deleted | A generated constant no component reads is not a contract |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A `minted` endpoint reappears in a fixture | `just contracts_check` | The builder has no flag to encode and the decoder refuses the unknown bit as `UnknownRequiredFlags` |
| The spawn count and the spawn ordering diverge again | `just sel4_generation_check` | `spawn preflight count … requested=1 parent=3` — the plane declares both a self-loop capability and a minted binding, which is the exact shape that was unspawnable |
| A probe's discriminator regresses to `startup_arg` | `just sel4_storage_check` and the four sibling probe gates | Two instances complete the scenario, or none does; each gate counts completions exactly |
| A gate loses a marker to a deleted mechanism | `just sel4_gate_control_check` | The pinned count no longer matches the gate's own `REQUIRED_MARKERS` |
| A derived supervision handle stops being transferable | `just sel4_dango_check` | `spawn-service`'s reply delegate fails and it exits 1 |
| B25's derive property is dropped again | `just sel4_supervision_check` | The child is collected once rather than twice — one handle, not two |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just generation_check` | PASS | Direct |
| `just sel4_gate_control_check` | PASS | Direct |
| `just contracts_check` | PASS | Direct |
| `just sel4_boot_layout_check` | PASS after `just sel4_boot_layout_bless` | Direct |
| `just sel4_spawn_check` | PASS — was admission-refused | Direct |
| `just sel4_supervision_check` | PASS — was missing the derive marker | Direct |
| `just sel4_generation_check` | PASS — was admission-refused | Direct |
| `just sel4_filesystem_check` | PASS — was admission-refused | Direct |
| `just sel4_directory_check`, `just sel4_input_check` | PASS — both were admission-refused | Direct |
| `just sel4_storage_check`, `sel4_store_check`, `sel4_rollback_check`, `sel4_recovery_plane_check`, `sel4_transfer_check` | PASS — all five were admission-refused | Direct |
| `just sel4_root_boot_check`, `sel4_channel_check`, `sel4_crossing_check`, `sel4_loan_check`, `sel4_sample_check`, `sel4_stream_check`, `sel4_qos_check`, `sel4_visibility_check`, `sel4_reclamation_check`, `sel4_powerbox_check` | PASS — no regression | Direct |
| `just fmt_check_all`, `just lint_all`, `just test_sel4_root`, `just test_host` | PASS | Direct |
| `just sel4_stress_check` | FAIL, and failed at HEAD before this change — now one layer deeper: the plan-budget marker it never reached is satisfied, and it stops at `never reclaimed to zero live tasks` | Direct |
| `just sel4_dango_check` | FAIL — the fixture is converted and both the control endpoints and the supervision handle now cross correctly (`sysinfo` runs through the profile, `dango>` prompts); `dango` then exits 1 in its scripted-input loop | Direct |

## Decisions

- Decision: convert each fixture to the authority its plane can transfer, rather
  than making the root tolerate a minted endpoint.
- Rationale: a `MintedBinding` of kind `endpoint` is unsatisfiable, not merely
  unused. Teaching `preflight_spawn_grants` to skip it would have preserved a
  declaration no party can honour — residue with a special case around it, which
  is what B50 exists to remove. The first attempt did exactly that and was
  reverted.
- Rejected alternative: declaring the spawn plane's seven minted endpoints as
  ordinary grants and nothing else. That admits and boots the graph, and then
  deletes the property under test — `check-sel4-spawn-plane.py` asserts the grant
  count *at the spawn marker*, and the plane's claim is that a parent hands a
  child capabilities at spawn. Six narrowed transferable directory views keep the
  claim while crossing authority the model can actually move.

- Decision: `supervision_derive` adds `RIGHT_TRANSFER` to the derived handle.
- Rationale: `spawn` returns a handle carrying `RIGHT_SUPERVISE` alone, and a
  service that must pass its child's outcome to a client has no other way to make
  a transferable copy. Derivation *is* the "I intend to hand this on" operation;
  the derived handle names the same task and adds only the right to move it,
  which the deriver already exercises by asking.
- Rejected alternative: granting services a transferable handle at spawn. The
  handle cannot exist before the task it names, so there is nothing to grant.

- Decision: the idle instance of a doubled executable holds a *loopback* endpoint
  rather than an empty slot.
- Rationale: with an empty slot the receive is refused and the component exits
  immediately, which reads as "no run token" but tests nothing. With a loopback
  nobody sends on, the idle instance blocks its bounded wait and concludes the
  same thing from arrival rather than presence — which is what actually
  distinguishes the two instances.
- Rejected alternative: keeping `startup_arg`. The root delivers a nonzero boot
  action only to the bootstrap instance, so every other instance reads zero and
  the guard was unreachable.

- Decision: each probe gate asserts its `idle without a run token` line by
  presence, not position.
- Rationale: the idle instance concludes it holds no peer only after a bounded
  wait, so its line lands wherever the scheduler puts it — including after the
  plane's terminal marker. Ordering it would assert a scheduling accident.

## Open risks and follow-ups

- [ ] `just sel4_dango_check` — `dango` exits 1 in its scripted-input loop
      (`input_read` returning an error, `dango.rs:53`). The minted-binding
      cutover is complete for this plane and observed working; the remaining
      failure is a separate scripted-input defect.
- [ ] `just sel4_stress_check` — pre-existing failure, now one layer deeper:
      `the graph never reclaimed to zero live tasks`. Untouched by this change.
- [ ] The `endpoint-factory` layout *role* remains a numbered entry in a
      generated contract, as recorded in B50's earlier `endpointCreate` deletion.
      Removing it renumbers every role after it.

## Artifacts and provenance

- Focused report: none; the decisive chain is in *Changes* and *Decisions*.
- Raw transcript: none retained. Every claim above is a gate result reproducible
  by its named `just` target.
- Serial/debugger/model output: quoted inline where a marker is the evidence.
- Related roadmap item: [B50](../../roadmap/00-backlog.md), building on
  [B46](../2026-08-13-b46-native-ipc-completion/index.md) and
  [B50's `endpointCreate` deletion](../2026-08-13-b50-endpoint-create-deletion/index.md).
