# Canonical system-image and test-run closures

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/system-image-closure/v1`, `contracts/system-test-run/v1`, closure resolver and image builder, generation/image build helpers, contract gates |
| Roadmap | CP11 |
| Gates | `just system_image_closure_check`, `just sel4_fault_check`, `just sel4_boot_selection_check` |
| Trigger | CP11 implementation after the component SDK and system-spec foundations |
| Baseline | Image selection was implicit in `build-sel4.py` variant tables, ambient environment controls, source-owned target directories, and checker-known output paths; no one normalized record named every image input |

## Summary

A versioned system-image closure now binds the logical system, component implementations, immutable SDK/toolchain and platform inputs, root and loader roles, and declared outputs into one verified identity before any compilation begins. A separate system-test-run contract owns execution-only fixtures and oracles. The generic builder resolves one closure into isolated generation, root, loader, and image outputs and emits normalized identity records; the focused gate observed identical bytes across independent build roots and an adversarial ambient environment.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Image contract | Added bounded Zutai closure, generated Python bindings, normalized identity, canonical channel record, SDK release record, and immutable QEMU seL4 prefix input | Every executable-affecting input is named and digest-verified before compilation |
| Test contract | Added bounded test-run data for execution profile, fixtures, runtime fault controls, timeout, marker oracle, and forbidden outcomes | Execution policy can vary without changing image identity |
| Resolver | Added exact record validation, path confinement, file/tree digest checks, SDK/profile/prefix/toolchain pairing, component selection validation, role checks, and complete release-input coverage | No ambient build tree, current-directory fallback, component registry, or undeclared input can satisfy a closure reference |
| Builder | Added `build-system-image.py CLOSURE OUTPUT_DIR`, isolated Cargo and generation roots, explicit targets/toolchain/prefix, versioned image identity, and versioned build result | One non-interactive command shape produces every declared output from resolved data |
| Shared helpers | Added explicit resolved generation/toolchain/target/prefix arguments and a closure-only hermetic component build profile; platform comparisons use stable names rather than singleton identity | Closure isolation does not suppress legacy scenario controls or lose platform-specific configuration |
| Gates | Registered both contracts and added closure refusal, identity-boundary, bound, clean-root, and adversarial-environment checks | Reproducibility and fail-closed resolution are executable claims |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A missing, changed, wrong-target, wrong-profile, unrecorded, or ambient input reaches compilation | `just system_image_closure_check` | Named refusal arm is accepted or ambient environment changes an output |
| Two clean builds embed host paths or otherwise diverge | `just system_image_closure_check` | Generation, root, loader, image, image identity, or build-result bytes differ |
| Closure hermeticity suppresses the fault scenario's compile-time implementation profile | `just sel4_fault_check` | Injected interposition death or bounded degradation markers disappear |
| Closure helper changes suppress boot-selector failure behavior | `just sel4_boot_selection_check` | Selector refusal, rollback, or promotion evidence fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just system_image_closure_check` | Passed; contract/model prerequisites passed, named invalid records were refused, and three isolated builds produced identical declared outputs | Direct |
| `just sel4_fault_check` | Passed; 10 markers across three causal chains observed the injected interposition death while unrelated routes remained isolated | Direct |
| `just sel4_boot_selection_check` | Passed; persisted attempts, rollback, stale-format refusal, and health promotion remained observed | Direct |
| `just ruff` | Passed | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed | Direct |

## Decisions

- Decision: Give closure component builds the distinct `closure` build profile rather than overloading `default`.
- Rationale: `default` is the legacy behavior and deliberately admits scenario inputs used by existing gates; closure builds must instead strip every undeclared executable-changing `SLIME_*` value.
- Rejected alternative: Remove scenario variables globally in CP11. That is CP14's migration and would silently invalidate current behavioral gates before their replacement identities exist.
- Decision: Keep test-run identity separate from image-closure identity.
- Rationale: Marker oracles, writable disks, runtime faults, and timeouts do not alter executable bytes and one image may be exercised by multiple test policies.

## Open risks and follow-ups

- [ ] CP12 must give all 42 compositions reviewed system specs and complete image closures before any legacy composition becomes non-authoritative.
- [ ] CP13–CP15 must migrate callers and remove the variant/output-path surface; CP11 intentionally keeps legacy behavior intact during that cutover.
- [ ] CP14 must replace remaining executable-changing scenario controls with named implementation/root profiles rather than continuing to admit them to ordinary legacy builds.

## Artifacts and provenance

- Focused report: none; the contracts, resolver, builder, and gate are the executable record.
- Raw transcript: none retained.
- Serial/debugger/model output: `just system_image_closure_check`, `just sel4_fault_check`, and `just sel4_boot_selection_check` command output observed in the implementation session.
- Related roadmap item: `roadmap/10-component-platform.md` CP11.
