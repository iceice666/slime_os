# B70's boot-action query: which composition am I booted into

| Field | Value |
|---|---|
| Date | 2026-08-21 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/syscall-abi/v1`, `slime-root/src/{ipc,main}.rs`, `components/runtime/src/syscall{,.rs,/sel4_transport.rs}`, `components/bins/src/generation_composition.rs` and 5 migrated call sites, `boot-contracts/src/generation.rs`, `components/proto/tests/syscall_abi.rs`, `scripts/check/check-generation.py`, `docs/syscall-abi.md` |
| Roadmap | B70, CP2 |
| Gates | `just contracts_check`, `just test_sel4_root`, `just test_host`, `just runtime_binding_resolution_check`, `just sel4_boot_check`, `just sel4_visibility_check`, `just sel4_dango_check`, `just sel4_boot_layout_check` |
| Trigger | B70's remaining clause: component sources still `include!` `build.rs`-private, manifest-derived constant tables |
| Baseline | 15 `include!` sites; `GENERATION_BOOT_ACTION` a compile-time `&str` in each |

## Summary

`GENERATION_BOOT_ACTION` was a per-plane `&str` that `components/bins/build.rs`
copied into `OUT_DIR`, and eleven fabric components branched on it to pick which
plane's schedule to run. A component's *behavior* was therefore selected at
compile time by one manifest, which is B70's coupling in its purest form: not a
slot number that could be resolved, but a composition identity.

The root already held the answer and already delivered it — as
`boot_action.id()` in the bootstrap thread's first C parameter — but only to the
bootstrap instance, and zero to everyone else. So the components that needed it
were exactly the ones that could not ask. This adds `BOOT_ACTION` (label 40),
which widens *who may ask* without widening what is disclosed, and migrates the
five sites whose only generated symbol was that string. Six `include!` sites
close, 15 → 9.

The backlog deferred this work pending "the label-40 per-generation scalar query
deliberately left unassigned in `ipc.rs`'s routes-nowhere list", to be done
"with the `fabric-graph` read". That read has since landed as labels 38/39, so
the recorded precondition was met.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1/schema.zt` | Declares `capabilityTable BOOT_ACTION 40` | Zutai is the only schema language; bindings generated |
| `docs/syscall-abi.md` | Operand/result row for 40 | Roadmap invariant 4 |
| `slime-root/src/ipc.rs` | Routes 40 to `SERVICE_LIFECYCLE`; releases 40 from the routes-nowhere control (now 41) | An assigned label must leave the negative control |
| `slime-root/src/main.rs` | Dispatcher arm answering `generation.boot_action.id()` | Root owns the mechanism |
| `components/runtime/` | `boot_action()` transport + wrapper, returning the raw id | `slime-rt` does not depend on `boot-contracts` |
| `boot-contracts/src/generation.rs` | `BootAction::ALL` and `BootAction::from_id` | The fold lives beside the enum, not in a component copy |
| `components/bins/src/generation_composition.rs` | New shared helper: one memoized decode | No per-component `match id { 28 => … }` |
| 5 call sites | `console`, `dango`, `fabric-intruder`, `fabric_boot`, `fabric_matrix` | 6 `include!` sites deleted |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Label 40 renumbered | `just test_host` | `operation labels_are_frozen`: `operation … was renumbered` |
| A composition added to the enum but not to `ALL` | `just test_host` | `<X> is frozen at <n> but missing from ALL` |
| Lifecycle stops being universal, denying the query | `just contracts_check` | `BadServiceBinding` |
| The query answers the wrong composition | `just sel4_boot_check`, `just sel4_visibility_check` | `SLIME_ROOT FATAL … fault` / plane timeout |
| The query is refused | `just sel4_dango_check` | plane timeout |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | pass, 27 operations documented | Direct |
| `just test_sel4_root` | 131/131 | Direct |
| `just test_host` | pass | Direct |
| `just generation_check` | byte-identical across two builds | Direct |
| `just system_spec_check`, `just ruff`, `just typos`, `just fmt_check_all` | pass | Direct |
| `just test` | pass; 33 gates reject 1279 mutated transcripts | Direct |
| `just sel4_boot_layout_check` | 26 plane layouts unchanged | Direct |
| `just runtime_binding_resolution_check` | pass | Direct |
| `sel4_channel/loan/dango/visibility/boot/matrix/traffic/stream_check` | all pass | Direct |
| Wrong-answer perturbation (root always answers `Product`) | `sel4_boot_check` fails `SLIME_ROOT FATAL … fabric-op-client-b-restart fault`; `sel4_visibility_check` times out at 240s | Direct |
| Refusal perturbation (root refuses 40) | `sel4_dango_check` times out at 300s — after the fail-closed fix; it **passed** before | Direct |
| Label pin perturbation (40 → 41) | `operation_labels_are_frozen` aborts | Direct |
| `ALL` completeness perturbation (`Demo` dropped from `ALL` only) | `boot_action_ids_round_trip` aborts | Direct |
| Authority survey: 84 migrated instances across 29 fixtures | all hold the gating service | Direct |

## Decisions

- **Decision:** gate label 40 on `SERVICE_LIFECYCLE`, not the `capabilityTable`
  service its label namespace names.
  **Rationale:** the service is the authority gate, and this operation must be
  answerable to every launched instance. Measured against the builder's own
  `declared_services`: 30 of 182 instances hold no capability-transfer service,
  while 0 of 182 lack lifecycle. Every caller reads a refusal as "not this
  plane", so gating on the capability table would let an unrelated grant shape
  choose a component's schedule.
  **Rejected alternative:** keeping it beside `RESOLVE_BINDING`/`GRAPH_READ`,
  which passes today only because the 84 migrated instances happen to qualify.

- **Decision:** answer the frozen numeric `BootAction` id, not the source
  spelling.
  **Rationale:** the bootstrap startup argument already carries exactly this
  number, so both delivery paths decode one encoding. It also keeps the reply in
  MR0 with no transfer window and no bytes to bound.

- **Decision:** `dango`'s `scripted_plane()` is fatal when the root cannot
  answer, unlike the other four sites.
  **Rationale:** three of its four uses are `!scripted_plane()`, so a refusal
  does not fall through to "not this plane" — it selects the *other* echo mode.
  **Measured, not assumed:** the two modes emit the same bytes in a different
  order, so no marker-based transcript assertion can tell them apart. A
  substring gate was written, observed to pass under a deliberately wrong
  answer, and reverted rather than shipped as false coverage.

- **Decision:** `BootAction::from_id` lives in `boot-contracts`, not in the
  component helper.
  **Rationale:** a component-side table goes stale in one direction only — a new
  composition folds to `None` and reads as "some older generation". The
  exhaustive `match` beside the enum makes that a compile error.

## Open risks and follow-ups

- [ ] 9 `include!` sites remain, all `fabric_profile` readers blocked on
  declared bounds that size fixed arrays (`FABRIC_TRACE_DEPTH`,
  `FABRIC_MAX_IN_FLIGHT_*`) rather than on any query. See B70's
  ceiling-vs-budget analysis; these are CP3/CP4 work.
- [ ] `sel4_dango_check` asserts that the boot action *resolves*, not which
  echo mode it selects. Distinguishing the modes needs an order-sensitive
  transcript comparison the gate does not currently do.

## Artifacts and provenance

- Related roadmap item: [B70](../../roadmap/00-backlog.md), [CP2](../../roadmap/10-component-platform.md)
- Contract: `contracts/syscall-abi/v1/schema.zt`, label 40
- Serial evidence: quoted inline in Verification; no raw transcript retained
