# Probe planes — the run token, the idle instance, and slot zero

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-{input,directory,storage,transfer}.zti`, `slime-root/src/{main,graph}.rs` |
| Roadmap | B41 |
| Gates | `just sel4_input_check`, `just sel4_directory_check`, `just sel4_storage_check`, `just sel4_dango_check` |
| Trigger | B41's exit condition names `just sel4_input_check`, which was red before any B41 work. |
| Baseline | `sel4_input_check` exceeded its 180s bound; `sel4_directory_check` and `sel4_storage_check` failed on a missing idle marker. |

## Summary

Four plane gates were red for three shared reasons, none of them in the
behaviour the gates test. Two were undeclared authority in fixtures the v5
cutover never migrated; the third was a root defect that made a capability
invisible to the component it had just been handed. `sel4_input_check`,
`sel4_directory_check`, `sel4_storage_check`, and `sel4_dango_check` now pass.

## Observable symptom

- Command: `just sel4_input_check`
- Expected: the scripted key session completes.
- Observed: `boot exceeded 180s`, preceded by
  `spawn preflight instance=sel4-input-probe reason=declared-count requested=1 bindings=0 minted=0`
  and `spawn refused task=0 slot=1 ungranted`.
- Then, once the spawn was admitted:
  `missing marker: the unconfigured instance parked without a run token`.
- Then: `missing marker: SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=input`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `drive_probe_plane` mints a run token and passes it at spawn | Undeclared authority crossing a spawn boundary |
| 2 | Every probe fixture declares one instance, `owner = init` | The idle half of the plane's claim had nothing to observe |
| 3 | `grep -rn "declared placed"` across the tree returns nothing | Three gates asserted output no code produced |
| 4 | Dango's composed launch refused with both caps transferred | Not a transfer failure |
| 5 | Bisected `valid_request`: `reject: cap zero` | A forwarded capability arrived as slot 0 |
| 6 | `main.rs:4102` allocated the receive slot from 0; every other runtime allocation uses 1 | Root cause of the dango failure |

## Root cause

**The run token was undeclared.** Every probe plane makes one claim: a
generation-declared device capability alone does not run a probe. The root
places that capability in the probe's own table, so an unconfigured copy holds
it too; what distinguishes the configured instance is a run token `init` mints
and passes at spawn. That token crosses a spawn boundary and so must be
declared — it is exactly what a `MintedBinding` is for — and it was not, so
spawn preflight refused the launch.

**The idle instance was undeclared.** The claim needs two instances of one
executable: the one `init` spawns with a token, and a root-launched copy that
parks. The fixtures declared only the first, so `[<probe>] idle without a run
token` could never appear.

**A received capability could land in slot 0.** The receive path allocated the
destination with `free_slot_from(0)`. That slot number is reported to the
receiver, and every protocol carrying one reads 0 as "no capability" — the
spawn request's `received_caps` among them, where `valid_request` requires the
first `capability_count` entries to be non-zero. A forwarded capability landing
there was invisible to the component it had just been given to. Every other
runtime slot allocation in the root already searched from 1.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-{input,directory,storage,transfer}.zti` | Run token declared as a `MintedBinding` | Authority crossing a spawn boundary is declared |
| `sel4-{input,directory,storage}.zti` | A second, root-owned instance with its own self-loop grants | The plane's claim has both halves to observe |
| `slime-root/src/main.rs` | Received capabilities allocate from slot 1 | A capability handed to a component is visible to it |
| `slime-root/src/main.rs` | `SLIME_GRAPH declared placed` emitted where the root installs a child's own declared authority | The record three gates asserted now exists |
| `slime-root/src/graph.rs` | `Resource::kind_name` | That record names the kind |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A minted run token goes undeclared again | `just sel4_input_check` | `spawn preflight … reason=declared-count` |
| A received capability lands where its holder cannot see it | `just sel4_dango_check` | the composed launch refused with caps present |
| The idle claim silently stops being tested | `just sel4_directory_check` | `missing marker: … idle without a run token` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_input_check` | Pass | Direct |
| `just sel4_directory_check` | Pass | Direct |
| `just sel4_storage_check` | Pass | Direct |
| `just sel4_dango_check` | Pass | Direct |
| `just sel4_boot_check`, `sel4_component_graph_check`, `sel4_root_boot_check`, `sel4_reclamation_check`, `sel4_capability_layout_check` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just test_sel4_root` | 140/140 | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- **Decision:** `SLIME_GRAPH declared placed` is emitted from the root's
  self-loop install path.
  **Rationale:** it records that a child's *own* declared authority reached it.
  Only the root can place those — the parent holds no copy to pass — so that is
  the only point at which it is observable. Three gates had asserted this
  record for some time with nothing emitting it.
  **Rejected alternative:** deleting the assertion as stale. It names a real
  property, and the property had no coverage.

- **Decision:** the idle instance is a second declared instance of the same
  executable, not a build flag or a component branch.
  **Rationale:** what the plane tests is a *generation* property — two
  instances, identical authority, different outcomes — so it belongs in the
  generation.

## Open risks and follow-ups

- [ ] `sel4_transfer_check` still fails, on `[sel4-transfer-probe] fail:
      manifest decode`. The run-token declaration was added to its fixture and
      admits cleanly; the remaining failure is in the probe's own decode path.
- [ ] `sel4-recovery.zti` has the same shape but no gate exercises it, so it
      was left alone rather than changed blind.
- [ ] B41 itself is untouched: `DebugWrite` and `InputRead` are still labels on
      the universal root dispatcher.

## Artifacts and provenance

- Related roadmap item: `roadmap/00-backlog.md` B41.
- Companion entry: [`devlog/2026-08-10-b41-dango-plane-declarations/`](../2026-08-10-b41-dango-plane-declarations/index.md).
