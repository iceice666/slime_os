# CP15 — the SDK publication clause: a bootable closure with no `slime_os` checkout

| Field | Value |
|---|---|
| Date | 2026-09-04 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/component-sdk-release/v1/`, `scripts/lib/component_sdk.py`, `scripts/lib/component_sdk_system.py`, `scripts/lib/component_sdk_system_entry.py`, `scripts/check/check-component-sdk-system-image.py`, `scripts/check/check-component-sdk-export.py`, `scripts/check/check-component-sdk-release.py`, `scripts/check/check-component-sdk-compatibility.py`, `scripts/check/check-sel4-channel-plane.py`, `scripts/build/publish-component-sdk.py`, `just/component-sdk.just`, `.github/workflows/publish-sdk.yml` |
| Roadmap | CP15 |
| Gates | `just component_sdk_system_image_check`, `just component_sdk_export_check`, `just component_sdk_release_check`, `just component_sdk_compatibility_check`, `just component_sdk_upgrade_check` |
| Trigger | CP15's 2026-09-04 legacy-deletion entry closed every migration deliverable but one: "the SDK publication clause... remains unstarted" |
| Baseline | `component_sdk.py` exported a component-development SDK — crates, a pinned seL4 prefix, target specs, linker scripts, a build/update tool — with no closure, generation builder, image packager, or QEMU runner; `build-system-image.py` and `system_image_closure.py` both required repository-relative paths only a `slime_os` checkout has |

## Summary

CP15's last required check was that an external SDK consumer build and boot one complete system closure using only immutable published inputs, with rollback to the previous release reproducing its previous image identity. `component_sdk_system.py` adds one `SystemImageAsset` to the release: a repository-shaped, self-contained source corpus — the `sel4-channel` closure, its test run, and every path the closure resolver and builder read — archived beside the SDK and bound by `archiveHash`/`treeHash`. The exporter canonicalizes the corpus's seL4 prefix the same way it already does for the crate SDK, re-identifies the closure and test-run references against the rewritten tree, and cross-checks the result against the release's own pinned compatibility asset rather than trusting a blind re-hash. `tools/sdk-system-image.py`, generated into the published SDK, extracts the corpus, verifies both hashes, rebuilds the declared closure through the unmodified resolver, and can boot it through the closure's own QEMU test run — all without a `slime_os` checkout. `check-component-sdk-system-image.py` proves the whole clause end to end, including rollback identity reproduction. A full-severity review pass caught two release-blocking defects (an unpopulated `deps/zutai` submodule in every worktree-based export, and a shipped tree verifier that did not exclude the corpus's own extraction cache) and eleven further correctness and structural gaps, all fixed and reverified before commit.

## Changes

| Change | Why |
|---|---|
| `component_sdk_system.py`: `export_asset` copies a fixed `COPY_ROOTS` set (`components`, `contracts`, `slime-root`, `deps/rust-sel4`, `deps/zutai`, `just`, `scripts/{check,build,lib}`, …) into a staging tree, canonicalizes the prefix, re-identifies the closure and test run, and archives the result | A closure and its build-result record are meaningless without the source they resolve against; publishing them without the corpus they name would be a claim nobody could check |
| `export_asset` refuses a `COPY_ROOTS` directory that exists but is empty | `git worktree add` leaves submodules unpopulated; an empty `deps/zutai` still satisfies `is_dir()`, so a silent empty copy would ship a corpus whose closure resolver cannot run |
| `publish-component-sdk.py` and `check-component-sdk-release.py`'s reverse-drift export now populate `deps/zutai` after `git worktree add`, the same way they already populated `deps/rust-sel4` | The corpus needs a working Zutai toolchain to resolve and compile closures; the worktree-based publish and drift-check paths both left it unpopulated, and the empty-input refusal above turned that into a hard publish failure rather than a silent gap |
| `export_asset` validates the canonicalized prefix's tree hash against the profile entry the release's own pinned `sdk-release.json` asset declares, instead of blindly re-hashing a file that never changes | The naive rebind recomputed an identity from bytes that are copied verbatim and therefore cannot move; the real property worth checking is that this export's prefix still matches the one the SDK release already vouches for |
| `tools/sdk-system-image.py` verifies `archiveHash` before extracting, and refuses a release record with no `systems` entry | Matches the sibling `sdk-build.py`'s prefix-archive check: a corrupted or substituted tar was previously parsed and written to disk before any hash caught it |
| `export_asset`'s emitted `treeHash` is verified to round-trip: the archive is extracted to a scratch directory and re-digested before the staging tree is trusted | `treeHash` is what a consumer checks against the *extracted* corpus; nothing previously proved the archive actually reproduces the tree it was digested from |
| `component_sdk.py`'s shipped `verify_tree` (embedded in `tools/sdk-build.py`) now excludes `.system-source` | The system-image tool extracts into `sdk/.system-source/<name>` inside the SDK tree itself; running `sdk-system-image.py` then `sdk-build.py` in the same tree previously reported a false tree-identity mismatch |
| `systems` is now a required release field, and a member of `structuralAxes`/`structuralKeys` (keyed by `name`) | It was declared optional and excluded from CP9's structural comparison, so a release whose bootable system genuinely changed classified as an unremarkable patch |
| `check-component-sdk-export.py`'s `MIRROR_PATHS` and `check-component-sdk-release.py`'s mutated-source mirror both now include `component_sdk_system.COPY_ROOTS` | Both gates derive their curated export-input list from `component_sdk`'s own declarations specifically so a new export input cannot leave the mirror silently incomplete — the same discipline that once caught the linker scripts. The new corpus roots were the second thing to repeat that gap |
| `check-component-sdk-compatibility.py`'s structural-axis negative control branches per axis shape (`systems` mutates a top-level `archiveHash`, not a nested `prefix.archiveHash`) | The `profiles`/`crates`-only assumption raised `KeyError` the moment a third structural axis with a different entry shape existed |
| `check-sel4-channel-plane.py --no-build` now refuses when `--image` is absent | `component_sdk_system_entry.py` is a second, external `--no-build` caller; without the guard a missing `--image` silently boots `None` |
| `just component_sdk_system_image_check` is wired as `component_sdk_upgrade_check`'s dependent, and the publish workflow now runs it explicitly | Every other SDK gate chains from its predecessor; the new gate previously ran only if an operator named it by hand, so a regression in system-image publication would not have blocked a release |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A worktree-based export ships an empty submodule directory | `export_asset`'s `origin.iterdir()` check | `system-image export input is empty: deps/zutai; run git submodule update` |
| A corpus's archive and its declared tree hash diverge | Round-trip extraction inside `export_asset` | `system corpus archive does not round-trip to its staged tree` |
| An extracted corpus tree cache pollutes the shipped tree-identity check | `.system-source` excluded from `verify_tree` in both the library and the shipped `tools/sdk-build.py` string | `exported tree identity mismatch` disappears for the documented `sdk-system-image.py` → `sdk-build.py` sequence |
| A changed bootable system classifies as a patch | `systems` in `STRUCTURAL_AXES`/`STRUCTURAL_KEYS`; `check-component-sdk-compatibility.py`'s per-axis negative control | `component SDK compatibility` gate fails if `systems` stops moving the classification |
| A new export input leaves either gate's mirror silently incomplete | `MIRROR_PATHS` and the mutated-source mirror both derive from `component_sdk_system.COPY_ROOTS` | `system-image export input is missing: <path>` inside `check-component-sdk-export.py` / `check-component-sdk-release.py` |
| `check-sel4-channel-plane.py --no-build` boots a stale or absent image | `--image` required alongside `--no-build` | `--no-build requires --image naming the already-built closure image` |
| The system-image gate stops running automatically | `component_sdk_system_image_check: component_sdk_upgrade_check` dependency; explicit CI step | Gate absent from `just component_sdk_upgrade_check`'s or the publish workflow's transitive run |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just component_sdk_system_image_check` | Two immutable SDK releases each built the declared `sel4-channel` closure with no `slime_os` checkout; the current release booted through its declared QEMU test run; rollback reproduced every previous build-result artifact identity before booting it again | Direct |
| `just component_sdk_export_check` | Two isolated exports byte-identical; release identity moves for an allowlisted source, a pin, and published product source (now including `slime-root/src/main.rs` and the demo composition, which the corpus publishes), and holds for a file the export never reads (`devlog/README.md`) | Direct |
| `just component_sdk_release_check` | Published, republished nothing, refused four malformed publications including the mutated-source control that now reaches the system-corpus export path, regenerated byte-identically from its recorded source commit | Direct |
| `just component_sdk_prefix_check` | Unaffected by this entry's changes; reverified green | Direct |
| `just component_sdk_compatibility_check` | Two immutable releases classified correctly; 5 scalar and 3 structural (crates, profiles, systems) negative controls forced their expected classification | Direct |
| `just component_sdk_upgrade_check` | Template consumer pinned, upgraded, rebuilt, survived five injected failures, rolled back byte-for-byte; unaffected by this entry beyond gate ordering | Direct |
| `just system_image_closure_aggregate_check`, `just sel4_boot_layout_check`, `just system_test_run_check` | Green after every closure and 45 test-run records were regenerated/reblessed to reflect the `just` directory's changed tree identity (`just/component-sdk.just` gained a new recipe, which is one of the closure's declared release inputs) | Direct |
| `just sel4_gate_control_check` | 45 gates, 1748 mutations, unchanged behavior | Direct |
| `just contracts_check`, `just fmt_check_all`, `just ruff`, `just machete`, `just test_host`, `just devlog_check` | All green | Direct |

## Decisions

- **The system corpus is a fixed set of repository roots, not a per-closure minimal slice.** `sel4-channel` is the one published closure; `COPY_ROOTS` names every path any closure's resolver or builder could read, so adding a second published closure later needs no corpus change.
- **The canonicalized prefix is checked against the SDK's own pinned release asset, not re-derived from nothing.** The naive fix (rebind the SDK-release reference's identity to its own unchanged bytes) was a no-op that proved nothing; the real invariant is that the corpus's rewritten prefix still matches what the release already vouches for.
- **`systems` is required, not optional.** An optional field the exporter always writes buys no compatibility and lets a corrupted record silently pass `verify_digests`.
- **A new devlog entry, not an addendum to the 2026-09-04 legacy-deletion entry.** That entry closed a distinct, already-landed deliverable (plane-gate migration and legacy deletion); the SDK publication clause is independently meaningful and was investigated and verified separately.

## Open risks and follow-ups

- [ ] Only `sel4-channel` is published as a system-image asset. Publishing additional closures is straightforward (`component_sdk_system.py` genuinely supports more than one, gated by `MAX_SYSTEMS`) but not done, since CP15 required only that the clause exist and be proven, not that every closure ship.
- [ ] The round-trip archive verification and the `.system-source` cache exclusion were found by review rather than by an initial design pass; no further review-driven gaps are open as of this entry.

## Artifacts and provenance

- Commits `9c37692ec7` (system-image corpus export, publication, and gate wiring), `f28b948955` (matrix evidence from this entry's verification runs), `103c9b1edc` (mirror-coverage and structural-axis fixes a full-severity review found).
- No `.rs` file changed in this entry's work.
