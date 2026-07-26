# C7.6 versioned sample descriptor

| Field | Value |
|---|---|
| Date | 2026-07-25 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/sample-descriptor/v1/`, `scripts/generate/generate-sample-descriptor-bindings.py`, `components/proto/src/{sample_descriptor.rs,lib.rs}`, `kernel/tests/sample_descriptor.rs`, `scripts/check/check-contracts.py`, `Justfile`, `docs/capability-matrix.md`; `just sample_descriptor_check` |
| Roadmap | C7.6 |
| Gates | `just sample_descriptor_check`, `just contracts_check` |
| Trigger | Roadmap C7 decomposition; C7.6 defines the versioned sample-descriptor contract over the C7.4/C7.5 sealed-mapping and loan lifecycle |
| Baseline | C7.5 loan lifecycle: a lender loans one exact sealed, page-aligned subrange to a named receiver through a `SharedBufferLoan` object with an unforgeable single-return identity; the loan mechanism was exercised directly by the gate, with no descriptor contract referencing the loan identity, offset, length, or type |

## Summary

C7.6 adds the versioned control-plane descriptor that lets a receiver validate a
bounded reference to a transferred loan before touching it. A new Zutai contract
(`contracts/sample-descriptor/v1/`) renders byte-identical `slime-proto` bindings
(`WireSampleDescriptor`) whose fixed control message is exactly the channel bound
(`DESCRIPTOR_LEN == MAX_MSG == 64`). The descriptor references a transferred
`SharedBufferLoan` by capability kind, unforgeable loan identity, page-aligned
offset/length, type identity, sequence, and known flags. `valid_sample_descriptor`
rejects every field that could steer a mapping or allocation — bad magic/version,
wrong capability kind, unknown flag bits, dirty reserved bytes, zero/mismatched
loan and type identities, non-power-of-two page size, checked-add offset/length
overflow, zero or misaligned length, and length beyond `MAX_SAMPLE_BYTES` —
before any `map_loan`. Because the descriptor fits one message, a payload larger
than `MAX_MSG` crosses as descriptor plus shared buffer without widening `MAX_MSG`
or copying payload bytes through the kernel queue. Status: verified under
`just sample_descriptor_check` (4 QEMU cases) and the full gate stack.

## Observable symptom

- Command: `just sample_descriptor_check`
- Expected: 4 QEMU cases pass; descriptor fits the message bound; admitted
  descriptors round-trip byte-identically and reject unsupported versions/flags;
  malformed descriptors fail before mapping; a receiver observes an 8192-byte
  payload mapped read-only from the exact loaned frames.
- Observed: all pass (see Verification).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The descriptor crosses a process boundary, so it must be a versioned Zutai schema (AGENTS.md), not a hand-written wire struct | Added `contracts/sample-descriptor/v1/{schema.zt,gen_rust.zt}` mirroring the spawn/fs sibling renderers; bindings are `@generated` and checked byte-identically |
| 2 | The roadmap requires the descriptor "fits the existing channel control-message bound" and never widens `MAX_MSG` | Sized the record at exactly 64 bytes; a compile+runtime assertion pins `DESCRIPTOR_LEN == MAX_MSG` |
| 3 | Validation "before mapping or allocating receiver state" is the exit-condition invariant | Put every bound in `valid_sample_descriptor` (host, `no_std`); the kernel `map_loan` independently re-validates loan identity, receiver binding, bounds, and read-only mapping as defense in depth |
| 4 | Test initially asserted `total_pages()==0` after `return_loan`, but the lender never released its buffer | Corrected: returning settles the loan and reclaims only the receiver mapping; a subsequent lender `release` returns every page and charge |
| 5 | `VirtAddr` exposes only `as_mut_ptr` | Read loaned bytes through `as_mut_ptr::<u8>().read()` under HHDM |

## Root cause

Not a defect fix; C7.6 is new capability. The until-now-missing surface is a
validated, versioned reference type: C7.5 shipped the loan mechanism and its
unforgeable identity, but nothing bounded a *description* of the loaned sample
(offset, length, type, sequence, flags) that a receiver could validate before
mapping, and no contract tied that description to the exact transferred loan.

## Changes

| Area | Change | Established invariant |
|---|---|---|
| `contracts/sample-descriptor/v1/schema.zt` | Versioned Zutai descriptor: magic, version, flags, capability_kind, loan_id, offset, length, type_identity, sequence, reserved; 64-byte packed layout | Zutai is the source of truth for the cross-boundary format |
| `contracts/sample-descriptor/v1/gen_rust.zt` | Schema-reflected renderer emitting offset consts + `WireSampleDescriptor` encode/decode; layout validated against reflected fields | Generated bindings cannot disagree on layout |
| `scripts/generate/generate-sample-descriptor-bindings.py` | Generator + `--check` staleness guard, mirroring `generate-spawn-bindings.py` | `just sample_descriptor_gen` regenerates; `contracts_check` fails on drift |
| `components/proto/src/sample_descriptor.rs` | Generated bindings (`DESCRIPTOR_LEN==MAX_MSG==64`, `MAX_SAMPLE_BYTES`, flags/kind consts) | Byte-identical round-trip |
| `components/proto/src/lib.rs` | `valid_sample_descriptor(descriptor, expected_loan, expected_type, page_size)` bounding version, flags, kind, identities, alignment, and overflow | Malformed descriptor fails before mapping/allocation |
| `kernel/tests/sample_descriptor.rs` | 4 QEMU cases over real page tables + C7.5 loans, incl. an 8192-byte payload carried without queue copy | Independently reviewable C7.6 gate |
| `scripts/check/check-contracts.py`, `Justfile` | Registered the contract + `sample_descriptor_gen`/`sample_descriptor_check` recipes | Wired into the canonical gate set |
| `docs/capability-matrix.md` | Descriptor-plane semantics note (userspace control message, no rights bit, no minted authority) | Matrix tracks the surface in the same change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Descriptor grows past the channel bound | `just sample_descriptor_check` | `descriptor_fits_the_channel_message_bound` fails |
| Binding drift from the schema | `just contracts_check` | `generate-sample-descriptor-bindings.py --check` reports stale |
| Unsupported version/flag or short buffer accepted | `just sample_descriptor_check` | `descriptor_round_trips_and_rejects_unsupported_versions_and_flags` fails |
| Overflow/misalignment/stale identity/type mismatch accepted before mapping | `just sample_descriptor_check` | `malformed_descriptors_fail_before_mapping` fails |
| Payload copied through the queue or wrong bytes mapped | `just sample_descriptor_check` | `receiver_observes_payload_larger_than_message_bound` fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sample_descriptor_check` | pass (4/4 QEMU cases) | Direct |
| `just contracts_check` | pass (byte-identical bindings; 19 boot-contracts lib tests) | Direct |
| `just test` (QEMU) | pass (full suite; only expected `should_panic`) | Direct |
| `just generation_check` | pass (byte-identical two builds) | Direct |
| `just fmt_check` / `just lint` | clean | Direct |
| `just fmt_check_components` / `just lint_components` | clean | Direct |
| `just framework_safety_check` | pass | Direct |

## Decisions

- Decision: The descriptor is a userspace control message, not a kernel object,
  and carries no rights bit.
- Rationale: C7.6 is a validated reference over the C7.5 loan; authority already
  travels with the `SharedBufferLoan` capability. Making the descriptor an object
  would duplicate authority and violate the kernel-policy-free rule.
- Rejected alternative: a kernel-minted descriptor object — it would add a
  redundant authority surface with no operation to gate.

- Decision: `valid_sample_descriptor` binds `expected_loan` and `expected_type`
  supplied by the receiver, rather than trusting the descriptor's self-reported
  identity alone.
- Rationale: The exit condition requires stale loan identity and mismatched type
  identity to fail; binding to receiver-held expectations closes both.
- Rejected alternative: validating only internal consistency — it would admit a
  well-formed descriptor naming a loan the receiver never held.

## Open risks and follow-ups

- [ ] C7.7 composes the factory, quotas, mapping, sealing, loan lifecycle, and
  this descriptor into two isolated components exchanging a payload larger than
  `MAX_MSG`, and owns `just sample_plane_check`. Tracked in `roadmap/02-core-runtime.md` C7.7.

## Artifacts and provenance

- Reviewer verdict: `history://SampleDescriptorReview` (`overall_correctness: correct`, confidence 0.86; only a P3 documentation finding, resolved by this entry and the roadmap status flip).
- Kernel gate: `kernel/tests/sample_descriptor.rs`; `just sample_descriptor_check`.
- Contract: `contracts/sample-descriptor/v1/`.
- Capability surface: `docs/capability-matrix.md`.
- Related roadmap item: `roadmap/02-core-runtime.md` C7.6.
