# C8.2 — Generation graph, QoS, and aggregate admission

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Change |
| Status | Verified |
| Scope | Zutai fabric-graph contract, boot-contracts decoder, generation manifest schema and fixture, generation builder, kernel generation decode, C8.2 checks |
| Roadmap | C8.2 |
| Gates | `just fabric_manifest_check` |
| Trigger | C8.2 opened after C8.1 landed the admitted interface set with no persistent route, QoS, or graph data |
| Baseline | C8.1 admitted interface schemas source-only; no generation carried a fabric graph, so no route, direction, QoS policy, visibility grant, interposition hop, or per-graph resource ceiling existed as generation data |

## Summary

C8.2 adds a versioned Zutai fabric-graph resource stored as an authenticated generation `KIND_RESOURCE` object, a bounded `no_std` decoder that validates it before any component launches, and host-builder admission that enforces the same rule set so a malformed graph fails the build rather than the boot. Route authority is the fold of route name, full C8.1 interface identity, and contract kind; participant authority additionally folds in component identity and direction, so possession of a name, a type string, or a graph observation grants nothing. The gate passes with a deterministic 896-byte resource, a 35-case negative corpus, 18 decoder tests, and 4 QEMU tests against the booted generation.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-graph/v1/` | Defined the schema/route/participant/interposition records, the `TransportQoS` vocabulary, the identity domains, and every structural ceiling; renders Python and Rust bindings through the shared `wire.python`/`wire.rust` modules | Zutai is the sole source contract for the format, and the two readers cannot disagree on layout |
| `boot-contracts/src/fabric_graph.rs` | Bounded decoder plus `validate_against`: sorted-unique tables, enum admissibility, index resolution, grant-identity re-derivation, QoS coherence, interposition acyclicity, canonical reserved bytes, declared-limit ceilings, and aggregate demand | A graph that decodes is one the fabric can honour with every declared participant live at once |
| `contracts/generation/v1/` | Added the optional `fabricGraph` manifest section and its fixture instance (2 routes, 4 participants, 1 interposition hop) | Graph, QoS, visibility, interposition, and resource ceilings are deterministic generation data, not runtime policy |
| `scripts/build/build-generation.py` | Emits the resource from admitted C8.1 interfaces; enforces limit ceilings, route budget, per-direction and ingress demand, loan/mapping coherence, sample bounds, and the full QoS truth table before packing | A manifest error fails the build instead of producing a green artifact that panics at boot |
| `kernel/src/runtime/generation.rs` | Validates a present graph at decode against the kernel's real ceilings, and pins the contract's copies of `MAX_MSG`/`MAX_TOTAL_PAGES`/`MAX_MAPPINGS`/`MAX_LOANS` with `const _: () = assert!` | Admission uses the kernel's numbers on both sides, and a bad graph fails the whole generation closed before launch |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A name, type, or graph observation becomes authority | `just fabric_manifest_check` → `route_authority_is_the_exact_tuple` | An ungranted component or an unheld direction resolves to a participant entry |
| Builder and decoder rule sets drift, so a bad graph builds green and panics at boot | `just fabric_manifest_check` → 35-case negative corpus, each required to fail via a builder `fail()` rather than an incidental exception | A mutation produces an artifact, or is rejected by `struct.error`/`KeyError` instead of a check |
| The contract's copies of the kernel bounds drift from the kernel | `const _: () = assert!` in `kernel/src/runtime/generation.rs`; any kernel build | Compile error at the assertion |
| An interposition chain gains a cycle or a self-hop bypass | `just fabric_manifest_check` → `declared_interposition_chain_terminates_without_bypass` and `interposition_cycles_and_bypasses_fail_closed` | A chain revisits a hop, exceeds the ceiling, or names its own participant |
| The resource stops being deterministic or canonical | `just fabric_manifest_check` (two builds compared) and `non_canonical_reserved_bytes_fail_closed`; `just generation_check` | Differing bytes across identical input, or two byte-distinct resources decoding identically |
| Offered/requested QoS acquires an implicit default | `offered_requested_compatibility_is_a_fixed_truth_table` | A request is satisfied by a strictly weaker offer on any axis |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_manifest_check` | Passed: deterministic 896-byte resource (2 schemas, 2 routes, 4 participants, 1 hop), 35-case negative corpus, 18 `boot-contracts` decoder tests, 4 QEMU tests on a real boot | Direct |
| `just contracts_check` | Passed, including the new fabric-graph schema/renderer and 42 `boot-contracts` lib tests | Direct |
| `just generation_check` | Passed: two byte-identical builds carrying the graph resource | Direct |
| `just test` | Passed: 165 assertions | Direct |
| `just sample_plane_live_check` | Passed: the C7 live plane is unaffected by the added resource object | Direct |
| `just framework_safety_check` | Passed | Direct |
| `just fmt_check`, `just lint`, `just fmt_check_components`, `just lint_components` | Passed | Direct |
| Fault injection: 35 manifest mutations run through the real builder | Each rejected with its intended, correctly attributed message; boundary values (`lifespan == deadline`, `historyDepth == limit`, `routes == len(table)`, `sampleBytes == ceiling`, `ingressSources == 8`, `bufferPages == 256`, `mappings == 64`, `loans == 64`) still build | Direct |

## Decisions

- Decision: Make route identity the fold of (route name, full interface identity, contract kind), and participant grant identity the fold of (route identity, component identity, direction).
- Rationale: The roadmap requires authority to derive from the exact tuple. Folding it into one 32-byte value makes a forged claim structurally detectable — the decoder re-derives every grant identity and rejects a mismatch — while keeping lookup a single comparison. Alternate names over one interface, conflicting interfaces under one name, and the same edge in the opposite direction all remain distinct authorities by construction.
- Rejected alternative: Storing name/type/direction as separate matched fields would make authority a multi-field comparison that a partial match could satisfy, and would put a variable-length name in the authenticated resource.
- Decision: Duplicate the admission rule set in the host builder rather than relying on the kernel decoder alone.
- Rationale: The decoder is the arm that protects a boot, but a producing side that does not check emits an artifact the kernel refuses, turning a manifest typo into a boot panic while `just generation_check` stays green. The first review round found exactly this: 14 distinct malformed graphs built clean 896-byte artifacts. The roadmap requires these to fail *before* launch.
- Rejected alternative: Leaving validation to the decoder only, on the grounds of one source of truth. That reading is what produced the green-build/unbootable-image gap; the gate now requires every negative case to be rejected by a builder check, so the duplication cannot silently rot.
- Decision: Pin the kernel bounds the builder needs (`MAX_MSG`, `MAX_TOTAL_PAGES`, `MAX_MAPPINGS`, `MAX_LOANS`) as schema constants with a kernel-side `const _: () = assert!`.
- Rationale: The builder cannot import kernel constants, and a hand-copied number would drift silently. A schema constant plus a compile-time assertion makes divergence a build error instead of a wrong admission.
- Rejected alternative: Hardcoding the numbers host-side with a comment, which is exactly the drift the assertion exists to prevent.
- Decision: Require every per-entry reserved and trailing-padding byte to be zero.
- Rationale: The resource is authenticated by its per-object digest, so a field the decoder skipped would let two byte-distinct resources with different digests decode to an identical graph, weakening "one authenticated resource deterministically fixes the graph" and consuming the fields' forward-compatibility headroom.
- Decision: Treat an incompatible offered/requested QoS pair as admissible data reported by `all_pairs_qos_compatible`, not as a decode error.
- Rationale: C8.5 owns incompatible-QoS as a structured event; refusing the generation would make an event the fabric is specified to emit unrepresentable.

## Open risks and follow-ups

- [ ] The fixture names `init` as the fabric component because it is today's provisioning holder; C8.3 must move that to the dedicated long-lived fabric service and update the manifest.
- [ ] `queueDepth`, `eventDepth`, `retries`, `inFlightCalls`, and `inFlightOperations` are admitted and bounded but nothing consumes them yet; C8.4–C8.7 must charge against them or they remain unexercised declarations.
- [ ] The graph declares visibility and interposition, but no component reads them; C8.8 owns filtered introspection and the declared proxy chain, so "no bypass" is proven structurally here and behaviourally there.
- [ ] The builder duplicates the decoder's rule set by design. The negative corpus is the only thing keeping the two in step; a rule added to one side without a corpus case would reintroduce the round-1 gap.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none captured; the QEMU arm's four assertions are reported by `just fabric_manifest_check`.
- Related roadmap item: [`C8.2`](../../roadmap/02-core-runtime.md#c82--generation-graph-qos-and-aggregate-admission).
