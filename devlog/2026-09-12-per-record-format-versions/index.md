# One `formatVersion` stamped three record types, two of which never changed

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/system-image-closure/v2/schema.zt` version constants, its `gen_python.zt` bindings, `scripts/lib/system_image_closure.py` build-result and negative-case decoding, `scripts/generate/generate-system-image-closures.py` negative-case emission, `slime-root/src/private_memory.rs` `GrowError::Frames`, `slime-root/src/main.rs` allocator static |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just contracts_check`, `just system_image_closure_check`, `just system_image_scenario_check`, `just system_test_run_check`, `just system_image_closure_aggregate_check`, `just sel4_root_boot_check`, `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | PR #25 review of `f996b47d` reported that the v2 bump stamped `SystemImageBuildResult` and `NegativeBuildCase` as v2 under v1 identity domains, that `GrowError::Frames` still documents returned backing, and that the allocator static carries a failed-experiment narrative |
| Baseline | Branch head `f996b47d`, where `formatVersion :: Int = 2` is the single constant all three record types stamp, while `buildResultIdentityDomain` and `negativeIdentityDomain` remain `*-v1:` |

## Summary

The closure contract's v2 bump moved one shared `formatVersion` constant, but
only `SystemImageClosure` changed shape. `SystemImageBuildResult` and
`NegativeBuildCase` kept their v1 identity domains — deliberately, and recorded
as such in the schema — while their records began declaring `formatVersion = 2`.
That is self-contradictory in exactly the way versioning exists to prevent: a
record asserting v2 hashed under a v1 domain, and a persisted v1 record of
either type had no decoder. Each format now carries its own version constant
paired with its own domain. Two smaller findings travelled with it: `GrowError::Frames`
still documented the extent-revoking rollback this branch removed, and the
allocator static's comment recorded a failed experiment rather than the
constraint that failure establishes.

## Observable symptom

- Command: `python3 scripts/generate/generate-system-image-closures.py` then reading `contracts/system-image-closure/v2/negative/sel4-b40-missing.zti`
- Expected: a record whose declared version matches the identity domain it hashes under
- Observed: `formatVersion = 2` in every negative case, against `negativeIdentityDomain = "slime-negative-build-case-v1:"`; `make_build_result` stamped the same `FORMAT_VERSION = 2` while hashing under `slime-system-image-build-result-v1:`
- Exit/fault/serial evidence: no gate failed, which is the point — both sides of the comparison were generated from the same constant, so the contradiction was invisible to every check

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `schema.zt` exported one `formatVersion`, consumed by all three record types' `formatVersion : Int` fields | A shape-specific declaration was being made by a contract-wide constant |
| 2 | The schema's own comment states `SystemImageBuildResult` and `NegativeBuildCase` "are unchanged; their identities move only through the closure identities they carry" | The contract already knew these shapes did not move; only the version constant disagreed |
| 3 | `make_build_result` writes `image_contract.FORMAT_VERSION` and hashes with `BUILD_RESULT_IDENTITY_DOMAIN` | A v2-declared record under a v1 domain |
| 4 | `compile_negative_case` compared against `FORMAT_VERSION`, and the generator emitted the same | Generator and decoder agreed with each other and with nothing else, so no gate could see it |

## Root cause

A version constant names a shape, and three shapes shared one constant. The v2
bump correctly split the *identity domains* per shape but left the *versions*
joined, so the two halves of the same statement disagreed for the two formats
that did not change.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Version constants | `buildResultFormatVersion = 1` and `negativeFormatVersion = 1` join `formatVersion = 2`, each beside its own identity domain | A format's declared version and its identity domain describe the same shape and move together |
| Bindings | `gen_python.zt` emits `BUILD_RESULT_FORMAT_VERSION` and `NEGATIVE_FORMAT_VERSION` | Generated consumers cannot reach for the wrong constant |
| Build results | `make_build_result` stamps `BUILD_RESULT_FORMAT_VERSION` | A build result decodes at the version it declares, under the domain it hashes with |
| Negative cases | Decoder and generator both use `NEGATIVE_FORMAT_VERSION`; all six records regenerate at `formatVersion = 1` | Persisted v1 negative cases remain decodable |
| `GrowError::Frames` | Documents that the attempt's objects are unwound but retained — frames unmapped, typed records reusable, leaf tables still mapped, no extent revoked | The error describes the rollback the code performs |
| Allocator static | States that the value must stay a static initializer because any constructing form materializes a multi-megabyte stack temporary, and that reducing image cost needs zero-valued sentinels | The comment states a constraint, not an experiment |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A record declares a version its decoder does not accept | `just system_image_closure_check` | `unsupported negative build case version N` / `unsupported image closure version N` |
| Generated bindings drift from the schema | `just contracts_check` | The bindings check reports the generated file is not current |
| Negative cases regenerate at the wrong version | `just system_image_scenario_check` | A negative case fails to compile or its identity moves unexpectedly |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | Passed; bindings current | Direct |
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified; closure `93f10144e918` built — the build result it writes and re-decodes now declares v1 under the v1 domain | Direct |
| `just system_image_scenario_check` | 53 closures resolve; 5 scenario and 5 root-role closures distinct; 8 malformed parameters refused | Direct |
| `just system_test_run_check` | 50 records, 46 resolving a closure, 4 closure-exempt | Direct |
| `just system_image_closure_aggregate_check` | 16 booted images, 8 closure-reachable and 8 exempt; 6 negative cases carry no plane image | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers | Direct |
| `just test_sel4_root` | 235/235 across 19 modules | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | Passed | Direct |
| Regenerated negative cases | All six emit `formatVersion = 1` | Direct |

## Decisions

- Decision: give each record type its own version constant rather than bumping the two unchanged domains to v2.
- Rationale: the schema already states these two shapes did not change, and their identities move only through the closure identity they embed. Bumping their domains would re-identify every persisted build result and negative case to assert a change that did not happen — the alternative this branch already rejected when introducing v2.
- Rejected alternative: keeping one constant and re-versioning all three domains together. Tidier to read, and false.

- Decision: describe the retained backing on `GrowError::Frames` rather than deleting the sentence.
- Rationale: what a caller needs is that `allocated` pages remain owned and available to a retry. Silence would leave the same question open for the next cleanup change, which is the misreading risk the reviewer named.

## Open risks and follow-ups

- [ ] The two RV64 recipe arms named in the preceding entry remain broken and out of scope here.
- [ ] The descriptor tables still cost ~1026 root CSlots of image on 19-bit kernels; eliminating that needs zero-valued sentinels in `AllocationRecord` and `ArenaAllocation`.

## Artifacts and provenance

- Focused report: none; each change is local to its named file.
- Raw transcript: none retained; every gate above is reproducible from the listed command.
- Serial/debugger/model output: `just sel4_root_boot_check` serial transcript from the pinned QEMU profile.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- Preceding investigation: [The descriptor tables are `.data`, not `.bss`, and three RV64 arms had no record](../2026-09-12-descriptor-residency-and-rv64-run-records/index.md)
