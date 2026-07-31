# C8.1 — Deterministic interface schemas and native bindings

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Change |
| Status | Verified |
| Scope | Zutai interface contracts, host normalization/admission, generation builder, slime-proto bindings, C8.1 checks |
| Roadmap | C8.1 |
| Gates | `just interface_schema_check` |
| Trigger | C8.1 opened after the C7 sample plane and B2 wait-set prerequisites completed |
| Baseline | C7 carried an unconstrained caller-chosen nonzero `u64` descriptor type identity and had no native interface-schema identity or binding set |

## Summary

C8.1 adds a bounded Zutai source contract for native interfaces, deterministic normalization and domain-separated identities, collision-checked generation-local type tags, and generated no-allocation Rust bindings for stream, call, and operation contracts. The generation builder now admits the declared schema set before creating its output directory, and the focused gate passed with deterministic generation output and host binding tests.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/interface-schema/v1/` | Defined the versioned schema AST, normalization domains, semantic ceilings, and representative Stream/Call/Operation inputs | Zutai is the sole source contract and equivalent declarations have one bounded normal form |
| `scripts/lib/interface_schema.py` | Added eager shape, name, kind, depth, field, sequence, encoded-size, normalized-size, generated-size, and admitted-set validation | Malformed or over-bound input fails before bindings or generation artifacts are emitted |
| Generation builder | Admits manifest-declared schemas, full identities, and derived tags before component or artifact construction | Distinct full identities cannot share one admitted descriptor tag |
| `slime-proto` | Generates full embedded identities, descriptor tags, bounded native records/sequences, and Stream/Call/Operation marker types | Native bindings remain `no_std`, bounded, deterministic, and allocation-free |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Formatting or declaration order changes identity/output | `just interface_schema_check` | Two-run normalized bytes, identities, tags, or generated Rust differ |
| Semantic layout changes reuse an identity | `just interface_schema_check` | Width, signedness, bound, field order, nesting, or contract-kind mutation retains the original digest |
| Malformed, duplicate, over-bound, or colliding schemas reach output | `just interface_schema_check` | Negative corpus compiles, emits bindings, or admits a forced collision |
| C7 descriptor no longer accepts the generated local tag | `just interface_schema_check` | `slime-proto` interface-schema test rejects the generated telemetry tag or accepts a different schema's tag |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just interface_schema_check` | Passed: contract/binding freshness, deterministic generation builds, normalization/mutation/negative corpus, and 3 generated-binding tests | Direct |

## Decisions

- Decision: Hash canonical schema records with `SHA-256("slime-interface-schema-v1:" || normalized_bytes)` and derive the local `u64` tag from a second domain-separated digest of the full identity.
- Rationale: The full digest remains authoritative; the retained C7 descriptor stays wire-stable while admission makes truncation collisions fail closed.
- Rejected alternative: Treating the 64-bit descriptor field as the schema identity would make a truncated lookup key authoritative and could not distinguish a forced collision.
- Decision: Keep C8.1 admission source-only; C8.2 will define the persistent fabric-graph resource containing admitted schemas and routes.
- Rationale: This closes C8.1's pre-artifact identity/tag checks without pulling graph, QoS, or route persistence into the schema milestone.
- Rejected alternative: Adding the C8.2 graph resource early would combine two independently gated state surfaces.

## Open risks and follow-ups

- C8.2 must persist the admitted full identities, tags, endpoint grants, and aggregate graph bounds; C8.1 intentionally supplies no fabric routes or runtime policy.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none; C8.1 is a host contract/generation and native-binding gate.
- Related roadmap item: [`C8.1`](../../roadmap/02-core-runtime.md#c81--deterministic-interface-schemas-and-native-bindings).
