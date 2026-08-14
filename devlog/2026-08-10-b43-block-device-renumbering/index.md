# B43 — a component's second block device was silently its first

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/main.rs`, `contracts/generation/v1/fixtures/sel4-{store,rollback,recovery,transfer}.zti`, `components/bins/src/bin/sel4-recovery-probe.rs`, `scripts/check/check-sel4-{storage,transfer}-plane.py` |
| Roadmap | B43 |
| Gates | `just sel4_transfer_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just sel4_recovery_plane_check`, `just sel4_storage_check`, `just sel4_device_check` |
| Trigger | B43 names six gates; four were red before any B43 work. |
| Baseline | `sel4_store_check`, `sel4_rollback_check`, and `sel4_transfer_check` timed out; `sel4_recovery_plane_check` did not compile. |

## Summary

Four of B43's six gates were red. Three were the run-token pattern already seen
on every other probe plane. The fourth was a real capability defect: block
devices are renumbered per binding on the boot-graph path and were not on the
spawn path, so a component holding two device capabilities saw both resolve to
device 0. The transfer plane is the only one holding two, and it had been
reading its manifest off the wrong disk. All six gates now pass — but B43's
exit condition is still unmet, because `BlockTransact` and `StoreTransact`
remain labels on the universal dispatcher.

## Observable symptom

- Command: `just sel4_transfer_check`
- Expected: the manifest decodes and the generation crosses.
- Observed: `[sel4-transfer-probe] manifest error=truncated`, after sixteen
  sectors were served successfully (`block served task=1 op=1 lba=1070
  status=0 sectors=1`).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Manifest at absolute LBA 1070 (`STORE_FIRST + 1030`) matches what the probe reads | The address is right |
| 2 | Host read of those bytes: magic `SLIMETR\0`, `total_len` 1030 at offset 232 | The disk content is right |
| 3 | Probe's slot constants, LBA constants, and `read_sector` match the recovery probe's | The component is right |
| 4 | Instrumented the probe: `declared=0 read=8192` | The guest reads zeros from a populated sector |
| 5 | `SLIME_GRAPH block served` carries no device index | Cannot tell which disk answered |
| 6 | Added the index: **all 38 reads `device=0`** | Both capabilities resolve to one device |
| 7 | `declared_resource` returns `Block { device: 0 }`; boot-graph renumbers at `main.rs:2422`, spawn path did not | Root cause |

## Root cause

`declared_resource` answers `Block { device: 0 }` for every block grant, with a
comment saying the caller renumbers — "a component declared several gets them
renumbered by the caller, which is the only place that knows how many it has
already placed." The boot-graph binding loop does exactly that. The spawn
path's self-loop install, added when self-loop grants started being installed
at all, did not.

The transfer plane declares two device grants — a writable receiver and a
read-only source — and is the only plane that declares two. Both resolved to
device 0, so every read went to the receiver, and the manifest sectors it read
were the receiver's zeros rather than the source's manifest.

Nothing failed loudly. The reads succeeded, the status was 0, the sector count
was 1. The only symptom was a decode error four layers up.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/main.rs` | Self-loop install renumbers block devices per binding | A component's *n*th device grant is its *n*th device |
| `slime-root/src/main.rs` | `block served` carries `device=` | The record says which device answered |
| `sel4-{store,rollback,recovery}.zti` | Run token and idle instance declared | The planes' probes can spawn |
| `sel4-transfer.zti` | Idle instance declared | The plane's idle claim has something to observe |
| `sel4-recovery-probe.rs` | Mutable borrow before `try_into` | `&mut [u8]` reaches the `&mut [u8; N]` impl |
| `check-sel4-transfer-plane.py` | Asserts `source-state travel`, the marker the probe emits | The assertion can match |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Device renumbering regresses on the spawn path | `just sel4_transfer_check` | `manifest error=truncated` |
| The wrong device answers | any storage gate | `block served … device=` names it |
| A probe plane's run token goes undeclared | `just sel4_store_check` etc. | `spawn preflight … reason=declared-count` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_transfer_check` | Pass; reads split `device=0` ×10, `device=1` ×17 | Direct |
| `just sel4_store_check`, `sel4_rollback_check`, `sel4_recovery_plane_check` | Pass | Direct |
| `just sel4_device_check`, `just sel4_storage_check` | Pass | Direct |
| `just sel4_boot_check`, `sel4_spawn_check`, `sel4_supervision_check`, `sel4_dango_check`, `sel4_input_check`, `sel4_directory_check`, `sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_reclamation_check`, `sel4_capability_layout_check` | Pass | Direct |
| `just contracts_check`, `just generation_check`, `just test_host` (7), `just test_sel4_root` (142) | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- **Decision:** add the device index to the marker before fixing anything.
  **Rationale:** every static check said the code was correct — right address,
  right bytes on disk, right constants, an implementation identical to a probe
  that works. The record could not distinguish which of two devices answered,
  which is exactly what multi-device selection claims, so the missing field was
  both the diagnostic gap and a coverage gap.

- **Decision:** B43 stays open despite all six gates passing.
  **Rationale:** its first clause is that a block or store request cannot be
  issued without the declared service capability. `BlockTransact` and
  `StoreTransact` are still labels on the universal dispatcher, so that is
  false. Counting green gates as the exit condition would be reading the
  evidence line and ignoring the claim.

## Open risks and follow-ups

- [ ] `BlockTransact` and `StoreTransact` remain on the universal dispatcher.
      Moving them shares B41's blocker.
- [ ] Two single-threaded routes for that blocker were tried and rejected:
      `seL4_NBRecv` polling starves the console and still blocks the client,
      and `seL4_NBSend` drops messages when nothing is receiving. Inline-only
      routing does not help either — 548 of 643 `debug_write` sites exceed the
      16-byte inline bound.

## Artifacts and provenance

- Kernel/library source consulted: `deps/rust-sel4/crates/sel4/src/syscalls.rs`
  (`nb_send`, `nb_recv`).
- Related roadmap item: `roadmap/00-backlog.md` B43.
