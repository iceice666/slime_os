# B44 — the generation and recovery labels were never reachable

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{ipc,main}.rs`, `components/runtime/src/syscall{,.rs}/sel4_transport.rs`, `components/bins/src/bin/init.rs`, `scripts/check/check-sel4-component-graph.py`, `contracts/generation/v1/fixtures/sel4-generation.zti` |
| Roadmap | B44 |
| Gates | `just sel4_generation_check`, `just sel4_boot_selection_check`, `just sel4_rollback_check`, `just sel4_recovery_plane_check`, `just sel4_transfer_check`, `just sel4_component_graph_check` |
| Trigger | B44: generation and recovery policy still entered the universal root dispatcher through `HealthConfirm`, `RecoveryReconstruct`, `GenerationTransact`, and `GenerationReceive`. |
| Baseline | Four labels in `Operation`; three classified `Mediation::Unavailable`, one with a handler arm. |

## Summary

B44's fix asked for dedicated endpoints. It did not need them: none of the four
labels was reachable. Three answered `UnsupportedOperation` and nothing else.
The fourth, `HealthConfirm`, had a real handler arm that never ran — boot
promotion happens from the supervisor's idle path once every required instance
parks, not from a component asking. All four are deleted, along with their
runtime wrappers, their clients, and the now-empty `Mediation::Unavailable`
class. A client is denied by seL4 lookup because there is no root-side path at
all. All five named gates pass.

## Changes

- **Four labels removed** from `slime-root/src/ipc.rs::Operation`, recorded as
  holes in `RETIRED_POLICY_LABELS` so `from_label` refuses them.
- **`Mediation::Unavailable` removed.** With B43's `StoreTransact` gone, the
  class had no members.
- **Runtime wrappers and transports removed**: `generation_transact`,
  `health_confirm`, `recovery_reconstruct`, `generation_receive`, and the
  root-endpoint `transact` helper whose last caller left with B43.
- **Clients deleted**: `recovery.rs` (in no manifest at all),
  `generation-list.rs`, `generation-manager.rs`, and init's
  `SLIME_TRANSFER_RECEIVER` branch — nothing set that flag, so the branch was
  unreachable, and the transfer plane uses `sel4-transfer-probe`.
- **`check-sel4-component-graph.py`'s assertion inverted**: it asserted each
  unmediated plane stayed unmediated; it now asserts no unmediated plane
  remains and that all seven retired labels refuse rather than resolve.
- **`sel4-generation.zti` fixed** — see below.

## Regression guards

- `ipc::tests::no_console_operation_is_reachable_on_the_universal_abi` refuses
  all seven retired labels, `RETIRED_POLICY_LABELS` among them.
- `check-sel4-component-graph.py` fails if any operation is classified
  `Mediation::Unavailable` again, or if a retired label resolves in
  `from_label` rather than being refused.
- `just sel4_generation_check` now passes, so the plane's run tokens and idle
  instances are asserted rather than merely present.

## Verification

`HealthConfirm`'s reachability was measured, not assumed: a
`sel4::debug_println!` at the top of the arm, then a full
`just sel4_boot_selection_check`. Zero hits. The gate passes either way,
which is why the arm survived this long.

| Check | Result |
|---|---|
| `just sel4_generation_check` | pass (was red before this item; see below) |
| `just sel4_boot_selection_check` | pass |
| `just sel4_rollback_check` | pass |
| `just sel4_recovery_plane_check` | pass |
| `just sel4_transfer_check` | pass |
| `just sel4_component_graph_check` | pass — "no unmediated plane remains and 7 retired labels stay refused" |
| Twelve further plane gates | pass |
| `cargo test -p slime-root --lib` | 143 passed |
| `just contracts_check`, `just generation_check`, `just test_host` | pass |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just machete` | clean |

## Decisions

**Removal satisfies the exit condition better than endpoints would.** B44 asks
that a client without the capability be "denied by seL4 lookup, not a
root-side resource table". Giving these operations dedicated endpoints would
have built a service for four operations nobody invokes. Deleting them means
there is no root-side path to deny — the strongest form of the property.

**`HealthConfirm`'s arm was not kept "just in case".** It was authorised,
plausible, and dead. Keeping an unreachable handler is how a system acquires
two promotion paths that disagree; the supervisor's idle path is the one that
runs and the one B35 made authoritative.

**Retired labels stay holes, as with B41 and B43.** Seven now: 5 was reused by
`FixtureDirective` and is checked by variant name instead.

### The generation plane was red before this item

`sel4_generation_check` was failing at the start of B44, verified by stashing
the work and re-running. Two causes, both the same class the probe planes
carried:

1. **No run tokens declared.** Init mints an endpoint pair and hands one end to
   each child, but `mintedBindings` was `[]`. The spawn preflight expects
   `parent_supplied + minted`, and the manager's device grant is a self-loop
   correctly excluded from the parent's count — so it saw one requested grant
   against zero declared and refused with `ungranted`.
2. **No idle instances.** The "idle without a client" and "idle without an
   endpoint" markers prove that generation-declared authority alone does not
   run the scenario, which needs a second root-owned copy of each executable
   holding no run token. Neither was declared, so neither marker had an
   emitter. The idle manager gets its own self-loop device grant so the two
   copies do not share one.

## Open risks and follow-ups

- `just sel4_boot_layout_check` is red and was verified red at the committed
  state before this work — the frozen fixtures have drifted from several
  planes' layouts, not only this one. Blessing them would freeze whatever
  moved without understanding it, so it is left for its own pass.
- `Operation` now has seven holes. That is the correct trade against silent
  misrouting, but the numbering is getting sparse enough that a future
  contiguity audit will want a single retired-label table rather than the
  three constants plus one array it has now.

## Artifacts and provenance

- Commits: `1fb1ecd` (the deletion), `00af727` (the generation fixture).
- `sel4_powerbox_check` remains red, verified inherited earlier in this
  session.
