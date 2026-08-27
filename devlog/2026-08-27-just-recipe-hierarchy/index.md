# Just recipe hierarchy without gate renames

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | Justfile recipe navigation and preserved devlog gate identifiers |
| Roadmap | none |
| Gates | `just devlog_check`, `just fmt_check_all` |
| Trigger | The repository exposed 171 public recipes as one flat list spanning product boots, QEMU planes, contracts, generation, SDK work, quality gates, hardware operations, and historical aliases |
| Baseline | `just --groups` reported no recipe groups, so task discovery exposed no responsibility hierarchy |

## Summary

Every Just recipe now carries one responsibility group while all recipe names, parameters, dependencies, and bodies remain unchanged. `just --list` separates the public surface into product, mechanism/runtime/fabric/storage planes, contracts, generation, component SDK, hardware, quality, and compatibility aliases. Historical gate identifiers remain top-level recipes, so existing roadmap and devlog references continue to resolve.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Task navigation | Annotated every recipe declaration with one `group` attribute | A developer can browse tasks by responsibility instead of scanning one flat 171-recipe namespace |
| Compatibility surface | Collected historical, roadmap-named, and fail-closed compatibility recipes in the `compatibility` group without renaming them | Existing evidence links and operator commands keep their stable identifiers |
| Verification surface | Separated canonical product and plane gates from generators, contract checks, quality gates, component SDK checks, and physical hardware operations | Task placement communicates lifecycle and evidence class without changing execution semantics |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A recipe is renamed, removed, or accidentally duplicated while grouping | Before/after declaration comparison plus `just --summary` | The ordered recipe-name sequence or public recipe count changes |
| Group annotations make the Justfile invalid or non-canonical | `just --fmt --check` | Just rejects or would rewrite the file |
| Historical devlog gates stop resolving | `just devlog_check` | A recorded `Gates` identifier is reported missing |
| The structural edit disturbs repository formatting | `just fmt_check_all` | A Rust workspace formatting check fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Before/after recipe declaration comparison | Passed; the same 173 declarations remained in the same order, including two private recipes | Direct |
| `just --groups` | Passed; 11 public responsibility groups listed (`compatibility`, `component sdk`, `contracts`, `generate`, `hardware`, four plane groups, `product`, `quality`) | Direct |
| `just --list` and `just --summary` | Passed; 171 public recipes remained available and were rendered by group | Direct |
| `just --fmt --check` | Passed after applying Just's formatter | Direct |
| `just devlog_check` | Passed after registration; 232 entries and 232 index rows validated, including every recorded gate identifier | Direct |
| `just fmt_check_all` | Passed | Direct |

## Decisions

- Decision: add groups before splitting the Justfile into imports.
  Rationale: grouping changes only navigation metadata, immediately tests the responsibility taxonomy, and preserves the checker that currently discovers devlog gates directly from the root `Justfile`.
  Rejected alternative: split recipes into imported files first, which would require changing and validating gate discovery in the same step as moving 171 public commands.
- Decision: retain compatibility aliases as top-level recipes in a dedicated group.
  Rationale: roadmap and devlog evidence treats recipe identifiers as stable references; names can remain resolvable without dominating the canonical task lists.
  Rejected alternative: remove or namespace historical aliases, which would invalidate recorded evidence and operator commands.
- Decision: classify by responsibility and evidence surface rather than filename or recipe prefix alone.
  Rationale: `sel4-*` contains product, mechanism, fabric, storage, and hardware tasks with materially different use and cost.
  Rejected alternative: one broad `sel4` group, which would reproduce the existing flatness inside a new label.

## Open risks and follow-ups

- [ ] `scripts/check/check-devlog.py` still discovers recipes by parsing only the root `Justfile`; update it to consume Just metadata before recipes are moved into imported files.
- [ ] Component and contract physical paths remain flat; their classification and discovery changes should follow as separate, behaviorally verified cuts.

## Artifacts and provenance

- Focused report: none; this entry is the focused record
- Raw transcript: command output observed directly in the implementation session; no separate frozen transcript was added
- Serial/debugger/model output: none; this change affects task discovery, not product runtime behavior
- Related roadmap item: none
