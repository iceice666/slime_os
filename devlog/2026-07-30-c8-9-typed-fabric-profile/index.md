# C8.9 — Typed full-profile and resource-bound closure

| Field | Value |
|---|---|
| Date | 2026-07-30 |
| Kind | Change |
| Status | Verified |
| Scope | Generation contracts, fabric graph admission, component build profile, normalized schema corpus, host checks |
| Roadmap | C8.9 |
| Gates | `just data_fabric_profile_check`, `just test`, `just lint`, `just lint_components`, `just fmt_check`, `just fmt_check_components` |
| Trigger | C8.9 implementation |
| Baseline | C8.8 authenticated graph bytes and userspace fabric tables were derived through separate manifest interpretations, while profile and shared-buffer fields were not fully schema-declared. |

## Summary

C8.9 closes the typed full-profile boundary. The generation contract now declares fabric profiles and shared-buffer budgets, one resolved profile produces both authenticated graph bytes and userspace tables, admission rejects mutually unsatisfiable resource ceilings, and admitted normalized interface bytes are retained in deterministic schema-identity order. The focused profile gate and the repository Rust/QEMU, lint, and formatting gates pass.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Generation and fabric schemas | Declared profile, interposition, resource-limit, shared-buffer-budget, and normalized-schema artifact fields in versioned Zutai contracts and regenerated bindings. | Load-bearing cross-boundary fields are schema-defined and bounded. |
| Generation builder | Resolves the selected full-graph profile once, then renders authenticated graph bytes, the userspace Rust profile, and the normalized schema corpus from that resolved value. | Host, kernel, and userspace cannot independently reinterpret route authority. |
| Admission | Checks kernel channel ceilings, fabric-holder pages/buffers/mappings/loans, generated capability slots, frame capacity, sample copy pages, and route-name bounds. | Individually legal but mutually unsatisfiable declarations fail before launch. |
| Component build | Consumes the generated profile when building a generation and uses a checked-in canonical default profile for standalone lint/check builds. | Generation builds use the selected authority; standalone builds remain deterministic and the focused gate detects stale fallback data. |
| Verification | Added malformed, duplicate, unknown, ambiguous, resource-conflict, deterministic-artifact, Python/Rust decoder, and fallback-parity checks. | Contract drift and split profile interpretation produce direct gate failures. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Profile/schema ambiguity or drift | `just data_fabric_profile_check` | Rejection corpus, artifact comparison, Rust decoder, or fallback-parity failure. |
| Kernel/userspace integration regression | `just test` | QEMU unit or integration test failure. |
| Rust warning or style regression | `just lint`, `just lint_components`, `just fmt_check`, `just fmt_check_components` | Nonzero command exit. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_profile_check` | Passed; deterministic generation artifacts, 50 boot-contract tests, 2 normalized-schema decoder tests, and the C8.9 negative corpus completed. | Direct |
| `just test` | Passed; kernel unit and integration suites completed under QEMU. | Direct |
| `just lint` | Passed. | Direct |
| `just lint_components` | Passed. | Direct |
| `just fmt_check` | Passed. | Direct |
| `just fmt_check_components` | Passed. | Direct |

## Decisions

- Decision: Keep the selected generation profile as a generated `SLIME_DATA_FABRIC_PROFILE` input to component builds, with a generated checked-in default only for standalone Cargo checks.
- Rationale: Generation builds preserve one resolved source of authority, while repository lint/check commands do not depend on a previously populated ignored `target/` directory or recursive Cargo invocation.
- Rejected alternative: Parse the generation fixture independently in `components/bins/build.rs`; that would recreate the split authority interpretation C8.9 removes.

## Open risks and follow-ups

- [ ] C8.10 must place the complete role graph into collision-free capability layouts and bounded route workers; C8.9 proves the declarations and generated ceilings, not simultaneous runtime bootstrap.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: observed through `just test` and `just data_fabric_profile_check`; no frozen sibling artifact retained.
- Related roadmap item: [`C8.9`](../../roadmap/02-core-runtime.md#c89--typed-full-profile-and-resource-bound-closure).
