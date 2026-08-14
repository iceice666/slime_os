# R2 — the builder assigns declared slots, and init reads its grant count

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/schema.zt`, `scripts/build/build-generation.py`, `components/bins/src/bin/init.rs`, `components/bins/src/default_fabric_profile.rs`, `contracts/generation/v1/fixtures/sel4-stream.zti` |
| Roadmap | B50, B46 |
| Gates | `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_root_boot_check`, `just ruff`, `just lint_all` |
| Trigger | B46's cutover produced six consecutive slot-collision failures, every one a hand-written number disagreeing with another hand-written number |
| Baseline | Every `bindings[].slot`, `mintedBindings[].slot`, and `notificationBindings[].slot` hand-assigned across 25 fixtures; init carrying one hardcoded spawn-grant list |

## Summary

B50's `fixed-slot constants` clause (1) — auto-allocating declared slot numbers
— is implemented, and clause (3) moved with it. `slot` is now optional on the
three binding records; the builder fills every omission at the single point the
manifest is decoded, reserving explicit slots first so nothing already pinned
moves. Separately, init no longer restates how many capabilities each child's
owner must supply at spawn: the builder emits that count from the manifest by
the same rule the root checks it against, which also retired a build flag that
was selecting the fabric's grant set. The three fabric gates this was expected
to unblock are still red, but for a different and now-visible reason — recorded
under Open risks rather than claimed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/schema.zt` | `slot?` on `InstanceBinding`, `MintedBinding`, and `NotificationBinding` | A manifest states a slot number only when something outside it — a frozen artifact, a component compiled against the number — actually pins that number |
| `build-generation.py::assign_declared_slots` | Fill every omitted slot at the one point the manifest is decoded, lowest free number in grant-name order, explicit slots reserved first | Every consumer downstream still reads a concrete number, so no other code learns that slots can be absent; and a manifest that pins everything encodes byte-for-byte as before |
| Namespace split | Capability and minted bindings share one namespace per holder; notification bindings get their own | The decoder refuses duplicates per holder because both land in the child's capability table — but a notification at 0 and a capability at 0 occupy disjoint runtime regions and were never a collision |
| `build-generation.py::declared_spawn_grant_counts` | One function computing what an owner must supply: the child's minted bindings plus its non-endpoint, non-self-loop grant bindings | The root's rule is implemented once. An owner that disagrees by one is refused with no way to see why, so a second implementation of this count is a latent boot failure |
| `FABRIC_MINTED_GRANTS` in the resolved profile, read by `init.rs` | Init reads the count instead of restating it | Init carried one hardcoded list, which was the stream graph's; every other plane's spawn was refused for a count it had no way to know |
| Same, emitted for fabricless manifests too | A graph with no fabric still has owners that spawn children | One `init.rs` compiles against every manifest, rather than the constant existing only where a fabric graph does |
| `init.rs` fabric grant selection | Slice by the declared count rather than by `SLIME_FABRIC_VISIBILITY_CHECK` | Stream declares five and visibility six; which set to pass is a manifest fact, and the same binary now reads it rather than being told by a build flag |
| `sel4-stream.zti` | Seven redundant slot numbers removed from the five holders whose numbering was pure drift | Demonstrates the mechanism on a real fixture rather than only on a constructed one |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Auto-allocation silently renumbers a frozen artifact | `just sel4_boot_layout_check` | The byte-pinned layout fixtures stop matching |
| Assignment stops being a function of the manifest | `just sel4_stream_check` | Any reordering changes the resolved profile and the plane's slots with it |
| The builder's grant count drifts from the root's rule | Every `just sel4_*_check` | `SLIME_GRAPH spawn preflight … reason=declared-count` at boot, before anything runs |
| The fabricless profile path regresses | `just sel4_channel_check`, `just sel4_crossing_check` | `cannot find value FABRIC_MINTED_GRANTS in module profile` at build |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| **Byte-identical image across the fixture change** | `build/slime-sel4-stream.elf` is `md5 6eff83ca36578c012cb667cb68cc5528` with 44 explicit slots and with 51 — the builder reproduces the hand-written numbering exactly | Direct |
| `just sel4_stream_check` | PASS, with the auto-allocated fixture | Direct |
| `just sel4_visibility_check` | PASS — declares six spawn grants where stream declares five, same binary, selected by manifest | Direct |
| `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_root_boot_check` | PASS — these regressed on the fabricless profile path and are green after it emits the table | Direct |
| `just test_sel4_root` | 118/118 across 13 modules | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff` | Clean | Direct |
| Assignment unit behaviour | Explicit slots preserved (`y`=0, `m0`=5, `n0`=0); omitted filled in name order across the shared namespace (`m1`=1, `x`=2, `z`=3); notifications numbered separately; repeated runs identical | Direct |
| The named structural hazard | An endpoint binding and a minted factory with no explicit slots resolve to 0 and 1 rather than colliding | Direct |
| `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_supervision_check` | Fail at their pre-existing baselines, unchanged by this entry | Direct (baselined) |

## Decisions

- Decision: resolve omitted slots once, immediately after the manifest is
  decoded, rather than at each of the ten-odd sites that read `["slot"]`.
- Rationale: every consumer keeps reading a concrete integer, so nothing else in
  the builder has to learn that a slot can be absent. The alternative spreads an
  `Option` through validation, encoding, and profile resolution, and any site
  that forgets it fails at boot rather than at build.
- Rejected alternative: renumbering every fixture to a canonical order. The
  boot-layout fixtures are byte-pinned and components compile against specific
  numbers, so a global renumbering is a much larger change that this one does
  not need — reserving explicit slots first makes adoption per-binding.

- Decision: emit the spawn-grant *count* rather than the list of grant names.
- Rationale: the count is what `preflight_spawn_grants` checks, and it is the
  only thing an owner can get wrong. Emitting names would invite an owner to
  match on them, which would be a second authority model beside the positional
  one the root actually implements.

## Open risks and follow-ups

- [ ] **`sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` are still
      red, and R2 was not what blocked them.** With the count now derived, the
      refusal is no longer init guessing: those fixtures genuinely declare
      capabilities as `mintedBindings` that init must create and hand over, and
      `drive_stream_plane` creates none of them. `sel4-qos.zti` asks for three
      `fabric-publisher-probe-*` carriers where the working `sel4-stream.zti`
      declares one ordinary `fabric-publisher-probe` grant the root materializes;
      `sel4-call.zti` and `sel4-operation.zti` still declare *every* control
      endpoint as minted, which is the pre-cutover shape B46 replaced elsewhere.
      The fix is the same class as the visibility fixture's and is fixture-side.
      Not attempted here: `sel4-call.zti` alone has twenty-one minted bindings
      across five holders, and each conversion must be checked against what the
      component expects in that slot.
- [ ] Clause (1) is implemented but adopted in one fixture. The other 24 still
      pin numbers by hand; each can drop the ones that are drift, and the
      byte-identical-image check above is how to confirm a conversion is inert.
- [ ] B50's remaining clauses (deleting the logical-capability model itself) are
      untouched.

## Artifacts and provenance

- Focused report: this entry
- Related roadmap item: `roadmap/00-backlog.md` B50 (open, clause 1 and 3 done),
  B46 (open)
- Preceding entry: `devlog/2026-08-12-b46-arena-slot-occupancy/` — the six
  slot-collision failures that scoped this clause
