# 23. Generation build-provenance attestations

| | |
| --- | --- |
| Status | parked |
| Route | updates |
| Depends on | M5.5 deterministic generation output and M5.8 release authorization (complete); provenance remains detached and is not parsed by stage-0 |
| Enables | rebuild verification, incident response, supply-chain audit for host-built generations |
| Now | Paper and host-side implementation are legal today. M5.8 supplies release-signing identities and detached-metadata discipline; the provenance-specific builder identity and storage convention remain open. |

## Motivation

Deterministic generation artifacts can carry a host-side attestation
naming the source revision, builder identity and version, build type,
normalized parameters, resolved dependency digests, and resulting
generation identity. Release signatures (M5.8) answer *who authorized
deployment*; provenance separately answers *how the bytes were
produced*. The two questions have different consumers: stage-0 enforces
authorization, while provenance supports rebuilding ("does a rebuild
from the named inputs reproduce this identity?") and incident response
("which generations were built with the compromised dependency?").

## What exists today

- M5.5 (complete) provides the subject: deterministic, byte-identical
  generation artifacts with a stable identity — an attestation's
  subject hash is meaningful precisely because the build is
  reproducible.
- `just generation_check` and `just contracts_check` already validate
  the deterministic output and manifest contracts a provenance verifier
  would re-derive.
- The SLSA build-provenance model is the reference; the register
  explicitly adopts its separation of build provenance from release
  authorization rather than the full framework.
- M5.8 (complete) supplies pinned release-signing identities and bounded
  detached metadata, but provenance still needs its own builder identity
  and attestation storage convention.

## Design sketch

The attestation is a deterministic, versioned, detached document —
same format discipline as the generation itself — binding: source
revision, builder identity and version, build type, normalized
parameters, resolved dependency digests, and the resulting generation
identity. Detached means stage-0 never parses it; verification is a
host-side act by developers, auditors, or CI.

Verification has two directions. Consistency: the attestation's subject
matches the generation identity, and any alteration of inputs,
dependency digests, builder identity, or output identity fails — the
register's exit condition. Reproduction: rebuild from the named input
closure and compare identities; the deterministic build makes this a
byte comparison, not a judgment call.

The dependency-digest list is the incident-response payload: "which
generations include dependency X at digest Y" is answerable by scanning
attestations, which is what makes a compromised toolchain or library
actionable.

## Open questions

- Should provenance reuse an M5.8 release signer identity or define a
  distinct builder identity with separate authorization?
- Storage location: alongside the generation as an object in the M5.4
  store, or host-side only?
- Normalized parameters: what is the canonical form shared with
  [entry 30](30-deterministic-on-device-builds.md)'s "normalized
  source" definition?
- Attestation for on-device builds (entry 30): does the same document
  cover self-hosted builds once they exist?

## Exit-condition sketch

A verifier accepts provenance whose subject matches the generation
identity and rejects altered inputs, dependency digests, builder
identity, or output identity.

## Probe guidance

Paper/host-side today: define the attestation schema and implement the
consistency verifier over the current deterministic artifacts (both
accept and all four reject cases from the exit condition). The probe
delivers the schema plus measured verification cost and resolves the
provenance-specific builder identity and storage convention.

## References

- [SLSA build provenance](https://slsa.dev/spec/v1.2/build-provenance)
