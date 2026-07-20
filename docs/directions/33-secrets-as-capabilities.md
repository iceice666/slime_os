# 33. Secrets as capabilities

| | |
| --- | --- |
| Status | parked |
| Route | authority |
| Depends on | M5.6b state bindings (complete) for at-rest storage; interacts with [entry 2](02-revocable-leases.md) revocation and [entry 11](11-flight-recorder-replay.md) recording; guest/personality delivery consumes [entry 31](31-compat-personality.md) |
| Enables | scoped, revocable, non-recorded credentials for agents and containers without ambient environment secrets |
| Now | Paper: the Secret object shape, its non-recordability rule, and the recorder interaction are a matrix amendment exercise legal today. |

## Motivation

Agents and containers need credentials — API keys, tokens, provider
secrets — and conventional systems inject them as ambient environment
variables, exactly the ambient authority Slime rejects. A daily-driver
running agents and containers needs a first-class way to hand a
component a scoped, revocable secret that it cannot copy, widen, or leak
into audit trails. Two existing directions collide here and neither
resolves it: [entry 11](11-flight-recorder-replay.md) records IPC to
make components replayable, but recording a secret defeats its
confidentiality; [entry 2](02-revocable-leases.md) can revoke a grant,
but a secret already copied into a trace or a log is beyond revocation.
A `Secret` object kind that is capability-gated *and* non-recordable
resolves both.

This is the credential half of the agent/container safety story:
[entry 18](18-network-authority.md) bounds *where* data can go, this
entry bounds *what secret material* a component holds and whether it can
ever escape into an observable channel.

## What exists today

- Spawn supplies no implicit environment (README agentic direction), so
  there is no ambient env-var channel to inject secrets through — the
  gap is a first-class replacement, not a leak to close.
- M5.6b state bindings (complete) with `preserve` /
  `snapshotBeforeUpgrade` policies are the natural at-rest home for a
  sealed secret, with the same rollback discipline as other state.
- [entry 11](11-flight-recorder-replay.md) plans to record IPC for
  replay; [entry 2](02-revocable-leases.md) plans revocation. Their
  interaction with confidential material is unspecified — this entry is
  where that tension is designed. [INFERENCE: neither entry mentions
  secrets.]
- The capability matrix grammar (`../capability-matrix.md`) fixes what
  a new object kind must satisfy; rights are a flat `u32` with bits
  15-31 free.

## Design sketch

A `Secret` object holds opaque material the holder can *use* but not
*read* in the general case: rights distinguish USE (present the secret
to a designated service — e.g., the network service attaches it as a
bearer token to a declared destination) from a narrower or absent
READ. The point is that a component can authenticate with a credential
it can never exfiltrate as bytes, because the kernel and the trusted
service, not the component, move the material.

The non-recordability rule is the load-bearing part. A Secret
capability, and any IPC message carrying secret material, is marked
non-recordable: [entry 11](11-flight-recorder-replay.md)'s recorder
must redact it (recording a commitment/handle, not the value), and
replay must re-inject from the sealed store rather than from the trace.
This makes "deterministic replay" and "confidential credential"
coexist: the trace is reproducible in structure without ever containing
the secret. The rule composes with
[entry 3](03-nondeterminism-as-capabilities.md)'s treatment of entropy
as similarly non-reproducible-from-trace input.

Delivery to foreign workloads: for a container/personality
([entry 31](31-compat-personality.md)) that genuinely needs the secret
value (a Linux program reading an env var), the personality is the
trust boundary — it holds the Secret capability and materializes the
value only inside the container's address space, never back across a
recordable channel. Revocation ([entry 2](02-revocable-leases.md))
invalidates the Secret capability; because the value never entered a
trace or a peer, revocation is actually effective.

At rest, a secret is a state binding sealed under a key the generation
does not carry in plaintext — the natural consumer of
[entry 5](05-tpm-bound-boot-state.md) TPM sealing when that lands, so a
secret is bound to a known-good boot state.

## Open questions

- Does the USE/READ split hold in practice, or do too many real
  credentials require the holder to read plaintext (making the
  personality the only viable delivery for those)?
- How does the recorder represent a redacted secret so replay stays
  deterministic — a stable handle, a commitment hash, or a fixture
  substitution?
- At-rest sealing before [entry 5](05-tpm-bound-boot-state.md) exists:
  is a generation-carried key acceptable as an interim, or must secrets
  wait for TPM binding?
- Revocation timing: is a revoked Secret capability refused at next USE,
  or must in-flight uses be interrupted?
- Does a secret's rollback follow `discardOnRollback` semantics by
  default, so a rolled-back generation cannot resurrect a rotated
  credential?

## Exit-condition sketch

A component holding a USE-only Secret capability authenticates to a
declared service without being able to read the secret bytes; the
flight recorder captures a replayable trace that contains no secret
material; revoking the capability denies the next use, and no earlier
trace or peer holds the value.

## Probe guidance

Paper: write the matrix amendment (Secret object shape, USE/READ
rights, the non-recordability marking) and specify the recorder and
revocation interactions explicitly — the redaction/replay scheme and
the revocation-effectiveness argument are the deliverables that make the
entry more than "add an object kind." Evaluate against a concrete
agent scenario (an agent calling a model-provider endpoint with a
bearer token) and a container scenario (a Linux program expecting an
env-var credential) so both the USE-only and value-delivery paths are
covered before any kernel work.
