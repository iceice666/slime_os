# C7.4 shared-buffer mapping and read-only sealing

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/memory/shared_buffer.rs`, `kernel/src/memory/vmm.rs`, `kernel/src/memory/address_space.rs`, `kernel/src/capability/mod.rs`, `kernel/src/syscall/mod.rs`; `just shared_buffer_mapping_check` |
| Roadmap | C7.4 |
| Gates | `just shared_buffer_mapping_check`, `just test` |
| Trigger | Roadmap C7 decomposition; C7.4 adds map/unmap/irreversible seal on top of C7.2 factory allocation and C7.3 per-holder accounting |
| Baseline | C7.3 shared-buffer table: bounded factory allocation charged to a supervision-subtree owner with per-holder `byte_pages`/`buffer_count` quotas; `mapping_count`/`loan_count` declared but unconsumed. `RIGHT_BUFFER_MAP`/`RIGHT_BUFFER_WRITE` existed but were ungated — no map/write/seal operation existed. |

## Summary

C7.4 turns the dormant `RIGHT_BUFFER_MAP`/`RIGHT_BUFFER_WRITE` rights into three
gated operations: `SYS_SHARED_BUFFER_MAP` (23), `SYS_SHARED_BUFFER_UNMAP` (24),
and `SYS_SHARED_BUFFER_SEAL` (25). Mapping installs only page-aligned,
non-executable, exact-frame user PTEs for the named buffer capability, charged
one unit against the holder's `mapping_count` quota under a new fixed
`MAX_MAPPINGS`=64 ceiling; offset/length/base are alignment-, overflow-, and
user-half-checked and confined to the exact buffer before any page-table change,
and a partial map is fully rolled back on the first failing page. Sealing is an
irreversible `Arc`-shared read-only transition that downgrades every live
writable PTE for the region before publishing the seal, so no holder can retain
or later regain write access; a created-read-only or sealed region can never
obtain a writable mapping. Unmap, buffer release, and supervision-subtree
reclamation remove the exact recorded PTEs before returning frames, never
disturbing an unrelated mapping. Status: verified under
`just shared_buffer_mapping_check` (8 QEMU cases) and the full gate stack.

## Observable symptom

- Command: `just shared_buffer_mapping_check`
- Expected: 8 QEMU cases pass; out-of-bounds/misaligned/overflow ranges, write
  widening, and lifecycle misuse fail before page-table changes; seal is
  irreversible.
- Observed: all pass (see Verification).
- Exit/fault/serial evidence: QEMU `Success`; each mapped page's translation
  equals `region.phys() + offset`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `map_page_in` calls `synchronize_kernel_mappings`, which takes `SCHEDULER`; the table lock is held during a map | A user-half mapper that never touches the scheduler lock is required to keep lock order `SCHEDULER -> SHARED_BUFFER_TABLE` |
| 2 | Mappings must be torn down on unmap, release, peer death, restart, and revocation, all of which already funnel through `task::terminate` -> `reclaim_owner` | Record each mapping (region id + root + base + pages) so exact PTEs are removed before frames are freed |
| 3 | A holder may map a region transferred to it, so an owner can carry a mapping charge with no buffer charge | Split `OwnerCharge` into independent page/buffer/mapping counters; keep the entry until all three are zero; widen the charge-owner ceiling to `MAX_SHARED_BUFFERS + MAX_MAPPINGS` |
| 4 | First `cargo clippy` run flagged `%`-alignment checks and an explicit map counter | Use `u64::is_multiple_of` and roll back with the loop `index` instead of a manual counter |

## Root cause

Not a defect fix; C7.4 is new mechanism. The pre-existing gap: the shared-buffer
object exposed map/write rights but no operation consumed them, so a created
buffer could never be mapped into an address space, and the C7.3
`mapping_count` quota field was inert.

## Changes

| Area | Change | Restored/added invariant |
|---|---|---|
| `kernel/src/capability/mod.rs` | `SharedRegion` gains an `Arc`-shared `sealed: AtomicBool` with `sealed()`/`seal()`, mirroring `DmaRegion.outstanding` | Seal is visible across every clone of the handle and never cleared |
| `kernel/src/memory/vmm.rs` | New `map_user_page_in` (user-half only, no kernel-half sync), `unmap_user_page_in`, `set_user_page_readonly_in`, and a private `user_leaf_mut` | Table lock never nests the scheduler lock; leaf downgrade/removal without allocating tables |
| `kernel/src/memory/shared_buffer.rs` | `MAX_MAPPINGS`=64, a bounded `Mapping` registry, per-owner `mappings` charge, `map`/`unmap`/`seal`, and mapping teardown wired into `release`/`reclaim_owner`; new `BadRange`/`WriteDenied`/`MappingsExhausted`/`MapConflict` errors | Exact-frame, quota-charged, roll-back-safe mapping; irreversible downgrade-then-publish seal; teardown-before-free |
| `kernel/src/memory/address_space.rs` | `user_translation` accessor | Gate asserts a mapping names the exact buffer frame |
| `kernel/src/syscall/mod.rs` | Syscalls 23/24/25 with `RIGHT_BUFFER_MAP` (+`RIGHT_BUFFER_WRITE` for writable/seal) gates; error-code mapping extended | Capability-gated map/unmap/seal; structured `ERR_*` for every failure |
| `docs/capability-matrix.md` | BUFFER_WRITE/BUFFER_MAP rows now gated (C7.4); mapping semantics bullet; `MAX_MAPPINGS` bound row; C7.3 future-tense bullet scoped to loans | Matrix tracks the now-live gate surface in the same change |
| `scripts/check/check-no-storage-authority.py` | Allowlist adds the three syscalls | Framework safety allowlist matches the real surface |
| `kernel/tests/shared_buffer_mapping.rs`, `Justfile` | `just shared_buffer_mapping_check` (8 cases) | Independently reviewable C7.4 gate |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Out-of-bounds / misaligned / overflow map accepted | `just shared_buffer_mapping_check` | `malformed_ranges_are_side_effect_free` fails |
| Write widening past created-read-only or seal | `just shared_buffer_mapping_check` | `seal_is_irreversible_and_downgrades_live_mappings` / `created_read_only_region_cannot_be_widened` fail |
| Partial-map PTE leak on conflict | `just shared_buffer_mapping_check` | `map_conflict_rolls_back_partial_page_table_changes` fails |
| Stale PTE to freed frame after release | `just shared_buffer_mapping_check` | `release_unmaps_before_free_and_rejects_stale_region` fails |
| Cross-subtree disturbance on reclaim | `just shared_buffer_mapping_check` | `subtree_cleanup_does_not_disturb_unrelated_mapping` fails |
| Lock-order inversion (`SHARED_BUFFER_TABLE` -> `SCHEDULER`) | `just test` | full-graph boot deadlock/hang |
| Syscall/rights surface drift | `just framework_safety_check` | allowlist mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just shared_buffer_mapping_check` | pass (8/8 QEMU cases) | Direct |
| `just shared_buffer_accounting_check` | pass (7/7; C7.3 unaffected) | Direct |
| `just shared_buffer_factory_check` | pass (8/8; C7.2 unaffected) | Direct |
| `just test` (QEMU) | pass (32 lib + main harness + integration suites) | Direct |
| `just contracts_check` | pass (incl. 18 boot-contracts lib tests) | Direct |
| `just generation_check` | pass (byte-identical two builds) | Direct |
| `just fmt_check` / `just lint` / `_components` | clean | Direct |
| `just framework_safety_check` | ok | Direct |

## Decisions

- Decision: Add dedicated user-half vmm primitives instead of reusing
  `map_page_in`. Rationale: `map_page_in` synchronizes the kernel half under
  `SCHEDULER`; calling it while holding `SHARED_BUFFER_TABLE` would invert lock
  order. Shared pages are user, task-private, and non-global, so no kernel-half
  propagation is needed. Rejected alternative: drop the table lock around each
  page map (opens a TOCTOU on seal/quota state).
- Decision: Seal downgrades live PTEs then publishes an `Arc`-shared flag under
  the table lock. Rationale: a single critical section closes the
  map-after-seal / seal-during-map race on the uniprocessor. Rejected
  alternative: a per-table `sealed` set keyed by id (loses visibility to handle
  clones held by other holders).
- Decision: Independent page/buffer/mapping charge counters per owner, entry
  retained until all zero. Rationale: a transferred region can be mapped by a
  holder that created no buffer. Rejected alternative: derive mapping count from
  buffers (wrong for transferred regions).

## Open risks and follow-ups

- [ ] The live generation manifest still declares no shared-buffer budget, so
  every holder is deny-by-default; the map path is proven by the kernel gate,
  and the live two-component exercise arrives with C7.7 integration.
- [ ] C7.5 loan/return will extend teardown to outstanding loans; the current
  `loan_count` quota remains declared-but-unconsumed until then.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: high-risk reviewer panel (canonical/correctness/security/
  concurrency/convention), all `overall_correctness: correct`.
- Serial/debugger/model output: `just shared_buffer_mapping_check` QEMU serial
  (8/8 passed).
- Related roadmap item: `roadmap/02-core-runtime.md` C7.4.
