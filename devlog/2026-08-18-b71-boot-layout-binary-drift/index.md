# The boot-layout resource's binary encoding silently drifted from the Rust table it is meant to match

| Field | Value |
|---|---|
| Date | 2026-08-18 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/build/boot_layout.py`, `scripts/build/build-generation.py` |
| Roadmap | B70, B71 |
| Gates | `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_component_graph_check` |
| Trigger | Attempting to extend `devlog/2026-08-18-cp2-capability-role-axis/`'s migration to `init.rs`'s `main()` |
| Baseline | `just sel4_component_graph_check` passes at `HEAD` (`934ca08`), booting `init.rs` unmigrated against the compiled `boot-layout-1.rs` constant table |

## Summary

Migrating `init.rs`'s console/dango/spawn-service spawns to CP2's `executable:`
runtime query broke a previously-passing gate. The query was correct; the data
it read was not. `scripts/build/build-generation.py` computes each bootstrap
binding's real slot from the manifest and applies it to the Rust constant
table `init.rs` compiles against, but never applies the same correction to the
*binary* `contracts/boot-layout/v1` resource embedded in `generation.bin` that
the root decodes at runtime — so the two representations of "where does
`spawn-service`'s executable capability live" can silently disagree (4 vs 5,
confirmed by direct byte decode). A second, independent defect compounds it:
unnamed layout roles (`storage-capability`, `shared-buffer-factory`, and
others) are never renumbered by component-set narrowing, so a narrowed
manifest's real named slots can collide with an unnamed role's stale,
full-layout position — confirmed already present, silently, in the currently
committed `HEAD`. Both root-caused; neither fixed this session, since a
correct fix to the first alone regresses builds that currently pass by
accident of the second. Filed as `roadmap/00-backlog.md` B71.

## Observable symptom

- Command: migrate `init.rs`'s `main()` to resolve `console`/`dango`/`spawn-service`'s executable slots via `slime_rt::resolve_binding(b"executable:...")` instead of the compiled `CONSOLE_SLOT`/`DANGO_SLOT`/`SPAWN_SERVICE_SLOT` constants, then `python3 scripts/check/check-sel4-component-graph.py`.
- Expected: identical boot behavior — the migration only changes *how* the same slot number is obtained, not which slot is used.
- Observed: `SLIME_GRAPH spawn preflight executable task-instance=2 slot=4 held=None required=0x10008 error=InvalidOperation`, `SLIME_GRAPH spawn refused task=0 slot=4 ungranted`, `SLIME_GRAPH component exit task=0 status=1`. `console` (instance 1) spawned correctly through the same query; `spawn-service` (instance 2) did not.
- Exit/fault/serial evidence: full transcript in the check's own tail-40 report, captured during the investigation session; not separately archived, since the finding was reproduced twice (once via QEMU, once via direct byte decode).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `check-sel4-component-graph.py`'s own required markers assert `slot=5 component=spawn-service` for the *manifest-derived* authorization path (line 73 of the check). | Root's own accounting says 5. The query said 4. One of the two representations is wrong. |
| 2 | `build/sel4-generation/boot-layout-1.rs` (the compiled Rust constant table for this exact build) declares `SPAWN_SERVICE_SLOT: u32 = 5`. | The Rust constant table agrees with root's accounting. Only the *binary* resource my query reads disagreed. |
| 3 | Direct byte search of `build/sel4-generation/generation.bin` for `spawn-service`'s `component_identity` hash, reading the 4 bytes immediately after it as a little-endian `u32` slot field. | The embedded binary resource genuinely encodes slot `4` for `spawn-service` — not a query bug, a data bug in what gets embedded. |
| 4 | Traced both call sites in `scripts/build/build-generation.py`: `render_boot_layout_rust(...)` (the Rust table) receives `binding_slots`/`role_bindings` from `bootstrap_binding_projection(manifest)`; `build_boot_layout(...)` (the binary resource) two lines above does not. | Root cause of defect 1: the binary encoder was never given the same correction the Rust encoder already receives. |
| 5 | Applied the missing correction to `build_boot_layout` and rebuilt. | `boot layout: slot 7 declared twice` — a new failure, at build time rather than at boot. |
| 6 | Decoded the corrected entry table by hand: correcting `spawn-service`→5 also correctly moved `sysinfo`→6 and `echo-agent`→7 (all three named, all three real `InstanceBinding` slots). Slot 7 was *already* claimed by the unnamed `storage-capability` role, which the static table places at slot 7 unconditionally and which component-narrowing never touches (`_entry_component` returns `None` for an unnamed role, and the filter keeps every `None`-owner entry regardless of `components`). | Root cause of defect 2: unnamed roles are outside the narrowing/renumbering that named entries get, so a narrowed manifest's real named slots can land on an unnamed role's stale full-layout position. |
| 7 | `git stash` to a clean `HEAD` checkout (reverting every change made this session) and regenerated `boot-layout-1.rs` for the same component-graph fixture. | `ECHO_AGENT_SLOT: u32 = 7` and `STORAGE_CAPABILITY_SLOT: u32 = 7` are *already* the same value at `HEAD`, independent of anything done this session. Defect 2 is pre-existing, not introduced by defect 1's fix. |

## Root cause

Two independent defects in `scripts/build/boot_layout.py`, both masked by the same fact: nothing had ever decoded the `contracts/boot-layout/v1` resource object's *content* before this session's CP2 runtime-binding query. The resource has existed since B10, structurally validated on every boot (`BootLayout::decode` checks magic, version, bounds, ascending-unique slots, and role/identity agreement), but no consumer ever compared one entry's value against ground truth.

1. `scripts/build/build-generation.py` computes `binding_slots, role_bindings = bootstrap_binding_projection(manifest)` — the bootstrap instance's real, per-manifest `InstanceBinding` slots — and passes them to `render_boot_layout_rust(...)`, which uses them to override the static `boot_layout.py` table before emitting `init.rs`'s compiled constants. The neighboring call to `build_boot_layout(...)`, which encodes the *binary* resource embedded in `generation.bin` and decoded by the root at runtime, receives neither. Wherever the static table's guess disagrees with the manifest's real binding — which happens whenever component-set narrowing changes which slots survive — the binary resource and the Rust constants diverge, and every consumer of the constants (all of `init.rs`, until this session) never noticed because it never read the binary resource at all.
2. `boot_layout.layout_for`'s component-narrowing (`kept = [entry for entry in entries if owner is None or owner in components]`) only ever filters and renumbers entries that name a component (`executable`, `endpoint-client`, `endpoint-service`). An unnamed, singular role (`shared-buffer-factory`, `storage-capability`, `generation-control`, `object-store`, `directory`, `input`, `endpoint-factory`) has `owner = None` from `_entry_component`, so the filter always keeps it — at its full-layout static slot, never renumbered alongside the named entries around it. A manifest that narrows the named set down (dropping most of `BASE_LAYOUT`'s ~40 components to 5) compacts the named entries into a low slot range that can coincide exactly with an unnamed role's untouched position.

Neither defect is masked by the other; defect 2 is pre-existing and independent, confirmed on a clean `HEAD` checkout with no session changes applied. They interact only in the sense that fixing defect 1 without first fixing defect 2 turns a silent wrong-slot bug into a build-time collision failure on any narrowed manifest where the two ranges meet — which is what happened in step 5.

## Changes

No fix is applied in this session. The `build_boot_layout` correction (defect 1's fix) was written, proven correct in isolation, and reverted rather than shipped: shipping it alone regresses every generation build whose narrowed named range reaches an unnamed role's stale position, which cannot be bounded and verified for all ~20+ `sel4-*.zti` fixtures within this session. Filed as `roadmap/00-backlog.md` B71 with the fix already scoped (fix defect 2 first, then defect 1) for whoever picks it up next.

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/ipc.rs` | `resolve_layout_slot` (this session's namespace-fix query) now refuses on more than one matching identity instead of returning the first match — see `devlog/2026-08-18-cp2-runtime-binding-query/index.md`'s `## Corrections` and the sibling role-axis entry for the matching discipline on `resolve_role_slot` | An ambiguous or corrupt layout is refused rather than silently answered wrong; defense in depth, independent of B71 |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| B71's fix ships for defect 1 alone, reintroducing collisions on narrowed manifests | None yet — B71's exit condition requires a full-table uniqueness check across every fixture before either defect is fixed | A future `boot-layout-N.rs`/binary resource pair declaring two different names at one slot |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_component_graph_check`, `just sel4_dango_check`, `just sel4_channel_check`, `just test_sel4_root` with the reverted (B71-fix-free) tree | All pass, matching `HEAD`'s baseline | Direct |
| `git stash` to `HEAD`, regenerate `boot-layout-1.rs` for the component-graph fixture | `ECHO_AGENT_SLOT`/`STORAGE_CAPABILITY_SLOT` both 7; `GENERATION_CONTROL_SLOT`/`SHARED_BUFFER_FACTORY_SLOT` both 8 — confirms defect 2 predates this session | Direct |
| Direct byte decode of `build/sel4-generation/generation.bin`'s embedded boot-layout resource for `spawn-service`'s identity | Slot field reads `4`, contradicting the Rust constant `5` — confirms defect 1 independent of any narration | Direct |

## Decisions

- **Decision:** revert the `build_boot_layout` correction rather than ship it partially.
  **Rationale:** it is provably correct in isolation (matches `render_boot_layout_rust`'s existing, already-trusted behavior) but its blast radius — every `sel4-*.zti` fixture whose narrowed named range can reach an unnamed role's position — was not boundable within this session. Shipping a correct fix that turns other gates' silent bugs into loud failures without also fixing what causes those failures is not mergeable.
- **Decision:** file as one backlog item (B71) with two problem statements, not two items.
  **Rationale:** the two defects are independently confirmed and independently true, but fixing one without the other regresses currently-passing builds — the fix order is a real dependency, not an artifact of filing convenience.
- **Rejected alternative:** patch around the collision for the one fixture this session's migration needed (component-graph), leaving the general defect unfixed. Rejected because it would re-hide the defect behind a single-fixture special case, exactly the kind of narrowing this whole investigation exists to remove.

## Open risks and follow-ups

- [ ] B71 unresolved: `build_boot_layout` still does not receive `binding_slots`/`role_bindings`; the binary boot-layout resource remains untrustworthy for any manifest where the static table's guess disagrees with the real bindings.
- [ ] Unnamed-role narrowing (defect 2) needs a design, not just a threading fix: dropping an unused role's slot changes the *count* of entries, which changes every subsequent (already-narrowed) entry's renumbered position too — this needs the same care CP1's `declared_spawn_grant_counts` ordering fix took, verified byte-identical against every currently-frozen baseline that does *not* hit the bug.
- [ ] `init.rs`'s `main()` migration to the `executable:` query (the reason B71 was found) is deferred until B71 closes; the compiled `CONSOLE_SLOT`/`DANGO_SLOT`/`SPAWN_SERVICE_SLOT` constants remain in use there.

## Artifacts and provenance

- Related roadmap item: `roadmap/00-backlog.md` B71; `roadmap/10-component-platform.md` CP2
- Prior entries this continues: `devlog/2026-08-18-cp2-runtime-binding-query/index.md`, `devlog/2026-08-18-cp2-capability-role-axis/index.md`

## Corrections

**2026-08-18 — resolved the same day; this entry's `Status: Root-caused` and its
"No fix is applied in this session" are superseded.** The body's judgement that
defect 1 could not be fixed without first designing defect 2's fix was correct in
its reasoning and wrong in its conclusion, because both defects share one cause
the body stopped one step short of naming: the static table is a *second*
statement of a placement the manifest already decides. Deleting the second
statement fixes both at once, and needs no renumbering design.

`boot_layout.layout_from_manifest` derives the layout from the bootstrap
instance's own `InstanceBinding` records — the only thing that decides where the
root places a capability. `build_boot_layout` encodes that table and
`render_rust` renders constants from it, so the two readings cannot drift
(defect 1), and a role the generation does not grant simply has no row, so it
renders `SLOT_ABSENT` instead of a live slot belonging to something else
(defect 2).

The kind→role mapping was derived empirically before being pinned, rather than
guessed: over all 25 seL4 plane fixtures, every one of the 106 rows the root
actually resolved is a bootstrap binding of `executable`,
`sharedBufferFactory`, or `directory`, agreeing on slot, role, *and* rights,
with no row unaccounted for and no kind ever landing in some planes and skipping
in others. `endpoint` never occupies a row — the root installs a declared
Endpoint into both declaring instances directly — which all 20 endpoint bindings
confirm. Endpoint bindings still hold real CSpace slots, so they reach the
constant table through the binding projection; that distinction cost one QEMU
cycle to find, when `SPAWN_SERVICE_RPC_SLOT` went `SLOT_ABSENT` and init could
not send its shutdown.

Verified: all 25 planes agree across resource, constants, and the frozen
`.layout` the root resolved (106 rows), by a new `check-boot-layout-resource.py`
arm that also refuses two differently-named constants sharing one slot. Both
halves proven non-vacuous by re-injecting the original defects: encoding the
static table instead of the derivation reports `the root resolved 5 slots, the
derived resource declares 64`; letting an ungranted role fall back to its static
slot reports `DIRECTORY_SLOT is 14, which is neither a resource row nor a
declared binding slot`. `just sel4_boot_layout_check` matches all 25 frozen
fixtures **unchanged** — the derivation reproduces what the root already did, so
no fixture was re-blessed, which is the strongest available evidence that the
static table was the wrong copy rather than the layout being redefined.

The `main()` migration this entry's third open risk deferred is done:
`console`, `dango`, and `spawn-service` resolve through the CP2 query, and
`sel4_component_graph_check`, `sel4_dango_check`, and
`sel4_generation_plane_check` pass.
