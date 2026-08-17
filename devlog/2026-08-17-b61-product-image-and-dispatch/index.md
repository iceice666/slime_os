# B61 — `just run` was booting a verification fixture, and one half of the fix needs a seam that does not exist

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `Justfile` (`run`, `sel4_product_image`, `test_sel4_root`), `slime-root/src/ipc.rs`, `slime-root/src/main.rs` |
| Roadmap | B61, B23, B46 |
| Gates | `just sel4_component_graph_check`, `just test_sel4_root`, `just sel4_boot_check` |
| Trigger | The structural audit traced `just run` through four indirections to `SLIME_ROOT_FIXTURE=1` |
| Baseline | `just run` documented as "the seL4 product image"; `lib.rs` states `main.rs` is deliberately untestable |

## Summary

`just run` built the default `fixture` variant, whose `SLIME_ROOT_FIXTURE=1`
compiles out the product generation-graph launcher entirely — so the command a
developer runs to see the system booted a two-fixture verification proof. It now
builds the `--component-graph` variant, which is what `init` launches in a real
generation, verified by the product image emitting zero `native fixture` markers.
The second half — making the dispatch path host-testable — landed partially and
deliberately: `service_for_root_label` moved into `ipc.rs` with three tests that
bite, but `serve_instance_graph` and the service handlers did not, because they
take live seL4 capability handles and testing them on host needs a
fault-injection seam this repository does not have.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `Justfile` | New `sel4_product_image` builds `--component-graph`; `run` boots `slime-sel4-graph.elf` | The command claiming to run the product runs the product |
| `slime-root/src/ipc.rs` | `service_for_root_label` moved here from the binary — shape-bounding and meaning-assignment are both this module's job | Label routing is reachable from host tests |
| `slime-root/src/ipc.rs` | Three tests: all 23 labels route to their owning mechanism; retired B46 gaps and out-of-table values are refused; the fixture directive is not a component service | A mis-route fails a test instead of a boot |
| `slime-root/src/main.rs` | Copy deleted, call site delegates, six now-unused service imports trimmed | One definition |
| `Justfile` | `test_sel4_root` pinned count 118 → 121, 14 → 15 modules, per B23's "raise it deliberately" rule | The count still catches a module losing coverage |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A label routes to the wrong mechanism | `just test_sel4_root` → `every_declared_label_routes_to_its_owning_service` | `label N routed to the wrong mechanism` |
| A retired B46 label is re-meaned, handing an old caller authority it never asked for | `retired_and_unknown_labels_are_refused` | `retired or unknown label N was routed to a mechanism` |
| The fixture handshake becomes reachable by any component that guesses its label | `the_fixture_directive_is_not_a_component_service` | Assertion failure |
| A module silently loses host coverage | `just test_sel4_root`'s pinned count | `ran N tests, expected 121` |
| `just run` reverts to a verification variant | `just sel4_component_graph_check` boots the same image the target builds | Its own marker chain fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Product image boots a real graph | `SLIME_ROOT graph admitted executables=5 instances=5`, `[init] launching component graph`, `SLIME_GRAPH healthy … required=3 live=3`, and **0** `native fixture` markers | Direct |
| `just sel4_component_graph_check` | pass — on the image `just run` now boots | Direct |
| `just test_sel4_root` | pass — 121/121 across 15 modules | Direct |
| Guard bites: route `directory_labels::DERIVE` to `SERVICE_SPAWN` | Suite aborts (`panic = "abort"`); reverted | Direct |
| `just sel4_boot_check`, `just sel4_spawn_check` | pass — real dispatch over the moved routing | Direct |
| `just sel4_root_boot_check` | pass — the fixture plane it still guards is unaffected | Direct |
| `just fmt_check_all`, `just lint_all` | pass, after trimming six imports the move orphaned | Direct |

## Decisions

- Decision: point `run` at the `--component-graph` variant rather than deleting the
  fixture variant. Rationale: the fixture path has its own gate
  (`sel4_root_boot_check`) covering root admission, allocator, timer, and fault
  isolation. It is verification code; the defect was that it was also the *default*.
  Rejected alternative: deleting it — that removes real coverage to fix a
  mislabelled build target.

- Decision: move `service_for_root_label` into `ipc.rs` rather than adding a new
  module. Rationale: `ipc.rs`'s stated job is the bounded envelope, and routing a
  decoded label to its owning mechanism is the same boundary. The audit separately
  noted `AGENTS.md` claims `ipc.rs` owns validation while it only decoded shape;
  this moves one real decision there rather than restating the claim.

- Decision: **stop** after the routing, and say so.
  Rationale: `serve_instance_graph` takes `sel4::cap::Endpoint`, a live
  `ObjectAllocator`, a `Generation`, and an IPC buffer, and its handlers invoke seL4
  objects directly. Testing them on host needs a fault-injection seam for object
  invocation — a mechanism that does not exist here and that is a larger change than
  all of B61. Claiming the item closed while quietly dropping that half would make
  the backlog's exit conditions untrustworthy, so the resolved entry states exactly
  what is not done and why.
  Rejected alternative: a trait-object seam over `sel4::cap::*` introduced
  speculatively — that is a large abstraction added for testability with no second
  implementation to justify it, and the repository's own rule is to refuse needless
  abstractions.

- Decision: raise the pinned test count rather than exempting the new module.
  Rationale: the pin exists so a module losing coverage is visible, and its comment
  says to raise it deliberately when tests are added. Exempting would blind it.

## Open risks and follow-ups

- [ ] `main.rs` is still 5786 lines and none of it is host-testable. The dispatch
  loop, spawn preflight, capability-transfer handlers, buffer/loan lifecycle, and
  healthy/wedge accounting remain reachable only through QEMU. A seL4 object
  invocation seam is the prerequisite; it wants its own entry rather than being
  smuggled into B61.
- [ ] The legacy two-fixture dispatch stack (458 lines, measured in the audit) still
  compiles into the `fixture` variant, which is still the build-script default when
  no plane flag is passed. `just run` no longer selects it, but a bare
  `python3 scripts/build/build-sel4.py` does.
- [ ] **[INFERENCE]** The product graph image is judged fixture-free because it
  emitted zero `native fixture` markers across a 60-second boot. That is an absence
  of output, not a proof the branch is absent from the binary; the `#[cfg]` gating
  makes the stronger claim, but no symbol-level check was taken.

## Artifacts and provenance

- Focused report: none; the audit that opened B61 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), which traced
  the `just run` → `SLIME_ROOT_FIXTURE=1` chain and measured the legacy stack.
- Raw transcript: none preserved; the product boot's markers are quoted in
  *Verification* and reproducible with `just sel4_product_image` followed by the
  `run` recipe's QEMU line.
- Serial/debugger/model output: quoted inline (`[init] launching component graph`,
  `SLIME_GRAPH healthy … required=3 live=3 idle=3 failed=0`).
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B61 in the resolved log with its deferred half stated; B63 and B65 open.
