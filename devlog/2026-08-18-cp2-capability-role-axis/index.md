# CP2 adds a capability-role query axis, and refuses ambiguity by design

| Field | Value |
|---|---|
| Date | 2026-08-18 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/ipc.rs`, `contracts/generation/v5/gen_rust.zt`, `boot-contracts/src/generated/generation.rs`, `components/bins/build.rs`, `components/bins/src/bin/spawn-service.rs`, `Justfile`, `docs/syscall-abi.md` |
| Roadmap | CP2, B70 |
| Gates | `just runtime_binding_resolution_check`, `just test_sel4_root`, `just sel4_dango_check`, `just sel4_component_graph_check` |
| Trigger | Continuing B70's third clause after the boot-layout namespace fix (`devlog/2026-08-18-cp2-runtime-binding-query/`) |
| Baseline | `CAPABILITY RESOLVE BINDING` answered a grant name or a namespaced boot-layout role; grant-name lookup alone could not migrate sites whose grant name differs across generations |

## Summary

Added a third form to `CAPABILITY RESOLVE BINDING`: `kind:<capabilityKind>` or
`kind:<capabilityKind>+<right>,<right>`, resolving a caller's own binding by
what the capability *is* rather than by what one generation happened to name
it. This exists because grant names are not stable across generations —
`spawn-service`'s shared-buffer factory grant is
`spawn-service-shared-buffer-factory` under `valid.zti` and its RPC endpoint is
`spawn-service-rpc` there but `dango-e-spawn-service-rpc` under
`sel4-dango.zti` — so a name written into a component would recreate the
coupling B70 exists to remove. `components/bins/build.rs` already asked this
exact question of the manifest by string search
(`binding_with_right_slot`, `related_binding_slot`); this moves the same
question to the root, answered from the activation record already installed,
and removes the two build-time functions that became unused.

`spawn-service` migrated one slot (its shared-buffer factory) this way. Its RPC
endpoint was migrated, observed to hang the dango plane, and reverted: the
query correctly refused an ambiguous role, and the bug was in the migration,
not the query. `sel4-dango.zti` grants `spawn-service` three `send`+`recv`
endpoints — the RPC channel plus one context endpoint per spawned command — so
`kind:endpoint+send,recv` matches three bindings and must refuse rather than
pick one, on the same discipline as the boot-layout namespace fix: a plausible
wrong answer is worse than no answer.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v5/gen_rust.zt` | Added `rustManifestRight`, emitting a `right_named(&str) -> Option<u64>` match from the same `rightBits` table that already renders `RIGHT_*` and the Python manifest-rights dict | One schema source for a manifest right's spelling in every generated language, not a hand-copied third table |
| `slime-root/src/ipc.rs` | `resolve_binding_slot` gained a `kind:` prefix branch; added `resolve_role_slot` (kind-exact, rights-superset match over the caller's own bindings, ambiguity → `None`) and `capability_kind_named` | A role query answers only the caller's own bindings, by capability identity, and never guesses among several matches |
| `components/bins/build.rs` | Removed `binding_with_right_slot` (now dead) after migrating its one caller; kept `related_binding_slot`/`minted_binding_slot` (still load-bearing for the ambiguous RPC endpoint) | No unused manifest-parsing code left beside its runtime replacement |
| `components/bins/src/bin/spawn-service.rs` | Shared-buffer factory slot resolved via `resolve_binding(b"kind:sharedBufferFactory+bufferCreate")`; `RPC_SLOT` restored to the generated constant with a comment recording why | One `include!` site narrowed by one real constant, not by a plausible-looking wrong one |
| `Justfile` | `runtime_binding_resolution_check` now depends on `sel4_dango_check` too; `test_sel4_root` count 124 → 127 | The gate exercises every plane the role axis touches; the coverage pin tracks the three new host tests |
| `docs/syscall-abi.md` | Documents the `kind:` form and its refusal rules | Invariant 4: syscall surface documented in the same change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A role query resolves to the wrong one of several matching bindings | `an_ambiguous_role_is_well_formed_yet_unanswerable` (host) + `sel4_dango_check` (QEMU, real 3-endpoint fixture) | Host: would need to assert a specific wrong slot instead of refusal. QEMU: dango plane hangs at `dango> $(sysinfo)` if refusal regresses to a tiebreak |
| Rust/Python rights-name tables drift apart | `manifest_right_spellings_match_their_bits` (host) | `right_named` disagrees with the generated `RIGHT_*` constant for a spelling both claim |
| An unrecognized kind or right is silently treated as "any" | `resolve_role_slot` returns `None` on an unknown right before scanning bindings; `every_manifest_capability_kind_is_askable` pins the known-kind set | A misspelled role would otherwise match the caller's first binding of a guessed kind |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 127/127, count assertion passes | Direct |
| `just sel4_component_graph_check` | Boots; `spawn-service` resolves its factory slot by role, serves and completes | Direct |
| `just sel4_dango_check` | Boots; `spawn-service` resolves its factory slot by role, RPC stays generated, `dango> $(sysinfo)` completes | Direct |
| Pre-fix: RPC endpoint resolved via `kind:endpoint+send,recv` on `sel4-dango.zti` | Boot hung — `dango> $(sysinfo)` printed, then `seL4 dango plane check: boot exceeded 300s without completing the plane` | Direct (negative control, then reverted) |
| `just contracts_check`, `just generation_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just machete` | All pass | Direct |

## Decisions

- **Decision:** ambiguous roles refuse (`None`), never resolve to the lowest matching slot.
  **Rationale:** a lowest-slot tiebreak would have silently answered
  `spawn-service`'s three-endpoint case with a plausible wrong slot instead of a
  visible refusal — the same failure class the boot-layout fallback produced
  twice, just one layer up.
  **Rejected alternative:** deterministic tiebreak by ascending slot. Rejected
  because it is guessing dressed as determinism: the caller asked a question
  that does not identify one capability, and a stable wrong answer is not
  progress over an unstable one.
- **Decision:** migrate only the slots the role axis answers unambiguously in every generation that declares the component (checked against `valid.zti` and `sel4-dango.zti` by hand before writing the migration), not every slot a first attempt reached.
  **Rationale:** the RPC-endpoint attempt was written, compiled, and only
  disproven on a real boot; checking the fixture shapes first would have caught
  it without a QEMU round-trip. Recorded here so the next site is checked the
  same way before code is written, not after it hangs.
  **Rejected alternative:** migrate the RPC endpoint too and special-case
  `sel4-dango.zti`. Rejected as re-introducing a manifest fact into component
  source, which is the coupling B70 removes.

## Open risks and follow-ups

- [ ] `spawn-service`'s RPC endpoint and `COMMAND_PROFILE` executable table need a binding to carry a stable logical role (e.g. a `role` field on `InstanceBinding`) before they can migrate; that is a `contracts/generation/v1` format change, tracked under B70's remaining surface.
- [ ] `init`'s remaining ~134 of 136 boot-layout constants are unmigrated; most name channel/executable roles the namespaced query already serves and were simply not yet touched.
- [ ] `fabric_profile`'s 64 constants remain entirely on `build.rs`; 46 are graph facts (routes, QoS, trace depth) that belong on an authenticated `fabric-graph` read, not a slot query — no such read exists yet (no resource-read syscall).

## Artifacts and provenance

- Related roadmap item: `roadmap/10-component-platform.md` CP2 "Progress (2026-08-18, role axis)"; `roadmap/00-backlog.md` B70
- Prior entry this continues: `devlog/2026-08-18-cp2-runtime-binding-query/index.md` and its `## Corrections`
