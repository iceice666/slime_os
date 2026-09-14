# Authority and trust plans

The current system has explicit capabilities, narrow-only derivation/transfer,
checked rights vocabulary, declared scheduling classes, and deterministic-
recording admission. The mechanisms below are not implemented and confer no
current authority.

## Revocation and leases

Add manifest-rooted derivation trees so a holder may revoke exactly a subtree it
created while parent and sibling grants survive. Specify use-after-revoke,
in-flight IPC, notification, expiry/renewal, reboot, rollback, and provenance.
The checked capability algebra must show that derive, transfer, lease transition,
and revoke never widen authority and that removing edges creates no reachability.

## Secrets as capabilities

Add a `Secret` object with USE distinct from READ, service-mediated use,
non-recordability, trace redaction/re-injection, rollback-safe rotation, and
revocation timing. A foreign-workload personality that must materialize plaintext
becomes the explicit trust boundary; no recordable channel receives ambient
credential bytes.

## Accelerator compute authority

Add rights-gated queue submission with a generation-declared quantity the
implementation can actually charge — tokens, work items, or queue time — plus
IOMMU-constrained DMA over held shared buffers and visible grant topology. It
must not inherit a fictional CPU budget from scheduling class; current kernels
are non-MCS.

## Physical trust and attestation

Add a bounded target TPM path for counters/sealed values, bind BootState facts to
TPM state, specify every disk/TPM desynchronization case, preserve selectable
boot roots, and expose only the generation-identity quote required by a verifier.
A virtual TPM may test models but cannot complete the Framework physical claim.

## Distributed capabilities

Define a cryptographic wire form that maps remote presentation to local grants,
retains revocation topology, binds session identity, rejects replay, and surfaces
partition/unreachable/revoked states as structured channel errors. Distributed
authority is separate from ROS typed-data interoperability.

## Cross-cutting requirements

Each mechanism lands with its capability-matrix and syscall-ABI changes, a
versioned Zutai contract for every wire/persisted record, bounded tables, explicit
rollback behavior, and a real denial gate. Work-item UUIDs and dependency edges
remain in `.tasks/items/`; exploratory origins remain in `docs/directions/`.

## References

- [`../capability-matrix.md`](../capability-matrix.md)
- [`../architecture/runtime-authority.md`](../architecture/runtime-authority.md)
- [`../decisions/mcs-cpu-budgets.md`](../decisions/mcs-cpu-budgets.md)
- `docs/directions/{02-revocable-leases,05-tpm-bound-boot-state,10-distributed-capabilities,24-rights-algebra-model,33-secrets-as-capabilities}.md`
