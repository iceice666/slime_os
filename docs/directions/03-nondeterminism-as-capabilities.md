# 3. Nondeterminism sources as capabilities

| | |
| --- | --- |
| Status | parked |
| Route | determinism |
| Depends on | capability-matrix rows for new object kinds; no milestone consumes it yet, so it is a matrix amendment proposal first |
| Enables | [entry 11](11-flight-recorder-replay.md) (replay half), [entry 26](26-hermetic-testing.md), [entry 30](30-deterministic-on-device-builds.md) |
| Now | The amendment proposal — object kinds, rights strings, the formal meaning of "deterministic component" — is a paper exercise legal today. Highest fan-out in the register: three parked entries consume it. |

## Motivation

Make wall clock and entropy kernel objects gated by rights. A manifest
can then declare a component deterministic — no clock/entropy grants —
making it a pure function of its IPC inputs: bit-reproducible across
boots. Today "this component is deterministic" is an unenforceable hope;
with the amendment it is manifest data, auditable by
[entry 9](09-grant-graph-introspection.md) queries like any other grant.

This is the shared foundation the flight recorder, replay, and
attestation directions all implicitly need. Recording IPC is not enough
to re-execute a component that also reads the clock; determinism must be
constructed, not assumed.

## What exists today

- All component input already crosses typed channel boundaries
  (M5.2a, complete): IPC is schema-first, which is what makes "a pure
  function of its IPC inputs" a meaningful sentence.
- The capability matrix (`../capability-matrix.md`) fixes the grammar
  new object kinds must satisfy; rights are a flat `u32` with bits
  15–31 free.
- M5.3 (complete) already treats recorded IPC as replayable for driver
  fault injection — the special case that works without this amendment
  because the driver fixture avoids nondeterminism by construction.
- Nothing in the kernel currently models clock or entropy as authority;
  components read them without mediation. [INFERENCE: from the absence
  of any clock/entropy row in the matrix.]

## Design sketch

Two new object kinds. Clock: rights distinguish monotonic read from
wall-clock read (a component may need timeouts without learning the date).
Entropy: a right to draw from the kernel pool, or — the more interesting
option — a per-spawn seeded deterministic stream minted by the spawner,
so "randomness" for a deterministic component is reproducible from the
seed recorded in the trace.

The manifest then declares determinism negatively: a component with no
clock/entropy grants is deterministic by construction, and the builder
can check the declaration statically. A virtual fixture clock (entry 26)
is just a different object implementing the same rights, so test
generations bind fixtures where production binds hardware.

Interaction with `derive` needs care in the amendment: a deterministic
component must not be able to receive a clock capability from a peer, or
the negative declaration is void. Options: channel schemas forbid
transferring clock/entropy kinds, or the manifest marks components
sealed against receiving them.

## Open questions

- Rights granularity for clock: single READ, or split MONOTONIC / WALL?
- Entropy as a capability, or a per-spawn seeded pool recorded in the
  generation — which composes better with replay (entry 11)?
- How is the negative declaration sealed against capability transfer
  from peers (schema rule vs manifest flag)?
- Does the scheduler's own timing (preemption) leak nondeterminism that
  must be modeled, or is IPC-order determinism sufficient?

## Exit-condition sketch

A manifest-declared deterministic component produces byte-identical
output across two boots given identical IPC inputs.

## Probe guidance

Paper: write the matrix amendment (object kinds, rights strings, the
sealing rule, and the determinism claim's formal statement) and evaluate
it against the existing component set — which current components would
qualify as deterministic unchanged? The probe's output decides the
entropy design (capability vs seeded pool) before any kernel work is
proposed.
