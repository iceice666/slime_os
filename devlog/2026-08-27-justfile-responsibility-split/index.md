# Justfile split by responsibility with parsed gate metadata

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | Justfile imports, devlog gate discovery, component-spec gate evidence, host script metadata helper |
| Roadmap | none |
| Gates | `just devlog_check`, `just component_spec_check`, `just fmt_check_all`, `just ruff` |
| Trigger | Grouping made the 171-command responsibility model visible, but all recipes still lived in one 1,401-line root Justfile and two semantic checks parsed only that physical file |
| Baseline | Moving any recipe into an imported Justfile made devlog targets or component-spec test environments appear nonexistent, and component marker discovery could not follow an imported recipe body |

## Summary

The root `Justfile` is now an 18-line façade importing eleven responsibility-owned files under `just/`. All 173 parsed recipe declarations, including two private recipes, retain byte-equivalent parsed Just metadata: names, parameters, attributes, dependencies, bodies, documentation, privacy, and the default recipe are unchanged. Devlog gate validation and component-spec test evidence now consume `just --dump --dump-format json`, so their view follows imports and Just's parser instead of maintaining a second partial parser for one physical file.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Root task interface | Replaced the monolithic root recipe body with eleven imports plus the private chooser recipe | The root file declares the task architecture instead of accumulating every implementation |
| Responsibility files | Moved recipes into product, four plane, contracts, generation, component SDK, hardware, quality, and compatibility files | Each task implementation has one stable ownership location matching its visible group |
| Parsed metadata | Added `scripts/lib/just_metadata.py`, which caches the recipe table emitted by Just itself | Checkers observe the fully imported task graph with the same parser that executes it |
| Devlog validation | Replaced root-file recipe regex discovery with parsed metadata target discovery | Recorded `Gates` identifiers remain valid after recipes move between imported files |
| Component specification | Replaced root-file target and recipe-body regexes with parsed recipe targets, dependencies, and command bodies | `requiredTestEnvironment` and `passFailCriteria` validation follow imported gates without weakening marker evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A move changes a recipe name, parameter, dependency, command body, attribute, documentation, privacy, or default selection | Exact normalized comparison of pre-split and post-split `just --dump --dump-format json` | Any metadata field differs |
| A devlog gate is hidden by an import | `just devlog_check` | A recorded gate identifier is reported missing |
| A component spec points at a missing gate or a criterion its gate does not observe | `just component_spec_check` | Corpus admission or one of 43 negative controls fails |
| Imported files are syntactically invalid or non-canonical | `just --fmt --check` | Just rejects or would rewrite a task file |
| Python metadata handling violates repository style | `just ruff` | Ruff reports a changed-script diagnostic |
| The structural split disturbs Rust formatting | `just fmt_check_all` | Workspace formatting fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre/post normalized Just JSON comparison | Passed; complete metadata objects were exactly equal across all 173 declarations | Direct |
| `just --groups`, `just --summary` | Passed; 11 public groups and 171 public recipe names remained available | Direct |
| Cross-import dry runs | Passed for dependency-only `sample_descriptor_check` and six-prerequisite `runtime_binding_resolution_check`; dependencies resolved across responsibility files | Direct |
| `just devlog_check` | Passed after registration; 233 entries and 233 index rows validated through imported recipe metadata | Direct |
| `just component_spec_check` | Passed; 42 records admitted, 43 named mutations refused, identities stable | Direct |
| `just --fmt --check` | Passed across the root and all imported Justfiles | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just ruff` | Passed | Direct |

## Decisions

- Decision: use ordinary `import` files, not Just modules.
  Rationale: imports preserve the existing top-level command surface and allow dependencies to cross responsibility files without namespacing or wrapper recipes.
  Rejected alternative: modules, which would change invocations to paths such as `just product run` and isolate dependencies between modules.
- Decision: treat `just --dump --dump-format json` as the recipe metadata authority.
  Rationale: Just already resolves imports, attributes, parameters, dependencies, interpolated command parts, and private recipes; duplicating that grammar in Python would become a second incomplete parser.
  Rejected alternative: recursively regex every `.just` file, which would still misparse multiline recipes and dependency syntax.
- Decision: keep every group attribute on each recipe after moving it into a group-owned file.
  Rationale: the group remains explicit metadata visible through Just rather than an inference from the containing filename.
  Rejected alternative: derive grouping from paths in a wrapper tool, which would make ordinary `just --list` flat again.
- Decision: follow only one prerequisite level when a component test gate has no direct check script.
  Rationale: this preserves the component-spec check's existing evidence contract exactly while changing only its parser; expanding transitive semantics belongs in a separate behavior change.
  Rejected alternative: recursively traverse the full dependency graph during this structural cut, which could silently broaden which script literals satisfy an existing criterion.

## Open risks and follow-ups

- [ ] `just/quality.just` remains the largest responsibility file at 378 lines; split it only if stable sub-ownership emerges between formatting, linting, host tests, and security tools.
- [ ] Components and contracts still need their own catalog/discovery cuts; this change deliberately moved orchestration only.

## Artifacts and provenance

- Focused report: none; this entry is the focused record
- Raw transcript: command output observed directly in the implementation session; no separate frozen transcript was added
- Serial/debugger/model output: none; the changed surface is task parsing and host-side contract evidence
- Related roadmap item: none
