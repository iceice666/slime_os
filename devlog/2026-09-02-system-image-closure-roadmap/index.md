# System-image closures replace composition build variants

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/10-component-platform.md`, `roadmap/README.md`, future `contracts/system-image-closure/v1`, future `contracts/system-test-run/v1`, `scripts/build/build-sel4.py`, seL4 plane checkers |
| Roadmap | CP11, CP12, CP13, CP14, CP15 |
| Gates | none |
| Trigger | Request to make every current test composition a Nix-like closure that directly builds a bootable image |
| Baseline | Forty-two seL4 composition manifests feed a generation builder, while `build-sel4.py` selects them through roughly forty plane flags, four variant tables, environment-applied generation deltas, compile-time scenario switches, and source-owned output paths; checkers name those variants and paths directly |

## Summary

The existing component SDK, system specification, generation builder, target-qualified platform prefixes, and seL4 image packager already contain the mechanisms required to build a bootable image from declared inputs. What remains missing is one canonical identity-bearing closure that binds those inputs and one generic builder that consumes it. CP11–CP15 add that layer, convert all 42 current test compositions, make executable-changing scenarios explicit, migrate the full QEMU and SDK corpus, and delete the variant surface. The test oracle remains separate: a different disk fixture, timeout, marker contract, or runtime fault schedule does not become a different executable image unless it changes image bytes.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Component-platform roadmap | Added CP11–CP15 for closure contracts, complete composition derivation, data-driven image building, explicit scenario identities, and whole-corpus cutover | Every planned migration step has a bounded deliverable, gate, and observable deletion condition |
| Roadmap index | Reopened the component-platform track at CP11 and extended its dependency graph through CP15 | The canonical roadmap identifies the next open gate and does not imply CP10 completed system-image composition |
| Build/test boundary | Chose separate `system-image-closure/v1` and `system-test-run/v1` contracts | Reproducible build identity is not polluted by checker-owned disks, emulator actions, timeouts, or marker oracles |
| Scenario identity | Required compile-time fault and behavior variants to be named implementation or root profiles in the closure; invalid mutations remain separately typed negative cases | No ambient environment switch can change executable or generation bytes without changing the build identity |

## Decisions

- Decision: Introduce a versioned system-image closure above `SystemSpec`, `ComponentSpec`, the SDK release, and platform-prefix contracts rather than expanding any one of those records into a universal manifest.
- Rationale: The existing contracts own distinct facts: component-local behavior, logical composition and authority, released build inputs, and platform artifacts. A closure should reference their identities and bind them into one build graph, not restate their fields and create a second authority vocabulary.
- Decision: Keep image construction and test execution as separate closures.
- Rationale: QEMU disks, network/device fixtures, corruption schedules, timeouts, and marker expectations often change without changing the executable image. Including them in the image closure would make identical image bytes appear to have different build identities and would move checker policy into the product builder.
- Decision: Convert all 42 current composition manifests before deleting the legacy builder surface.
- Rationale: A partial permanent cutover would leave two composition conventions and make every future change choose between them. CP12 establishes byte-identical generated closure coverage first; CP13–CP15 then migrate callers and remove the old path cleanly.
- Decision: Treat environment-driven generation deltas and compile-time scenarios differently according to whether they change generation data, executable bytes, or only runtime fixtures.
- Rationale: Generation fields belong directly in closure data; executable-changing scenarios require distinct implementation identities; runtime-only fixtures belong in the test-run contract. One untyped `parameters` map would hide these security- and reproducibility-relevant distinctions.
- Decision: Do not build a general Nix-compatible package manager, dependency solver, distributed builder, binary cache, or garbage collector as part of CP11–CP15.
- Rationale: The required product capability is narrower: a complete normalized closure deterministically produces a target-qualified bootable image. General package-management machinery would add authority and failure modes without being needed to remove the current variant coupling.
- Rejected alternative: Keep `build-sel4.py` plane flags as friendly aliases over the closure builder indefinitely. Rejected because aliases preserve a second source-owned mapping from scenario names to closure paths and output names; CP15 requires a clean cutover, with convenience commands generated from or explicitly naming closure data instead.
- Rejected alternative: Put fault injection, disks, marker contracts, and build inputs into one test composition record. Rejected because it conflates artifact identity with execution policy and prevents one image from being exercised by multiple independent test runs without rebuilding or renaming it.
- Rejected alternative: Rewrite every `.zti` composition by hand as another complete closure. Rejected because immediate-mode `.zti` is inert data and cannot share common structure; system specs and pure `.zt` generators should own derivation, while each resolved closure remains complete canonical data for the builder.

## Open risks and follow-ups

- [ ] CP11 must choose the minimal reference and identity encoding without duplicating the fields already authenticated by SDK and platform-prefix release records.
- [ ] CP12 may expose generation-manifest fields that `system-spec/v1` cannot currently derive; each requires an owning semantic field or an explicit proof that it is obsolete, never a composition-name branch.
- [ ] CP14 must classify every existing `SLIME_*` build knob by whether it changes generation data, component bytes, root bytes, platform instrumentation, or runtime-only test input.
- [ ] Boot-selector and physical-board instrumentation closures need closed root/platform role vocabularies that cannot be selected by ordinary product compositions.
- [ ] The complete-corpus byte-identical baseline must be captured before implementation; any unavoidable identity movement requires a separate reviewed decision rather than silent rebasing.

## Artifacts and provenance

- Focused report: none; the planned decomposition and acceptance conditions are in `roadmap/10-component-platform.md` CP11–CP15.
- Raw transcript: none retained.
- Serial/debugger/model output: none; this is a proposed roadmap and architecture decision.
- Related roadmap item: `roadmap/10-component-platform.md` (CP11–CP15) and `roadmap/README.md`.
