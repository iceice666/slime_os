# Generation manifest source contract separated from boot wire versions

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation-manifest/v1`, `contracts/generation/v{2..5}`, generation builders/checks, current documentation, historical devlog links |
| Roadmap | CP1 |
| Gates | `just contracts_check`, `just system_spec_check`, `just generation_check`, `just devlog_check` |
| Trigger | The host manifest source schema and boot binary schemas shared one version namespace, while product compositions were mixed with schema-conformance fixtures. |
| Baseline | `contracts/generation/v1` was the host source schema, `contracts/generation/v5` was the current boot wire schema, and 49 product inputs lived beside two conformance fixtures. |

## Summary

Moved the host-side source contract to `contracts/generation-manifest/v1`, split its two schema-conformance fixtures from 36 product and plane compositions, and left `contracts/generation/v{2..5}` as the boot-time binary format history. Shared path constants now keep builders and gates on one directory spine. The rebuilt `sel4` and `sel4-qos` generations are byte-identical to their pre-move baselines.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract tree | Moved source schema and checks to `contracts/generation-manifest/v1`; moved `sel4*.zti` and rationale files to `compositions/` | Source-schema version 1 no longer reads as an obsolete boot-wire version beside v2–v5 |
| Host tooling | Added `GENERATION_CONTRACT`, `GENERATION_FIXTURES`, and `GENERATION_COMPOSITIONS` in `scripts/lib/harness.py`; repointed builders, gates, and system-spec derivation | Every executable consumer resolves the split through one shared path definition |
| Authority gate | Updated the framework writer inventory to retain the existing run/idle service pairs now visible when all compositions are scanned | The gate validates the unchanged seven authorized product compositions rather than assuming one writer instance per file |
| Documentation | Updated current source references and corrected six live links in five frozen devlog entries with appended corrections | Current documentation names the new boundary and historical links remain resolvable without rewriting conclusions |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A composition path was missed | `just contracts_check` | Any of the 36 manifests fails to encode as SLIMEG5 |
| Derived outputs land in the wrong directory | `just system_spec_check` | `valid.zti` or `sel4-channel.zti` reports stale or missing |
| Relocation changes emitted bytes | Direct `cmp` of pre/post `generation.bin` for `sel4` and `sel4-qos` | Non-zero comparison or changed SHA-256 digest |
| Historical links break | `just devlog_check` | Dead relative link or invalid correction section |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre/post `cmp` for `sel4` | Identical; SHA-256 `3c271a906f1765530ac93ea33f8b0992e3a9573c932c92823665c1ab922d09b5` | Direct |
| Pre/post `cmp` for `sel4-qos` | Identical; SHA-256 `e7e3c62e86a8c5e617be0689b70ccfc4362ef8f67791233616fa007abe53b0fd` | Direct |
| `just system_spec_check` | Passed; both derived manifests current and all 36 seL4 manifests encoded as SLIMEG5 | Direct |
| `just framework_safety_check` | Passed; seven product compositions grant `blockWrite` only to approved service owners | Direct |
| `just contracts_check` | Passed; contract checks, bindings, and all 36 SLIMEG5 composition builds succeeded | Direct |
| `just generation_check` | Passed, including the real seL4 component-graph QEMU boot and determinism checks | Direct |
| `just devlog_check` | Passed: 228 entries indexed with no dead relative links | Direct |
| `just ruff`; `just typos`; `just fmt_check_all`; `just lint_all` | Passed | Direct |

## Decisions

- Decision: preserve manifest `formatVersion = 1` while changing only its namespace and directory ownership.
- Rationale: the source format version is embedded in every manifest and is independent of boot binary versions.
- Rejected alternative: deriving composition membership from a glob; the explicit `SEL4_MANIFESTS` table remains the authoritative plane inventory.

## Open risks and follow-ups

- [ ] None specific to the relocation.

## Artifacts and provenance

- Approved execution plan: `local://generation-contract-split-plan.md` in the implementing session.
- Related roadmap item: [CP1](../../roadmap/10-component-platform.md)
