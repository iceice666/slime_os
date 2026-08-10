# B42 — the supervision handle becomes the lifecycle identity

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/spawn/v1/schema.zt`, `components/proto/src/spawn.rs`, `components/runtime/src/syscall.rs`, `components/bins/src/bin/{spawn-service,dango,init}.rs`, `scripts/check/check-lifecycle-identity.py`, ten `scripts/check/check-sel4-*-plane.py`, `slime-root/src/main.rs` |
| Roadmap | B42 |
| Gates | `just sel4_spawn_check`, `just sel4_supervision_check`, `just sel4_reclamation_check`, `just sel4_dango_check`, `just contracts_check` |
| Trigger | B42: spawn returned a numeric `task_id` that the wait protocol sent back across a process boundary. |
| Baseline | `sel4_spawn_check` and `sel4_supervision_check` red; `WireSpawnReply.task_id` and `slime_rt::Spawned::task_id` present. |

## Summary

A numeric task id is not authority — it is a name anyone can forge by counting
— and the spawn protocol both returned one and accepted it back as a wait
handle. The supervision capability was already travelling in the same reply,
unused for waiting. The field is now gone from the schema, the generated wire
record, and the runtime's public type, and a lint refuses its return. Getting
there first required repairing gates that had been asserting markers the root
stopped emitting at B34.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/spawn/v1/schema.zt` | `task_id` removed; bindings regenerated | No wire record names a bare task id |
| `spawn-service.rs` | Live table keyed on the supervision slot | The handle is the identity the service resolves |
| `dango.rs` | Waits on the handle it holds | A client needs no name it could have guessed |
| `components/runtime/src/syscall.rs` | `Spawned::task_id` removed | No public runtime type exposes one |
| `check-lifecycle-identity.py` | Declaration-shaped lint over schemas, generated protocol Rust, runtime surface | The removal cannot silently regress |
| `init.rs`, supervision gate | A collected handle refuses a second query | Consumption of the identity is observable |

### The gate repairs underneath

B42's exit condition names four gates and two were red, so the work started
with why. None of the causes was in the behaviour under test:

- **Ten gates asserted `components=N`.** B34 split the executable catalogue
  from the instance list and renamed the markers reporting both. The gates were
  never updated, so they failed on the first marker and everything behind it
  went untested.
- **Three assertions could never have matched.** `SLIME_ROOT graph admitted;
  legacy SLIMECM images not activated` spliced prose into the marker text;
  `staged … executables=2` named a field that record never had; `activated
  components=N` froze a count that only ever covered root-launched instances,
  which is one where a fixture has init spawn the rest.
- **Two markers had no emitter anywhere in the tree.** `factory placed` belongs
  where the boot graph installs a declared binding — a factory is authority to
  mint, so which slot it lands in is the difference between a component
  reaching its own and reaching none. `channel copied` belongs where a parent's
  channel end is copied into a child, the one capability crossing that boundary
  the child cannot name in advance.
- **`spawned … channels=N` undercounted.** It reported only
  generation-declared re-installs; a minted end has no catalogue entry, so the
  copy in `construct_child` is the only way it arrives.
- **The supervision gate double-counted.** It expected three root-launched
  instances to terminate; that fixture declares three but only `init` is
  root-owned, and the other two are already inside its spawn count.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A task-id-shaped field returns | `just contracts_check` | `a task-id-shaped lifecycle field is declared where a capability belongs` |
| A collected handle answers twice | `just sel4_supervision_check` | `a collected handle still answered` |
| The spawn result stops carrying the handle | `just sel4_spawn_check` | the plane's termination arm |
| A gate drifts from the markers the root emits | the repaired gates themselves | a missing first marker rather than silent success |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_supervision_check` | Pass | Direct |
| `just sel4_reclamation_check`, `just sel4_dango_check` | Pass | Direct |
| `just contracts_check` (includes the new lint) | Pass | Direct |
| Lint catches reintroduction | `task_id` reinstated in the spawn schema, refusal observed, reverted | Direct |
| `just sel4_boot_check`, `sel4_input_check`, `sel4_capability_layout_check`, `sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_directory_check`, `sel4_storage_check` | Pass | Direct |
| `just generation_check`, `just test_host` (7), `just test_sel4_root` (142) | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- **Decision:** the lint matches declarations, not mentions.
  **Rationale:** a field named `task_id` in a schema or a public struct is the
  reintroduction; a comment explaining the ban is not, and the root's own
  in-memory `TaskId` crosses no boundary so it is explicitly out of scope. A
  substring match over the tree would have been noise that someone eventually
  silences.
  **Rejected alternative:** grepping for the identifier anywhere, which would
  have flagged this entry's own prose.

- **Decision:** stale coverage probes collection, not `cap_drop`.
  **Rationale:** I wrote the `cap_drop` version first and the guest refused it
  — a collected handle's record is already gone, so the drop fails before the
  staleness is observable. Collection itself is the transition worth asserting.

- **Decision:** repair the gates rather than route around them.
  **Rationale:** two of B42's four gates were red on stale assertions, so a
  green result would have meant nothing. Fixing them also revealed two markers
  with no emitter, which is coverage that had been silently absent.

## Open risks and follow-ups

- [ ] `sel4_channel_check` needs its fixture migrated to the declaration model:
      it currently falls through to the P5.1 fixture roles and the console
      component never launches, so the channel scenario does not run.
- [ ] `sel4_sample_check` runs its whole loan scenario and then hits `Caught
      cap fault` inside `shared_buffer_return` — the same aliasing class as the
      stream plane's residual failure.
- [ ] The root's dispatcher still carries the `Spawn`, `Exit`, health,
      `SupervisionStatus`, `SupervisionDerive`, and `CapDrop` labels. B42's
      exit condition is about the *identity* being capability-shaped, which it
      now is; moving those labels off the universal endpoint is B41's
      structural problem and shares its blocker.

## Artifacts and provenance

- Related roadmap item: `roadmap/00-backlog.md` B42, now in the resolved log.
- Companion entries: [`devlog/2026-08-10-b41-console-endpoint/`](../2026-08-10-b41-console-endpoint/index.md).
