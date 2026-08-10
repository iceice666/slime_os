# B41 — a console endpoint per process

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Monitoring |
| Scope | `scripts/build/build-generation.py`, `scripts/check/check-generation.py`, `boot-contracts/src/generation.rs`, `slime-root/src/{main,task}.rs` |
| Roadmap | B41 |
| Gates | `just sel4_boot_check`, `just sel4_capability_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root` |
| Trigger | B41: `DebugWrite` and console-adjacent control share the root's badged endpoint and dispatcher with lifecycle, storage, and fabric traffic. |
| Baseline | One endpoint object per system, one dispatcher loop, `Mediation::RootService` for `DebugWrite` and `InputRead`. |

## Summary

Half of B41. Every process now holds a console/debug endpoint object distinct
from the root service endpoint, declared in the plan, resolved by the decoder,
minted by the root, and checked by the B40 CSpace audit. Nothing routes to it
yet: the root has one blocking dispatcher, and the `DebugWrite` handler reads
state that dispatcher owns. `DebugWrite` and `InputRead` remain on the
universal ABI, so B41's exit condition is unmet and the entry stays open.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `build-generation.py` | A `SERVICE_CONSOLE` binding and an endpoint object per process | Console authority is declared, not ambient |
| `check-generation.py` | Console slot pinned; write-only enforced; unknown service discriminants refused | The host twin refuses what the decoder refuses |
| `boot-contracts` | `ChildSlotPlan::console`, refusing a duplicate or receive-capable binding | The root reads the slot from the plan |
| `slime-root/src/task.rs` | `ChildSlots::console` minted write-only; occupancy audit and `validate` cover it | The capability exists in the child's CSpace and is audited |

### Why slot 32

A grant's binding slot is the *component's own* numbering and starts at 0, so
every low slot is already spoken for in a migrated fixture — the boot plane's
`init` binds 0 through 5. A fixed console slot has to clear all of them. 32 is
the first power of two above the highest slot any seL4 fixture declares (22),
which leaves child CNodes a round six bits. The first attempt used slot 4 and
B40's CSpace audit refused it on the next boot with
`CSpaceMismatch { slot: 4, occupied: true }`, which is the audit doing exactly
what it was built for.

### Why write-only

Every process shares one console dispatcher. A process that could receive on
its console endpoint would dequeue another process's output before the console
saw it — the same confinement the root service endpoint needs, and for the same
reason. Enforced in the builder, the host checker, and the decoder.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A plan hands a process a receive-capable console endpoint | `just generation_check` | `BadServiceBinding` |
| The console slot collides with declared authority | `just sel4_boot_check` | `CSpaceMismatch { slot: 32, occupied: true }` |
| The console capability silently stops being installed | `just sel4_capability_layout_check` | the audit's missing-slot arm |
| The slot drifts back under grant numbering | `just test_sel4_root` | `the_console_slot_clears_grant_numbering` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_check` | Pass | Direct |
| `just sel4_capability_layout_check` | Pass | Direct |
| `just sel4_input_check`, `sel4_dango_check`, `sel4_directory_check`, `sel4_storage_check` | Pass | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_reclamation_check` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just test_sel4_root` | 142/142 | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |

Routing `debug_write` to the new endpoint was tried and reverted: with nothing
receiving, every component blocks on its first debug line and the boot plane
hits its 300s bound. That is the expected result, and it is why the client
half is not in this change.

## Decisions

- **Decision:** provision the endpoint before routing to it.
  **Rationale:** the two halves fail differently. Provisioning is checkable
  against the plan today — the capability is present, write-only, audited, and
  every gate still passes. Routing changes what 62 files do at runtime and
  needs a receiver first. Landing them together would have made a hang
  indistinguishable from a declaration defect.

- **Decision:** the console slot is fixed, not per-plane.
  **Rationale:** `components/runtime` resolves the root endpoint from a
  constant and would resolve this one the same way. A per-plane slot would
  reintroduce exactly the drift B40's service-slot pin closed.

## Open risks and follow-ups

- [ ] Nothing receives on the console endpoint. The root's dispatcher is a
      single blocking `ipc::recv_request(endpoint)`, and the `DebugWrite`
      handler reads the caller's transfer window and the root's single
      `ScratchPage` — both owned by that dispatcher. A console thread needs its
      own scratch mapping and a rule for concurrent window reads. Two `SAFETY`
      comments and eleven statics in `main.rs` rest on the current
      single-threaded assumption.
- [ ] A bound notification does not substitute: it signals, and the console
      still needs its own receive to carry the payload.
- [ ] `InputRead` is untouched. It is a read with a reply, so it needs the
      round trip `DebugWrite` does not, and its plane already passes through
      the root.
- [ ] The gate-control mutation B41's exit condition asks for — one that
      restores a root fallback and fails — cannot be written until the fallback
      is gone.

## Artifacts and provenance

- Related roadmap item: `roadmap/00-backlog.md` B41.
- Prerequisites, both landed first: [`devlog/2026-08-10-b41-dango-plane-declarations/`](../2026-08-10-b41-dango-plane-declarations/index.md)
  and [`devlog/2026-08-10-probe-plane-run-tokens/`](../2026-08-10-probe-plane-run-tokens/index.md).
