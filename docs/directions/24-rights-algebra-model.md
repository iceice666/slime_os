# 24. Checked model of the capability rights algebra

| | |
| --- | --- |
| Status | promoted → [Authority A0](../../roadmap/06-authority-trust.md#a0--checked-capability-rights-algebra) (design retained) |
| Route | authority |
| Depends on | nothing; host-side contracts work consuming only the capability matrix |
| Enables | [entry 1](01-authority-diff-gate.md) (widening definition), [entry 27](27-policy-carrying-generations.md) (invariant semantics), [entry 2](02-revocable-leases.md) (does revocation preserve the algebra) |
| Now | Probe complete; promoted as the checked rights-algebra baseline and lockstep discipline in Authority A0. |

## Motivation

M5.6a/M5.6b established the checked-contract methodology for BootState,
state, and GC semantics. Authority now has the same kind of executable
specification: a bounded transition model, must-fail mutations, and a gate that
runs with generation-contract validation.

## Probe outcome

The model covers `derive`, `spawnGrant`, `export`, `finalize`, `import`, and
`cancel`. `derive` and `spawnGrant` share the same non-consuming narrow-only
rule; transfer follows the consuming export/finalize/import path, with cancel
restoring the source before finalization.

Seven state safety properties are checked:

- `DeriveOnlyNarrows`
- `TransferOnlyNarrows`
- `TransferRequiresTransferRight`
- `TransferFollowsDeclaredEdge`
- `RightsValidForKind`
- `NoTransferDuplication`
- `NoAuthorityWidening`, retained as a weaker corollary rather than the primary
  theorem

The original manifest-authority-closure exit condition was rejected by the
probe. Per-component declared rights are violated by honest transfer, with a
243-state counterexample. A union-of-all-declared-rights closure is vacuous. An
edge-scoped closure is true for the honest model but blind to widening derive:
if a source may delegate its full rights, those rights already appear in the
target's closure. The load-bearing results are therefore the per-operation
conservation laws, not a closure formulation that cannot be both non-vacuous
and true.

## Bounds and measured cost

The model has three components, one object, and four symbolic rights representing
the transfer meta-right, two independently narrowable same-kind rights, and one
foreign-kind right. The main scenario explores 300 states and 1436 transitions.
All seven scenarios completed in about 1.5 seconds wall time on an M5 Pro during
the probe.

State-space cost grew by about 6.3 times per added right: three rights took
0.55 seconds, four took 3.1 seconds, five took 19 seconds, and six took 121
seconds. Extrapolation reaches roughly eight hours at nine rights, so the real
33-name vocabulary is not exhaustively checkable in this transition shape. The
four-class abstraction is required, not merely convenient. For scale, the
BootState model explores 5416 states and takes about 80 seconds.

## Vocabulary synchronisation

The probe selected deterministic validation, option 3 from the register's three
possible synchronisation strategies. `rightBits` now has one source in
`contracts/generation/v5/vocab/rights.zt`; both generation schema rendering and
package consumers resolve that table. Two Rust tests pin the canonical names to
real enforcement: `capability_rights_valid` partitions manifest-declarable from
rejected rights, and `graph` pins root-only supervision rights against every
declared-but-ungated bit.

Direct reuse of `schema.zt` was unavailable because Zutai refuses `..` import
traversal and that file's final value is an effectful `main`, not a vocabulary
record. Directly consuming all 33 names in the model is computationally
infeasible under the measured cost curve. The symbolic rights are equivalence
classes, not a second vocabulary copy; the single source plus total partition
tests are the anti-drift mechanism.

## Mutations

Six mutations must each produce a counterexample:

| Mutation | Required violation |
| --- | --- |
| widening derive | `DeriveOnlyNarrows` |
| widening transfer | `TransferOnlyNarrows` |
| export without transfer authority | `TransferRequiresTransferRight` |
| kind-blind derive | `RightsValidForKind` |
| install during finalize while pending remains live | `NoTransferDuplication` |
| export or spawn grant across an undeclared edge | `TransferFollowsDeclaredEdge` |

The gate was also exercised with the widening-derive fault disabled. It exited
non-zero with `FAILED (expected violation of "DeriveOnlyNarrows", none found)`,
so an unexpectedly passing mutation fails closed rather than weakening the
property.

## Known abstraction gaps

- The `retain` non-consuming export variant is not modelled; export always
  consumes, matching the direction's transfer framing and the `retain == false`
  implementation arm.
- Descriptor and native-endpoint ticket movement between `export` and `import`
  is collapsed into one pending record.
- Native `Endpoint` transferability lives in `PeerEndpointTable`, not in endpoint
  rights bits; the model represents it as retaining or narrowing away
  `#transfer`.
- The equivalence-class abstraction is documented rather than machine-checked;
  a refinement proof remains outside this probe.

## Findings beyond the model

The vocabulary partition exposed eight rights that are declared, named, and
inside `RIGHT_ALL`, but admitted by no `CapabilityKind` or runtime `rights_type!`:
`MAP_MMIO`, `DMA_PIN`, `DMA_RELEASE`, `IRQ_ACK`, `STORE_READ`, `STORE_WRITE`,
`HEALTH_CONFIRM`, and `BOOT_UPDATE`. `docs/capability-matrix.md` incorrectly
called those bits unassigned; it now records them as named but ungated and names
their retired-kernel provenance. The partition tests require that document to
move with any future enforcement change.

## Promotion recommendation

Promote the result with this exit condition: a bounded model of the six
authority operations, seven named properties, six must-fail mutations, and a
single-sourced rights vocabulary pinned against real enforcement. Do not retain
the original “manifest authority closure” wording; the probe proved it cannot
serve as the primary theorem.

Authority A0 owns the lockstep rule already used by M5.6: a change to
`rightBits`, `capability_rights_valid`, or a `rights_type!` `VALID` mask lands in
the same commit as every resulting model and capability-matrix update. The
partition test's `RIGHT_ALL` totality assertion makes an unclassified new right
fail rather than silently drift.
