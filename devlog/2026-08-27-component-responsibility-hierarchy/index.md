# Component crates grouped by lifecycle responsibility

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | `components/`, workspace membership, component discovery, source-path checks, build support, repository navigation, component onboarding documentation |
| Roadmap | none |
| Gates | `just component_crate_split_check`, `just sel4_component_graph_check` |
| Trigger | The flat `components/bins/` directory mixed shipped system code, resident services, applications, and verification-only components. |
| Baseline | Each component was already an independent CP3 crate with stable package and binary identities, but physical location carried no lifecycle ownership. |

## Summary

The 65 independent component crates now live under four responsibility roots: `system/`, `services/`, `applications/`, and `testkit/`. Discovery is recursive and canonicalized in `scripts/lib/component_paths.py`; build and verification code resolves components by binary identity rather than reconstructing a flat path. Cargo package names, binary targets, feature tables, component specifications, and the generated product graph remain unchanged, and the seL4 component graph booted successfully after the move.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Component tree | Moved 3 system crates, 4 resident services, 7 applications, and 51 probes/fixtures/workers into lifecycle-owned roots. | Directory location communicates whether a crate ships as platform policy, resident service, application, or verification apparatus. |
| Workspace and manifests | Replaced the flat member glob with four lifecycle globs and repaired relative dependencies without renaming packages or binary targets. | Cargo continues to expose the same 65 component package and target identities. |
| Discovery | Added `scripts/lib/component_paths.py`; component specification and source-sensitive checks consume its recursive binary-to-crate catalogue. | One manifest-derived lookup owns component location; callers do not grow category conditionals or flat-path assumptions. |
| Build support | Changed linker-script fallback discovery to walk ancestors to the shared `components/` linker root. | In-tree builds remain independent of lifecycle nesting depth. |
| Structural guard | Updated `check-component-crate-split.py` to inspect all four roots and reject category-level crates while allowing private modules inside leaf crates. | Lifecycle roots are grouping boundaries, and every component remains one independently buildable leaf crate. |
| Navigation | Updated `AGENTS.md` and `docs/getting-started/05-add-a-component.md` to route maintainers and new component authors through the lifecycle-owned roots. | Repository guidance and copyable examples match the executable tree. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A moved or newly added crate is omitted from discovery, violates the one-crate/one-binary shape, or turns a category root into a crate. | `just component_crate_split_check` | Missing crate/spec identity, invalid manifest shape, absent release profile, or category-level `Cargo.toml`/`src` fails the gate. |
| Cargo path changes alter the built generation or prevent components from linking and launching. | `just sel4_component_graph_check` | Product image build, root admission, component launch, or ordered graph markers fail. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre/post `cargo metadata --format-version 1 --no-deps` comparison | All 65 `slime-component-*` package names, target names/kinds, and feature tables were byte-equivalent after normalization. | Direct |
| `python3 scripts/check/check-component-crate-split.py` | Passed: 65 crates across 4 lifecycle roots; category roots are not crates. | Direct |
| `just test_host` | Passed host contract/protocol unit suites. | Direct |
| `just sel4_component_graph_check` | Passed seL4 pin verification, rebuilt the component graph image, and observed the required QEMU boot markers. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied. | Direct |
| `just ruff` | Passed. | Direct |
| `just typos` | Passed after updating the component onboarding guide. | Direct |

## Decisions

- Decision: Group by lifecycle responsibility, not protocol domain or current composition.
- Rationale: A component has one lifecycle owner even when it participates in several protocols and generation compositions; this keeps every crate at one canonical path.
- Rejected alternative: Domain folders such as `fabric/`, `storage/`, and `generation/` would make multi-protocol components ambiguous and couple source layout to changing product compositions.
- Decision: Keep component identity derived from each leaf `Cargo.toml` and centralize only path lookup.
- Rationale: Package and binary names are stable product identities; category names are repository organization and must not leak into generation contracts.
- Rejected alternative: Encoding category in package names or component specifications would turn a source-tree refactor into a wire/product identity migration.

## Open risks and follow-ups

- [ ] The 51-crate `testkit/` root may eventually deserve a second grouping level by verification mechanism, but only after discovery and workspace membership deliberately support that depth; this change does not invent that hierarchy prematurely.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: observed through `just sel4_component_graph_check`; no sibling capture retained.
- Related roadmap item: none.
